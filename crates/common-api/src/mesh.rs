//! Common Mesh in the daemon API (F-02-01/02/05, F-06-02, F-06-05, F-06-07, F-06-10; the backends
//! of the desktop pairing UI F-10-05 and privacy selector F-10-10).
//!
//! Secure defaults, persisted in `COMMON_HOME/config.json` (0600): privacy `local_only`, peer
//! networking off, serving peers off. Nothing binds the peer endpoint until
//! `PUT /api/network {"enabled": true}`; `PUT /api/privacy` to a non-local mode returns
//! [`DISCLOSURE`] until the caller acknowledges it; `swarm_allowed` is refused until Gate F.
//! Every off-device action is audited with ids, counts and codes only.
//!
//! `config.json` is re-read whenever it changes on disk, and `PUT /api/network|privacy` are
//! read-modify-writes of the file, so `common privacy local-only` from another process applies to
//! the next request and is never written back over.
//!
//! Connections: the inbound accept loop and every outbound connect attach the same two tasks to a
//! `PeerConnection`, once per connection: `remote::serve_peer_with` (default deny per request;
//! [`PeerQuota`] token buckets; a peer takes a free engine slot of the local admission or gets
//! `overloaded`, never a place in the local queue) and [`control_loop`], which offers this
//! device's revocations to peers that shared a group with the revoked device and applies (and
//! passes on) the peer's (ADR-0003 D7). While networking is on, [`sync_loop`] keeps a connection
//! to every pinned device (1, 2, 4 … 30 s backoff), so a revocation reaches members that are not
//! mid-request and offline members converge when they return. Turning networking off also closes
//! an open pairing window.

use crate::{ApiError, AppState, Json};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_identity::{utc_now, DeviceIdentity, Group, PeerId, PeerRecord, Revocation};
use common_network::discovery::{Advertiser, Browser, DiscoveredPeer, MAX_RECORD_AGE_SECS};
use common_network::health::{HealthTracker, PeerHealth};
use common_network::pairing;
use common_network::proto::{control_message::Body, read_frame, write_frame, ControlMessage, RevocationMsg};
use common_network::transport::{ControlStream, Node, PeerConnection};
use common_network::{remote, NetworkConfig, MAX_CONTROL_FRAME};
use common_observability::Audit;
use common_policy::{PolicyConfig, PolicyEngine, PrivacyMode, QuotaConfig, Quotas};
use common_runtime::{GenerateEvent, GenerateRequest, GenerateStream};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{ready, Context, Poll};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// Shown before a non-local privacy mode can be selected (F-10-10, THREAT_MODEL §4). Returned by
/// `GET /api/privacy` and, as the error message, by an unacknowledged `PUT /api/privacy`.
pub const DISCLOSURE: &str =
    "Trusted devices: a request you send to a paired device travels encrypted, but that device \
decrypts it and runs the model on your prompt in plain text inside its own process. It can read, log or keep what you \
send and what the model answers; Common cannot prevent that. Only \"local only\" keeps everything on this computer. \
Pair only devices you trust with your prompts.";

const SETTINGS_FILE: &str = "config.json";
const DEFAULT_DEVICE_NAME: &str = "Common device";
/// How often [`sync_loop`] looks for pinned devices without a connection.
const SYNC_TICK: Duration = Duration::from_secs(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// User settings behind `PUT /api/network` and `PUT /api/privacy`. Every default is the safe one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Settings {
    #[serde(default)]
    pub privacy: PrivacyMode,
    #[serde(default)]
    pub network_enabled: bool,
    #[serde(default)]
    pub serve_peers: bool,
    /// Limits on what paired devices may run here (F-06-04); read once when the daemon starts.
    #[serde(default)]
    pub peer_quota: PeerQuota,
}

/// `config.json` `"peer_quota"`: per-peer token buckets for requests served to paired devices.
/// Every field is optional; 0 means unlimited for the two rates. Defaults are
/// `common_policy::QuotaConfig::default()`. A request over a limit gets `overloaded` (429 at the
/// sending device's API), never a queue slot: a peer takes a free engine slot or is refused, a
/// slot that frees up goes to a waiting local request first, and peers together never hold the
/// last engine slot ([`PeerQuota::peer_slots`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PeerQuota {
    /// Requests one device may start per minute (burst up to the same number).
    pub requests_per_minute: u32,
    /// Generated tokens one device may use per minute; a request's whole `max_tokens` is charged
    /// when it starts.
    pub tokens_per_minute: u64,
    /// Generations one device may run here at once; also the cap across all devices together,
    /// which is further capped at the engine's slots minus one.
    pub max_concurrent: u32,
}

impl Default for PeerQuota {
    fn default() -> Self {
        let q = QuotaConfig::default();
        Self {
            requests_per_minute: q.requests_per_minute,
            tokens_per_minute: q.tokens_per_minute,
            max_concurrent: q.max_concurrent,
        }
    }
}

impl PeerQuota {
    /// Peer generations this daemon runs at once across all peers: `max_concurrent` (at least 1)
    /// but at most `engine_slots - 1`, so one engine slot always stays with local work and a peer
    /// generation (up to the peer's `max_tokens`, minutes) can never make local requests time out
    /// in the queue (F-06-04). A single-slot engine (`--max-concurrent 1`) serves no peers.
    // ponytail: a fixed reservation. Preempting a peer generation when a local request queues
    // would let a single-slot device serve peers too; that needs a cancel hook in common-network.
    pub(crate) fn peer_slots(&self, engine_slots: usize) -> usize {
        (self.max_concurrent.max(1) as usize).min(engine_slots.saturating_sub(1))
    }

    /// The daemon's peer-serving options: these quotas, the API's load options (a peer request
    /// never forces a reload) and its admission semaphore (`engine_slot_count` permits) as the
    /// engine slots (ADR-0003 D5), peers capped at [`PeerQuota::peer_slots`].
    pub(crate) fn serve_options(
        self,
        load: common_runtime::manager::LoadOptions,
        engine_slots: Arc<tokio::sync::Semaphore>,
        engine_slot_count: usize,
    ) -> remote::ServeOptions {
        remote::ServeOptions {
            load,
            quotas: Arc::new(Quotas::new(QuotaConfig {
                requests_per_minute: self.requests_per_minute,
                tokens_per_minute: self.tokens_per_minute,
                bytes_per_second: 0,
                max_concurrent: self.max_concurrent,
            })),
            engine_slots: Some(engine_slots),
            peer_slots: Arc::new(tokio::sync::Semaphore::new(self.peer_slots(engine_slot_count))),
        }
    }
}

impl Settings {
    pub fn path(paths: &Paths) -> PathBuf {
        paths.root.join(SETTINGS_FILE)
    }

    /// `None` when no file exists yet. A corrupt file is an `Io` error, never silently the default.
    pub fn load(paths: &Paths) -> Result<Option<Settings>> {
        let path = Self::path(paths);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|e| CommonError::io(format!("{} is not valid: {e}", path.display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        std::fs::create_dir_all(&paths.root)?;
        write_private(&Self::path(paths), &serde_json::to_vec_pretty(self)?)
    }
}

/// `(mtime, len)` of `config.json`, `None` when absent: the change detector for [`SettingsCell`].
fn file_stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(path)
        .ok()
        .map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

/// The settings as last read from `config.json`, and that file's stamp at the time.
pub(crate) struct SettingsCell {
    value: Settings,
    stamp: Option<(SystemTime, u64)>,
}

impl SettingsCell {
    /// The file, or (no file yet) the defaults with `network.enabled` from the API config.
    pub(crate) fn load(paths: &Paths, config: &crate::ApiConfig) -> Result<SettingsCell> {
        // Stamp first: a write between the two reads is then seen as a change on the next read.
        let stamp = file_stamp(&Settings::path(paths));
        let value = Settings::load(paths)?.unwrap_or_else(|| seed(config));
        Ok(SettingsCell { value, stamp })
    }

    pub(crate) fn peer_quota(&self) -> PeerQuota {
        self.value.peer_quota
    }
}

/// What applies while no `config.json` exists.
fn seed(config: &crate::ApiConfig) -> Settings {
    Settings {
        network_enabled: config.network.enabled,
        ..Default::default()
    }
}

/// `<path>.tmp` + rename, mode 0600 (the identity crate's discipline; its helper is private).
fn write_private(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_extension("json.tmp");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// The peer Node and what hangs off it. `node` is `Some` only while networking is enabled.
#[derive(Default)]
pub struct Net {
    pub node: Option<Arc<Node>>,
    cfg: Option<NetworkConfig>,
    advertiser: Option<Advertiser>,
    browser: Option<Browser>,
    accept: Option<tokio::task::JoinHandle<()>>,
    sync: Option<tokio::task::JoinHandle<()>>,
}

impl Net {
    async fn stop(&mut self) {
        for t in [self.accept.take(), self.sync.take()].into_iter().flatten() {
            t.abort();
        }
        self.advertiser = None;
        self.browser = None;
        if let Some(node) = self.node.take() {
            node.shutdown().await;
        }
        self.cfg = None;
    }

    /// Re-announce the mDNS record with (`pair=1,pairport=`) or without a pairing port.
    fn advertise(&mut self, identity: Arc<DeviceIdentity>, pairing_port: Option<u16>) -> Result<()> {
        let (Some(node), Some(cfg)) = (&self.node, &self.cfg) else {
            return Ok(());
        };
        self.advertiser = None; // goodbye for the old record before the new one is registered
        self.advertiser = Some(Advertiser::start(
            cfg,
            identity,
            node.local_addr()?.port(),
            pairing_port,
        )?);
        Ok(())
    }

    pub fn discovered(&self) -> Vec<DiscoveredPeer> {
        self.browser
            .as_ref()
            .map_or_else(Vec::new, |b| b.peers(MAX_RECORD_AGE_SECS))
    }

    fn mdns_active(&self) -> bool {
        self.advertiser.as_ref().is_some_and(Advertiser::active)
    }
}

/// One pairing window (ADR-0003 D2). `result` is filled when the initiator's task ends.
pub struct Pairing {
    pub port: u16,
    pub expires_at: u64,
    result: Arc<Mutex<Option<Result<PeerRecord>>>>,
    /// Aborts `pairing::start`'s task; dropping it drops (and so closes) the pairing endpoint.
    abort: tokio::task::AbortHandle,
    /// The task that records `result`; awaited by [`cancel_pairing`].
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Pairing {
    fn active(&self) -> bool {
        lock(&self.result).is_none()
    }
}

/// Close an open pairing window (its endpoint goes with the aborted task, so nothing listens
/// and nothing can be pinned afterwards) and wait until its result is recorded (`cancelled`).
async fn cancel_pairing(slot: &mut Option<Pairing>) {
    if let Some(p) = slot.as_mut() {
        p.abort.abort();
        if let Some(t) = p.task.take() {
            let _ = t.await;
        }
    }
}

impl AppState {
    /// The current settings: `config.json` as it is now (re-read when its mtime or size changed,
    /// so another process's `common privacy local-only` applies to the next request). A file that
    /// no longer parses keeps the rest but forces `local_only` (fail closed, logged).
    pub fn settings(&self) -> Settings {
        self.fresh_settings(&mut lock(&self.settings), false)
    }

    fn fresh_settings(&self, cell: &mut SettingsCell, force: bool) -> Settings {
        let stamp = file_stamp(&Settings::path(&self.paths));
        if force || stamp != cell.stamp {
            cell.value = match Settings::load(&self.paths) {
                Ok(Some(s)) => s,
                Ok(None) => seed(&self.config),
                Err(e) => {
                    tracing::warn!(error = %e, "settings file unreadable; privacy forced to local_only");
                    Settings {
                        privacy: PrivacyMode::LocalOnly,
                        ..cell.value
                    }
                }
            };
            cell.stamp = stamp;
        }
        cell.value
    }

    /// Replace the in-memory settings until `config.json` next changes (not persisted;
    /// `serve_on` uses it to show networking off when the endpoint could not start).
    pub fn set_settings(&self, s: Settings) {
        lock(&self.settings).value = s;
    }

    /// Read-modify-write of `config.json` under the settings lock: `f` sees the file as it is now
    /// (never an in-memory override), and a changed result is saved before anything uses it.
    /// Returns `(before, after)`.
    // ponytail: in-process lock only; a CLI write landing between this read and save (same
    // millisecond) is lost. A flock on config.json is the upgrade if that ever matters.
    fn update_settings(&self, f: impl FnOnce(Settings) -> Settings) -> Result<(Settings, Settings)> {
        let mut cell = lock(&self.settings);
        let before = self.fresh_settings(&mut cell, true);
        let after = f(before);
        if after != before {
            after.save(&self.paths)?;
            cell.value = after;
            cell.stamp = file_stamp(&Settings::path(&self.paths));
        }
        Ok((before, after))
    }

    /// The policy engine for the current settings. Built per call (two bools and an `Arc` clone),
    /// so there is one source of truth; swarm can never be enabled in M3.
    pub fn policy(&self) -> Arc<PolicyEngine> {
        let s = self.settings();
        Arc::new(PolicyEngine::new(
            PolicyConfig {
                default_privacy: s.privacy,
                network_enabled: s.network_enabled,
                serve_peers: s.serve_peers,
                swarm_enabled: false,
            },
            self.peers.clone(),
        ))
    }

    /// Append an audit line; a failure is logged at warn, never fatal for the request. (A request
    /// that leaves the device is refused instead when its entry cannot be written; see
    /// [`generate_remote_audited`].)
    pub fn audit(&self, kind: &str, peer: Option<&PeerId>, fields: Value) {
        let hex = peer.map(PeerId::to_hex);
        if let Err(e) = self.audit.record(kind, hex.as_deref(), fields) {
            tracing::warn!(kind, error = %e, "audit record not written");
        }
    }

    /// Close any pairing window, stop whatever runs, then start the Node, discovery, the accept
    /// loop and the sync loop when `settings().network_enabled`. `Node::start` refuses unless the
    /// config says enabled, so a disabled setting can never leave an endpoint bound.
    pub async fn apply_network(self: &Arc<Self>) -> Result<()> {
        let mut pairing = self.pairing.lock().await;
        cancel_pairing(&mut pairing).await;
        let s = self.settings();
        let mut net = self.net.lock().await;
        net.stop().await;
        if !s.network_enabled {
            return Ok(());
        }
        let cfg = NetworkConfig {
            enabled: true,
            ..self.config.network.clone()
        };
        let node = Node::start(
            cfg.clone(),
            self.identity.clone(),
            self.peers.clone(),
            self.policy(),
            self.audit.clone(),
        )
        .await?;
        *lock(&node.cached_models) = self
            .store
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.sha256)
            .collect();
        let port = node.local_addr()?.port();
        net.advertiser = Some(Advertiser::start(&cfg, self.identity.clone(), port, None)?);
        net.browser = Some(Browser::start_excluding(&cfg, self.peers.clone(), self.identity.id)?);
        net.accept = Some(tokio::spawn(accept_loop(self.clone(), node.clone())));
        net.sync = Some(tokio::spawn(sync_loop(self.clone(), node.clone())));
        net.cfg = Some(cfg);
        net.node = Some(node);
        Ok(())
    }

    /// Close any pairing window and the peer endpoint; afterwards no socket of the mesh is open.
    pub async fn stop_network(&self) {
        let mut pairing = self.pairing.lock().await;
        cancel_pairing(&mut pairing).await;
        self.net.lock().await.stop().await;
    }

    /// The running Node, or `PolicyDenied` while peer networking is off.
    pub async fn node(&self) -> Result<Arc<Node>> {
        self.net.lock().await.node.clone().ok_or_else(|| {
            CommonError::new(ErrorCode::PolicyDenied, "peer networking is disabled in settings")
                .with_hint("PUT /api/network {\"enabled\": true}")
        })
    }

    /// A peer named by its 64-hex id or the `xxxx-xxxx-xxxx` short label of a pinned peer.
    pub fn resolve_peer(&self, s: &str) -> Result<PeerId> {
        let s = s.trim();
        if let Ok(id) = PeerId::parse_hex(s) {
            return Ok(id);
        }
        let label = short_label(s).ok_or_else(|| {
            CommonError::invalid("peer id must be 64 hex characters or a short label like 1a2b-3c4d-5e6f")
        })?;
        let hits: Vec<PeerId> = self
            .peers
            .list()?
            .into_iter()
            .filter(|p| p.id.short_label() == label)
            .map(|p| p.id)
            .collect();
        match hits.as_slice() {
            [id] => Ok(*id),
            [] => Err(
                CommonError::new(ErrorCode::PeerUnauthorized, format!("no paired device {label}"))
                    .with_hint("GET /api/peers"),
            ),
            _ => Err(CommonError::invalid(format!(
                "short label {label} matches more than one paired device; use the full id"
            ))),
        }
    }

    /// The hash a peer must have for `model_id`: the local manifest, else the catalog entry.
    /// No pinned hash means no request; the peer refuses any other file (F-02-10).
    pub fn model_sha256(&self, model_id: &str) -> Result<String> {
        if let Some(m) = self.store.get(model_id)? {
            return Ok(m.sha256);
        }
        self.catalog.get(model_id).map(|e| e.sha256.clone()).ok_or_else(|| {
            CommonError::new(
                ErrorCode::ModelNotFound,
                format!("no model named {model_id} in the catalog or the local store"),
            )
            .with_hint("GET /v1/models")
        })
    }

    /// An authenticated connection to `id`: the open one when there is one (the [`survivor`] of a
    /// crossed dial, not the duplicate about to close), else a new connect to the address mDNS
    /// sees now, then the pinned `last_addr` (see [`dial_order`]). `last_seen_utc` is refreshed,
    /// and `last_addr` only when this dial's handshake to it is the connection returned. Typed
    /// errors from `Node::connect`; an unreachable address falls through to the next one, a
    /// refusal (revoked, unpinned) does not.
    pub async fn peer_connection(self: &Arc<Self>, node: &Arc<Node>, id: PeerId) -> Result<Arc<PeerConnection>> {
        let open: Vec<Arc<PeerConnection>> = node.connections().into_iter().filter(|c| c.peer_id == id).collect();
        let flags: Vec<bool> = open.iter().map(|c| c.outbound).collect();
        if let Some(i) = survivor(self.identity.id < id, &flags) {
            return Ok(open[i].clone());
        }
        let discovered = self
            .net
            .lock()
            .await
            .discovered()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.addr);
        let pinned = self
            .peers
            .get(&id)?
            .and_then(|p| p.last_addr)
            .and_then(|a| a.parse().ok());
        let mut last = CommonError::new(
            ErrorCode::PeerLost,
            format!("no known address for device {}", id.short_label()),
        )
        .with_hint("check that it is awake and on the same network");
        for addr in dial_order(discovered, pinned) {
            match node.connect(addr, id).await {
                Ok(conn) => {
                    // `connect` may hand back a connection it did not dial here (one that
                    // registered meanwhile, or the survivor of a crossed dial): `addr` unproven.
                    let dialled = conn.outbound && same_addr(conn.remote_addr, addr);
                    self.touch(id, dialled.then_some(addr));
                    self.attach(node, conn.clone());
                    return Ok(conn);
                }
                Err(e) if matches!(e.code, ErrorCode::PeerLost | ErrorCode::Timeout) => last = e,
                Err(e) => return Err(e),
            }
        }
        Err(last)
    }

    /// Record a contact with a pinned peer: `last_seen_utc` always, `last_addr` only for an address
    /// this device dialled (an inbound source may be a short-lived client port such as
    /// `common run --peer`). Under `peer_writes`, so it never re-pins a device unpaired meanwhile;
    /// `PeerStore::pin` refuses a revoked one.
    fn touch(&self, id: PeerId, dialled: Option<SocketAddr>) {
        let _g = lock(&self.peer_writes);
        let Ok(Some(mut p)) = self.peers.get(&id) else {
            return;
        };
        if let Some(a) = dialled {
            p.last_addr = Some(a.to_string());
        }
        p.last_seen_utc = Some(utc_now());
        if let Err(e) = self.peers.pin(p) {
            tracing::debug!(peer = %id.short_label(), code = %e.code, "peer address not refreshed");
        }
    }

    /// Serve the peer's requests (default deny per request, this daemon's quotas and engine slots)
    /// and run the control loop; once per connection, however often it is handed out.
    fn attach(self: &Arc<Self>, node: &Arc<Node>, conn: Arc<PeerConnection>) {
        let id = conn.conn.stable_id();
        if !lock(&self.attached).insert(id) {
            return;
        }
        let (st, served) = (self.clone(), conn.clone());
        tokio::spawn(async move {
            let r = remote::serve_peer_with(
                served.clone(),
                st.manager.clone(),
                st.policy(),
                st.audit.clone(),
                st.peer_serve.clone(),
            )
            .await;
            if let Err(e) = r {
                tracing::info!(peer = %served.peer_id.short_label(), code = %e.code, "peer connection ended");
            }
            lock(&st.attached).remove(&id);
        });
        tokio::spawn(control_loop(self.clone(), node.clone(), conn));
    }

    /// The devices that share a group with `target` here, except `target` and this device: who
    /// a revocation of `target` is for. Taken before the revocation, which drops its memberships.
    fn revocation_audience(&self, target: &PeerId) -> HashSet<PeerId> {
        self.peers
            .groups_of(target)
            .unwrap_or_default()
            .into_iter()
            .flat_map(|g| g.members)
            .filter(|m| m != target && *m != self.identity.id)
            .collect()
    }

    /// Write `r` to `peer`'s control stream when it is due there ([`RevocationFanout::due`]);
    /// false when the stream is gone. A completed write is not a delivery (the frame is only
    /// buffered and may die with a duplicate or dropped connection, or the peer may refuse it for
    /// now), so nothing is marked: the next connection offers it again (ADR-0003 D7).
    async fn offer_revocation<W>(&self, send: &mut W, peer: PeerId, r: &Revocation) -> bool
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        let due = {
            let fanout = lock(&self.fanout);
            fanout.due(r, &peer, || self.peers.groups_of(&peer).is_ok_and(|g| !g.is_empty()))
        };
        !due || write_frame(send, &revocation_msg(r)).await.is_ok()
    }

    async fn network_status(&self) -> Value {
        let s = self.settings();
        let net = self.net.lock().await;
        let node = net.node.as_ref();
        json!({
            "enabled": node.is_some(),
            "serve_peers": s.serve_peers,
            "listen_addr": node.and_then(|n| n.local_addr().ok()).map(|a| a.to_string()),
            "mdns": net.mdns_active(),
            "peer_id": self.identity.id.to_hex(),
            "short_label": self.identity.id.short_label(),
            "connections": node.map_or(0, |n| n.connections().len()),
            "peer_quota": self.peer_quota,
            "peer_slots": self.peer_quota.peer_slots(self.admission.max_concurrent),
        })
    }
}

/// `xxxx-xxxx-xxxx` (12 hex in three groups), lowercased; `None` for anything else.
fn short_label(s: &str) -> Option<String> {
    let ok = s.len() == 14
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 9 {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        });
    ok.then(|| s.to_ascii_lowercase())
}

/// Where to dial a peer, best first: the address in a fresh signed mDNS record (it follows DHCP
/// and Wi-Fi changes), then the pinned `last_addr`; duplicates removed.
fn dial_order(discovered: Option<SocketAddr>, pinned: Option<SocketAddr>) -> Vec<SocketAddr> {
    let mut v: Vec<SocketAddr> = discovered.into_iter().chain(pinned).collect();
    v.dedup();
    v
}

/// Which of the open connections to one peer (their `outbound` flags, oldest first) to use: the
/// one initiated by the lower id, which `Node::register` keeps when dials cross, else the oldest.
// ponytail: mirrors common-network's private `transport::survivor`; call a public `Node::live`
// instead once that crate exposes one, so the two rules cannot drift.
fn survivor(me_lower: bool, outbound: &[bool]) -> Option<usize> {
    outbound
        .iter()
        .position(|o| *o == me_lower)
        .or((!outbound.is_empty()).then_some(0))
}

/// Same UDP address, an IPv4 address and its IPv4-mapped IPv6 form counting as one (a dual-stack
/// endpoint reports the mapped form).
fn same_addr(a: SocketAddr, b: SocketAddr) -> bool {
    a.port() == b.port() && a.ip().to_canonical() == b.ip().to_canonical()
}

async fn accept_loop(st: Arc<AppState>, node: Arc<Node>) {
    while let Some(conn) = node.accept().await {
        st.touch(conn.peer_id, None);
        st.attach(&node, conn);
    }
}

/// ADR-0003 D7: keep a connection (and so a control loop) to every pinned device while networking
/// is on. A device that could not be reached waits `HealthTracker::backoff_secs(failures - 1)`
/// (1, 2, 4 … 30 s) before the next dial; due dials run concurrently.
// ponytail: every failed dial is one `peer_rejected` audit line (written by `Node::connect`), so an
// offline paired device costs at most two lines a minute. Dial only devices mDNS sees, or let
// `Node::connect` skip auditing plain timeouts, if that ever bloats the audit log.
async fn sync_loop(st: Arc<AppState>, node: Arc<Node>) {
    let mut failures: HashMap<PeerId, (u32, Instant)> = HashMap::new();
    loop {
        let connected: HashSet<PeerId> = node.connections().iter().map(|c| c.peer_id).collect();
        let now = Instant::now();
        let due: Vec<PeerId> = st
            .peers
            .list()
            .unwrap_or_default()
            .into_iter()
            .map(|p| p.id)
            .filter(|id| !connected.contains(id) && failures.get(id).is_none_or(|(_, next)| *next <= now))
            .collect();
        let results = futures_util::future::join_all(due.iter().map(|id| st.peer_connection(&node, *id))).await;
        for (id, r) in due.into_iter().zip(results) {
            match r {
                Ok(_) => {
                    failures.remove(&id);
                }
                Err(_) => {
                    let n = failures.get(&id).map_or(0, |(n, _)| *n) + 1;
                    let wait = Duration::from_secs(HealthTracker::backoff_secs(n - 1));
                    failures.insert(id, (n, Instant::now() + wait));
                }
            }
        }
        tokio::time::sleep(SYNC_TICK).await;
    }
}

fn revocation_msg(r: &Revocation) -> ControlMessage {
    Body::Revocation(RevocationMsg {
        seq: r.seq,
        peer_id: r.peer_id.0.to_vec(),
        revoked_by: r.revoked_by.0.to_vec(),
        at_utc: r.at_utc.clone(),
        signature: hex::decode(&r.signature).unwrap_or_default(),
    })
    .into()
}

fn revocation_from_msg(m: RevocationMsg) -> Result<Revocation> {
    let id = |b: &[u8]| -> Result<PeerId> {
        b.try_into()
            .map(PeerId)
            .map_err(|_| CommonError::invalid("revocation peer id must be 32 bytes"))
    };
    if m.at_utc.len() > 64 || m.signature.len() != 64 {
        return Err(CommonError::invalid("malformed revocation"));
    }
    Ok(Revocation {
        seq: m.seq,
        peer_id: id(&m.peer_id)?,
        revoked_by: id(&m.revoked_by)?,
        at_utc: m.at_utc,
        signature: hex::encode(m.signature),
    })
}

/// ADR-0003 D7 fan-out, per revocation record: the devices it is for (those that shared a group
/// with the revoked device here when it was revoked or applied) and the devices that sent it to
/// this one. A control loop offers a record to its peer when the peer is in the audience and has
/// not sent it here, on every connection (at least once: a write is never taken as delivery); a
/// peer that already holds it ignores it without an audit line.
// ponytail: in memory. A record revoked before this daemon started (or by `common peers revoke`
// while no daemon ran) has no recorded audience; it is offered once per connection to any device
// in one of this device's groups. Persist this map next to peers.json if that over-sharing ever
// matters; an applied-ack from the peer would let a reconnect skip records it already holds.
#[derive(Default)]
pub(crate) struct RevocationFanout(HashMap<(PeerId, u64, PeerId), Fanout>);

#[derive(Default)]
struct Fanout {
    audience: Option<HashSet<PeerId>>,
    done: HashSet<PeerId>,
}

fn fanout_key(r: &Revocation) -> (PeerId, u64, PeerId) {
    (r.revoked_by, r.seq, r.peer_id)
}

impl RevocationFanout {
    pub(crate) fn set_audience(&mut self, r: &Revocation, audience: HashSet<PeerId>) {
        self.0.entry(fanout_key(r)).or_default().audience = Some(audience);
    }

    /// `peer` has `r`: it sent it here.
    pub(crate) fn done(&mut self, r: &Revocation, peer: PeerId) {
        self.0.entry(fanout_key(r)).or_default().done.insert(peer);
    }

    /// Should `r` go to `peer` now? Never to the revoked device itself or to a peer that has it;
    /// else to the recorded audience, or (none recorded) when `in_a_group()` says `peer` is in one
    /// of this device's groups.
    pub(crate) fn due(&self, r: &Revocation, peer: &PeerId, in_a_group: impl FnOnce() -> bool) -> bool {
        if *peer == r.peer_id {
            return false;
        }
        match self.0.get(&fanout_key(r)) {
            Some(f) if f.done.contains(peer) => false,
            Some(Fanout { audience: Some(a), .. }) => a.contains(peer),
            _ => in_a_group(),
        }
    }
}

/// A revocation from a peer: ignored when it names this device or is already held; otherwise
/// checked and stored by `PeerStore::apply_revocation` (issuer pinned and sharing a group with the
/// target, signature valid, not superseded by a later local re-pair, so a replay on reconnect
/// cannot undo that re-pair), the revoked device disconnected, audited once, and offered to the
/// connected peers that shared a group with the revoked device here (not back to `from`).
/// The held-check and the apply run under `peer_writes`, so two connections delivering the same
/// record at once write one `peer_revoked` line.
async fn apply_peer_revocation(st: &AppState, node: &Node, from: PeerId, r: Revocation) {
    if r.peer_id == st.identity.id {
        return;
    }
    {
        let _g = lock(&st.peer_writes);
        if st.peers.revocations_since(0).unwrap_or_default().contains(&r) {
            return;
        }
        let audience = st.revocation_audience(&r.peer_id);
        if let Err(e) = st.peers.apply_revocation(&r) {
            tracing::info!(peer = %from.short_label(), code = %e.code, "revocation from peer ignored");
            return;
        }
        lock(&st.fanout).set_audience(&r, audience);
    }
    node.disconnect(&r.peer_id, "revoked").await;
    st.audit(
        "peer_revoked",
        Some(&r.peer_id),
        json!({ "seq": r.seq, "by": r.revoked_by.short_label(), "source": "peer" }),
    );
    let _ = st.revocations.send(r);
}

/// Owns the control stream for the life of the connection: first offers every revocation this
/// store holds (ADR-0003 D7 `revocations_since` on every reconnect), then new ones (local or applied
/// from another peer), each only where [`RevocationFanout::due`] says, and applies the peer's
/// ([`apply_peer_revocation`]). `Status` and unknown messages are ignored in this minor.
async fn control_loop(st: Arc<AppState>, node: Arc<Node>, conn: Arc<PeerConnection>) {
    let peer = conn.peer_id;
    let mut rx = st.revocations.subscribe();
    let mut guard = conn.control.lock().await;
    let ControlStream { send, recv } = &mut *guard;
    for r in st.peers.revocations_since(0).unwrap_or_default() {
        if !st.offer_revocation(send, peer, &r).await {
            return;
        }
    }
    loop {
        // One read future lives across the inner loop, so a write never cancels a half-read frame.
        let mut read = std::pin::pin!(read_frame(recv, MAX_CONTROL_FRAME));
        let frame = loop {
            tokio::select! {
                r = &mut read => break r,
                r = rx.recv() => match r {
                    Ok(rev) => {
                        if !st.offer_revocation(send, peer, &rev).await {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return,
                },
            }
        };
        match frame {
            Ok(Some(ControlMessage {
                body: Some(Body::Revocation(m)),
            })) => match revocation_from_msg(m) {
                Ok(r) => {
                    lock(&st.fanout).done(&r, peer);
                    apply_peer_revocation(&st, &node, peer, r).await;
                }
                Err(e) => {
                    tracing::info!(peer = %peer.short_label(), code = %e.code, "malformed revocation ignored");
                }
            },
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return,
        }
    }
}

// ---------------------------------------------------------------- remote requests (F-06-10)

/// `remote::generate_remote_with_id` with the sending side's audit (F-06-10):
/// - `phase: "sent"` (peer, model id) before any byte of the request leaves the device. When it
///   cannot be written nothing is sent and the call fails with `io`. A privacy mode other than
///   `trusted_group` is refused (`policy_denied`) before this entry.
/// - `phase: "finished"`: the outcome, `done`, the error code, `incomplete`, or `cancelled` when
///   the caller dropped the stream (HTTP client gone, Stop); one per request. It carries the
///   `request_id` of the `RemoteRequest`, the id the serving device writes into its own
///   `remote_request` entry, so the two sides join on it; `null` when the request failed before
///   its frame left (privacy refused, connection lost).
///
/// Ids, counts and durations only, never text. The API's `target: peer:<id>` and
/// `common run --peer` use it.
// ponytail: the request id is chosen inside `generate_remote_with_id`, after "sent" is written,
// so only "finished" carries it. A common-network variant taking a caller-chosen id would let
// "sent" carry it too.
pub async fn generate_remote_audited(
    audit: Arc<Audit>,
    conn: Arc<PeerConnection>,
    model_id: &str,
    model_sha256: &str,
    req: GenerateRequest,
    privacy: &str,
    cancel: CancellationToken,
) -> Result<GenerateStream> {
    let peer = conn.peer_id;
    // Any other mode is refused inside `generate_remote_with_id` before a stream opens.
    let leaves = serde_json::from_value::<PrivacyMode>(Value::String(privacy.into()))
        .is_ok_and(|m| m == PrivacyMode::TrustedGroup);
    if leaves {
        let sent = json!({ "role": "client", "phase": "sent", "peer": peer.short_label(), "model_id": model_id });
        audit
            .record("remote_request", Some(&peer.to_hex()), sent)
            .map_err(|e| CommonError::io(format!("audit log not writable, request not sent: {}", e.message)))?;
    }
    let mut outcome = Outcome::new(audit, peer, model_id);
    match remote::generate_remote_with_id(conn, model_id, model_sha256, req, privacy, cancel).await {
        Ok((request_id, inner)) => {
            outcome.request_id = Some(request_id);
            Ok(Audited { inner, outcome }.boxed())
        }
        Err(e) => {
            outcome.finish(e.code.as_str());
            Err(e)
        }
    }
}

/// The `finished` entry of one remote request; written once, by `finish` or else on drop.
struct Outcome {
    audit: Arc<Audit>,
    peer: PeerId,
    model_id: String,
    request_id: Option<String>,
    started: Instant,
    completion_tokens: u32,
    written: bool,
}

impl Outcome {
    fn new(audit: Arc<Audit>, peer: PeerId, model_id: &str) -> Self {
        Self {
            audit,
            peer,
            model_id: model_id.to_string(),
            request_id: None,
            started: Instant::now(),
            completion_tokens: 0,
            written: false,
        }
    }

    fn finish(&mut self, outcome: &str) {
        if std::mem::replace(&mut self.written, true) {
            return;
        }
        let fields = json!({
            "role": "client",
            "phase": "finished",
            "request_id": self.request_id,
            "peer": self.peer.short_label(),
            "model_id": self.model_id,
            "outcome": outcome,
            "completion_tokens": self.completion_tokens,
            "duration_ms": self.started.elapsed().as_millis() as u64,
        });
        if let Err(e) = self.audit.record("remote_request", Some(&self.peer.to_hex()), fields) {
            tracing::warn!(error = %e, "remote_request outcome not audited");
        }
    }
}

impl Drop for Outcome {
    fn drop(&mut self) {
        self.finish("cancelled");
    }
}

/// A remote generation that writes its [`Outcome`] when it ends, fails or is dropped.
struct Audited {
    inner: GenerateStream,
    outcome: Outcome,
}

impl Stream for Audited {
    type Item = Result<GenerateEvent>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let item = ready!(this.inner.poll_next_unpin(cx));
        match &item {
            Some(Ok(GenerateEvent::Done { usage, .. })) => {
                this.outcome.completion_tokens = usage.completion_tokens;
                this.outcome.finish("done");
            }
            Some(Err(e)) => this.outcome.finish(e.code.as_str()),
            Some(Ok(_)) => {}
            None => this.outcome.finish("incomplete"),
        }
        Poll::Ready(item)
    }
}

// ---------------------------------------------------------------- handlers

pub async fn network_get(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    axum::Json(st.network_status().await)
}

#[derive(Deserialize, Default)]
pub struct NetworkBody {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub serve_peers: Option<bool>,
}

/// Changes are persisted (a read-modify-write of `config.json`, so a concurrent CLI privacy change
/// is kept) and the Node is restarted so every connection sees the new policy; it is also
/// (re)started when the file says enabled but no endpoint runs. When the endpoint cannot start,
/// networking is recorded as off and the error is returned.
pub async fn network_put(
    State(st): State<Arc<AppState>>,
    Json(body): Json<NetworkBody>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    let (before, after) = st.update_settings(|s| Settings {
        network_enabled: body.enabled.unwrap_or(s.network_enabled),
        serve_peers: body.serve_peers.unwrap_or(s.serve_peers),
        ..s
    })?;
    let running = st.net.lock().await.node.is_some();
    if after != before || after.network_enabled != running {
        if let Err(e) = st.apply_network().await {
            st.update_settings(|s| Settings {
                network_enabled: false,
                ..s
            })?;
            return Err(e.into());
        }
        st.audit(
            "network_changed",
            None,
            json!({ "enabled": after.network_enabled, "serve_peers": after.serve_peers }),
        );
    }
    Ok(axum::Json(st.network_status().await))
}

/// Pinned peers (with `connected`, `health` and their groups), peers mDNS currently sees (never
/// this device; `trusted` only when pinned here, never a trust decision by itself) and the
/// revocations this store holds. `health` is `Node::health` (F-02-05): `connected`, `unhealthy`,
/// `reconnecting` or `disconnected`; `disconnected` too while networking is off or before the
/// first connection to that device.
pub async fn peers_get(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    let (node, discovered) = {
        let net = st.net.lock().await;
        (net.node.clone(), net.discovered())
    };
    let connected: HashSet<PeerId> = node
        .as_ref()
        .map(|n| n.connections().iter().map(|c| c.peer_id).collect())
        .unwrap_or_default();
    let health = |id: &PeerId| {
        node.as_ref()
            .and_then(|n| n.health(id))
            .unwrap_or(PeerHealth::Disconnected)
    };
    let groups = st.peers.groups()?;
    let peers: Vec<Value> = st
        .peers
        .list()?
        .into_iter()
        .map(|p| {
            let names: Vec<&str> = groups
                .iter()
                .filter(|g| g.members.contains(&p.id))
                .map(|g| g.name.as_str())
                .collect();
            let mut v = json!(p);
            v["short_label"] = json!(p.id.short_label());
            v["connected"] = json!(connected.contains(&p.id));
            v["health"] = json!(health(&p.id));
            v["groups"] = json!(names);
            v
        })
        .collect();
    Ok(axum::Json(json!({
        "peers": peers,
        "discovered": discovered,
        "revocations": st.peers.revocations_since(0)?,
    })))
}

/// Unpin without revoking (the device may pair again) and close its connections.
pub async fn peer_delete(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    let id = st.resolve_peer(&id)?;
    {
        let _g = lock(&st.peer_writes);
        st.peers.remove(&id)?;
    }
    let node = st.net.lock().await.node.clone();
    if let Some(node) = node {
        node.disconnect(&id, "unpaired").await;
    }
    st.audit("peer_removed", Some(&id), json!({}));
    Ok(StatusCode::NO_CONTENT)
}

/// F-06-05: unpin, record the signed revocation, close every connection to the device with the
/// REVOKED code and send the revocation to the connected peers that shared a group with it here
/// (ADR-0003 D7; their control loops apply it).
pub async fn peer_revoke(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> std::result::Result<axum::Json<Revocation>, ApiError> {
    let id = st.resolve_peer(&id)?;
    let rev = {
        let _g = lock(&st.peer_writes);
        let audience = st.revocation_audience(&id);
        let rev = st.peers.revoke(&id, &st.identity)?;
        lock(&st.fanout).set_audience(&rev, audience);
        rev
    };
    let node = st.net.lock().await.node.clone();
    if let Some(node) = node {
        node.disconnect(&id, "revoked").await;
    }
    let _ = st.revocations.send(rev.clone());
    st.audit(
        "peer_revoked",
        Some(&id),
        json!({ "seq": rev.seq, "by": st.identity.id.short_label(), "source": "local" }),
    );
    Ok(axum::Json(rev))
}

#[derive(Deserialize, Default)]
pub struct PairStartBody {
    #[serde(default)]
    pub name: Option<String>,
}

/// Open the pairing window (one at a time; 409 `overloaded` while one is active). Returns the
/// 6-digit code, `pair_addr` (`ip:port` another device dials: the LAN address when the endpoint
/// is bound on every interface) and the QR payload carrying that address; `GET /api/pair/status`
/// reports the outcome. Turning networking off closes the window (`cancelled`).
pub async fn pair_start(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PairStartBody>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    let name = body.name.unwrap_or_else(|| DEFAULT_DEVICE_NAME.into());
    let node = st.node().await?;
    let mut slot = st.pairing.lock().await;
    if slot.as_ref().is_some_and(Pairing::active) {
        return Err(ApiError {
            error: CommonError::new(
                ErrorCode::Overloaded,
                "a pairing session is already open; wait for it to finish or expire",
            )
            .with_hint("GET /api/pair/status"),
            status: StatusCode::CONFLICT,
        });
    }
    let bind: IpAddr = node
        .config
        .bind_addr
        .parse()
        .map_err(|e| CommonError::invalid(format!("network bind address: {e}")))?;
    let (session, handle) = pairing::start(node, &name).await?;
    let bound = SocketAddr::new(bind, session.port);
    let pair_addr = routable(bound);
    // The QR carries the bind address; 0.0.0.0 is not dialable from another machine.
    let qr = session.qr_payload.replace(&bound.to_string(), &pair_addr.to_string());
    if let Err(e) = st.net.lock().await.advertise(st.identity.clone(), Some(session.port)) {
        tracing::warn!(error = %e, "mdns pairing advert failed");
    }
    let result = Arc::new(Mutex::new(None));
    let (st2, res2) = (st.clone(), result.clone());
    let abort = handle.abort_handle();
    let task = tokio::spawn(async move {
        let r = match handle.await {
            Ok(r) => r,
            Err(e) if e.is_cancelled() => {
                st2.audit(
                    "pairing_failed",
                    None,
                    json!({ "role": "initiator", "reason": "cancelled" }),
                );
                Err(CommonError::new(
                    ErrorCode::Cancelled,
                    "the pairing window was closed because peer networking was turned off or restarted",
                ))
            }
            Err(e) => Err(CommonError::internal(format!("pairing task failed: {e}"))),
        };
        *lock(&res2) = Some(r);
        if let Err(e) = st2.net.lock().await.advertise(st2.identity.clone(), None) {
            tracing::warn!(error = %e, "mdns advert refresh failed");
        }
    });
    *slot = Some(Pairing {
        port: session.port,
        expires_at: session.expires_at_unix,
        result,
        abort,
        task: Some(task),
    });
    Ok(axum::Json(json!({
        "code": session.code,
        "port": session.port,
        "pair_addr": pair_addr.to_string(),
        "qr_payload": qr,
        "expires_at": session.expires_at_unix,
    })))
}

/// The address a LAN peer should dial when `addr` is bound on every interface: the IP of the
/// default route. The UDP `connect` sends nothing (it only asks the kernel which source address it
/// would use) and the probe socket is dropped at once; this is the only socket this module opens
/// outside `common-network`. A specific address, or no route, returns `addr` unchanged.
// ponytail: one default-route guess; a multi-homed host gets the first route. mDNS carries the
// address the other device actually sees. `common_cli::mesh::routable` is the same function and
// can call this one.
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

pub async fn pair_status(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    let slot = st.pairing.lock().await;
    let v = match slot.as_ref() {
        None => json!({ "active": false, "result": Value::Null }),
        Some(p) => match lock(&p.result).clone() {
            None => json!({ "active": true, "result": Value::Null, "port": p.port, "expires_at": p.expires_at }),
            Some(Ok(rec)) => json!({ "active": false, "result": { "peer": rec } }),
            Some(Err(e)) => {
                let mut err = e.to_openai_json();
                json!({ "active": false, "result": { "error": err["error"].take() } })
            }
        },
    };
    axum::Json(v)
}

#[derive(Deserialize)]
pub struct PairJoinBody {
    /// `ip:port` of the initiator's pairing endpoint (required with a 6-digit code; overrides the
    /// address inside a QR payload).
    #[serde(default)]
    pub addr: Option<String>,
    /// The 6-digit code, or the whole `common-pair:v1:...` QR payload.
    pub code: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// Join a pairing window from this device's workload endpoint; on success both sides are pinned.
/// With a QR payload only the device the QR names can complete the handshake.
pub async fn pair_join(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PairJoinBody>,
) -> std::result::Result<axum::Json<PeerRecord>, ApiError> {
    let name = body.name.unwrap_or_else(|| DEFAULT_DEVICE_NAME.into());
    let node = st.node().await?;
    let given: Option<SocketAddr> = match body.addr.as_deref() {
        Some(a) => Some(a.parse().map_err(|_| CommonError::invalid("addr must be ip:port"))?),
        None => None,
    };
    let (addr, code, expected) = if body.code.starts_with("common-pair:") {
        let (id, qr_addr, secret) = pairing::expected_peer_from_qr(&body.code)?;
        (given.unwrap_or(qr_addr), secret, Some(id))
    } else {
        let addr = given.ok_or_else(|| {
            CommonError::invalid("addr (ip:port of the other device's pairing endpoint) is required with a code")
        })?;
        (addr, body.code, None)
    };
    // A QR names the device: any other key fails the TLS handshake before SPAKE2, so nothing is
    // pinned and no revocation is touched on either side (`pairing_failed`, `qr_peer_mismatch`).
    let rec = pairing::join_with(node, addr, &code, &name, expected).await?;
    Ok(axum::Json(rec))
}

pub async fn groups_get(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    Ok(axum::Json(json!({ "groups": st.peers.groups()? })))
}

/// Insert or replace one group (`{name, members: [peer ids], policy: {allow_remote_inference,
/// allow_pooled_inference}}`); the full list comes back. Default policy denies everything.
pub async fn groups_put(
    State(st): State<Arc<AppState>>,
    Json(group): Json<Group>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    if group.name.len() > 64 || group.members.len() > 256 {
        return Err(CommonError::invalid("group name must be <= 64 bytes and members <= 256").into());
    }
    st.peers.upsert_group(group.clone())?;
    st.audit(
        "group_updated",
        None,
        json!({
            "name": group.name,
            "members": group.members.len(),
            "allow_remote_inference": group.policy.allow_remote_inference,
            "allow_pooled_inference": group.policy.allow_pooled_inference,
        }),
    );
    Ok(axum::Json(json!({ "groups": st.peers.groups()? })))
}

pub async fn privacy_get(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    axum::Json(json!({ "mode": st.settings().privacy, "disclosure": DISCLOSURE }))
}

#[derive(Deserialize)]
pub struct PrivacyBody {
    pub mode: PrivacyMode,
    #[serde(default)]
    pub acknowledged: bool,
}

/// `local_only` always; `trusted_group` only with `"acknowledged": true` (else 403 carrying the
/// disclosure text); `swarm_allowed` is refused until Gate F.
pub async fn privacy_put(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PrivacyBody>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    match body.mode {
        PrivacyMode::LocalOnly => {}
        PrivacyMode::TrustedGroup if body.acknowledged => {}
        PrivacyMode::TrustedGroup => {
            return Err(CommonError::new(ErrorCode::PolicyDenied, DISCLOSURE)
                .with_hint("resend with \"acknowledged\": true to accept this")
                .into())
        }
        PrivacyMode::SwarmAllowed => {
            return Err(CommonError::new(
                ErrorCode::PolicyDenied,
                "public swarm execution is disabled until it passes its security gate",
            )
            .into())
        }
    }
    let (before, after) = st.update_settings(|s| Settings {
        privacy: body.mode,
        ..s
    })?;
    if before != after {
        st.audit(
            "privacy_changed",
            None,
            json!({ "from": before.privacy, "to": body.mode, "acknowledged": body.acknowledged }),
        );
    }
    Ok(axum::Json(json!({ "mode": body.mode, "disclosure": DISCLOSURE })))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_audit_n")]
    pub n: usize,
}

fn default_audit_n() -> usize {
    100
}

/// The last `n` (1..=1000, default 100) audit events, oldest first.
pub async fn audit_get(
    State(st): State<Arc<AppState>>,
    Query(q): Query<AuditQuery>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    Ok(axum::Json(json!({ "events": st.audit.tail(q.n.clamp(1, 1000))? })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_off_and_round_trip_0600() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        assert_eq!(Settings::load(&paths).unwrap(), None);
        let d = Settings::default();
        assert_eq!(d.privacy, PrivacyMode::LocalOnly);
        assert!(!d.network_enabled && !d.serve_peers);
        let s = Settings {
            privacy: PrivacyMode::TrustedGroup,
            network_enabled: true,
            serve_peers: false,
            ..Settings::default()
        };
        s.save(&paths).unwrap();
        assert_eq!(Settings::load(&paths).unwrap(), Some(s));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Settings::path(&paths)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        // Unknown keys are tolerated, missing ones default; garbage is an error, not a default.
        std::fs::write(Settings::path(&paths), r#"{"privacy": "trusted_group", "future": 1}"#).unwrap();
        let s = Settings::load(&paths).unwrap().unwrap();
        assert_eq!((s.privacy, s.network_enabled), (PrivacyMode::TrustedGroup, false));
        std::fs::write(Settings::path(&paths), "{nope").unwrap();
        assert_eq!(Settings::load(&paths).unwrap_err().code, ErrorCode::Io);
    }

    /// `peer_quota` is optional field by field; the defaults are the policy crate's.
    #[test]
    fn peer_quota_defaults_and_partial_config() {
        let q = QuotaConfig::default();
        let d = PeerQuota::default();
        assert_eq!(
            (d.requests_per_minute, d.tokens_per_minute, d.max_concurrent),
            (q.requests_per_minute, q.tokens_per_minute, q.max_concurrent)
        );
        let s: Settings = serde_json::from_str(r#"{"peer_quota": {"max_concurrent": 1}}"#).unwrap();
        assert_eq!(
            s.peer_quota,
            PeerQuota {
                max_concurrent: 1,
                ..PeerQuota::default()
            }
        );
        let slots = Arc::new(tokio::sync::Semaphore::new(4));
        let o = s.peer_quota.serve_options(Default::default(), slots.clone(), 4);
        assert_eq!(o.peer_slots.available_permits(), 1);
        assert!(o.engine_slots.is_some_and(|e| Arc::ptr_eq(&e, &slots)));
        let zero = PeerQuota {
            max_concurrent: 0,
            ..PeerQuota::default()
        };
        assert_eq!(zero.peer_slots(4), 1, "0 is one generation, not unlimited");
    }

    /// F-06-04: peers together never hold the last engine slot, whatever `peer_quota` says, so a
    /// local request never waits behind peer generations. Defaults: the library's one engine slot
    /// serves no peers; `common serve` (4 slots, quota 4) lets peers use 3.
    #[test]
    fn peers_never_get_the_last_engine_slot() {
        let d = PeerQuota::default();
        assert_eq!(d.max_concurrent, 4);
        let api_default = crate::ApiConfig::default().max_concurrent;
        assert_eq!((api_default, d.peer_slots(api_default)), (1, 0));
        assert_eq!(d.peer_slots(4), 3, "`common serve` default --max-concurrent 4");
        assert_eq!(d.peer_slots(2), 1);
        assert_eq!(d.peer_slots(16), 4, "the quota still caps");
        assert_eq!(d.peer_slots(0), 0);
        let o = d.serve_options(Default::default(), Arc::new(tokio::sync::Semaphore::new(1)), 1);
        assert_eq!(
            o.peer_slots.available_permits(),
            0,
            "one slot: every peer request is refused"
        );
    }

    /// Crossed dials: the connection the lower id initiated, as `Node::register` keeps it; else the
    /// oldest open one.
    #[test]
    fn survivor_is_the_connection_the_lower_id_dialled() {
        // Lower id here: an inbound duplicate registered first must not be the one used.
        assert_eq!(survivor(true, &[false, true]), Some(1));
        assert_eq!(survivor(true, &[false]), Some(0));
        // Higher id: the peer's (inbound) connection wins over this device's own dial.
        assert_eq!(survivor(false, &[true, false]), Some(1));
        assert_eq!(survivor(false, &[true]), Some(0));
        assert_eq!(survivor(true, &[true, true]), Some(0), "the older of two own dials");
        assert_eq!(survivor(false, &[]), None);
    }

    #[test]
    fn same_addr_treats_v4_mapped_as_v4() {
        let v4: SocketAddr = "192.168.1.20:8193".parse().unwrap();
        assert!(same_addr(v4, "[::ffff:192.168.1.20]:8193".parse().unwrap()));
        assert!(!same_addr(v4, "192.168.1.20:8194".parse().unwrap()));
        assert!(!same_addr(v4, "192.168.1.21:8193".parse().unwrap()));
    }

    /// ADR-0003 D7: a record goes to its audience, on every connection until the member sends it
    /// back (a write is not a delivery); never to the revoked device, never back to the device it
    /// came from; without a recorded audience, to group members only.
    #[test]
    fn revocation_fanout_offers_a_record_to_its_audience_until_it_comes_back() {
        let id = |b: u8| PeerId([b; 32]);
        let (issuer, target, member, outsider) = (id(1), id(2), id(3), id(4));
        let r = Revocation {
            seq: 1,
            peer_id: target,
            revoked_by: issuer,
            at_utc: "2026-10-15T00:00:00Z".into(),
            signature: hex::encode([0u8; 64]),
        };
        let mut f = RevocationFanout::default();
        // Unknown audience (revoked before this daemon started): group members only.
        assert!(f.due(&r, &member, || true));
        assert!(!f.due(&r, &outsider, || false));
        assert!(!f.due(&r, &target, || true), "never to the revoked device");
        f.set_audience(&r, [member, target].into_iter().collect());
        assert!(f.due(&r, &member, || false));
        assert!(!f.due(&r, &outsider, || true), "a recorded audience decides");
        assert!(!f.due(&r, &target, || true));
        assert!(f.due(&r, &member, || false), "still due on the next connection");
        f.done(&r, member);
        assert!(!f.due(&r, &member, || true), "the member sent it here: it has it");
        // Received from a peer: never sent back to it, audience or not.
        let other = Revocation { seq: 2, ..r.clone() };
        f.done(&other, outsider);
        assert!(!f.due(&other, &outsider, || true));
        assert!(f.due(&other, &member, || true));
    }

    #[test]
    fn short_label_shape() {
        assert_eq!(short_label("1A2b-3c4d-5e6f"), Some("1a2b-3c4d-5e6f".into()));
        for bad in ["1a2b3c4d5e6f", "1a2b-3c4d-5e6", "1a2b-3c4d-5e6g", "", "1a2b_3c4d_5e6f"] {
            assert_eq!(short_label(bad), None, "{bad}");
        }
    }

    #[test]
    fn revocation_message_round_trip_and_bounds() {
        let me = DeviceIdentity::generate().unwrap();
        let victim = DeviceIdentity::generate().unwrap();
        let r = Revocation {
            seq: 7,
            peer_id: victim.id,
            revoked_by: me.id,
            at_utc: "2026-10-14T00:00:00Z".into(),
            signature: hex::encode([9u8; 64]),
        };
        let ControlMessage {
            body: Some(Body::Revocation(m)),
        } = revocation_msg(&r)
        else {
            panic!("not a revocation frame")
        };
        assert_eq!(revocation_from_msg(m.clone()).unwrap(), r);
        let short_sig = RevocationMsg {
            signature: vec![1; 63],
            ..m.clone()
        };
        assert_eq!(
            revocation_from_msg(short_sig).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        let bad_id = RevocationMsg {
            peer_id: vec![1; 31],
            ..m
        };
        assert_eq!(revocation_from_msg(bad_id).unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn dial_order_prefers_the_fresh_mdns_address() {
        let (a, b): (SocketAddr, SocketAddr) = ("192.168.1.20:8193".parse().unwrap(), "10.0.0.5:8193".parse().unwrap());
        assert_eq!(dial_order(Some(a), Some(b)), vec![a, b]);
        assert_eq!(dial_order(Some(a), Some(a)), vec![a]);
        assert_eq!(dial_order(None, Some(b)), vec![b]);
        assert_eq!(dial_order(Some(a), None), vec![a]);
        assert!(dial_order(None, None).is_empty());
    }

    #[test]
    fn routable_replaces_only_the_unspecified_address() {
        let specific: SocketAddr = "127.0.0.1:7".parse().unwrap();
        assert_eq!(routable(specific), specific);
        let r = routable("0.0.0.0:7".parse().unwrap());
        assert_eq!(r.port(), 7);
        // Without any route (offline CI) the address comes back unchanged; with one it is dialable.
        if !r.ip().is_unspecified() {
            assert!(!r.ip().is_loopback(), "{r}");
        }
    }

    fn done(tokens: u32) -> Result<GenerateEvent> {
        Ok(GenerateEvent::Done {
            finish_reason: "length".into(),
            usage: common_runtime::Usage {
                prompt_tokens: 3,
                completion_tokens: tokens,
            },
            timings: Default::default(),
        })
    }

    fn token(t: &str) -> Result<GenerateEvent> {
        Ok(GenerateEvent::Token {
            text: t.into(),
            token_ids: vec![],
        })
    }

    /// The `finished` entries a wrapped stream leaves: one per request, whatever ends it, and never
    /// the generated text.
    #[tokio::test]
    async fn audited_stream_records_one_outcome_whatever_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let audit = Arc::new(Audit::open(&Paths::at(dir.path())).unwrap());
        let peer = DeviceIdentity::generate().unwrap().id;
        let wrap = |items: Vec<Result<GenerateEvent>>| Audited {
            inner: futures_util::stream::iter(items).boxed(),
            outcome: Outcome::new(audit.clone(), peer, "m"),
        };
        let all: Vec<_> = wrap(vec![token("SECRET-TEXT"), done(5)]).collect().await;
        assert_eq!(all.len(), 2);
        let mut s = wrap(vec![token("a"), token("b"), done(2)]);
        assert!(s.next().await.is_some());
        drop(s); // the HTTP client went away mid-stream
        let _: Vec<_> = wrap(vec![Err(CommonError::new(ErrorCode::PeerLost, "gone"))])
            .collect()
            .await;
        let _: Vec<_> = wrap(vec![token("a")]).collect().await;
        let finished: Vec<(String, u64)> = audit
            .tail(100)
            .unwrap()
            .into_iter()
            .map(|e| {
                assert_eq!(
                    (e.kind.as_str(), e.fields["phase"].as_str()),
                    ("remote_request", Some("finished"))
                );
                assert_eq!(e.peer_id.as_deref(), Some(peer.to_hex().as_str()));
                let f = &e.fields;
                (
                    f["outcome"].as_str().unwrap().to_string(),
                    f["completion_tokens"].as_u64().unwrap(),
                )
            })
            .collect();
        let want = [("done", 5), ("cancelled", 0), ("peer_lost", 0), ("incomplete", 0)];
        assert_eq!(finished, want.map(|(o, n)| (o.to_string(), n)));
        let text = std::fs::read_to_string(dir.path().join("logs/audit.jsonl")).unwrap();
        assert!(!text.contains("SECRET-TEXT"), "{text}");
    }

    #[test]
    fn disclosure_names_the_risk() {
        assert!(DISCLOSURE.contains("plain text") && DISCLOSURE.contains("local only"));
        assert!(DISCLOSURE.len() < common_observability::MAX_FIELD_STRING);
    }
}
