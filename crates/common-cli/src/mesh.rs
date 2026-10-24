//! Common Mesh, CLI side (F-02-*, F-06-05, F-06-02, F-10-10): settings, peers, groups, privacy,
//! audit, diagnostics. The daemon side (Node, discovery, inbound serving) lives in
//! `common_api::mesh` and runs inside `common serve`.
//!
//! Settings are the API's `common_api::mesh::Settings` in `COMMON_HOME/config.json`
//! (`{privacy, network_enabled, serve_peers}`, 0600). Pinned peers, groups and revocations live in
//! `identity/peers.json` (`common_identity::PeerStore`).
//!
//! A running `common serve` re-reads both files when they change and writes them read-modify-write,
//! so a direct edit is never lost and needs no restart. Some effects only the daemon can produce
//! (closing a revoked device's sessions, sending the revocation to connected peers, starting the
//! peer endpoint), so it records its API address in `COMMON_HOME/serve.json` and every mutation
//! here (`peers rm|revoke`, `groups create|add|remove|policy`, `privacy`) goes through its API when
//! that address answers with this device's id ([`Daemon`]); the file is re-read afterwards to
//! confirm it was persisted. With no daemon the file is written directly. `peers pair|join` refuse
//! while a daemon runs: they would open a second endpoint for this identity, and the other device
//! would record that short-lived endpoint's address instead of the daemon's.

use crate::shutdown_signal;
use clap::{Args, Subcommand, ValueEnum};
use common_artifacts::Store;
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_identity::{DeviceIdentity, Group, GroupPolicy, PeerId, PeerRecord, PeerStore, Revocation};
use common_network::discovery::{Advertiser, Browser, DiscoveredPeer};
use common_network::transport::Node;
use common_network::{pairing, NetworkConfig};
use common_observability::Audit;
use common_policy::{PolicyConfig, PolicyEngine, PrivacyMode};
use reqwest::Method;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::{BufRead, IsTerminal, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub use common_api::mesh::{Settings, DISCLOSURE};

/// `config.json`, or every default (local-only, nothing listens, nothing served) when absent.
pub fn load_settings(paths: &Paths) -> Result<Settings> {
    Ok(Settings::load(paths)?.unwrap_or_default())
}

/// The serde string of the privacy mode (`local_only`, `trusted_group`, …).
pub fn privacy_str(s: &Settings) -> String {
    serde_json::to_value(s.privacy)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "local_only".into())
}

/// Peer endpoint flags shared by `serve --network`, `peers pair` and `peers join`.
#[derive(Args, Debug, Clone)]
pub struct NetArgs {
    /// UDP port of the peer endpoint
    #[arg(long, default_value_t = 8193)]
    pub listen_port: u16,
    /// Bind address of the peer endpoint (0.0.0.0 = every interface)
    #[arg(long, default_value = "0.0.0.0")]
    pub listen_addr: String,
    /// Do not advertise or browse on mDNS
    #[arg(long)]
    pub no_mdns: bool,
}

impl NetArgs {
    pub fn config(&self) -> NetworkConfig {
        NetworkConfig {
            enabled: true,
            listen_port: self.listen_port,
            bind_addr: self.listen_addr.clone(),
            mdns: !self.no_mdns,
            ..NetworkConfig::default()
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum PeersCmd {
    /// Pinned devices: id, label, name, groups, last seen
    List,
    /// Devices advertising on the LAN right now (signed mDNS records; untrusted unless pinned)
    Discover {
        /// How long to listen
        #[arg(long, default_value_t = 3)]
        wait_secs: u64,
        #[command(flatten)]
        net: NetArgs,
    },
    /// Start pairing: shows a 6-digit code and waits for one device to join (Ctrl-C cancels)
    Pair {
        /// Name this device shows on the other side (default: hostname)
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        net: NetArgs,
    },
    /// Join a device that is pairing: its pairing address `ip:port` plus the 6-digit code, or its QR payload
    Join {
        target: String,
        code: Option<String>,
        /// Name this device shows on the other side (default: hostname)
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        net: NetArgs,
    },
    /// Forget a device (no revocation record; it can pair again)
    Rm { id: String },
    /// Revoke a device: unpins it, records a signed revocation, refuses it at every later handshake
    Revoke { id: String },
}

#[derive(Subcommand, Debug)]
pub enum GroupsCmd {
    List,
    /// Create (or reset) a group; policies default to deny
    Create {
        name: String,
        /// Members may run whole requests on this device
        #[arg(long)]
        allow_remote: bool,
        /// Members may host or use layer ranges (M4)
        #[arg(long)]
        allow_pooled: bool,
    },
    /// Add a pinned device to a group
    Add {
        group: String,
        peer: String,
    },
    /// Remove a device from a group
    Remove {
        group: String,
        peer: String,
    },
    /// Change a group's policy: --allow-remote=true|false --allow-pooled=true|false
    Policy {
        group: String,
        #[arg(long, require_equals = true)]
        allow_remote: Option<bool>,
        #[arg(long, require_equals = true)]
        allow_pooled: Option<bool>,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivacyChoice {
    /// Refuses every off-device request (default). Downloading a model or the engine still uses the internet
    LocalOnly,
    /// Requests may run on paired devices in a trusted group (also enables peer networking)
    TrustedDevices,
}

#[derive(Args, Debug)]
pub struct PrivacyArgs {
    /// New mode; omitted shows the current one
    #[arg(value_enum)]
    pub mode: Option<PrivacyChoice>,
    /// Accept the disclosure without the interactive confirmation
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args, Debug)]
pub struct AuditArgs {
    /// Last N entries
    #[arg(long, default_value_t = 50)]
    pub n: usize,
}

#[derive(Args, Debug)]
pub struct DiagnosticsArgs {
    /// Write the bundle here instead of stdout
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// Everything a mesh command needs, opened from `COMMON_HOME`.
pub struct Mesh {
    pub paths: Paths,
    pub settings: Settings,
    pub identity: Arc<DeviceIdentity>,
    pub store: Arc<PeerStore>,
    pub audit: Arc<Audit>,
}

impl Mesh {
    pub fn open() -> Result<Mesh> {
        let paths = Paths::from_env();
        paths.ensure_dirs()?;
        Ok(Mesh {
            settings: load_settings(&paths)?,
            identity: Arc::new(DeviceIdentity::load_or_create(&paths)?),
            store: Arc::new(PeerStore::open(&paths)?),
            audit: Arc::new(Audit::open(&paths)?),
            paths,
        })
    }

    pub fn policy(&self, network_enabled: bool, serve_peers: bool) -> Arc<PolicyEngine> {
        Arc::new(PolicyEngine::new(
            PolicyConfig {
                default_privacy: self.settings.privacy,
                network_enabled,
                serve_peers,
                swarm_enabled: false,
            },
            self.store.clone(),
        ))
    }

    /// Bind the workload endpoint. Fills `cached_models` so peers see what this device has.
    pub async fn node(&self, config: NetworkConfig, serve_peers: bool) -> Result<Arc<Node>> {
        let node = Node::start(
            config,
            self.identity.clone(),
            self.store.clone(),
            self.policy(true, serve_peers),
            self.audit.clone(),
        )
        .await
        .map_err(|e| match e.code {
            ErrorCode::Io => e.with_hint(
                "another `common serve --network` or `common peers pair` may hold this port; stop it or pass --listen-port",
            ),
            _ => e,
        })?;
        let store = Store::new(self.paths.clone());
        let cached: Vec<String> = store
            .list()?
            .into_iter()
            .filter(|m| store.blob_path(&m.sha256).is_file())
            .map(|m| m.sha256)
            .collect();
        *node.cached_models.lock().unwrap_or_else(|e| e.into_inner()) = cached;
        Ok(node)
    }

    /// A pinned peer by 64-hex id, short label (`ab12-cd34-ef56`) or any unique hex prefix.
    /// A revoked id is `PeerRevoked`, an unknown one `PeerUnauthorized`.
    pub fn resolve_peer(&self, s: &str) -> Result<PeerRecord> {
        let key = hex_key(s)?;
        let mut found: Vec<PeerRecord> = self
            .store
            .list()?
            .into_iter()
            .filter(|p| p.id.to_hex().starts_with(&key))
            .collect();
        match found.len() {
            1 => Ok(found.remove(0)),
            0 => {
                let revoked = self
                    .store
                    .revocations_since(0)?
                    .into_iter()
                    .any(|r| r.peer_id.to_hex().starts_with(&key) && self.store.is_revoked(&r.peer_id));
                if revoked {
                    Err(
                        CommonError::new(ErrorCode::PeerRevoked, format!("device {s} was revoked"))
                            .with_hint("pair it again if you trust it"),
                    )
                } else {
                    Err(
                        CommonError::new(ErrorCode::PeerUnauthorized, format!("device {s} is not paired"))
                            .with_hint("common peers list"),
                    )
                }
            }
            _ => Err(CommonError::invalid(format!(
                "`{s}` matches {} paired devices; use more of the id",
                found.len()
            ))),
        }
    }

    /// Where to reach `rec`: its last known address, else mDNS for a few seconds.
    // ponytail: last_addr first (DHCP moves are rare on a LAN); upgrade: retry via discovery on connect failure.
    pub async fn peer_addr(&self, rec: &PeerRecord) -> Result<SocketAddr> {
        if let Some(a) = rec.last_addr.as_deref().and_then(|a| a.parse().ok()) {
            return Ok(a);
        }
        let browser = Browser::start(
            &NetworkConfig {
                enabled: true,
                ..NetworkConfig::default()
            },
            self.store.clone(),
        )?;
        if browser.active() {
            for _ in 0..30 {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if let Some(p) = browser.peers(10).into_iter().find(|p| p.id == rec.id) {
                    return Ok(p.addr);
                }
            }
        }
        Err(CommonError::new(
            ErrorCode::PeerLost,
            format!("no known address for device {}", rec.id.short_label()),
        )
        .with_hint("make sure it runs `common serve --network` on this LAN, or pair it again"))
    }

    /// E5: whether this home's running daemon holds `id` (Disconnect, until Connect or a restart).
    /// The hold lives in the daemon's `Node`; a process dialling with its own Node takes it over
    /// with `Node::hold` so the transport refuses the dial as the daemon's would. No daemon, no
    /// hold (it ends with the service); a daemon that cannot list its peers fails the caller
    /// rather than let it dial a device the user disconnected.
    pub async fn held_by_daemon(&self, id: &PeerId) -> Result<bool> {
        let Some(d) = Daemon::find(&self.paths).await? else {
            return Ok(false);
        };
        let v = d.call(Method::GET, "/api/peers", None).await?;
        let hex = id.to_hex();
        Ok(v["peers"]
            .as_array()
            .is_some_and(|ps| ps.iter().any(|p| p["id"] == hex.as_str() && p["held"] == true)))
    }
}

/// A peer id, short label or hex prefix as lowercase hex without dashes; anything else is invalid.
fn hex_key(s: &str) -> Result<String> {
    let key = s.trim().to_ascii_lowercase().replace('-', "");
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(CommonError::invalid(format!(
            "`{s}` is not a peer id (64 hex) or a short label like ab12-cd34-ef56"
        )));
    }
    Ok(key)
}

/// The one member of `group` named by `s` (id, short label or prefix); none or several is an error.
fn member_by_prefix(members: &[PeerId], s: &str, group: &str) -> Result<PeerId> {
    let key = hex_key(s)?;
    let hits: Vec<PeerId> = members
        .iter()
        .filter(|p| p.to_hex().starts_with(&key))
        .copied()
        .collect();
    match hits[..] {
        [id] => Ok(id),
        [] => Err(CommonError::invalid(format!("`{s}` is not a member of {group}"))),
        _ => Err(CommonError::invalid(format!(
            "`{s}` matches {} members of {group}; use more of the id",
            hits.len()
        ))),
    }
}

/// `COMMON_HOME/serve.json`: `{"addr", "pid"}` of the `common serve` running on this home. No secrets.
pub fn serve_file(paths: &Paths) -> PathBuf {
    paths.root.join("serve.json")
}

/// Written by `common serve` once its API listener is bound (an unspecified bind is recorded as
/// loopback, which it also answers on).
pub fn write_serve_file(paths: &Paths, addr: SocketAddr) -> Result<()> {
    let ip = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    let v = json!({ "addr": SocketAddr::new(ip, addr.port()).to_string(), "pid": std::process::id() });
    std::fs::write(serve_file(paths), serde_json::to_vec(&v)?)?;
    Ok(())
}

/// Removed when `common serve` exits, only while it still names this process (a second daemon
/// on the same home may have replaced it). A crash leaves it behind; [`Daemon::find`] copes.
pub fn remove_serve_file(paths: &Paths) {
    let path = serve_file(paths);
    let ours = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .is_some_and(|v| v["pid"] == std::process::id());
    if ours {
        let _ = std::fs::remove_file(path);
    }
}

/// The `common serve` running on this home (F-06-05, F-02-09): CLI mutations go through its API so
/// their live effects (sessions closed, revocations sent, endpoint started) happen now.
pub struct Daemon {
    pub url: String,
    http: reqwest::Client,
    key: Option<String>,
}

impl Daemon {
    /// The daemon `serve.json` names, if it answers `GET /api/network` with this device's id
    /// (`identity/device.pub`). No file, a stale one (crash: nothing answers) or another home's
    /// daemon on that port is `None`. A daemon that wants an API key this process lacks is
    /// `Unauthorized`: writing the file instead would skip its live effects (a revoked device's
    /// open sessions would stay open).
    // ponytail: the bearer key (COMMON_API_KEY) goes to whatever answers on that loopback port;
    // the re-read in `verify` catches a squatter that lies, not one that keeps the key.
    pub async fn find(paths: &Paths) -> Result<Option<Daemon>> {
        let read = |p: PathBuf| std::fs::read_to_string(p).ok();
        let (Some(file), Some(me)) = (read(serve_file(paths)), read(paths.identity().join("device.pub"))) else {
            return Ok(None);
        };
        let addr = serde_json::from_str::<Value>(&file)
            .ok()
            .and_then(|v| v["addr"].as_str()?.parse::<SocketAddr>().ok());
        let Some(addr) = addr else { return Ok(None) };
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| CommonError::internal(format!("http client: {e}")))?;
        let d = Daemon {
            url: format!("http://{addr}"),
            http,
            key: std::env::var("COMMON_API_KEY").ok().filter(|k| !k.is_empty()),
        };
        match d.call(Method::GET, "/api/network", None).await {
            Ok(v) if v["peer_id"].as_str() == Some(me.trim()) => Ok(Some(d)),
            Err(e) if e.code == ErrorCode::Unauthorized => Err(CommonError::new(
                ErrorCode::Unauthorized,
                format!("`common serve` at {} runs on this home and wants an API key", d.url),
            )
            .with_hint("set COMMON_API_KEY to the key `common serve --api-key` was given, or stop it")),
            _ => Ok(None),
        }
    }

    /// One JSON call; a non-2xx answer becomes the daemon's typed error.
    pub async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self.http.request(method, format!("{}{path}", self.url));
        if let Some(k) = &self.key {
            req = req.bearer_auth(k);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let io = |e: reqwest::Error| CommonError::io(format!("{}{path}: {e}", self.url));
        let resp = req.send().await.map_err(io)?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(io)?;
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.is_success() {
            Ok(v)
        } else {
            Err(daemon_error(&v, status.as_u16()))
        }
    }

    /// The daemon persists before it answers; re-read the file so a squatter or a failed write
    /// shows up as an error, never as a change that only looked applied.
    fn verify(&self, ok: bool) -> Result<()> {
        if ok {
            return Ok(());
        }
        Err(CommonError::internal(format!(
            "`common serve` at {} answered, but the change is not on disk",
            self.url
        ))
        .with_hint("check `common serve` is this COMMON_HOME's daemon, then retry"))
    }
}

/// The API's `{"error": {code, message, hint}}` as the `CommonError` it was made from.
fn daemon_error(v: &Value, status: u16) -> CommonError {
    let e = &v["error"];
    let code = serde_json::from_value(e["code"].clone()).unwrap_or(ErrorCode::Internal);
    let message = e["message"]
        .as_str()
        .map_or_else(|| format!("`common serve` answered HTTP {status}"), str::to_string);
    let err = CommonError::new(code, message);
    match e["hint"].as_str() {
        Some(h) => err.with_hint(h),
        None => err,
    }
}

/// `peers pair`/`join` open their own endpoint; next to a daemon that is a second endpoint for this
/// identity (same default port), and the other device would pin its short-lived address. The pin
/// itself would reach the daemon (it re-reads `peers.json`).
// ponytail: refuse rather than drive /api/pair/*; upgrade: route it like revoke if CLI pairing
// next to a running daemon is wanted.
async fn refuse_if_daemon(paths: &Paths) -> Result<()> {
    match Daemon::find(paths).await? {
        Some(d) => Err(CommonError::invalid(format!(
            "`common serve` at {} runs this device's peer endpoint; pairing goes through it",
            d.url
        ))
        .with_hint("pair from the desktop app, or stop `common serve` and retry")),
        None => Ok(()),
    }
}

/// Insert or replace a group: through the daemon when one runs, else straight to the file.
async fn save_group(m: &Mesh, g: &Group) -> Result<()> {
    match Daemon::find(&m.paths).await? {
        Some(d) => {
            d.call(Method::PUT, "/api/groups", Some(serde_json::to_value(g)?))
                .await?;
            d.verify(PeerStore::open(&m.paths)?.groups()?.contains(g))
        }
        None => m.store.upsert_group(g.clone()),
    }
}

/// This host's name, what a paired device shows for us (one definition, shared with the daemon).
pub use common_api::mesh::device_name;

/// The address a LAN peer should use for `addr` when it was bound on every interface: the IP of
/// the default route. UDP `connect` sends nothing; it only asks the kernel which source it would
/// pick. The bound address is returned unchanged when it is specific.
// ponytail: one default-route guess; multi-homed hosts get the first route. mDNS carries the real list.
pub fn routable(addr: SocketAddr) -> SocketAddr {
    if !addr.ip().is_unspecified() {
        return addr;
    }
    let probe = std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| s.connect("192.0.2.1:9").map(|()| s))
        .and_then(|s| s.local_addr());
    match probe {
        Ok(local) if !local.ip().is_unspecified() => SocketAddr::new(local.ip(), addr.port()),
        _ => addr,
    }
}

fn out<T: Serialize>(json: bool, value: &T, text: String) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        println!("{text}");
    }
    Ok(())
}

pub async fn peers(cmd: &PeersCmd, json: bool) -> Result<()> {
    let m = Mesh::open()?;
    match cmd {
        PeersCmd::List => peers_list(&m, json).await,
        PeersCmd::Discover { wait_secs, net } => {
            let browser = Browser::start(&net.config(), m.store.clone())?;
            if !browser.active() {
                return Err(CommonError::new(ErrorCode::PolicyDenied, "mDNS discovery is disabled")
                    .with_hint("drop --no-mdns / COMMON_MDNS=0"));
            }
            tokio::time::sleep(Duration::from_secs(*wait_secs)).await;
            let found = browser.peers(10);
            if json {
                println!("{}", serde_json::to_string_pretty(&found)?);
                return Ok(());
            }
            println!("{:<14} {:<22} {:<9} {:<8} ID", "LABEL", "ADDRESS", "TRUSTED", "PAIRING");
            for p in &found {
                println!(
                    "{:<14} {:<22} {:<9} {:<8} {}",
                    p.id.short_label(),
                    p.addr,
                    if p.trusted { "yes" } else { "no" },
                    p.pair_port.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                    p.id
                );
            }
            if found.is_empty() {
                println!("no devices seen in {wait_secs} s");
            }
            Ok(())
        }
        PeersCmd::Pair { name, net } => {
            refuse_if_daemon(&m.paths).await?;
            pair(&m, name.clone(), net, json).await
        }
        PeersCmd::Join {
            target,
            code,
            name,
            net,
        } => {
            refuse_if_daemon(&m.paths).await?;
            join(&m, target, code.as_deref(), name.clone(), net, json).await
        }
        PeersCmd::Rm { id } => {
            let rec = m.resolve_peer(id)?;
            let daemon = Daemon::find(&m.paths).await?;
            match &daemon {
                Some(d) => {
                    d.call(Method::DELETE, &format!("/api/peers/{}", rec.id), None).await?;
                    d.verify(!PeerStore::open(&m.paths)?.is_pinned(&rec.id))?;
                }
                None => m.store.remove(&rec.id)?,
            }
            out(
                json,
                &json!({ "id": rec.id, "removed": true, "daemon_applied": daemon.is_some() }),
                format!("forgot {} ({})", rec.name, rec.id.short_label()),
            )
        }
        PeersCmd::Revoke { id } => {
            let rec = m.resolve_peer(id)?;
            let daemon = Daemon::find(&m.paths).await?;
            let r: Revocation = match &daemon {
                // The daemon revokes, audits, closes the device's sessions and tells connected peers.
                Some(d) => {
                    let v = d
                        .call(Method::POST, &format!("/api/peers/{}/revoke", rec.id), None)
                        .await?;
                    d.verify(PeerStore::open(&m.paths)?.is_revoked(&rec.id))?;
                    serde_json::from_value(v)?
                }
                None => {
                    let r = m.store.revoke(&rec.id, &m.identity)?;
                    if let Err(e) = m.audit.record(
                        "peer_revoked",
                        Some(&rec.id.to_hex()),
                        json!({ "seq": r.seq, "peer": rec.id.short_label(), "source": "local" }),
                    ) {
                        tracing::warn!(error = %e, "audit record not written");
                    }
                    r
                }
            };
            let effect = match &daemon {
                Some(d) => format!(
                    "`common serve` at {} closed its sessions with it and sent the revocation to connected devices",
                    d.url
                ),
                None => {
                    "no `common serve` runs here; group members receive the revocation when they next connect to it"
                        .into()
                }
            };
            let mut v = serde_json::to_value(&r)?;
            v["daemon_applied"] = json!(daemon.is_some());
            out(
                json,
                &v,
                format!(
                    "revoked {} ({}); its key is refused at every later handshake; {effect}.",
                    rec.name,
                    rec.id.short_label()
                ),
            )
        }
    }
}

#[derive(Serialize)]
struct PeerRow {
    id: PeerId,
    label: String,
    name: String,
    groups: Vec<String>,
    paired_at_utc: String,
    last_seen_utc: Option<String>,
    last_addr: Option<String>,
    /// From the running daemon's `GET /api/peers`; null when no daemon answers (unknown).
    connected: Option<bool>,
    /// The daemon's liveness for the device (`connected`, `unhealthy`, `reconnecting`,
    /// `disconnected`) when it reports one.
    health: Option<String>,
}

/// Pinned devices from the store; with a daemon answering, also its live state (`connected`,
/// `health`) and the devices its mDNS browser sees that are not paired (listed, never trusted).
/// JSON: `{this_device, daemon, peers, discovered}`, `daemon` null when none answers.
async fn peers_list(m: &Mesh, json: bool) -> Result<()> {
    let mut rows: Vec<PeerRow> = m
        .store
        .list()?
        .into_iter()
        .map(|p| {
            Ok(PeerRow {
                label: p.id.short_label(),
                groups: m.store.groups_of(&p.id)?.into_iter().map(|g| g.name).collect(),
                id: p.id,
                name: p.name,
                paired_at_utc: p.paired_at_utc,
                last_seen_utc: p.last_seen_utc,
                last_addr: p.last_addr,
                connected: None,
                health: None,
            })
        })
        .collect::<Result<_>>()?;
    let me = m.identity.id;
    // A read-only listing still works when the daemon wants a key this process lacks.
    let (daemon, discovered, note) = match Daemon::find(&m.paths).await {
        Ok(Some(d)) => {
            let live = d.call(Method::GET, "/api/peers", None).await?;
            let seen = apply_live(&mut rows, &live, &me)?;
            (Some(d.url), seen, None)
        }
        Ok(None) => (
            None,
            vec![],
            Some("`common serve` is not running here: connection state unknown and no discovered devices (`common peers discover` listens now)".to_string()),
        ),
        Err(e) => (None, vec![], Some(format!("{}: connection state unknown", e.message))),
    };
    if json {
        let v = json!({ "this_device": me, "daemon": daemon, "peers": rows, "discovered": discovered });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!("this device {me} ({})", me.short_label());
    println!(
        "{:<14} {:<20} {:<16} {:<22} {:<20} {:<12} ID",
        "LABEL", "NAME", "GROUPS", "ADDRESS", "LAST SEEN", "CONNECTED"
    );
    for r in &rows {
        let live = match (&r.health, r.connected) {
            (Some(h), _) => h.clone(),
            (None, Some(c)) => if c { "yes" } else { "no" }.to_string(),
            (None, None) => "-".to_string(),
        };
        println!(
            "{:<14} {:<20} {:<16} {:<22} {:<20} {:<12} {}",
            r.label,
            r.name.chars().take(20).collect::<String>(),
            r.groups.join(","),
            r.last_addr.as_deref().unwrap_or("-"),
            r.last_seen_utc.as_deref().unwrap_or("-"),
            live,
            r.id
        );
    }
    if rows.is_empty() {
        println!("no paired devices; run `common peers pair` here and `common peers join` on the other device");
    }
    if !discovered.is_empty() {
        println!("seen on this network, not paired (untrusted; nothing is sent to them):");
        for p in &discovered {
            let pairing = p.pair_port.map_or("-".to_string(), |port| format!("pairing on {port}"));
            println!("{:<14} {:<22} {:<16} {}", p.id.short_label(), p.addr, pairing, p.id);
        }
    }
    if let Some(n) = note {
        println!("{n}");
    }
    Ok(())
}

/// Fill `connected`/`health` of `rows` from a daemon's `GET /api/peers` and return what its mDNS
/// browser sees that is neither pinned here nor this device.
fn apply_live(rows: &mut [PeerRow], live: &Value, me: &PeerId) -> Result<Vec<DiscoveredPeer>> {
    let pinned = live["peers"].as_array().map(Vec::as_slice).unwrap_or_default();
    for r in rows.iter_mut() {
        let p = pinned.iter().find(|p| p["id"].as_str() == Some(r.id.to_hex().as_str()));
        r.connected = Some(p.and_then(|p| p["connected"].as_bool()).unwrap_or(false));
        r.health = p.and_then(|p| p["health"].as_str()).map(str::to_string);
    }
    let seen: Option<Vec<DiscoveredPeer>> = serde_json::from_value(live["discovered"].clone())?;
    Ok(seen
        .unwrap_or_default()
        .into_iter()
        .filter(|p| !p.trusted && p.id != *me && !rows.iter().any(|r| r.id == p.id))
        .collect())
}

async fn pair(m: &Mesh, name: Option<String>, net: &NetArgs, json: bool) -> Result<()> {
    let cfg = net.config();
    let node = m.node(cfg.clone(), false).await?;
    let name = name.unwrap_or_else(device_name);
    let (session, handle) = pairing::start(node.clone(), &name).await?;
    let bind: IpAddr = cfg
        .bind_addr
        .parse()
        .map_err(|e| CommonError::invalid(format!("bind address: {e}")))?;
    let pair_addr = routable(SocketAddr::new(bind, session.port));
    // The QR carries the bind address; a LAN peer needs a routable one.
    let qr = session
        .qr_payload
        .replace(&SocketAddr::new(bind, session.port).to_string(), &pair_addr.to_string());
    // Advertise `pair=1,pairport` so `common peers discover` on the other device shows us.
    let _advertiser = Advertiser::start(&cfg, m.identity.clone(), node.local_addr()?.port(), Some(session.port))?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "code": session.code, "peer_id": m.identity.id, "pair_addr": pair_addr.to_string(),
                "qr": qr, "expires_at_unix": session.expires_at_unix,
            })
        );
    } else {
        println!("code: {}", session.code);
        println!("peer-id: {}", m.identity.id.grouped());
        println!("pair-address: {pair_addr}");
        println!("qr: {qr}");
        println!("expires: {}", common_identity::format_utc(session.expires_at_unix));
        println!(
            "waiting for a device to join (on it: common peers join {pair_addr} {}); Ctrl-C cancels",
            session.code
        );
    }
    let _ = std::io::stdout().flush();
    let r = tokio::select! {
        r = handle => r.map_err(|e| CommonError::internal(format!("pairing task: {e}")))?,
        _ = shutdown_signal() => Err(CommonError::new(ErrorCode::Cancelled, "pairing cancelled")),
    };
    node.shutdown().await;
    let rec = r?;
    out(
        json,
        &rec,
        format!(
            "paired {} ({}) at {}",
            rec.name,
            rec.id.short_label(),
            rec.last_addr.as_deref().unwrap_or("-")
        ),
    )
}

async fn join(
    m: &Mesh,
    target: &str,
    code: Option<&str>,
    name: Option<String>,
    net: &NetArgs,
    json: bool,
) -> Result<()> {
    let (expected, addr, pw) = if target.starts_with("common-pair:") {
        let (id, addr, secret) = pairing::expected_peer_from_qr(target)?;
        (Some(id), addr, secret)
    } else {
        let addr: SocketAddr = target
            .parse()
            .map_err(|_| CommonError::invalid(format!("`{target}` is not ip:port or a common-pair QR payload")))?;
        let code = code.ok_or_else(|| CommonError::invalid("the 6-digit code is required with ip:port"))?;
        (None, addr, code.to_string())
    };
    pairing::check_code(&pw)?; // before the node binds its port
    let node = m.node(net.config(), false).await?;
    let name = name.unwrap_or_else(device_name);
    // A QR payload names the device: any other key fails the TLS handshake, before SPAKE2 and
    // before anything is pinned or a revocation touched.
    let r = tokio::select! {
        r = pairing::join_with(node.clone(), addr, &pw, &name, expected) => r,
        _ = shutdown_signal() => Err(CommonError::new(ErrorCode::Cancelled, "pairing cancelled")),
    };
    node.shutdown().await;
    let rec = r?;
    out(
        json,
        &rec,
        format!(
            "paired {} ({}) at {}",
            rec.name,
            rec.id.short_label(),
            rec.last_addr.as_deref().unwrap_or("-")
        ),
    )
}

pub async fn groups(cmd: &GroupsCmd, json: bool) -> Result<()> {
    let m = Mesh::open()?;
    let find = |name: &str| -> Result<Group> {
        m.store
            .groups()?
            .into_iter()
            .find(|g| g.name == name)
            .ok_or_else(|| CommonError::invalid(format!("no group named `{name}`")).with_hint("common groups list"))
    };
    match cmd {
        GroupsCmd::List => {
            let gs = m.store.groups()?;
            if json {
                println!("{}", serde_json::to_string_pretty(&gs)?);
                return Ok(());
            }
            for g in &gs {
                println!(
                    "{:<16} remote={} pooled={} members={}",
                    g.name,
                    g.policy.allow_remote_inference,
                    g.policy.allow_pooled_inference,
                    g.members.iter().map(|p| p.short_label()).collect::<Vec<_>>().join(",")
                );
            }
            if gs.is_empty() {
                println!(
                    "no groups; `common groups create <name> --allow-remote` then `common groups add <name> <peer>`"
                );
            }
            Ok(())
        }
        GroupsCmd::Create {
            name,
            allow_remote,
            allow_pooled,
        } => {
            let g = Group {
                name: name.clone(),
                members: vec![],
                policy: GroupPolicy {
                    allow_remote_inference: *allow_remote,
                    allow_pooled_inference: *allow_pooled,
                },
            };
            save_group(&m, &g).await?;
            out(
                json,
                &g,
                format!("created group {name} (remote={allow_remote}, pooled={allow_pooled})"),
            )
        }
        GroupsCmd::Add { group, peer } => {
            let rec = m.resolve_peer(peer)?;
            let mut g = find(group)?;
            if !g.members.contains(&rec.id) {
                g.members.push(rec.id);
            }
            save_group(&m, &g).await?;
            out(json, &g, format!("added {} to {group}", rec.id.short_label()))
        }
        GroupsCmd::Remove { group, peer } => {
            let mut g = find(group)?;
            let id = member_by_prefix(&g.members, peer, group)?;
            g.members.retain(|p| *p != id);
            save_group(&m, &g).await?;
            out(json, &g, format!("removed {} from {group}", id.short_label()))
        }
        GroupsCmd::Policy {
            group,
            allow_remote,
            allow_pooled,
        } => {
            let mut g = find(group)?;
            if let Some(v) = allow_remote {
                g.policy.allow_remote_inference = *v;
            }
            if let Some(v) = allow_pooled {
                g.policy.allow_pooled_inference = *v;
            }
            save_group(&m, &g).await?;
            out(
                json,
                &g,
                format!(
                    "{group}: remote={} pooled={}",
                    g.policy.allow_remote_inference, g.policy.allow_pooled_inference
                ),
            )
        }
    }
}

pub async fn privacy(a: &PrivacyArgs, json: bool) -> Result<()> {
    let paths = Paths::from_env();
    let mut s = load_settings(&paths)?;
    let Some(mode) = a.mode else {
        return out(
            json,
            &s,
            format!(
                "privacy {} (network {}, serving peers {})",
                privacy_str(&s),
                if s.network_enabled { "on" } else { "off" },
                if s.serve_peers { "on" } else { "off" }
            ),
        );
    };
    match mode {
        PrivacyChoice::LocalOnly => s.privacy = PrivacyMode::LocalOnly,
        PrivacyChoice::TrustedDevices => {
            if s.privacy != PrivacyMode::TrustedGroup {
                eprintln!("{DISCLOSURE}");
                if !a.yes {
                    if !std::io::stdin().is_terminal() {
                        return Err(
                            CommonError::invalid("switching away from local-only needs confirmation")
                                .with_hint("re-run with --yes after reading the disclosure above"),
                        );
                    }
                    eprint!("Allow requests to run on paired devices? [y/N] ");
                    std::io::stderr().flush()?;
                    let mut line = String::new();
                    std::io::stdin().lock().read_line(&mut line)?;
                    if !matches!(line.trim(), "y" | "Y" | "yes") {
                        eprintln!("unchanged");
                        return Ok(());
                    }
                }
            }
            s.privacy = PrivacyMode::TrustedGroup;
            s.network_enabled = true;
        }
    }
    match Daemon::find(&paths).await? {
        // The disclosure was shown and accepted above; the daemon persists config.json itself.
        Some(d) => {
            d.call(
                Method::PUT,
                "/api/privacy",
                Some(json!({ "mode": s.privacy, "acknowledged": true })),
            )
            .await?;
            if mode == PrivacyChoice::TrustedDevices {
                d.call(Method::PUT, "/api/network", Some(json!({ "enabled": true })))
                    .await?;
            }
            let saved = load_settings(&paths)?;
            d.verify(saved.privacy == s.privacy)?;
            s = saved;
        }
        None => s.save(&paths)?,
    }
    out(json, &s, format!("privacy {}", privacy_str(&s)))
}

pub fn audit(a: &AuditArgs, json: bool) -> Result<()> {
    let events = Audit::open(&Paths::from_env())?.tail(a.n)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&events)?);
        return Ok(());
    }
    for e in &events {
        println!(
            "{} {:<20} {:<14} {}",
            e.ts_utc,
            e.kind,
            e.peer_id
                .as_deref()
                .and_then(|p| PeerId::parse_hex(p).ok())
                .map(|p| p.short_label())
                .unwrap_or_else(|| "-".into()),
            e.fields
        );
    }
    Ok(())
}

pub fn diagnostics(a: &DiagnosticsArgs) -> Result<()> {
    let paths = Paths::from_env();
    let settings = load_settings(&paths)?;
    let bundle = common_observability::bundle(&paths, serde_json::json!({ "settings": settings }))?;
    let text = serde_json::to_string_pretty(&bundle)?;
    match &a.out {
        Some(p) => {
            std::fs::write(p, text)?;
            eprintln!("wrote redacted diagnostics bundle to {}", p.display());
        }
        None => println!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> Paths {
        let dir = std::env::temp_dir().join(format!(
            "common-cli-mesh-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Paths::at(dir)
    }

    /// No `config.json` is the secure default (local-only, nothing listens, nothing served); the
    /// API's file shape is read as-is and a corrupt file is an error, never silently the default.
    #[test]
    fn settings_default_deny_and_privacy_str() {
        let paths = temp();
        let s = load_settings(&paths).unwrap();
        assert_eq!(s.privacy, PrivacyMode::LocalOnly);
        assert!(!s.network_enabled && !s.serve_peers);
        assert_eq!(privacy_str(&s), "local_only");
        std::fs::write(
            Settings::path(&paths),
            br#"{"privacy":"trusted_group","network_enabled":true,"serve_peers":false}"#,
        )
        .unwrap();
        let s = load_settings(&paths).unwrap();
        assert_eq!(
            (s.privacy, s.network_enabled, s.serve_peers),
            (PrivacyMode::TrustedGroup, true, false)
        );
        assert_eq!(privacy_str(&s), "trusted_group");
        std::fs::write(Settings::path(&paths), b"{not json").unwrap();
        assert_eq!(load_settings(&paths).unwrap_err().code, ErrorCode::Io);
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn resolve_peer_by_id_label_prefix_revoked_and_unknown() {
        let paths = temp();
        paths.ensure_dirs().unwrap();
        let m = Mesh {
            settings: Settings::default(),
            identity: Arc::new(DeviceIdentity::generate().unwrap()),
            store: Arc::new(PeerStore::open(&paths).unwrap()),
            audit: Arc::new(Audit::open(&paths).unwrap()),
            paths: paths.clone(),
        };
        let other = DeviceIdentity::generate().unwrap();
        m.store
            .pin(PeerRecord {
                id: other.id,
                name: "laptop".into(),
                paired_at_utc: common_identity::utc_now(),
                last_seen_utc: None,
                last_addr: Some("10.0.0.2:8193".into()),
            })
            .unwrap();
        assert_eq!(m.resolve_peer(&other.id.to_hex()).unwrap().name, "laptop");
        assert_eq!(m.resolve_peer(&other.id.short_label()).unwrap().id, other.id);
        assert_eq!(
            m.resolve_peer(&other.id.to_hex()[..6].to_uppercase()).unwrap().id,
            other.id
        );
        assert_eq!(m.resolve_peer("zz").unwrap_err().code, ErrorCode::InvalidRequest);
        assert_eq!(
            m.resolve_peer("ffff-0000-1234").unwrap_err().code,
            ErrorCode::PeerUnauthorized
        );
        m.store.revoke(&other.id, &m.identity).unwrap();
        let e = m.resolve_peer(&other.id.short_label()).unwrap_err();
        assert_eq!(e.code, ErrorCode::PeerRevoked, "{e}");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    #[test]
    fn groups_remove_names_exactly_one_member() {
        let a = PeerId([0xa1; 32]);
        let mut k = [0xa1; 32];
        k[31] = 0;
        let b = PeerId(k);
        let c = PeerId([0xc3; 32]);
        let members = [a, b, c];
        // `a1` used to drop both a and b; now it is ambiguous and nothing is removed.
        let e = member_by_prefix(&members, "a1", "home").unwrap_err();
        assert!(e.message.contains("matches 2 members"), "{e}");
        assert_eq!(member_by_prefix(&members, &a.to_hex(), "home").unwrap(), a);
        assert_eq!(member_by_prefix(&members, &c.short_label(), "home").unwrap(), c);
        assert_eq!(member_by_prefix(&members, "C3C3", "home").unwrap(), c);
        assert!(member_by_prefix(&members, "ff", "home")
            .unwrap_err()
            .message
            .contains("not a member of home"));
        for bad in ["", "-", "zz"] {
            assert_eq!(
                member_by_prefix(&members, bad, "home").unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn daemon_errors_keep_their_code_message_and_hint() {
        for code in [
            ErrorCode::PeerRevoked,
            ErrorCode::Unauthorized,
            ErrorCode::InvalidRequest,
        ] {
            let e = CommonError::new(code, "why").with_hint("do this");
            let back = daemon_error(&e.to_openai_json(), e.http_status());
            assert_eq!(
                (back.code, back.message.as_str(), back.hint.as_deref()),
                (code, "why", Some("do this"))
            );
        }
        let e = daemon_error(&Value::Null, 502);
        assert_eq!(
            (e.code, e.message.as_str()),
            (ErrorCode::Internal, "`common serve` answered HTTP 502")
        );
    }

    /// One-thread HTTP server on 127.0.0.1 answering every request with `status` and `body`.
    fn canned(status: &'static str, body: String) -> SocketAddr {
        use std::io::Read as _;
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { return };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        addr
    }

    /// `serve.json` names the daemon; only one that answers with this device's id is used, a
    /// stale file or another home's daemon is ignored, and a key-protected one is an error.
    #[tokio::test]
    async fn daemon_is_found_only_when_it_answers_with_this_device_id() {
        let paths = temp();
        assert!(Daemon::find(&paths).await.unwrap().is_none(), "no serve.json");
        let me = DeviceIdentity::generate().unwrap().id;
        std::fs::create_dir_all(paths.identity()).unwrap();
        std::fs::write(paths.identity().join("device.pub"), format!("{me}\n")).unwrap();

        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        write_serve_file(&paths, closed).unwrap();
        assert!(
            Daemon::find(&paths).await.unwrap().is_none(),
            "stale file: nothing answers"
        );

        let other = canned("200 OK", json!({ "peer_id": PeerId([7; 32]).to_hex() }).to_string());
        write_serve_file(&paths, other).unwrap();
        assert!(Daemon::find(&paths).await.unwrap().is_none(), "another home's daemon");

        let ours = canned("200 OK", json!({ "peer_id": me.to_hex() }).to_string());
        write_serve_file(&paths, SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), ours.port())).unwrap();
        let file: Value = serde_json::from_slice(&std::fs::read(serve_file(&paths)).unwrap()).unwrap();
        assert_eq!(file["addr"], ours.to_string(), "0.0.0.0 is recorded as loopback");
        let d = Daemon::find(&paths).await.unwrap().expect("our daemon");
        assert_eq!(d.url, format!("http://{ours}"));
        assert!(d.verify(true).is_ok());
        assert_eq!(d.verify(false).unwrap_err().code, ErrorCode::Internal);
        assert_eq!(
            refuse_if_daemon(&paths).await.unwrap_err().code,
            ErrorCode::InvalidRequest
        );

        let keyed = canned(
            "401 Unauthorized",
            CommonError::new(ErrorCode::Unauthorized, "missing or invalid API key")
                .to_openai_json()
                .to_string(),
        );
        write_serve_file(&paths, keyed).unwrap();
        let e = Daemon::find(&paths).await.err().expect("unauthorized");
        assert_eq!(e.code, ErrorCode::Unauthorized);
        assert!(e.hint.unwrap().contains("COMMON_API_KEY"));

        // Removed only while it names this process.
        remove_serve_file(&paths);
        assert!(!serve_file(&paths).exists());
        std::fs::write(serve_file(&paths), br#"{"addr":"127.0.0.1:1","pid":1}"#).unwrap();
        remove_serve_file(&paths);
        assert!(serve_file(&paths).exists(), "another daemon's file is left alone");
        let _ = std::fs::remove_dir_all(&paths.root);
    }

    /// `peers list` next to a daemon: `connected`/`health` come from its answer; a discovered device
    /// is listed only when it is neither pinned here nor this device.
    #[test]
    fn live_peers_merge_state_and_list_only_unpaired_strangers() {
        let me = PeerId([1; 32]);
        let (a, b, stranger) = (PeerId([0xa0; 32]), PeerId([0xb0; 32]), PeerId([0xc0; 32]));
        let row = |id: PeerId| PeerRow {
            id,
            label: id.short_label(),
            name: "x".into(),
            groups: vec![],
            paired_at_utc: "t".into(),
            last_seen_utc: None,
            last_addr: None,
            connected: None,
            health: None,
        };
        let mut rows = vec![row(a), row(b)];
        let seen = |id: PeerId, trusted: bool| {
            json!({ "id": id, "addr": "192.168.1.5:8193", "pairing": false, "pair_port": null,
                    "proto": "1", "trusted": trusted, "seen_utc": "t" })
        };
        let live = json!({
            "peers": [{ "id": a, "connected": true, "health": "unhealthy" }, { "id": b, "connected": false }],
            // b is pinned here though the daemon's flag says otherwise: still not a stranger.
            "discovered": [seen(a, true), seen(me, false), seen(stranger, false), seen(b, false)],
        });
        let found = apply_live(&mut rows, &live, &me).unwrap();
        assert_eq!(
            (rows[0].connected, rows[0].health.as_deref()),
            (Some(true), Some("unhealthy"))
        );
        assert_eq!((rows[1].connected, rows[1].health.as_deref()), (Some(false), None));
        assert_eq!(found.iter().map(|p| p.id).collect::<Vec<_>>(), vec![stranger]);
        // No `discovered`, a pinned device the daemon does not list: not connected, nothing found.
        let found = apply_live(&mut rows, &json!({ "peers": [] }), &me).unwrap();
        assert!(found.is_empty() && rows.iter().all(|r| r.connected == Some(false) && r.health.is_none()));
        // A malformed answer is an error, never a silently empty list.
        assert!(apply_live(&mut rows, &json!({ "discovered": [{ "id": 3 }] }), &me).is_err());
    }

    #[test]
    fn routable_keeps_specific_addresses_and_device_name_is_nonempty() {
        let a: SocketAddr = "127.0.0.1:5".parse().unwrap();
        assert_eq!(routable(a), a);
        let r = routable("0.0.0.0:7".parse().unwrap());
        assert_eq!(r.port(), 7);
        assert!(!device_name().is_empty());
    }
}
