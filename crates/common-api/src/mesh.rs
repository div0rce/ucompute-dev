//! Common Mesh in the daemon API (F-02-01/02/05, F-06-02, F-06-05, F-06-07, F-06-10; the backends
//! of the desktop pairing UI F-10-05 and privacy selector F-10-10).
//!
//! Secure defaults, persisted in `COMMON_HOME/config.json` (0600): privacy `local_only`, peer
//! networking off, serving peers off. Nothing binds the peer endpoint until
//! `PUT /api/network {"enabled": true}`; `PUT /api/privacy` to a non-local mode returns
//! [`DISCLOSURE`] until the caller acknowledges it; `swarm_allowed` is refused until Gate F.
//! Every off-device action is audited with ids, counts and codes only.
//!
//! Connections: the inbound accept loop and every outbound connect attach the same two tasks to a
//! `PeerConnection`: `remote::serve_peer` (default deny per request) and [`control_loop`], which
//! sends this device's revocations and applies the peer's (ADR-0003 D7), so a revocation reaches
//! every connected group member and the revoked device is disconnected everywhere.

use crate::{ApiError, AppState, Json};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_identity::{DeviceIdentity, Group, PeerId, PeerRecord, Revocation};
use common_network::discovery::{Advertiser, Browser, DiscoveredPeer, MAX_RECORD_AGE_SECS};
use common_network::pairing;
use common_network::proto::{control_message::Body, read_frame, write_frame, ControlMessage, RevocationMsg};
use common_network::transport::{ControlStream, Node, PeerConnection};
use common_network::{remote, NetworkConfig, MAX_CONTROL_FRAME};
use common_policy::{PolicyConfig, PolicyEngine, PrivacyMode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::sync::broadcast;

/// Shown before a non-local privacy mode can be selected (F-10-10, THREAT_MODEL §4). Returned by
/// `GET /api/privacy` and, as the error message, by an unacknowledged `PUT /api/privacy`.
pub const DISCLOSURE: &str = "Trusted devices: a request you send to a paired device travels encrypted, but that device \
decrypts it and runs the model on your prompt in plain text inside its own process. It can read, log or keep what you \
send and what the model answers; Common cannot prevent that. Only \"local only\" keeps everything on this computer. \
Pair only devices you trust with your prompts.";

const SETTINGS_FILE: &str = "config.json";
const DEFAULT_DEVICE_NAME: &str = "Common device";

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
}

impl Net {
    async fn stop(&mut self) {
        if let Some(t) = self.accept.take() {
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
        self.advertiser = Some(Advertiser::start(cfg, identity, node.local_addr()?.port(), pairing_port)?);
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
}

impl Pairing {
    fn active(&self) -> bool {
        lock(&self.result).is_none()
    }
}

impl AppState {
    pub fn settings(&self) -> Settings {
        *self.settings.read().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn set_settings(&self, s: Settings) {
        *self.settings.write().unwrap_or_else(PoisonError::into_inner) = s;
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

    /// Append an audit line; a failure is logged, never fatal for the request.
    pub fn audit(&self, kind: &str, peer: Option<&PeerId>, fields: Value) {
        let hex = peer.map(PeerId::to_hex);
        if let Err(e) = self.audit.record(kind, hex.as_deref(), fields) {
            tracing::debug!(kind, error = %e, "audit record not written");
        }
    }

    /// Stop whatever runs, then start the Node, discovery and the accept loop when
    /// `settings().network_enabled`. `Node::start` refuses unless the config says enabled, so a
    /// disabled setting can never leave an endpoint bound.
    pub async fn apply_network(self: &Arc<Self>) -> Result<()> {
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
        net.browser = Some(Browser::start(&cfg, self.peers.clone())?);
        net.accept = Some(tokio::spawn(accept_loop(self.clone(), node.clone())));
        net.cfg = Some(cfg);
        net.node = Some(node);
        Ok(())
    }

    pub async fn stop_network(&self) {
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
            [] => Err(CommonError::new(
                ErrorCode::PeerUnauthorized,
                format!("no paired device {label}"),
            )
            .with_hint("GET /api/peers")),
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
        self.catalog
            .get(model_id)
            .map(|e| e.sha256.clone())
            .ok_or_else(|| {
                CommonError::new(
                    ErrorCode::ModelNotFound,
                    format!("no model named {model_id} in the catalog or the local store"),
                )
                .with_hint("GET /v1/models")
            })
    }

    /// An authenticated connection to `id`: the open one when there is one, else a new connect to
    /// the pinned `last_addr` (or the address mDNS saw). Typed errors from `Node::connect`.
    pub async fn peer_connection(self: &Arc<Self>, node: &Arc<Node>, id: PeerId) -> Result<Arc<PeerConnection>> {
        if let Some(c) = node.connections().into_iter().find(|c| c.peer_id == id) {
            return Ok(c);
        }
        let addr = self.peer_addr(id).await?;
        let conn = node.connect(addr, id).await?;
        self.attach(node, conn.clone());
        Ok(conn)
    }

    async fn peer_addr(&self, id: PeerId) -> Result<SocketAddr> {
        let pinned = self
            .peers
            .get(&id)?
            .and_then(|p| p.last_addr)
            .and_then(|a| a.parse().ok());
        if let Some(a) = pinned {
            return Ok(a);
        }
        let discovered = self
            .net
            .lock()
            .await
            .discovered()
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.addr);
        discovered.ok_or_else(|| {
            CommonError::new(
                ErrorCode::PeerLost,
                format!("no known address for device {}", id.short_label()),
            )
            .with_hint("check that it is awake and on the same network")
        })
    }

    /// Serve the peer's requests (default deny per request) and run the control loop.
    fn attach(self: &Arc<Self>, node: &Arc<Node>, conn: Arc<PeerConnection>) {
        let (manager, policy, audit) = (self.manager.clone(), self.policy(), self.audit.clone());
        let served = conn.clone();
        tokio::spawn(async move {
            if let Err(e) = remote::serve_peer(served.clone(), manager, policy, audit).await {
                tracing::info!(peer = %served.peer_id.short_label(), code = %e.code, "peer connection ended");
            }
        });
        tokio::spawn(control_loop(self.clone(), node.clone(), conn));
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
        })
    }
}

/// `xxxx-xxxx-xxxx` (12 hex in three groups), lowercased; `None` for anything else.
fn short_label(s: &str) -> Option<String> {
    let ok = s.len() == 14
        && s.bytes()
            .enumerate()
            .all(|(i, b)| if i == 4 || i == 9 { b == b'-' } else { b.is_ascii_hexdigit() });
    ok.then(|| s.to_ascii_lowercase())
}

async fn accept_loop(st: Arc<AppState>, node: Arc<Node>) {
    while let Some(conn) = node.accept().await {
        st.attach(&node, conn);
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

/// Owns the control stream for the life of the connection: first sends every revocation this
/// store holds (ADR-0003 D7 `revocations_since` on reconnect), then forwards new local ones and
/// applies the peer's (signature and issuer checked by `PeerStore::apply_revocation`; the revoked
/// device is disconnected here too). `Status` and unknown messages are ignored in this minor.
async fn control_loop(st: Arc<AppState>, node: Arc<Node>, conn: Arc<PeerConnection>) {
    let mut rx = st.revocations.subscribe();
    let mut guard = conn.control.lock().await;
    let ControlStream { send, recv } = &mut *guard;
    for r in st.peers.revocations_since(0).unwrap_or_default() {
        if write_frame(send, &revocation_msg(&r)).await.is_err() {
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
                    Ok(msg) => {
                        if write_frame(send, &msg).await.is_err() {
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
            })) => {
                let applied = revocation_from_msg(m).and_then(|r| {
                    st.peers.apply_revocation(&r)?;
                    Ok(r)
                });
                match applied {
                    Ok(r) => {
                        node.disconnect(&r.peer_id, "revoked").await;
                        st.audit(
                            "peer_revoked",
                            Some(&r.peer_id),
                            json!({ "seq": r.seq, "by": r.revoked_by.short_label(), "source": "peer" }),
                        );
                    }
                    Err(e) => {
                        tracing::info!(peer = %conn.peer_id.short_label(), code = %e.code, "revocation from peer ignored");
                    }
                }
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return,
        }
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

/// Changes are persisted and the Node is restarted so every connection sees the new policy. When
/// the endpoint cannot start, networking is recorded as off and the error is returned.
pub async fn network_put(
    State(st): State<Arc<AppState>>,
    Json(body): Json<NetworkBody>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    let before = st.settings();
    let after = Settings {
        network_enabled: body.enabled.unwrap_or(before.network_enabled),
        serve_peers: body.serve_peers.unwrap_or(before.serve_peers),
        ..before
    };
    if after != before {
        st.set_settings(after);
        if let Err(e) = st.apply_network().await {
            let off = Settings {
                network_enabled: false,
                ..after
            };
            st.set_settings(off);
            off.save(&st.paths)?;
            return Err(e.into());
        }
        after.save(&st.paths)?;
        st.audit(
            "network_changed",
            None,
            json!({ "enabled": after.network_enabled, "serve_peers": after.serve_peers }),
        );
    }
    Ok(axum::Json(st.network_status().await))
}

/// Pinned peers (with `connected` and their groups), peers mDNS currently sees (`trusted` only
/// when pinned here; never a trust decision by itself) and the revocations this store holds.
pub async fn peers_get(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    let (connected, discovered) = {
        let net = st.net.lock().await;
        let connected: HashSet<PeerId> = net
            .node
            .as_ref()
            .map(|n| n.connections().iter().map(|c| c.peer_id).collect())
            .unwrap_or_default();
        (connected, net.discovered())
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
    st.peers.remove(&id)?;
    let node = st.net.lock().await.node.clone();
    if let Some(node) = node {
        node.disconnect(&id, "unpaired").await;
    }
    st.audit("peer_removed", Some(&id), json!({}));
    Ok(StatusCode::NO_CONTENT)
}

/// F-06-05: unpin, record the signed revocation, close every connection to the device with the
/// REVOKED code and send the revocation to every connected peer (their control loops apply it).
pub async fn peer_revoke(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> std::result::Result<axum::Json<Revocation>, ApiError> {
    let id = st.resolve_peer(&id)?;
    let rev = st.peers.revoke(&id, &st.identity)?;
    let node = st.net.lock().await.node.clone();
    if let Some(node) = node {
        node.disconnect(&id, "revoked").await;
    }
    let _ = st.revocations.send(revocation_msg(&rev));
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
/// 6-digit code and the QR payload; `GET /api/pair/status` reports the outcome.
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
    let (session, handle) = pairing::start(node, &name).await?;
    if let Err(e) = st.net.lock().await.advertise(st.identity.clone(), Some(session.port)) {
        tracing::warn!(error = %e, "mdns pairing advert failed");
    }
    let result = Arc::new(Mutex::new(None));
    let (st2, res2) = (st.clone(), result.clone());
    tokio::spawn(async move {
        let r = handle
            .await
            .unwrap_or_else(|e| Err(CommonError::internal(format!("pairing task failed: {e}"))));
        *lock(&res2) = Some(r);
        if let Err(e) = st2.net.lock().await.advertise(st2.identity.clone(), None) {
            tracing::warn!(error = %e, "mdns advert refresh failed");
        }
    });
    *slot = Some(Pairing {
        port: session.port,
        expires_at: session.expires_at_unix,
        result,
    });
    Ok(axum::Json(json!({
        "code": session.code,
        "port": session.port,
        "qr_payload": session.qr_payload,
        "expires_at": session.expires_at_unix,
    })))
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
    let rec = pairing::join(node, addr, &code, &name).await?;
    if expected.is_some_and(|id| id != rec.id) {
        st.peers.remove(&rec.id)?;
        return Err(CommonError::new(
            ErrorCode::PairingFailed,
            "the device that answered is not the one in the QR code; nothing was pinned",
        )
        .into());
    }
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
    let before = st.settings();
    if before.privacy != body.mode {
        let after = Settings {
            privacy: body.mode,
            ..before
        };
        after.save(&st.paths)?;
        st.set_settings(after);
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
        assert_eq!(revocation_from_msg(short_sig).unwrap_err().code, ErrorCode::InvalidRequest);
        let bad_id = RevocationMsg {
            peer_id: vec![1; 31],
            ..m
        };
        assert_eq!(revocation_from_msg(bad_id).unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn disclosure_names_the_risk() {
        assert!(DISCLOSURE.contains("plain text") && DISCLOSURE.contains("local only"));
        assert!(DISCLOSURE.len() < common_observability::MAX_FIELD_STRING);
    }
}
