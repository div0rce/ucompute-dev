//! Common Mesh, CLI side (F-02-*, F-06-05, F-06-02, F-10-10): settings, peers, groups, privacy,
//! audit, diagnostics. The daemon side (Node, discovery, inbound serving) lives in
//! `common_api::mesh` and runs inside `common serve`.
//!
//! Settings are the API's `common_api::mesh::Settings` in `COMMON_HOME/config.json`
//! (`{privacy, network_enabled, serve_peers}`, 0600). Pinned peers, groups and revocations live in
//! `identity/peers.json` (`common_identity::PeerStore`). Every command here is a separate process
//! that reads both at start; a running `common serve` keeps its own copy, so changes made while it
//! runs apply when it restarts (said in the output, not hidden).

use crate::shutdown_signal;
use clap::{Args, Subcommand, ValueEnum};
use common_artifacts::Store;
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_identity::{DeviceIdentity, Group, GroupPolicy, PeerId, PeerRecord, PeerStore};
use common_network::discovery::{Advertiser, Browser};
use common_network::transport::Node;
use common_network::{pairing, NetworkConfig};
use common_observability::Audit;
use common_policy::{PolicyConfig, PolicyEngine, PrivacyMode};
use serde::Serialize;
use std::io::{BufRead, IsTerminal, Write};
use std::net::{IpAddr, SocketAddr};
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
    /// Requests never leave this device (default)
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
        let key = s.trim().to_ascii_lowercase().replace('-', "");
        if key.is_empty() || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(CommonError::invalid(format!(
                "`{s}` is not a peer id (64 hex) or a short label like ab12-cd34-ef56"
            )));
        }
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
}

/// This host's name, what a paired device shows for us.
pub fn device_name() -> String {
    #[cfg(unix)]
    {
        let mut buf = [0u8; 256];
        // SAFETY: gethostname writes at most buf.len() bytes into a buffer we own.
        if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0 {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            if let Ok(s) = std::str::from_utf8(&buf[..end]) {
                if !s.trim().is_empty() {
                    return s.trim().to_string();
                }
            }
        }
    }
    "common".into()
}

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
        PeersCmd::List => peers_list(&m, json),
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
        PeersCmd::Pair { name, net } => pair(&m, name.clone(), net, json).await,
        PeersCmd::Join {
            target,
            code,
            name,
            net,
        } => join(&m, target, code.as_deref(), name.clone(), net, json).await,
        PeersCmd::Rm { id } => {
            let rec = m.resolve_peer(id)?;
            m.store.remove(&rec.id)?;
            out(
                json,
                &serde_json::json!({ "id": rec.id, "removed": true }),
                format!("forgot {} ({})", rec.name, rec.id.short_label()),
            )
        }
        PeersCmd::Revoke { id } => {
            let rec = m.resolve_peer(id)?;
            let r = m.store.revoke(&rec.id, &m.identity)?;
            if let Err(e) = m.audit.record(
                "peer_revoked",
                Some(&rec.id.to_hex()),
                serde_json::json!({ "seq": r.seq, "peer": rec.id.short_label() }),
            ) {
                tracing::debug!(error = %e, "audit record not written");
            }
            out(
                json,
                &r,
                format!(
                    "revoked {} ({}); its key is refused at every later handshake. A running `common serve` applies this when it restarts.",
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
}

// ponytail: store only; "connected" lives in the daemon's process. Upgrade: ask the API's /api/peers.
fn peers_list(m: &Mesh, json: bool) -> Result<()> {
    let rows: Vec<PeerRow> = m
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
            })
        })
        .collect::<Result<_>>()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!("this device {} ({})", m.identity.id, m.identity.id.short_label());
    println!(
        "{:<14} {:<20} {:<16} {:<22} {:<20} ID",
        "LABEL", "NAME", "GROUPS", "ADDRESS", "LAST SEEN"
    );
    for r in &rows {
        println!(
            "{:<14} {:<20} {:<16} {:<22} {:<20} {}",
            r.label,
            r.name.chars().take(20).collect::<String>(),
            r.groups.join(","),
            r.last_addr.as_deref().unwrap_or("-"),
            r.last_seen_utc.as_deref().unwrap_or("-"),
            r.id
        );
    }
    if rows.is_empty() {
        println!("no paired devices; run `common peers pair` here and `common peers join` on the other device");
    }
    Ok(())
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
    let node = m.node(net.config(), false).await?;
    let name = name.unwrap_or_else(device_name);
    let r = tokio::select! {
        r = pairing::join(node.clone(), addr, &pw, &name) => r,
        _ = shutdown_signal() => Err(CommonError::new(ErrorCode::Cancelled, "pairing cancelled")),
    };
    node.shutdown().await;
    let rec = r?;
    if expected.is_some_and(|e| e != rec.id) {
        m.store.remove(&rec.id)?;
        return Err(CommonError::new(
            ErrorCode::PairingFailed,
            "the device that answered is not the one named in the QR code; nothing was pinned",
        ));
    }
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

pub fn groups(cmd: &GroupsCmd, json: bool) -> Result<()> {
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
            m.store.upsert_group(g.clone())?;
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
            m.store.upsert_group(g.clone())?;
            out(json, &g, format!("added {} to {group}", rec.id.short_label()))
        }
        GroupsCmd::Remove { group, peer } => {
            let mut g = find(group)?;
            let key = peer.trim().to_ascii_lowercase().replace('-', "");
            let before = g.members.len();
            g.members.retain(|p| !p.to_hex().starts_with(&key) || key.is_empty());
            if g.members.len() == before {
                return Err(CommonError::invalid(format!("`{peer}` is not a member of {group}")));
            }
            m.store.upsert_group(g.clone())?;
            out(json, &g, format!("removed {peer} from {group}"))
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
            m.store.upsert_group(g.clone())?;
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

pub fn privacy(a: &PrivacyArgs, json: bool) -> Result<()> {
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
    s.save(&paths)?;
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
    fn routable_keeps_specific_addresses_and_device_name_is_nonempty() {
        let a: SocketAddr = "127.0.0.1:5".parse().unwrap();
        assert_eq!(routable(a), a);
        let r = routable("0.0.0.0:7".parse().unwrap());
        assert_eq!(r.port(), 7);
        assert!(!device_name().is_empty());
    }
}
