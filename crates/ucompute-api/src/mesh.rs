//! UCompute Mesh in the daemon API (F-02-01/02/05, F-06-02, F-06-05, F-06-07, F-06-10; the backends
//! of the desktop pairing UI F-10-05 and privacy selector F-10-10).
//!
//! Secure defaults, persisted in `UCOMPUTE_HOME/config.json` (0600): privacy `local_only`, peer
//! networking off, serving peers off. Nothing binds the peer endpoint until
//! `PUT /api/network {"enabled": true}`; `PUT /api/privacy` to a non-local mode returns
//! [`DISCLOSURE`] until the caller acknowledges it; `swarm_allowed` is refused until Gate F.
//! Every off-device action is audited with ids, counts and codes only.
//!
//! `config.json` is re-read whenever it changes on disk, and `PUT /api/network|privacy` are
//! read-modify-writes of the file, so `ucompute privacy local-only` from another process applies to
//! the next request and is never written back over.
//!
//! Connections: the inbound accept loop and every outbound connect attach the same two tasks to a
//! `PeerConnection`, once per connection: `remote::serve_peer_with` (default deny per request;
//! [`PeerQuota`] token buckets; a peer takes a free engine slot of the local admission or gets
//! `overloaded`, never a place in the local queue) and [`control_loop`], which offers this
//! device's revocations to peers that shared a group with the revoked device and applies (and
//! passes on) the peer's (ADR-0003 D7). While networking is on, [`sync_loop`] keeps a connection
//! to every pinned device (1, 2, 4 … 300 s backoff, reset by a connection, a new mDNS address or
//! `POST /api/peers/{id}/connect`), so a revocation reaches members that are not mid-request and
//! offline members converge when they return. Turning networking off also closes an open pairing
//! window.
//!
//! Addresses are only hints: every dial (mDNS, pinned, typed by the user) must end in a TLS
//! handshake that presents the pinned key of the device asked for, so a wrong or spoofed address
//! yields a refused handshake and is never stored.

use crate::{ApiError, AppState, Json};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use futures_util::future::BoxFuture;
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
use ucompute_core::{ErrorCode, Paths, Result, UComputeError};
use ucompute_identity::{utc_now, DeviceIdentity, Group, PeerId, PeerRecord, Revocation};
use ucompute_network::discovery::{Advertiser, Browser, DiscoveredPeer, UnreachablePeer, LISTED_SECS};
use ucompute_network::health::PeerHealth;
use ucompute_network::local_priority::LocalPriority;
use ucompute_network::pairing;
use ucompute_network::proto::{control_message::Body, read_frame, write_frame, ControlMessage, RevocationMsg};
use ucompute_network::transport::{ControlStream, HeldControl, Node, PeerConnection};
use ucompute_network::{remote, NetworkConfig, MAX_CONTROL_FRAME};
use ucompute_observability::Audit;
use ucompute_policy::{PolicyConfig, PolicyEngine, PrivacyMode, QuotaConfig, Quotas};
use ucompute_runtime::{GenerateEvent, GenerateRequest, GenerateStream};

/// Shown before a non-local privacy mode can be selected (F-10-10, THREAT_MODEL §4). Returned by
/// `GET /api/privacy` and, as the error message, by an unacknowledged `PUT /api/privacy`.
pub const DISCLOSURE: &str =
    "Trusted devices: a request you send to a paired device travels encrypted, but that device \
decrypts it and runs the model on your prompt in plain text inside its own process. It can read, log or keep what you \
send and what the model answers; UCompute cannot prevent that. Only \"local only\" keeps everything on this computer. \
Pair only devices you trust with your prompts.";

const SETTINGS_FILE: &str = "config.json";
/// How often [`sync_loop`] looks for pinned devices without a connection.
const SYNC_TICK: Duration = Duration::from_secs(1);
/// Longest wait between two dials to an unreachable pinned device ([`sync_backoff`]).
const SYNC_BACKOFF_CAP: Duration = Duration::from_secs(300);
/// `last_seen_utc` of a connected device is rewritten at most this often ([`seen_due`]).
const LAST_SEEN_EVERY: Duration = Duration::from_secs(60);

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
    /// Local work first on CPU engines (D-026); `refuse_new` when absent. Applies to the next
    /// peer request, without restarting the endpoint.
    #[serde(default)]
    pub local_priority: LocalPriority,
}

/// `config.json` `"peer_quota"`: per-peer token buckets for requests served to paired devices.
/// Every field is optional; 0 means unlimited for the two rates. Defaults are
/// `ucompute_policy::QuotaConfig::default()`. A request over a limit gets `overloaded` (429 at the
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
    // would let a single-slot device serve peers too; that needs a cancel hook in ucompute-network.
    pub(crate) fn peer_slots(&self, engine_slots: usize) -> usize {
        (self.max_concurrent.max(1) as usize).min(engine_slots.saturating_sub(1))
    }

    /// The daemon's peer-serving options: these quotas, the API's load options (a peer request
    /// never forces a reload) and its admission semaphore (`engine_slot_count` permits) as the
    /// engine slots (ADR-0003 D5), peers capped at [`PeerQuota::peer_slots`].
    pub(crate) fn serve_options(
        self,
        load: ucompute_runtime::manager::LoadOptions,
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
            jobs: ucompute_observability::jobs::Jobs::new(),
            // set by `AppState::new` (WP-04 stage host; D-026 local work)
            rpc: None,
            local: None,
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
                .map_err(|e| UComputeError::io(format!("{} is not valid: {e}", path.display()))),
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

    pub(crate) fn local_priority(&self) -> LocalPriority {
        self.value.local_priority
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
        // Abort, then wait for each task to be dropped: they hold the Node (and so the endpoint's
        // UDP socket), and a restart on the same fixed port must not race their teardown.
        for t in [self.accept.take(), self.sync.take()].into_iter().flatten() {
            t.abort();
            let _ = t.await;
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
        self.browser.as_ref().map_or_else(Vec::new, |b| b.peers(LISTED_SECS))
    }

    /// Devices mDNS sees at no address this device can dial ("discovered, no reachable address").
    fn unreachable(&self) -> Vec<UnreachablePeer> {
        self.browser
            .as_ref()
            .map_or_else(Vec::new, |b| b.unreachable(LISTED_SECS))
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
    /// so another process's `ucompute privacy local-only` applies to the next request). A file that
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
        self.local_work.set_mode(cell.value.local_priority);
        cell.value
    }

    /// Replace the in-memory settings until `config.json` next changes (not persisted;
    /// `serve_on` uses it to show networking off when the endpoint could not start).
    pub fn set_settings(&self, s: Settings) {
        self.local_work.set_mode(s.local_priority);
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
            self.local_work.set_mode(after.local_priority);
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
    /// config says enabled, so a disabled setting can never leave an endpoint bound. A restart that
    /// keeps networking on (a Sharing change) keeps the devices disconnected by the user (E5);
    /// turning networking off ends those holds.
    pub async fn apply_network(self: &Arc<Self>) -> Result<()> {
        let mut pairing = self.pairing.lock().await;
        cancel_pairing(&mut pairing).await;
        let s = self.settings();
        let mut net = self.net.lock().await;
        // Only paired devices: a hold belongs to the pairing it was set on (`peer_delete`).
        let holds: Vec<PeerId> = match &net.node {
            Some(node) if s.network_enabled => node.held().into_iter().filter(|id| self.peers.is_pinned(id)).collect(),
            _ => vec![],
        };
        net.stop().await;
        // A (re)started network dials every pinned device at once, as a user toggling it expects.
        lock(&self.redial).clear();
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
        // A held device that connected before this line is closed by `hold`, one mid-handshake by
        // `Node::register`.
        for id in &holds {
            node.hold(id);
        }
        node.set_held_control(Box::new(HeldRevocations(Arc::downgrade(self))));
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
        self.on_node(Arc::clone).await
    }

    /// `f` on the running Node under the `net` lock, so no restart ([`AppState::apply_network`],
    /// which carries the Node's holds over) runs between: a hold is never set on, nor a release
    /// lost with, a Node being replaced. `PolicyDenied` while peer networking is off.
    async fn on_node<R>(&self, f: impl FnOnce(&Arc<Node>) -> R) -> Result<R> {
        self.net.lock().await.node.as_ref().map(f).ok_or_else(|| {
            UComputeError::new(ErrorCode::PolicyDenied, "peer networking is disabled in settings")
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
            UComputeError::invalid("peer id must be 64 hex characters or a short label like 1a2b-3c4d-5e6f")
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
                UComputeError::new(ErrorCode::PeerUnauthorized, format!("no paired device {label}"))
                    .with_hint("GET /api/peers"),
            ),
            _ => Err(UComputeError::invalid(format!(
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
            UComputeError::new(
                ErrorCode::ModelNotFound,
                format!("no model named {model_id} in the catalog or the local store"),
            )
            .with_hint("GET /v1/models")
        })
    }

    /// An authenticated connection to `id`: [`AppState::peer_connection_via`] without a hint.
    pub async fn peer_connection(self: &Arc<Self>, node: &Arc<Node>, id: PeerId) -> Result<Arc<PeerConnection>> {
        self.peer_connection_via(node, id, None).await
    }

    /// An authenticated connection to `id`: the open one when there is one (the [`survivor`] of a
    /// crossed dial, not the duplicate about to close), else a new connect to `hint` (an address
    /// the user typed), the addresses mDNS sees now, then the pinned `last_addr` (see
    /// [`dial_order`]). Each is only a hint: `Node::connect` accepts nothing but `id`'s pinned key.
    /// `last_seen_utc` is refreshed, and `last_addr` only when this dial's handshake to it is the
    /// connection returned. A failure tries the next address: no answer, another device, or a
    /// refusal at that address. A refusal closed before the device proved its pinned key with
    /// authenticated control data reads as another device, never as `peer_revoked` or `peer_held`,
    /// however late in the TLS handshake it came (a close in an Initial packet is forgeable by
    /// anything on the path: `Node::connect`), so it never ends the walk. This store's own refusal
    /// (revoked or unpaired here) sends nothing at any address (`Node::connect`). A proven refusal
    /// (`peer_held` sent on the control stream, a close after it, this store's `peer_revoked`) ends
    /// the dial: every address of that device says the same, and a later timeout must not replace
    /// it as the device's `last_error`. Otherwise the error returned is the last refusal if there
    /// was one (it says more than a later timeout), else the last failure.
    pub async fn peer_connection_via(
        self: &Arc<Self>,
        node: &Arc<Node>,
        id: PeerId,
        hint: Option<SocketAddr>,
    ) -> Result<Arc<PeerConnection>> {
        // A held device (E5) is refused before any address is looked at, known or not.
        node.refuse_held(&id)?;
        let open: Vec<Arc<PeerConnection>> = node.connections().into_iter().filter(|c| c.peer_id == id).collect();
        let flags: Vec<bool> = open.iter().map(|c| c.outbound).collect();
        if let Some(i) = survivor(self.identity.id < id, &flags) {
            return Ok(open[i].clone());
        }
        let discovered: Vec<SocketAddr> = self
            .net
            .lock()
            .await
            .discovered()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| std::iter::once(p.addr).chain(p.addrs).collect())
            .unwrap_or_default();
        let pinned = self
            .peers
            .get(&id)?
            .and_then(|p| p.last_addr)
            .and_then(|a| a.parse().ok());
        let mut last: Option<UComputeError> = None;
        let local = node.local_addr()?;
        // ponytail: up to 4 sequential dials x idle timeout per request; parallel dials if that latency bites
        for addr in dial_order(hint, &discovered, pinned, local) {
            match node.connect(addr, id).await {
                Ok(conn) => {
                    // `connect` may hand back a connection it did not dial here (one that
                    // registered meanwhile, or the survivor of a crossed dial): `addr` unproven.
                    let dialled = conn.outbound && same_addr(conn.remote_addr, addr);
                    self.touch(id, dialled.then_some(addr));
                    self.attach(conn.clone());
                    return Ok(conn);
                }
                Err(e) if matches!(e.code, ErrorCode::PeerHeld | ErrorCode::PeerRevoked) => return Err(e),
                Err(e) => last = Some(prefer_refusal(last, e)),
            }
        }
        let e = last.unwrap_or_else(|| {
            UComputeError::new(
                ErrorCode::PeerLost,
                format!("no known address for device {}", id.short_label()),
            )
        });
        Err(match e.hint {
            Some(_) => e,
            None => e.with_hint(
                "check that it is awake and on the same network, or enter its address (Devices in the desktop app)",
            ),
        })
    }

    /// Record a contact with a pinned peer: `last_seen_utc` always, `last_addr` when `addr` is
    /// given: an address this device dialled and authenticated, or the source IP of an
    /// authenticated inbound connection with the pinned port ([`inbound_hint`]). Under
    /// `peer_writes`, so it never re-pins a device unpaired meanwhile; `PeerStore::pin` refuses a
    /// revoked one.
    fn touch(&self, id: PeerId, addr: Option<SocketAddr>) {
        let _g = lock(&self.peer_writes);
        let Ok(Some(mut p)) = self.peers.get(&id) else {
            return;
        };
        if let Some(a) = addr {
            p.last_addr = Some(a.to_string());
        }
        p.last_seen_utc = Some(utc_now());
        if let Err(e) = self.peers.pin(p) {
            tracing::debug!(peer = %id.short_label(), code = %e.code, "peer address not refreshed");
        }
    }

    /// Serve the peer's requests (default deny per request, this daemon's quotas and engine slots)
    /// and run the control loop; once per connection, however often it is handed out.
    fn attach(self: &Arc<Self>, conn: Arc<PeerConnection>) {
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
        tokio::spawn(control_loop(self.clone(), conn));
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

    /// Offer every revocation this store holds to `peer` where it is due (ADR-0003 D7
    /// `revocations_since` on every connection); false when the stream is gone.
    async fn offer_revocations<W>(&self, send: &mut W, peer: PeerId) -> bool
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        for r in self.peers.revocations_since(0).unwrap_or_default() {
            if !self.offer_revocation(send, peer, &r).await {
                return false;
            }
        }
        true
    }

    async fn network_status(&self) -> Value {
        let s = self.settings();
        let net = self.net.lock().await;
        let node = net.node.as_ref();
        let listen = node.and_then(|n| n.local_addr().ok());
        json!({
            "enabled": node.is_some(),
            "serve_peers": s.serve_peers,
            "local_priority": s.local_priority,
            "listen_addr": listen.map(|a| a.to_string()),
            // null too when bound on every interface without a route: 0.0.0.0 is not dialable.
            "dial_addr": listen.map(routable).filter(|a| !a.ip().is_unspecified()).map(|a| a.to_string()),
            "device_name": device_name(),
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

/// Where to dial a peer, best first: the address the user typed, the addresses in a fresh signed
/// mDNS record (they follow DHCP and Wi-Fi changes), then the pinned `last_addr`; each address
/// once, in that order. A known address the endpoint at `local` cannot dial is left out (IPv6 on an
/// IPv4 endpoint: quinn refuses it at once, and that error would replace a useful one); a typed
/// one is kept, so its error says why.
fn dial_order(
    hint: Option<SocketAddr>,
    discovered: &[SocketAddr],
    pinned: Option<SocketAddr>,
    local: SocketAddr,
) -> Vec<SocketAddr> {
    let dialable = |a: &SocketAddr| local.is_ipv6() || a.is_ipv4();
    let known = discovered.iter().copied().chain(pinned).filter(dialable);
    let mut v: Vec<SocketAddr> = Vec::new();
    for a in hint.into_iter().chain(known) {
        if !v.contains(&a) {
            v.push(a);
        }
    }
    v
}

/// The error worth showing after two failed dials: a refusal (another device or key at that
/// address, revoked, incompatible) says more than a later "no answer", so it is kept over one.
fn prefer_refusal(last: Option<UComputeError>, e: UComputeError) -> UComputeError {
    let silent = |e: &UComputeError| matches!(e.code, ErrorCode::PeerLost | ErrorCode::Timeout);
    match last {
        Some(l) if !silent(&l) && silent(&e) => l,
        _ => e,
    }
}

/// Where to dial a pinned device that just connected in: its authenticated source IP with the
/// pinned port (the source port may be a short-lived `ucompute run --peer` client). `None` without a
/// pinned `ip:port` to take the port from.
// ponytail: a source behind NAT stores the NAT IP, a dead hint; harmless, discovered/manual addresses are tried first
fn inbound_hint(remote: SocketAddr, pinned: Option<&str>) -> Option<SocketAddr> {
    let port = pinned?.parse::<SocketAddr>().ok()?.port();
    Some(SocketAddr::new(remote.ip().to_canonical(), port))
}

/// An address typed by the user for `POST /api/peers/{id}/connect`: `ip:port` of a device, never
/// port 0, an unspecified (0.0.0.0, ::) or a multicast address.
fn parse_dial_addr(s: &str) -> Result<SocketAddr> {
    let a: SocketAddr = s
        .trim()
        .parse()
        .map_err(|_| UComputeError::invalid("addr must be ip:port, e.g. 192.168.1.20:8193"))?;
    let ip = a.ip().to_canonical();
    if a.port() == 0 || ip.is_unspecified() || ip.is_multicast() {
        return Err(UComputeError::invalid(format!(
            "{a} is not a device address (port 0, unspecified or multicast)"
        )));
    }
    Ok(a)
}

/// This host's name, what a paired device shows for this one: the Computer Name from System
/// Settings (macOS, else its Local Hostname), the hostname (other unix), `COMPUTERNAME` (Windows),
/// else "ucompute". The default for pairing and `GET /api/network`. Not the macOS kernel hostname:
/// without a HostName set, DHCP or reverse DNS sets it, so it changes with the network and is often
/// just "Mac" or a dhcp-... name.
pub fn device_name() -> String {
    #[cfg(target_os = "macos")]
    {
        // ponytail: read once per process (a rename shows after a restart); scutil is a process spawn.
        static NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let scutil = |key: &str| {
            let out = std::process::Command::new("/usr/sbin/scutil")
                .args(["--get", key])
                .output()
                .ok()?;
            let s = String::from_utf8(out.stdout).ok()?;
            (out.status.success() && !s.trim().is_empty()).then(|| s.trim().to_string())
        };
        if let Some(n) = NAME.get_or_init(|| scutil("ComputerName").or_else(|| scutil("LocalHostName"))) {
            return n.clone();
        }
    }
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
    #[cfg(windows)]
    if let Some(n) = std::env::var_os("COMPUTERNAME").and_then(|n| n.into_string().ok()) {
        if !n.trim().is_empty() {
            return n.trim().to_string();
        }
    }
    "ucompute".into()
}

/// Which of the open connections to one peer (their `outbound` flags, oldest first) to use: the
/// one initiated by the lower id, which `Node::register` keeps when dials cross, else the oldest.
// ponytail: mirrors ucompute-network's private `transport::survivor`; call a public `Node::live`
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
        // The source IP is authenticated: QUIC Retry validated it and the TLS key is pinned.
        let pinned = st.peers.get(&conn.peer_id).ok().flatten().and_then(|p| p.last_addr);
        st.touch(conn.peer_id, inbound_hint(conn.remote_addr, pinned.as_deref()));
        st.attach(conn);
    }
}

/// Wait before the next dial after `n` failed dials in a row: 1, 2, 4 … 256 s, then
/// [`SYNC_BACKOFF_CAP`].
fn sync_backoff(n: u32) -> Duration {
    let cap = SYNC_BACKOFF_CAP.as_secs();
    Duration::from_secs(1u64.checked_shl(n.saturating_sub(1)).map_or(cap, |s| s.min(cap)))
}

/// Forget the backoff of devices that are connected now (an inbound reconnect counts: an
/// authenticated proof the device is there). A device whose mDNS addresses appeared or changed
/// since the last tick is due now but keeps its failure count, and at most once per
/// [`SYNC_BACKOFF_CAP`] between connections (`resets`): anyone on the LAN can send a goodbye for it
/// and replay its signed record with another address (only the port is signed), so discovery
/// alone must not set the dial rate. A device that left mDNS keeps its wait.
// ponytail: rate-limited instead of proven fresh; reset only on a newer signed `ts` once
// ucompute-network's DiscoveredPeer exposes it.
fn reset_backoff(
    redial: &mut HashMap<PeerId, (u32, Instant)>,
    resets: &mut HashMap<PeerId, Instant>,
    connected: &HashSet<PeerId>,
    seen_before: &HashMap<PeerId, Vec<SocketAddr>>,
    seen_now: &HashMap<PeerId, Vec<SocketAddr>>,
    now: Instant,
) {
    redial.retain(|id, _| !connected.contains(id));
    resets.retain(|id, _| !connected.contains(id));
    for (id, (_, next)) in redial.iter_mut() {
        let moved = seen_now.get(id).is_some_and(|a| seen_before.get(id) != Some(a));
        if moved
            && resets
                .get(id)
                .is_none_or(|t| now.duration_since(*t) >= SYNC_BACKOFF_CAP)
        {
            resets.insert(*id, now);
            *next = now;
        }
    }
}

/// The connected devices whose `last_seen_utc` is due a rewrite: never written by this loop or
/// written at least [`LAST_SEEN_EVERY`] ago (an open connection means keep-alives were answered
/// within the idle timeout). Records `now` for them and forgets devices no longer connected, so
/// peers.json is written at most once a minute per device.
fn seen_due(touched: &mut HashMap<PeerId, Instant>, live: &HashSet<PeerId>, now: Instant) -> Vec<PeerId> {
    touched.retain(|id, _| live.contains(id));
    let due: Vec<PeerId> = live
        .iter()
        .filter(|id| {
            touched
                .get(id)
                .is_none_or(|t| now.duration_since(*t) >= LAST_SEEN_EVERY)
        })
        .copied()
        .collect();
    for id in &due {
        touched.insert(*id, now);
    }
    due
}

/// ADR-0003 D7: keep a connection (and so a control loop) to every pinned device while networking
/// is on. A device that could not be reached waits [`sync_backoff`] (1, 2, 4 … 300 s) before the
/// next dial ([`reset_backoff`] says when it does not); due dials run concurrently. Connected
/// devices get `last_seen_utc` refreshed once a minute ([`seen_due`]).
// ponytail: `Node::connect` audits a failed dial once per (device, address, error code) until a
// connection succeeds, so an offline device costs one line per state change here and, at the far
// side, at most one inbound rejection per 300 s (capped at 20 lines a minute there). Spoofed mDNS
// addresses (unsigned) add a line per new address, at most two dials per 300 s ([`reset_backoff`]);
// dedupe per (device, code) in ucompute-network if that ever crowds the audit log.
// ponytail: join_all waits for the slowest due dial (at most 4 x idle); spawn per peer if one dead peer delays the others
async fn sync_loop(st: Arc<AppState>, node: Arc<Node>) {
    let mut seen_before: HashMap<PeerId, Vec<SocketAddr>> = HashMap::new();
    let mut touched: HashMap<PeerId, Instant> = HashMap::new();
    let mut resets: HashMap<PeerId, Instant> = HashMap::new();
    loop {
        let connected: HashSet<PeerId> = node.connections().iter().map(|c| c.peer_id).collect();
        let seen_now: HashMap<PeerId, Vec<SocketAddr>> = st
            .net
            .lock()
            .await
            .discovered()
            .into_iter()
            .map(|p| (p.id, p.addrs))
            .collect();
        let now = Instant::now();
        let due: Vec<PeerId> = {
            let mut redial = lock(&st.redial);
            reset_backoff(&mut redial, &mut resets, &connected, &seen_before, &seen_now, now);
            st.peers
                .list()
                .unwrap_or_default()
                .into_iter()
                .map(|p| p.id)
                .filter(|id| {
                    !connected.contains(id) && !node.is_held(id) && redial.get(id).is_none_or(|(_, next)| *next <= now)
                })
                .collect()
        };
        seen_before = seen_now;
        for id in seen_due(&mut touched, &connected, now) {
            st.touch(id, None);
        }
        let results = futures_util::future::join_all(due.iter().map(|id| st.peer_connection(&node, *id))).await;
        {
            let mut redial = lock(&st.redial);
            for (id, r) in due.into_iter().zip(results) {
                if r.is_ok() {
                    redial.remove(&id);
                } else {
                    let n = redial.get(&id).map_or(0, |(n, _)| *n) + 1;
                    redial.insert(id, (n, Instant::now() + sync_backoff(n)));
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
            .map_err(|_| UComputeError::invalid("revocation peer id must be 32 bytes"))
    };
    if m.at_utc.len() > 64 || m.signature.len() != 64 {
        return Err(UComputeError::invalid("malformed revocation"));
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
// ponytail: in memory. A record revoked before this daemon started (or by `ucompute peers revoke`
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
/// cannot undo that re-pair), the revoked device disconnected and its hold ended (a hold belongs
/// to the pairing it was set on, as for `peer_revoke`), audited once, and offered to the connected
/// peers that shared a group with the revoked device here (not back to `from`). Only a record
/// this store holds (already, or now) is marked as `from`'s ([`RevocationFanout::done`]): a peer
/// sending records that fail the checks leaves nothing behind. The held-check and the apply run
/// under `peer_writes`, so two connections delivering the same record at once write one
/// `peer_revoked` line.
async fn apply_peer_revocation(st: &AppState, from: PeerId, r: Revocation) {
    if r.peer_id == st.identity.id {
        return;
    }
    {
        let _g = lock(&st.peer_writes);
        let held = st.peers.revocations_since(0).unwrap_or_default().contains(&r);
        if !held {
            let audience = st.revocation_audience(&r.peer_id);
            if let Err(e) = st.peers.apply_revocation(&r) {
                tracing::info!(peer = %from.short_label(), code = %e.code, "revocation from peer ignored");
                return;
            }
            lock(&st.fanout).set_audience(&r, audience);
        }
        lock(&st.fanout).done(&r, from);
        if held {
            return;
        }
    }
    // On the running Node under the `net` lock, not on `node`: a restart (Sharing change) may have
    // replaced it, carried the hold over and let the device connect again before the revocation
    // was stored. A replaced `node` was shut down (all its connections closed) under that lock.
    {
        let net = st.net.lock().await;
        if let Some(n) = &net.node {
            n.disconnect(&r.peer_id, "revoked").await;
            n.release(&r.peer_id);
        }
    }
    st.audit(
        "peer_revoked",
        Some(&r.peer_id),
        json!({ "seq": r.seq, "by": r.revoked_by.short_label(), "source": "peer" }),
    );
    let _ = st.revocations.send(r);
}

/// A control message from `peer`: a revocation is applied ([`apply_peer_revocation`]); `Status`
/// and unknown messages are ignored in this minor.
async fn control_received(st: &AppState, peer: PeerId, msg: ControlMessage) {
    let Some(Body::Revocation(m)) = msg.body else {
        return;
    };
    match revocation_from_msg(m) {
        Ok(r) => apply_peer_revocation(st, peer, r).await,
        Err(e) => {
            tracing::info!(peer = %peer.short_label(), code = %e.code, "malformed revocation ignored");
        }
    }
}

/// E5 + ADR-0003 D7: a device held on either side still exchanges revocations with this one on
/// each connection it opens, before the holder closes it (`Node::hold`). Weak: the Node must not
/// keep the daemon alive.
struct HeldRevocations(std::sync::Weak<AppState>);

impl HeldControl for HeldRevocations {
    fn offer<'a>(&'a self, peer: PeerId, control: &'a mut ControlStream) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            match self.0.upgrade() {
                Some(st) => st.offer_revocations(&mut control.send, peer).await,
                None => false,
            }
        })
    }

    fn received<'a>(&'a self, _node: &'a Node, peer: PeerId, msg: ControlMessage) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            if let Some(st) = self.0.upgrade() {
                control_received(&st, peer, msg).await;
            }
        })
    }
}

/// Owns the control stream for the life of the connection: first offers every revocation this
/// store holds (ADR-0003 D7 `revocations_since` on every reconnect), then new ones (local or applied
/// from another peer), each only where [`RevocationFanout::due`] says, and applies the peer's
/// ([`control_received`]).
async fn control_loop(st: Arc<AppState>, conn: Arc<PeerConnection>) {
    let peer = conn.peer_id;
    let mut rx = st.revocations.subscribe();
    let mut guard = conn.control.lock().await;
    let ControlStream { send, recv } = &mut *guard;
    if !st.offer_revocations(send, peer).await {
        return;
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
            Ok(Some(msg)) => control_received(&st, peer, msg).await,
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
/// `ucompute run --peer` use it.
// ponytail: the request id is chosen inside `generate_remote_with_id`, after "sent" is written,
// so only "finished" carries it. A ucompute-network variant taking a caller-chosen id would let
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
    generate_remote_audited_with_id(audit, conn, model_id, model_sha256, req, privacy, cancel)
        .await
        .map(|(_, stream)| stream)
}

/// [`generate_remote_audited`], also returning the `request_id` both sides' entries (and job
/// rows) carry.
pub async fn generate_remote_audited_with_id(
    audit: Arc<Audit>,
    conn: Arc<PeerConnection>,
    model_id: &str,
    model_sha256: &str,
    req: GenerateRequest,
    privacy: &str,
    cancel: CancellationToken,
) -> Result<(String, GenerateStream)> {
    let peer = conn.peer_id;
    // Any other mode is refused inside `generate_remote_with_id` before a stream opens.
    let leaves = serde_json::from_value::<PrivacyMode>(Value::String(privacy.into()))
        .is_ok_and(|m| m == PrivacyMode::TrustedGroup);
    if leaves {
        let sent = json!({ "role": "client", "phase": "sent", "peer": peer.short_label(), "model_id": model_id });
        audit
            .record("remote_request", Some(&peer.to_hex()), sent)
            .map_err(|e| UComputeError::io(format!("audit log not writable, request not sent: {}", e.message)))?;
    }
    let mut outcome = Outcome::new(audit, peer, model_id);
    match remote::generate_remote_with_id(conn, model_id, model_sha256, req, privacy, cancel).await {
        Ok((request_id, inner)) => {
            outcome.request_id = Some(request_id.clone());
            Ok((request_id, Audited { inner, outcome }.boxed()))
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
    /// D-026: `refuse_new`, `preempt` or `none`; any other value is `invalid_request`.
    #[serde(default)]
    pub local_priority: Option<LocalPriority>,
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
        local_priority: body.local_priority.unwrap_or(s.local_priority),
        ..s
    })?;
    if after.local_priority != before.local_priority {
        st.audit(
            "local_priority_changed",
            None,
            json!({ "local_priority": after.local_priority }),
        );
    }
    // The local-priority rule is read per peer request; only the rest restarts the endpoint.
    let network_changed = Settings {
        local_priority: before.local_priority,
        ..after
    } != before;
    let running = st.net.lock().await.node.is_some();
    if network_changed || after.network_enabled != running {
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

/// Peer-supplied text shown as is: at most 64 characters.
fn cap(s: &str) -> String {
    s.chars().take(64).collect()
}

/// A peer's hardware report with every string at most 64 characters and every list at most 32
/// long: it is the peer's own JSON, re-typed, and served on every `/api/peers` poll.
fn cap_hardware(mut h: ucompute_hardware::HardwareReport) -> ucompute_hardware::HardwareReport {
    const MAX_ITEMS: usize = 32;
    let host = &mut h.host;
    for s in [&mut host.os, &mut host.os_version, &mut host.arch, &mut host.chip] {
        *s = cap(s);
    }
    h.cpu.model = cap(&h.cpu.model);
    h.cpu.arch = cap(&h.cpu.arch);
    h.cpu.features.truncate(MAX_ITEMS);
    h.cpu.features.iter_mut().for_each(|f| *f = cap(f));
    h.gpus.truncate(MAX_ITEMS);
    for g in &mut h.gpus {
        for s in [&mut g.name, &mut g.vendor, &mut g.backend] {
            *s = cap(s);
        }
    }
    h.backends.truncate(MAX_ITEMS);
    for b in &mut h.backends {
        b.name = cap(&b.name);
        b.reason = b.reason.as_deref().map(cap);
    }
    h
}

/// E1: what a connected peer signed at this connection's handshake, re-typed here (its raw JSON is
/// never passed on): hardware and free memory as measured when its networking started, engines,
/// version, and the models it caches (resolved to ids by `ids`, sha256 → this device's store or
/// catalog id; `null` for a file this device does not know). `hardware` is `null` when the peer's
/// report does not parse as this version's `HardwareReport`.
fn capabilities_json(pc: &PeerConnection, ids: &HashMap<String, String>) -> Value {
    let Some(c) = lock(&pc.capabilities).clone() else {
        return Value::Null;
    };
    let hardware = serde_json::from_str::<ucompute_hardware::HardwareReport>(&c.hardware_json)
        .ok()
        .map(cap_hardware);
    let models: Vec<Value> = c
        .cached_model_sha256
        .iter()
        .filter(|sha| sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|sha| json!({ "sha256": sha, "id": ids.get(&sha.to_ascii_lowercase()) }))
        .collect();
    json!({
        "ucompute_version": cap(&c.ucompute_version),
        "protocol_minor": pc.negotiated_minor,
        "engines": c.engines.iter().map(|e| cap(e)).collect::<Vec<_>>(),
        "issued_at_utc": ucompute_identity::format_utc(c.issued_at),
        "free_memory_bytes": c.free_memory_bytes,
        "hardware": hardware,
        "models": models,
    })
}

/// Pinned peers (with `connected`, `health`, `last_error`, `held`, their groups and, while
/// connected, `capabilities` and `connection`), peers mDNS currently sees (never this device;
/// `addrs` best first; `trusted` only when pinned here, never a trust decision by itself), those it
/// sees at no address this device can dial (`unreachable`: `id`, `reason` `link_local_only` or
/// `other_family`, `trusted`, `seen_utc`; never dialled from discovery) and the revocations this
/// store holds. `health` is `Node::health` (F-02-05): `connected`, `unhealthy`,
/// `reconnecting` or `disconnected`; `disconnected` too while networking is off or before the first
/// connection to that device. `last_error` is `Node::dial_failure`: the latest failed dial to it
/// (address, code, message, hint, time) since its last connection, `null` when there is none or
/// networking is off. `held`: disconnected here by the user (E5) until Connect or a restart.
pub async fn peers_get(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    let (node, discovered, unreachable) = {
        let net = st.net.lock().await;
        (net.node.clone(), net.discovered(), net.unreachable())
    };
    // Per peer, the connection to use comes first in `connections()`.
    let mut conns: HashMap<PeerId, Arc<PeerConnection>> = HashMap::new();
    for c in node.as_ref().map(|n| n.connections()).unwrap_or_default() {
        conns.entry(c.peer_id).or_insert(c);
    }
    let mut ids: HashMap<String, String> = st
        .catalog
        .models
        .iter()
        .map(|e| (e.sha256.to_ascii_lowercase(), e.id.clone()))
        .collect();
    // Only to name a peer's cached models: an unreadable store never fails the peer list (ids fall
    // back to the catalog or null). ponytail: the manifests are read on every poll; cache the map
    // if a store grows past a few dozen models.
    let local = st.store.list().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "model store not listed; peer model ids from the catalog only");
        vec![]
    });
    ids.extend(local.into_iter().map(|m| (m.sha256.to_ascii_lowercase(), m.id)));
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
            let conn = conns.get(&p.id);
            let mut v = json!(p);
            v["short_label"] = json!(p.id.short_label());
            v["connected"] = json!(conn.is_some());
            v["health"] = json!(health(&p.id));
            v["last_error"] = json!(node.as_ref().and_then(|n| n.dial_failure(&p.id)));
            v["held"] = json!(node.as_ref().is_some_and(|n| n.is_held(&p.id)));
            v["groups"] = json!(names);
            v["capabilities"] = conn.map_or(Value::Null, |c| capabilities_json(c, &ids));
            v["connection"] = json!(conn.map(|c| c.info()));
            v
        })
        .collect();
    Ok(axum::Json(json!({
        "peers": peers,
        "discovered": discovered,
        "unreachable": unreachable,
        "revocations": st.peers.revocations_since(0)?,
    })))
}

#[derive(Deserialize, Default)]
pub struct ConnectBody {
    /// `ip:port` where the device can be reached now, tried before its mDNS and pinned addresses.
    #[serde(default)]
    pub addr: Option<String>,
}

/// Connect to a paired device now, optionally at an address the user typed (networks that drop
/// mDNS, a device whose IP changed). Nothing is sent for an id that is not paired here. The address
/// is only a hint: the handshake must present the device's pinned key, else `peer_unauthorized`
/// and nothing is stored; `last_addr` changes only after a handshake at that address returned the
/// connection. Clears the device's redial backoff. Returns its record once connected.
pub async fn peer_connect(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ConnectBody>,
) -> std::result::Result<axum::Json<PeerRecord>, ApiError> {
    let id = st.resolve_peer(&id)?;
    paired(&st, &id)?;
    let hint = body.addr.as_deref().map(parse_dial_addr).transpose()?;
    st.on_node(|n| n.release(&id)).await?;
    let node = st.node().await?;
    lock(&st.redial).remove(&id);
    st.peer_connection_via(&node, id, hint).await?;
    Ok(axum::Json(paired(&st, &id)?))
}

/// The record of a device paired here, else `peer_unauthorized` (nothing is sent for it).
fn paired(st: &AppState, id: &PeerId) -> Result<PeerRecord> {
    st.peers.get(id)?.ok_or_else(|| {
        UComputeError::new(
            ErrorCode::PeerUnauthorized,
            format!("no paired device {}", id.short_label()),
        )
        .with_hint("pair it first")
    })
}

/// E5: close every connection to a paired device (`CLOSE_HELD`; audited `peer_disconnected`,
/// reason `by_user`) and refuse it in both directions, including this device's own requests and
/// redials, until `POST /api/peers/{id}/connect`, networking is turned off, or the service
/// restarts (a settings change that keeps networking on keeps it: [`AppState::apply_network`]).
/// Revocations still cross: each connection the device opens exchanges them before it is closed
/// (`Node::hold`). The device sees `peer_held`. Returns its record.
pub async fn peer_disconnect(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> std::result::Result<axum::Json<PeerRecord>, ApiError> {
    let id = st.resolve_peer(&id)?;
    // Checked and held under `peer_writes` (an unpair or revoke cannot run between) and the `net`
    // lock (nor a restart): a device unpaired meanwhile is never held.
    let rec = st
        .on_node(|n| {
            let _g = lock(&st.peer_writes);
            let rec = paired(&st, &id)?;
            n.hold(&id);
            Ok::<_, UComputeError>(rec)
        })
        .await??;
    Ok(axum::Json(rec))
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
    if let Ok(node) = st.node().await {
        node.disconnect(&id, "unpaired").await;
    }
    // A hold belongs to the pairing it was set on: pairing the device again starts unheld.
    let _ = st.on_node(|n| n.release(&id)).await;
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
    if let Ok(node) = st.node().await {
        node.disconnect(&id, "revoked").await;
    }
    let _ = st.on_node(|n| n.release(&id)).await;
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
    /// This device's name as the other side will see it; empty or absent: [`device_name`].
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
    let name = body.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(device_name);
    let node = st.node().await?;
    let mut slot = st.pairing.lock().await;
    if slot.as_ref().is_some_and(Pairing::active) {
        return Err(ApiError {
            error: UComputeError::new(
                ErrorCode::Overloaded,
                "a pairing session is already open; wait for it to finish or expire",
            )
            .with_hint("GET /api/pair/status"),
            status: StatusCode::CONFLICT,
        });
    }
    let bind: IpAddr = node.config.bind_ip()?.into();
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
                Err(UComputeError::new(
                    ErrorCode::Cancelled,
                    "the pairing window was closed because peer networking was turned off or restarted",
                ))
            }
            Err(e) => Err(UComputeError::internal(format!("pairing task failed: {e}"))),
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
/// outside `ucompute-network`. A specific address, or no route, returns `addr` unchanged.
// ponytail: one default-route guess; a multi-homed host gets the first route. mDNS carries the
// address the other device actually sees. `ucompute_cli::mesh::routable` is the same function and
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
    /// The 6-digit code, or the whole `ucompute-pair:v1:...` QR payload.
    pub code: String,
    /// This device's name as the other side will see it; empty or absent: [`device_name`].
    #[serde(default)]
    pub name: Option<String>,
}

/// Join a pairing window from this device's workload endpoint; on success both sides are pinned.
/// With a QR payload only the device the QR names can complete the handshake.
pub async fn pair_join(
    State(st): State<Arc<AppState>>,
    Json(body): Json<PairJoinBody>,
) -> std::result::Result<axum::Json<PeerRecord>, ApiError> {
    let name = body.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(device_name);
    let node = st.node().await?;
    let given: Option<SocketAddr> = match body.addr.as_deref() {
        Some(a) => Some(a.parse().map_err(|_| UComputeError::invalid("addr must be ip:port"))?),
        None => None,
    };
    let (addr, code, expected) = if pairing::is_qr_payload(&body.code) {
        let (id, qr_addr, secret) = pairing::expected_peer_from_qr(&body.code)?;
        (given.unwrap_or(qr_addr), secret, Some(id))
    } else {
        let addr = given.ok_or_else(|| {
            UComputeError::invalid("addr (ip:port of the other device's pairing endpoint) is required with a code")
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
        return Err(UComputeError::invalid("group name must be <= 64 bytes and members <= 256").into());
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
            return Err(UComputeError::new(ErrorCode::PolicyDenied, DISCLOSURE)
                .with_hint("resend with \"acknowledged\": true to accept this")
                .into())
        }
        PrivacyMode::SwarmAllowed => {
            return Err(UComputeError::new(
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

    /// A paired device's hardware report is its own text: every string 64 characters at most,
    /// every list 32 items at most, before `/api/peers` serves it.
    #[test]
    fn peer_hardware_text_is_bounded() {
        use ucompute_hardware::{BackendInfo, GpuInfo, HardwareReport};
        let long = "x".repeat(10_000);
        let mut h = HardwareReport::default();
        h.host.chip = long.clone();
        h.cpu.model = long.clone();
        h.cpu.features = vec![long.clone(); 500];
        h.gpus = vec![
            GpuInfo {
                name: long.clone(),
                vendor: long.clone(),
                backend: long.clone(),
                ..GpuInfo::default()
            };
            100
        ];
        h.backends = vec![
            BackendInfo {
                name: long.clone(),
                available: false,
                reason: Some(long.clone())
            };
            100
        ];
        let c = cap_hardware(h);
        assert_eq!((c.cpu.features.len(), c.gpus.len(), c.backends.len()), (32, 32, 32));
        let v = serde_json::to_value(&c).unwrap();
        fn longest(v: &Value) -> usize {
            match v {
                Value::String(s) => s.chars().count(),
                Value::Array(a) => a.iter().map(longest).max().unwrap_or(0),
                Value::Object(o) => o.values().map(longest).max().unwrap_or(0),
                _ => 0,
            }
        }
        assert_eq!(longest(&v), 64, "{v}");
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
    /// serves no peers; `ucompute serve` (4 slots, quota 4) lets peers use 3.
    #[test]
    fn peers_never_get_the_last_engine_slot() {
        let d = PeerQuota::default();
        assert_eq!(d.max_concurrent, 4);
        let api_default = crate::ApiConfig::default().max_concurrent;
        assert_eq!((api_default, d.peer_slots(api_default)), (1, 0));
        assert_eq!(d.peer_slots(4), 3, "`ucompute serve` default --max-concurrent 4");
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

    /// An AppState over a temp UCOMPUTE_HOME with networking on (loopback, no mDNS), started.
    async fn networked_state(dir: &std::path::Path) -> Arc<AppState> {
        let paths = Paths::at(dir);
        paths.ensure_dirs().unwrap();
        let backend = ucompute_runtime::backend_by_name("llama.cpp", dir.join("engine"), paths.clone()).unwrap();
        let store = ucompute_artifacts::Store::new(paths.clone());
        let manager = ucompute_runtime::manager::ModelManager::new(paths.clone(), store, backend, Default::default());
        let config = crate::ApiConfig {
            network: NetworkConfig {
                listen_port: 0,
                bind_addr: "127.0.0.1".into(),
                mdns: false,
                ..NetworkConfig::default()
            },
            ..Default::default()
        };
        let catalog = ucompute_artifacts::Catalog::builtin().unwrap();
        let st = Arc::new(AppState::new(config, paths, catalog, Arc::new(manager)).unwrap());
        st.update_settings(|s| Settings {
            network_enabled: true,
            ..s
        })
        .unwrap();
        st.apply_network().await.unwrap();
        st
    }

    fn pin_id(st: &AppState, id: PeerId) {
        st.peers
            .pin(PeerRecord {
                id,
                name: "X".into(),
                paired_at_utc: utc_now(),
                last_seen_utc: None,
                last_addr: None,
            })
            .unwrap();
    }

    /// ADR-0003 D7 bound: a peer's revocation frames leave fan-out state only for records this
    /// store holds, so a connected (or held) peer streaming well-formed but invalid records (any
    /// `seq`, a bad signature, an issuer not paired here) grows nothing; the map stays within the
    /// stored records (each issuer adds at most one per fellow group member). A valid record is
    /// applied, marked as the sender's, and ends the revoked device's hold (E5: a hold belongs to
    /// the pairing it was set on), as a local revoke does. Loopback only, nothing is dialled.
    #[tokio::test]
    async fn peer_revocations_leave_state_only_for_stored_records_and_end_a_hold() {
        let dir = tempfile::tempdir().unwrap();
        let st = networked_state(dir.path()).await;
        let node = st.node().await.unwrap();
        let (b, c) = (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap());
        let stranger = DeviceIdentity::generate().unwrap();
        for id in [b.id, c.id] {
            pin_id(&st, id);
        }
        st.peers
            .upsert_group(Group {
                name: "home".into(),
                members: vec![b.id, c.id],
                policy: Default::default(),
            })
            .unwrap();
        node.hold(&b.id);
        let signed = |by: &DeviceIdentity, seq: u64| {
            let at_utc = utc_now();
            let sig = by.sign(&ucompute_identity::revocation_message(seq, &b.id, &at_utc));
            Revocation {
                seq,
                peer_id: b.id,
                revoked_by: by.id,
                at_utc,
                signature: hex::encode(sig),
            }
        };
        for seq in 0..500u64 {
            let forged = Revocation {
                signature: hex::encode([7u8; 64]),
                ..signed(&c, seq)
            };
            control_received(&st, c.id, revocation_msg(&forged)).await;
            control_received(&st, c.id, revocation_msg(&signed(&stranger, seq))).await;
        }
        assert_eq!(lock(&st.fanout).0.len(), 0, "nothing rejected is kept");
        assert!(st.peers.is_pinned(&b.id) && node.is_held(&b.id));

        let r = signed(&c, 1);
        control_received(&st, c.id, revocation_msg(&r)).await;
        assert!(st.peers.is_revoked(&b.id));
        assert!(!node.is_held(&b.id), "the revocation ended the hold");
        control_received(&st, b.id, revocation_msg(&r)).await;
        assert_eq!(lock(&st.fanout).0.len(), 1);
        assert!(!lock(&st.fanout).due(&r, &c.id, || true), "c sent it here");
        st.stop_network().await;
    }

    /// E5: a peer's revocation applied on a Node that a restart (Sharing change) already replaced
    /// ends the hold on the running Node: the release runs there under the `net` lock, so a device
    /// no longer paired does not stay held (and start held when it is paired again). Loopback only.
    #[tokio::test]
    async fn a_peer_revocation_on_a_replaced_node_ends_the_hold_on_the_running_one() {
        let dir = tempfile::tempdir().unwrap();
        let st = networked_state(dir.path()).await;
        let old = st.node().await.unwrap();
        let (b, c) = (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap());
        for id in [b.id, c.id] {
            pin_id(&st, id);
        }
        st.peers
            .upsert_group(Group {
                name: "home".into(),
                members: vec![b.id, c.id],
                policy: Default::default(),
            })
            .unwrap();
        old.hold(&b.id);
        st.apply_network().await.unwrap();
        let running = st.node().await.unwrap();
        assert!(!Arc::ptr_eq(&old, &running) && running.is_held(&b.id));
        let at_utc = utc_now();
        let r = Revocation {
            seq: 1,
            peer_id: b.id,
            revoked_by: c.id,
            signature: hex::encode(c.sign(&ucompute_identity::revocation_message(1, &b.id, &at_utc))),
            at_utc,
        };
        control_received(&st, c.id, revocation_msg(&r)).await;
        assert!(st.peers.is_revoked(&b.id));
        assert!(!running.is_held(&b.id), "the running Node no longer holds it");
        st.stop_network().await;
    }

    /// Round-5 residual: a peer's revocation that arrives on a Node a restart (Sharing change)
    /// already replaced closes the revoked device's connection on the running Node (under the
    /// `net` lock), not only on the Node it arrived on. SINGLE-HOST (loopback, no mDNS).
    #[tokio::test(flavor = "multi_thread")]
    async fn a_peer_revocation_on_a_replaced_node_closes_the_connection_on_the_running_one_single_host() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (a, b) = (networked_state(da.path()).await, networked_state(db.path()).await);
        let c = DeviceIdentity::generate().unwrap();
        let bid = b.identity.id;
        for id in [bid, c.id] {
            pin_id(&a, id);
        }
        pin_id(&b, a.identity.id);
        a.peers
            .upsert_group(Group {
                name: "home".into(),
                members: vec![bid, c.id],
                policy: Default::default(),
            })
            .unwrap();
        let old = a.node().await.unwrap();
        a.apply_network().await.unwrap();
        let running = a.node().await.unwrap();
        assert!(!Arc::ptr_eq(&old, &running));
        let b_node = b.node().await.unwrap();
        b.peer_connection_via(&b_node, a.identity.id, Some(running.local_addr().unwrap()))
            .await
            .unwrap();
        let t0 = Instant::now();
        while !running.connections().iter().any(|x| x.peer_id == bid) {
            assert!(t0.elapsed() < Duration::from_secs(5), "B's connection registered on A");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let at_utc = utc_now();
        let r = Revocation {
            seq: 1,
            peer_id: bid,
            revoked_by: c.id,
            signature: hex::encode(c.sign(&ucompute_identity::revocation_message(1, &bid, &at_utc))),
            at_utc,
        };
        control_received(&a, c.id, revocation_msg(&r)).await;
        assert!(a.peers.is_revoked(&bid));
        assert!(
            running.connections().iter().all(|x| x.peer_id != bid),
            "the running Node closed the revoked device's connection"
        );
        a.stop_network().await;
        b.stop_network().await;
    }

    /// E5: `POST /api/peers/{id}/disconnect` checks the pairing and holds under one lock: a device
    /// unpaired after the request started but before the hold is refused (403 `peer_unauthorized`)
    /// and never held. Loopback only.
    #[tokio::test]
    async fn a_disconnect_holds_only_a_device_paired_when_held() {
        let dir = tempfile::tempdir().unwrap();
        let st = networked_state(dir.path()).await;
        let id = DeviceIdentity::generate().unwrap().id;
        pin_id(&st, id);
        let net = st.net.lock().await; // a restart or another Node call in progress
        let node = net.node.clone().unwrap();
        let req = tokio::spawn(peer_disconnect(State(st.clone()), Path(id.to_hex())));
        tokio::time::sleep(Duration::from_millis(200)).await;
        st.peers.remove(&id).unwrap(); // unpaired while the request waits
        drop(net);
        assert!(req.await.unwrap().is_err());
        assert!(!node.is_held(&id), "a device not paired is never held");
        st.stop_network().await;
    }

    /// E5: a restart that keeps networking on carries only holds on devices still paired: one
    /// unpinned by any path (a peer's revocation, an unpair racing the restart) does not come back
    /// held when it is paired again. Loopback only.
    #[tokio::test]
    async fn a_restart_drops_holds_on_devices_no_longer_paired() {
        let dir = tempfile::tempdir().unwrap();
        let st = networked_state(dir.path()).await;
        let (kept, gone) = (
            DeviceIdentity::generate().unwrap().id,
            DeviceIdentity::generate().unwrap().id,
        );
        for id in [kept, gone] {
            pin_id(&st, id);
            st.node().await.unwrap().hold(&id);
        }
        st.peers.remove(&gone).unwrap();
        st.apply_network().await.unwrap();
        pin_id(&st, gone);
        let node = st.node().await.unwrap();
        assert!(node.is_held(&kept));
        assert!(!node.is_held(&gone), "the hold belonged to the old pairing");
        st.stop_network().await;
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

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    /// D2/D3: the address the user typed, then every mDNS address, then the pinned one; each once
    /// even when not adjacent (the old `dedup` kept a repeat that was not next to its twin).
    #[test]
    fn dial_order_puts_the_manual_hint_first_and_dedupes() {
        let (h, d1, d2, p) = (
            sa("172.20.10.6:8193"),
            sa("10.0.0.9:8193"),
            sa("172.20.10.9:8193"),
            sa("192.168.1.20:8193"),
        );
        let l = sa("0.0.0.0:8193");
        assert_eq!(dial_order(Some(h), &[d1, d2], Some(p), l), vec![h, d1, d2, p]);
        assert_eq!(
            dial_order(None, &[d1, d2], Some(d1), l),
            vec![d1, d2],
            "pinned = first mDNS"
        );
        assert_eq!(dial_order(Some(p), &[d1], Some(p), l), vec![p, d1], "hint = pinned");
        assert_eq!(dial_order(Some(d2), &[d1, d2], None, l), vec![d2, d1]);
        assert_eq!(dial_order(None, &[], Some(p), l), vec![p]);
        assert_eq!(dial_order(Some(h), &[], None, l), vec![h]);
        assert!(dial_order(None, &[], None, l).is_empty());
    }

    /// An IPv4 endpoint (the default bind) cannot dial the IPv6 address mDNS also announces, so it
    /// is not a candidate: its instant "invalid remote address" would replace the IPv4 dial's
    /// "no answer … UDP port" as the device's error. A dual-stack endpoint dials both; an address
    /// the user typed is always tried.
    #[test]
    fn dial_order_leaves_out_what_the_endpoint_cannot_dial() {
        let (v4, v6) = (sa("10.0.0.5:8193"), sa("[2001:db8::5]:8193"));
        let pinned6 = sa("[fd00::5]:8193");
        assert_eq!(dial_order(None, &[v4, v6], Some(pinned6), sa("0.0.0.0:8193")), vec![v4]);
        assert_eq!(
            dial_order(None, &[v4, v6], Some(pinned6), sa("[::]:8193")),
            vec![v4, v6, pinned6]
        );
        assert_eq!(dial_order(Some(v6), &[v4], None, sa("0.0.0.0:8193")), vec![v6, v4]);
    }

    /// D2a/D5: after several failed dials the error shown is a refusal when there was one.
    #[test]
    fn prefer_refusal_over_timeout() {
        let e = |c: ErrorCode| UComputeError::new(c, c.as_str());
        let refused = e(ErrorCode::PeerUnauthorized);
        assert_eq!(prefer_refusal(None, e(ErrorCode::PeerLost)).code, ErrorCode::PeerLost);
        assert_eq!(
            prefer_refusal(Some(refused.clone()), e(ErrorCode::PeerLost)).code,
            ErrorCode::PeerUnauthorized
        );
        assert_eq!(
            prefer_refusal(Some(refused.clone()), e(ErrorCode::Timeout)).code,
            ErrorCode::PeerUnauthorized
        );
        assert_eq!(
            prefer_refusal(Some(e(ErrorCode::PeerLost)), refused.clone()).code,
            ErrorCode::PeerUnauthorized
        );
        assert_eq!(
            prefer_refusal(Some(refused), e(ErrorCode::ProtocolIncompatible)).code,
            ErrorCode::ProtocolIncompatible,
            "of two refusals, the later"
        );
        assert_eq!(
            prefer_refusal(Some(e(ErrorCode::Timeout)), e(ErrorCode::PeerLost)).code,
            ErrorCode::PeerLost
        );
    }

    /// D3b: an authenticated inbound connection teaches the device's new IP, never its source port
    /// (a `ucompute run --peer` client's port is ephemeral).
    #[test]
    fn inbound_hint_keeps_the_pinned_port() {
        let from = sa("[::ffff:172.20.10.6]:53211");
        assert_eq!(
            inbound_hint(from, Some("172.20.10.2:8193")),
            Some(sa("172.20.10.6:8193"))
        );
        assert_eq!(
            inbound_hint(sa("10.0.0.9:1"), Some("[fd00::5]:9000")),
            Some(sa("10.0.0.9:9000"))
        );
        assert_eq!(inbound_hint(from, None), None);
        assert_eq!(inbound_hint(from, Some("garbage")), None);
        assert_eq!(inbound_hint(from, Some("172.20.10.2")), None, "no port to keep");
    }

    #[test]
    fn manual_addresses_must_name_a_device() {
        assert_eq!(parse_dial_addr(" 172.20.10.6:8193 ").unwrap(), sa("172.20.10.6:8193"));
        assert_eq!(parse_dial_addr("[fd00::5]:8193").unwrap(), sa("[fd00::5]:8193"));
        for bad in [
            "",
            "172.20.10.6",
            "host.local:8193",
            "172.20.10.6:0",
            "0.0.0.0:8193",
            "[::]:8193",
            "[::ffff:0.0.0.0]:8193",
            "224.0.0.251:5353",
            "[ff02::fb]:5353",
        ] {
            assert_eq!(
                parse_dial_addr(bad).unwrap_err().code,
                ErrorCode::InvalidRequest,
                "{bad}"
            );
        }
    }

    /// E4: 1, 2, 4 … 256 s, then 300 s for good (the old cap was 30 s plus the 4 s timeout: a
    /// dial and an audit line every ~35 s forever).
    #[test]
    fn sync_backoff_is_exponential_and_capped() {
        let secs: Vec<u64> = (1..=11).map(|n| sync_backoff(n).as_secs()).collect();
        assert_eq!(secs, vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300]);
        assert_eq!(sync_backoff(0), Duration::from_secs(1));
        assert_eq!(sync_backoff(64), SYNC_BACKOFF_CAP);
        assert_eq!(sync_backoff(u32::MAX), SYNC_BACKOFF_CAP);
    }

    /// E4: a connection (either direction) clears a device's backoff. A new or changed mDNS
    /// address makes it due now but keeps its failure count; leaving mDNS changes nothing, and
    /// other devices keep theirs.
    #[test]
    fn backoff_resets_on_connection_or_discovery_change() {
        let id = |b: u8| PeerId([b; 32]);
        let (connected, appeared, moved, same, left, quiet) = (id(1), id(2), id(3), id(4), id(5), id(6));
        let t0 = Instant::now();
        let later = t0 + Duration::from_secs(300);
        let mut redial: HashMap<PeerId, (u32, Instant)> = [connected, appeared, moved, same, left, quiet]
            .into_iter()
            .map(|p| (p, (9, later)))
            .collect();
        let before: HashMap<PeerId, Vec<SocketAddr>> = [
            (moved, vec![sa("10.0.0.2:8193")]),
            (same, vec![sa("10.0.0.3:8193")]),
            (left, vec![sa("10.0.0.4:8193")]),
        ]
        .into();
        let now: HashMap<PeerId, Vec<SocketAddr>> = [
            (appeared, vec![sa("10.0.0.1:8193")]),
            (moved, vec![sa("172.20.10.6:8193"), sa("10.0.0.2:8193")]),
            (same, vec![sa("10.0.0.3:8193")]),
        ]
        .into();
        reset_backoff(&mut redial, &mut HashMap::new(), &[connected].into(), &before, &now, t0);
        let mut kept: Vec<PeerId> = redial.keys().copied().collect();
        kept.sort();
        assert_eq!(kept, vec![appeared, moved, same, left, quiet]);
        let mut due: Vec<PeerId> = redial
            .iter()
            .filter(|(_, (_, next))| *next <= t0)
            .map(|(id, _)| *id)
            .collect();
        due.sort();
        assert_eq!(due, vec![appeared, moved]);
        assert!(redial.values().all(|(n, _)| *n == 9), "failure counts kept: {redial:?}");
    }

    /// E4 against a LAN attacker: anyone can send an mDNS goodbye for a device and replay its
    /// signed record with another address (only the port is signed), so it leaves and comes back
    /// in discovery as often as the attacker likes. Ten minutes of 1 s ticks with every dial
    /// failing: at most two dials per SYNC_BACKOFF_CAP (its backoff, one discovery reset), where
    /// clearing the backoff on every reappearance dialled every other tick.
    #[test]
    fn spoofed_discovery_churn_does_not_set_the_dial_rate() {
        let x = PeerId([9; 32]);
        let t0 = Instant::now();
        let mut redial: HashMap<PeerId, (u32, Instant)> = [(x, (9, t0 + SYNC_BACKOFF_CAP))].into();
        let (mut resets, mut before) = (HashMap::new(), HashMap::new());
        let mut dials = Vec::new();
        for s in 0..600u64 {
            let now = t0 + Duration::from_secs(s);
            let seen: HashMap<PeerId, Vec<SocketAddr>> = if s % 2 == 0 {
                [(
                    x,
                    vec![SocketAddr::from(([10, 0, (s / 256) as u8, (s % 256) as u8], 8193))],
                )]
                .into()
            } else {
                HashMap::new()
            };
            reset_backoff(&mut redial, &mut resets, &HashSet::new(), &before, &seen, now);
            if redial.get(&x).is_none_or(|(_, next)| *next <= now) {
                dials.push(s);
                let n = redial.get(&x).map_or(0, |(n, _)| *n) + 1;
                redial.insert(x, (n, now + sync_backoff(n)));
            }
            before = seen;
        }
        assert!(dials.len() <= 4, "{} dials: {dials:?}", dials.len());
        // A real connection proves the device is there: the next reappearance may reset again.
        reset_backoff(
            &mut redial,
            &mut resets,
            &[x].into(),
            &before,
            &before,
            t0 + Duration::from_secs(601),
        );
        assert!(redial.is_empty() && resets.is_empty());
    }

    /// E5: a connected device's `last_seen_utc` is due at once, then not again for a minute;
    /// a device that disconnects is forgotten and is due at once when it is back.
    #[test]
    fn last_seen_refresh_is_bounded() {
        let (a, b) = (PeerId([1; 32]), PeerId([2; 32]));
        let mut touched = HashMap::new();
        let t0 = Instant::now();
        let mut due = seen_due(&mut touched, &[a, b].into(), t0);
        due.sort();
        assert_eq!(due, vec![a, b]);
        for s in [1, 30, 59] {
            assert!(
                seen_due(&mut touched, &[a, b].into(), t0 + Duration::from_secs(s)).is_empty(),
                "{s} s"
            );
        }
        assert_eq!(
            seen_due(&mut touched, &[a].into(), t0 + Duration::from_secs(60)),
            vec![a]
        );
        assert!(!touched.contains_key(&b), "b disconnected: forgotten");
        assert_eq!(
            seen_due(&mut touched, &[a, b].into(), t0 + Duration::from_secs(61)),
            vec![b]
        );
        assert!(seen_due(&mut touched, &HashSet::new(), t0 + Duration::from_secs(500)).is_empty());
        assert!(touched.is_empty());
    }

    /// E4: turning networking off and on (or any settings change that restarts it) clears the
    /// redial backoff, as the old per-loop map did, so the new sync loop dials every device now.
    #[tokio::test]
    async fn network_restart_clears_the_redial_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        paths.ensure_dirs().unwrap();
        let backend = ucompute_runtime::backend_by_name("llama.cpp", dir.path().join("engine"), paths.clone()).unwrap();
        let store = ucompute_artifacts::Store::new(paths.clone());
        let manager = ucompute_runtime::manager::ModelManager::new(paths.clone(), store, backend, Default::default());
        let catalog = ucompute_artifacts::Catalog::builtin().unwrap();
        let st = Arc::new(AppState::new(crate::ApiConfig::default(), paths, catalog, Arc::new(manager)).unwrap());
        let later = Instant::now() + SYNC_BACKOFF_CAP;
        lock(&st.redial).insert(PeerId([7; 32]), (9, later));
        st.apply_network().await.unwrap();
        assert!(lock(&st.redial).is_empty());
    }

    /// E4, SINGLE-HOST (loopback, no mDNS): `POST /api/peers/{id}/connect` clears the device's
    /// backoff, so the sync loop dials its pinned address on its next tick while the handler is
    /// still waiting on the typed address (a handshake gives up after ~3 s, RFC 9000 10.1).
    #[tokio::test(flavor = "multi_thread")]
    async fn connect_clears_the_backoff_so_the_sync_loop_dials_at_once_single_host() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        paths.ensure_dirs().unwrap();
        let backend = ucompute_runtime::backend_by_name("llama.cpp", dir.path().join("engine"), paths.clone()).unwrap();
        let store = ucompute_artifacts::Store::new(paths.clone());
        let manager = ucompute_runtime::manager::ModelManager::new(paths.clone(), store, backend, Default::default());
        let catalog = ucompute_artifacts::Catalog::builtin().unwrap();
        let config = crate::ApiConfig {
            network: NetworkConfig {
                listen_port: 0,
                bind_addr: "127.0.0.1".into(),
                mdns: false,
                idle_timeout_ms: 1000,
                ..NetworkConfig::default()
            },
            ..Default::default()
        };
        let st = Arc::new(AppState::new(config, paths, catalog, Arc::new(manager)).unwrap());
        st.update_settings(|s| Settings {
            network_enabled: true,
            ..s
        })
        .unwrap();
        st.apply_network().await.unwrap();
        let pinned = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let typed = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let x = DeviceIdentity::generate().unwrap().id;
        lock(&st.redial).insert(x, (9, Instant::now() + SYNC_BACKOFF_CAP));
        st.peers
            .pin(PeerRecord {
                id: x,
                name: "X".into(),
                paired_at_utc: utc_now(),
                last_seen_utc: None,
                last_addr: Some(pinned.local_addr().unwrap().to_string()),
            })
            .unwrap();
        let (mut b1, mut b2) = ([0u8; 2048], [0u8; 2048]);
        let held = tokio::time::timeout(Duration::from_millis(1500), pinned.recv_from(&mut b1)).await;
        assert!(held.is_err(), "the backoff holds the sync loop back");

        let body = ConnectBody {
            addr: Some(typed.local_addr().unwrap().to_string()),
        };
        let handler = tokio::spawn(peer_connect(State(st.clone()), Path(x.to_hex()), Json(body)));
        let wait = Duration::from_secs(2);
        let (t, p) = tokio::join!(
            tokio::time::timeout(wait, typed.recv_from(&mut b1)),
            tokio::time::timeout(wait, pinned.recv_from(&mut b2))
        );
        assert!(t.is_ok(), "the handler dials the typed address");
        assert!(p.is_ok(), "the sync loop dials the pinned address on its next tick");
        assert!(handler.await.unwrap().is_err(), "nothing answers at either address");
        st.stop_network().await;
    }

    #[test]
    fn device_name_is_nonempty() {
        let n = device_name();
        assert!(!n.trim().is_empty() && n == n.trim(), "{n:?}");
    }

    /// E1 on macOS: the name the user set in System Settings, not the network-dependent hostname.
    #[cfg(target_os = "macos")]
    #[test]
    fn device_name_on_macos_is_the_computer_name() {
        let out = std::process::Command::new("/usr/sbin/scutil")
            .args(["--get", "ComputerName"])
            .output()
            .unwrap();
        let set = String::from_utf8(out.stdout).unwrap();
        if out.status.success() && !set.trim().is_empty() {
            assert_eq!(device_name(), set.trim());
        }
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
            usage: ucompute_runtime::Usage {
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
        let _: Vec<_> = wrap(vec![Err(UComputeError::new(ErrorCode::PeerLost, "gone"))])
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
        assert!(DISCLOSURE.len() < ucompute_observability::MAX_FIELD_STRING);
    }
}
