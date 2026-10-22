//! The node: one quinn Endpoint for workloads (+ a temporary one for pairing, see `pairing`).
//!
//! Wire sequence after the TLS handshake (ADR-0003 D5): the connector opens the first
//! bidirectional stream (the control stream); both sides exchange `Hello` (major must match,
//! minor = min), then signed `Capabilities` (≤ 64 KiB, signature and `peer_id` checked against
//! the TLS identity). Anything else closes the connection with a typed application code.
//!
//! Inbound connections from an unvalidated source address get a QUIC Retry first (a spoofed
//! source cannot answer it), and `peer_rejected` audit lines are capped per minute, so a LAN
//! flood costs neither TLS work per spoofed packet nor unbounded audit growth. A failed outbound
//! dial is audited once per state change: again only after a connection to that peer succeeded,
//! or with another error at that address ([`Node::dial_failure`] keeps the latest).
use crate::health::{HealthTracker, PeerHealth};
use crate::proto::{
    control_message::Body, read_frame, write_frame, Capabilities, ControlMessage, Hello, SignedCapabilities,
};
use crate::tls::{self, Check, PeerPolicy, Verdict};
use crate::{NetworkConfig, ALPN_WORKLOAD, MAX_CAPABILITIES_BYTES, MAX_CONTROL_FRAME, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use common_core::{CommonError, ErrorCode, Result};
use common_identity::{utc_now, DeviceIdentity, PeerId, PeerStore};
use common_observability::Audit;
use common_policy::PolicyEngine;
use prost::Message;
use quinn::{ConnectionError, IdleTimeout, TransportConfig, TransportErrorCode, VarInt};
use serde::Serialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Application close codes (QUIC CONNECTION_CLOSE, application layer).
pub const CLOSE_OK: u32 = 0;
/// `'RE'`: the peer was revoked (F-06-05).
pub const CLOSE_REVOKED: u32 = 0x5245;
/// `'UA'`: Hello/Capabilities did not match the TLS identity.
pub const CLOSE_UNAUTHORIZED: u32 = 0x5541;
/// `'PV'`: protocol major mismatch or a malformed control exchange.
pub const CLOSE_PROTOCOL: u32 = 0x5056;
/// `'FT'`: a control frame exceeded its bound.
pub const CLOSE_FRAME_TOO_LARGE: u32 = 0x4654;
/// `'HD'`: this device's owner disconnected the peer ([`Node::hold`]); it is refused until they
/// connect it again. Read as `peer_held` by builds that know it, `peer_lost` by older ones.
pub const CLOSE_HELD: u32 = 0x4844;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// `peer_rejected` lines written per minute from inbound connections; the rest are counted.
const REJECTIONS_PER_MINUTE: u32 = 20;
/// Domain separation: a capabilities signature can never be replayed as another signed message
/// (mDNS record, revocation) of the same device key.
const CAPABILITIES_CONTEXT: &[u8] = b"common/1 capabilities\0";
/// A retired duplicate (see `Node::register`) is checked for in-flight work at this interval and
/// closed at the first check that finds none, never sooner: a request the peer opened on it just
/// before it, too, switched to the surviving connection arrives within that time and is served.
// ponytail: polling, and a request the peer opens at the very instant of an idle check is still cut
// (`peer_lost`). A GOAWAY control message telling the peer to stop using the connection closes it.
const RETIRE_POLL: Duration = Duration::from_millis(500);
/// Bound on remembered dial failures (see `Node::dial_failed`).
const MAX_DIAL_FAILURES: usize = 64;

/// A failed outbound dial ([`Node::dial_failure`]).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DialFailure {
    pub addr: SocketAddr,
    pub code: ErrorCode,
    pub message: String,
    pub hint: Option<String>,
    pub at_utc: String,
}

/// The first bidirectional stream (opened by the connector): Hello/Capabilities, then Status and
/// Revocation messages from later modules. Other work opens its own streams on `conn`.
pub struct ControlStream {
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
}

/// An authenticated connection to a pinned peer.
pub struct PeerConnection {
    /// Unique per connection in this process (never reused, unlike `quinn::Connection::stable_id`).
    pub id: u64,
    pub peer_id: PeerId,
    pub remote_addr: SocketAddr,
    /// When the control handshake finished.
    pub opened_at_utc: String,
    opened: Instant,
    pub conn: quinn::Connection,
    /// Received right after Hello; refreshed by Status messages.
    pub capabilities: std::sync::Mutex<Option<Capabilities>>,
    pub negotiated_minor: u32,
    pub control: tokio::sync::Mutex<ControlStream>,
    /// True when this node dialled it (`Node::connect`), false when it was accepted.
    pub outbound: bool,
    /// Work in flight on it, either direction ([`PeerConnection::in_flight`]).
    in_flight: AtomicUsize,
    /// Lost duplicate resolution: no longer handed out, closed by this node once idle.
    retired: AtomicBool,
}

impl PeerConnection {
    /// Marks one unit of work (a request stream, either direction) on this connection until the
    /// guard drops: a duplicate is closed only when no guard is held (see `Node::register`).
    /// `remote::generate_remote` and `remote::serve_peer_with` hold one per request stream; other
    /// modules that open their own streams on `conn` should too.
    pub fn in_flight(self: &Arc<Self>) -> InFlight {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(self.clone())
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired.load(Ordering::SeqCst)
    }

    /// Whole seconds since the control handshake finished.
    pub fn connected_s(&self) -> u64 {
        self.opened.elapsed().as_secs()
    }

    /// Who opened it, its age and its traffic now (`GET /api/peers`).
    pub fn info(&self) -> ConnectionInfo {
        let stats = self.conn.stats();
        ConnectionInfo {
            id: self.id,
            opened_at_utc: self.opened_at_utc.clone(),
            remote_addr: self.remote_addr.to_string(),
            direction: if self.outbound { "outbound" } else { "inbound" },
            rtt_ms: self.conn.rtt().as_secs_f64() * 1000.0,
            tx_bytes: stats.udp_tx.bytes,
            rx_bytes: stats.udp_rx.bytes,
            lost_packets: stats.path.lost_packets,
        }
    }
}

/// [`PeerConnection::info`]. Byte counts are UDP payload bytes since the connection opened,
/// keep-alives included; `id` changes when the connection does, so rates are taken within one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectionInfo {
    pub id: u64,
    pub opened_at_utc: String,
    pub remote_addr: String,
    pub direction: &'static str,
    pub rtt_ms: f64,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub lost_packets: u64,
}

static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);

/// Guard from [`PeerConnection::in_flight`].
pub struct InFlight(Arc<PeerConnection>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Closes a retired duplicate (`CLOSE_OK`) at the first [`RETIRE_POLL`] tick that finds no work in
/// flight on it; ends early when it closed otherwise (revoke, shutdown, the peer).
async fn close_when_idle(pc: Arc<PeerConnection>) {
    loop {
        tokio::select! {
            _ = pc.conn.closed() => return,
            _ = tokio::time::sleep(RETIRE_POLL) => {}
        }
        if pc.in_flight.load(Ordering::SeqCst) == 0 {
            tracing::debug!(peer = %pc.peer_id.short_label(), outbound = pc.outbound, "duplicate connection closed");
            pc.conn.close(VarInt::from_u32(CLOSE_OK), b"duplicate connection");
            return;
        }
    }
}

impl std::fmt::Debug for PeerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerConnection")
            .field("peer_id", &self.peer_id)
            .field("remote_addr", &self.remote_addr)
            .field("negotiated_minor", &self.negotiated_minor)
            .field("outbound", &self.outbound)
            .finish_non_exhaustive()
    }
}

pub struct Node {
    pub config: NetworkConfig,
    pub identity: Arc<DeviceIdentity>,
    pub peers: Arc<PeerStore>,
    pub policy: Arc<PolicyEngine>,
    pub audit: Arc<Audit>,
    pub endpoint: quinn::Endpoint,
    /// `common hardware --json` taken once at start; sent in every Capabilities record.
    pub hardware_json: String,
    pub free_memory_bytes: u64,
    /// SHA-256 of locally cached models, advertised in Capabilities. Filled by the daemon.
    // ponytail: the node has no model-store handle; the daemon sets this. A Status refresh later.
    pub cached_models: Mutex<Vec<String>>,
    transport: Arc<TransportConfig>,
    conns: Mutex<Vec<Arc<PeerConnection>>>,
    inbound: tokio::sync::Mutex<mpsc::Receiver<Arc<PeerConnection>>>,
    /// Per-peer liveness (F-02-05), driven by `health::watch` and `health::supervise`.
    pub(crate) health: Mutex<HashMap<PeerId, HealthTracker>>,
    /// Cancelled by `shutdown`/drop: ends watchers and supervisors.
    pub(crate) stop: CancellationToken,
    me: Weak<Node>,
    rejections: Mutex<RejectionLog>,
    /// Latest failed outbound dial per (peer, address) since the peer's last connection.
    dial_failures: Mutex<HashMap<(PeerId, SocketAddr), (DialFailure, Instant)>>,
    /// Peers this device's owner disconnected ([`Node::hold`]), each with whether a refused
    /// inbound connection from it was audited during this hold. In memory: a new Node (networking
    /// or service restart) starts empty.
    held: Mutex<HashMap<PeerId, bool>>,
}

/// Fixed one-minute window over inbound `peer_rejected` audit lines.
// ponytail: one global window, not per source address; a flooder also crowds out other rejection
// lines for the rest of that minute (the `suppressed` count says so). Per-source buckets if needed.
#[derive(Debug)]
struct RejectionLog {
    window: Instant,
    written: u32,
    suppressed: u64,
}

impl RejectionLog {
    fn new(now: Instant) -> Self {
        Self {
            window: now,
            written: 0,
            suppressed: 0,
        }
    }
    /// `Some(dropped since the last written line)` when this rejection may be written now.
    fn admit(&mut self, now: Instant) -> Option<u64> {
        if now.saturating_duration_since(self.window) >= Duration::from_secs(60) {
            self.window = now;
            self.written = 0;
        }
        if self.written >= REJECTIONS_PER_MINUTE {
            self.suppressed += 1;
            return None;
        }
        self.written += 1;
        Some(std::mem::take(&mut self.suppressed))
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// ADR-0003 D3 transport parameters: keep-alive / idle from config, windows fixed, no datagrams.
pub fn transport_config(cfg: &NetworkConfig) -> Arc<TransportConfig> {
    let mut t = TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_millis(cfg.keep_alive_ms)));
    let idle = IdleTimeout::try_from(Duration::from_millis(cfg.idle_timeout_ms))
        .unwrap_or_else(|_| IdleTimeout::from(VarInt::from_u32(4000)));
    t.max_idle_timeout(Some(idle));
    t.stream_receive_window(VarInt::from_u32(8 << 20));
    t.receive_window(VarInt::from_u32(64 << 20));
    t.max_concurrent_bidi_streams(VarInt::from_u32(64));
    t.datagram_receive_buffer_size(None);
    Arc::new(t)
}

fn short(s: &str) -> String {
    s.chars().take(32).collect()
}

/// A dial whose handshake never got an answer (quinn's idle timeout during the handshake).
fn no_answer(addr: SocketAddr, idle_timeout_ms: u64) -> CommonError {
    CommonError::new(
        ErrorCode::PeerLost,
        format!("no answer from {addr} within {} s", idle_timeout_ms.div_ceil(1000)),
    )
    .with_hint(format!(
        "the device may be asleep or on another network, or a firewall may block UDP port {}",
        addr.port()
    ))
}

/// A handshake refusal is `expected`'s verdict only once its key was `shown`: rustls checks the
/// key right before the server's signature and a bad signature closes the dial here, so a refusal
/// arriving after that came from the key's holder. Before it, anything at `addr` can send one (an
/// Initial CONNECTION_CLOSE needs only the public connection id), or it showed another key: that is
/// another device at `addr` (`peer_unauthorized`), never "revoked".
fn unproven(e: CommonError, shown: bool, addr: SocketAddr, expected: &PeerId) -> CommonError {
    if shown || !matches!(e.code, ErrorCode::PeerRevoked | ErrorCode::PeerUnauthorized) {
        return e;
    }
    tracing::debug!(%addr, code = %e.code, message = %e.message, "refusal before the pinned key was shown");
    CommonError::new(
        ErrorCode::PeerUnauthorized,
        format!("the device at {addr} did not prove it is {}", expected.short_label()),
    )
    .with_hint(
        "nothing was trusted or saved; check the address that device shows for itself (Devices in its desktop app)",
    )
}

/// A quinn connection error as a typed `CommonError`. TLS alerts carry the verifier's verdict
/// (`certificate_revoked`, `unknown_ca`, `no_application_protocol`), application close codes carry
/// ours; everything else is `PeerLost`.
pub fn map_conn_error(e: &ConnectionError) -> CommonError {
    match e {
        ConnectionError::ConnectionClosed(c) => tls_error(c.error_code, "closed by peer"),
        ConnectionError::TransportError(t) => tls_error(t.code, "closed locally"),
        ConnectionError::ApplicationClosed(a) => app_error(u64::from(a.error_code), &a.reason),
        ConnectionError::TimedOut => CommonError::new(
            ErrorCode::PeerLost,
            "idle timeout: the peer stopped answering keep-alives",
        ),
        other => CommonError::new(ErrorCode::PeerLost, other.to_string()),
    }
}

fn tls_error(code: TransportErrorCode, side: &str) -> CommonError {
    use rustls::AlertDescription as A;
    let is = |alerts: &[A]| alerts.iter().any(|a| code == TransportErrorCode::crypto((*a).into()));
    let (ec, why) = if is(&[A::CertificateRevoked]) {
        (ErrorCode::PeerRevoked, "the peer's key is revoked")
    } else if is(&[
        A::UnknownCA,
        A::BadCertificate,
        A::CertificateRequired,
        A::CertificateUnknown,
        A::UnsupportedCertificate,
        A::AccessDenied,
    ]) {
        (ErrorCode::PeerUnauthorized, "the peer's key is not pinned")
    } else if is(&[
        A::NoApplicationProtocol,
        A::ProtocolVersion,
        A::HandshakeFailure,
        A::InsufficientSecurity,
    ]) {
        (
            ErrorCode::ProtocolIncompatible,
            "no protocol in common (ALPN or TLS version); update Common on both devices",
        )
    } else {
        (ErrorCode::PeerLost, "transport error")
    };
    CommonError::new(ec, format!("{why} ({side}: {code})"))
}

fn app_error(code: u64, reason: &[u8]) -> CommonError {
    let reason = short(&String::from_utf8_lossy(reason));
    let (ec, why) = match code {
        c if c == u64::from(CLOSE_REVOKED) => (ErrorCode::PeerRevoked, "the peer revoked this device"),
        c if c == u64::from(CLOSE_UNAUTHORIZED) => (ErrorCode::PeerUnauthorized, "the peer refused this device"),
        c if c == u64::from(CLOSE_PROTOCOL) => (ErrorCode::ProtocolIncompatible, "the peer rejected the protocol"),
        c if c == u64::from(CLOSE_FRAME_TOO_LARGE) => {
            (ErrorCode::FrameTooLarge, "the peer rejected a frame as too large")
        }
        c if c == u64::from(CLOSE_HELD) => (ErrorCode::PeerHeld, "the other device disconnected this one"),
        _ => (ErrorCode::PeerLost, "closed by peer"),
    };
    CommonError::new(ec, format!("{why} (code {code:#x} {reason})"))
}

fn close_code(e: &CommonError) -> u32 {
    match e.code {
        ErrorCode::PeerRevoked => CLOSE_REVOKED,
        ErrorCode::PeerUnauthorized => CLOSE_UNAUTHORIZED,
        ErrorCode::FrameTooLarge => CLOSE_FRAME_TOO_LARGE,
        ErrorCode::PeerHeld => CLOSE_HELD,
        _ => CLOSE_PROTOCOL,
    }
}

/// Protocol negotiation (F-08-10): same major or `ProtocolIncompatible` naming both versions;
/// minor = min.
pub fn negotiate(ours: &Hello, theirs: &Hello) -> Result<u32> {
    if theirs.major != ours.major {
        return Err(CommonError::new(
            ErrorCode::ProtocolIncompatible,
            format!(
                "peer speaks protocol {}.{} (Common {}), this device speaks {}.{} (Common {}); update Common on the older device",
                theirs.major,
                theirs.minor,
                short(&theirs.common_version),
                ours.major,
                ours.minor,
                ours.common_version
            ),
        ));
    }
    Ok(ours.minor.min(theirs.minor))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `"<name> <release> <platform>"` of the pinned engine for this platform, e.g.
/// `llama.cpp b11609 macos-arm64` (catalog/backends.json). Empty when none is pinned here.
pub fn engines() -> Vec<String> {
    common_artifacts::backend::BackendPins::builtin()
        .ok()
        .and_then(|p| {
            p.for_current_platform("llama.cpp")
                .map(|b| format!("{} {} {}", b.name, b.release, b.platform))
        })
        .into_iter()
        .collect()
}

/// Build this device's signed Capabilities record from `common hardware --json` and the store.
pub fn local_capabilities(
    identity: &DeviceIdentity,
    groups: &[String],
    cached_models: &[String],
) -> Result<SignedCapabilities> {
    let hw = common_hardware::probe()?;
    sign_capabilities(
        identity,
        serde_json::to_string(&hw)?,
        hw.memory.available_bytes,
        groups,
        cached_models,
    )
}

fn capabilities_message(record: &[u8]) -> Vec<u8> {
    [CAPABILITIES_CONTEXT, record].concat()
}

/// Sign already-encoded `Capabilities` bytes as they will travel (a newer minor may carry fields
/// this build does not know; the signature covers whatever bytes are sent).
pub fn sign_capabilities_record(identity: &DeviceIdentity, record: Vec<u8>) -> SignedCapabilities {
    SignedCapabilities {
        signature: identity.sign(&capabilities_message(&record)).to_vec(),
        record,
    }
}

/// Build and sign this device's Capabilities record; refuses one over the 64 KiB frame bound.
pub fn sign_capabilities(
    identity: &DeviceIdentity,
    hardware_json: String,
    free_memory_bytes: u64,
    groups: &[String],
    cached_models: &[String],
) -> Result<SignedCapabilities> {
    let c = Capabilities {
        schema_version: 1,
        peer_id: identity.id.0.to_vec(),
        common_version: common_core::VERSION.into(),
        hardware_json,
        free_memory_bytes,
        engines: engines(),
        cached_model_sha256: cached_models.to_vec(),
        groups: groups.to_vec(),
        issued_at: now_unix(),
    };
    let signed = sign_capabilities_record(identity, c.encode_to_vec());
    let len = ControlMessage::from(Body::Capabilities(signed.clone())).encoded_len();
    if len > MAX_CAPABILITIES_BYTES {
        return Err(CommonError::new(
            ErrorCode::FrameTooLarge,
            format!("own capabilities frame is {len} bytes, bound is {MAX_CAPABILITIES_BYTES}"),
        ));
    }
    Ok(signed)
}

/// The signature must verify under the TLS identity over the exact bytes received, and only then
/// is the record decoded; its `peer_id` must be that identity too.
pub fn verify_capabilities(c: &SignedCapabilities, peer: &PeerId) -> Result<Capabilities> {
    let unauthorized = |m: &str| CommonError::new(ErrorCode::PeerUnauthorized, m);
    if !common_identity::verify(peer, &capabilities_message(&c.record), &c.signature) {
        return Err(unauthorized("capabilities signature does not verify"));
    }
    let caps = Capabilities::decode(c.record.as_slice())
        .map_err(|_| CommonError::new(ErrorCode::ProtocolIncompatible, "undecodable capabilities record"))?;
    if caps.peer_id != peer.0 {
        return Err(unauthorized("capabilities peer_id does not match the TLS identity"));
    }
    Ok(caps)
}

/// Which of this node's live connections to one peer (their `outbound` flags, oldest first) it
/// retires as duplicates. A node retires only connections it initiated: an outbound one when the
/// peer has the lower id and an inbound one exists (the peer's connection wins), or when an older
/// outbound one exists. The peer applies the same rule on its side, so exactly the connection
/// initiated by the lower id survives a crossed dial, and no side closes a connection whose
/// control handshake the other side may still be finishing.
fn duplicates_to_close(me_lower: bool, outbound: &[bool]) -> Vec<usize> {
    let inbound_exists = outbound.iter().any(|o| !o);
    let first_outbound = outbound.iter().position(|o| *o);
    (0..outbound.len())
        .filter(|&i| outbound[i] && ((!me_lower && inbound_exists) || first_outbound != Some(i)))
        .collect()
}

/// Among registered, not retired connections to `peer`, the oldest one initiated by the lower id,
/// else the oldest (`min_by_key` returns the first of equal keys).
fn survivor(conns: &[Arc<PeerConnection>], peer: &PeerId, me_lower: bool) -> Option<Arc<PeerConnection>> {
    conns
        .iter()
        .filter(|c| c.peer_id == *peer && !c.is_retired())
        .min_by_key(|c| c.outbound != me_lower)
        .cloned()
}

/// The store's current verdict on `peer` as a typed error (`Ok` only when pinned, not revoked).
fn allowed(store: &PeerStore, peer: &PeerId) -> Result<()> {
    match tls::store_verdict(store, peer) {
        Verdict::Allowed => Ok(()),
        Verdict::Revoked => Err(CommonError::new(
            ErrorCode::PeerRevoked,
            format!("peer {} is revoked", peer.short_label()),
        )),
        Verdict::Unpinned => Err(CommonError::new(
            ErrorCode::PeerUnauthorized,
            format!("peer {} is not paired", peer.short_label()),
        )),
    }
}

impl Node {
    /// Bind the workload endpoint (refuses when `config.enabled == false`), start accepting.
    pub async fn start(
        config: NetworkConfig,
        identity: Arc<DeviceIdentity>,
        peers: Arc<PeerStore>,
        policy: Arc<PolicyEngine>,
        audit: Arc<Audit>,
    ) -> Result<Arc<Node>> {
        if !config.enabled {
            return Err(CommonError::new(ErrorCode::PolicyDenied, "network disabled")
                .with_hint("set [network] enabled = true to listen for paired devices"));
        }
        let addr: SocketAddr = format!("{}:{}", config.bind_addr, config.listen_port)
            .parse()
            .map_err(|e| CommonError::invalid(format!("network bind address: {e}")))?;
        let transport = transport_config(&config);
        let mut sc = tls::server_config(&identity, PeerPolicy::Pinned(peers.clone()), ALPN_WORKLOAD)?;
        sc.transport_config(transport.clone());
        let endpoint = bind_endpoint(sc, addr).await?;
        let hw = common_hardware::probe()?;
        let hardware_json = serde_json::to_string(&hw)?;
        let (tx, rx) = mpsc::channel(16);
        let node = Arc::new_cyclic(|me| Node {
            config,
            identity,
            peers,
            policy,
            audit,
            endpoint: endpoint.clone(),
            hardware_json,
            free_memory_bytes: hw.memory.available_bytes,
            cached_models: Mutex::new(vec![]),
            transport,
            conns: Mutex::new(vec![]),
            inbound: tokio::sync::Mutex::new(rx),
            health: Mutex::new(HashMap::new()),
            stop: CancellationToken::new(),
            me: me.clone(),
            rejections: Mutex::new(RejectionLog::new(Instant::now())),
            dial_failures: Mutex::new(HashMap::new()),
            held: Mutex::new(HashMap::new()),
        });
        tracing::info!(addr = %endpoint.local_addr()?, "peer endpoint listening");
        // The loop holds only a Weak: dropping the last Node closes the endpoint (Drop) and ends it.
        let weak = Arc::downgrade(&node);
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                // Address validation before any TLS work or audit line: a sender that cannot
                // receive at its source address (spoofed Initial) never gets past the Retry.
                if !incoming.remote_address_validated() {
                    let _ = incoming.retry();
                    continue;
                }
                let Some(node) = weak.upgrade() else { break };
                let tx = tx.clone();
                tokio::spawn(async move { node.serve_incoming(incoming, tx).await });
            }
        });
        Ok(node)
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Append an audit line; a failed write is a warning (the audit trail has a gap), never fatal.
    pub(crate) fn audit(&self, kind: &str, peer: Option<&PeerId>, fields: serde_json::Value) {
        let hex = peer.map(|p| p.to_hex());
        if let Err(e) = self.audit.record(kind, hex.as_deref(), fields) {
            tracing::warn!(kind, error = %e, "audit record not written");
        }
    }

    fn hello(&self) -> Hello {
        let mut nonce = [0u8; 16];
        let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce);
        Hello {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            common_version: common_core::VERSION.into(),
            peer_id: self.identity.id.0.to_vec(),
            nonce: nonce.to_vec(),
        }
    }

    fn capabilities(&self) -> Result<SignedCapabilities> {
        // ponytail: every local group counts as "mine"; membership semantics belong to F-02-09.
        let groups: Vec<String> = self
            .peers
            .groups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.name)
            .collect();
        let cached = lock(&self.cached_models).clone();
        sign_capabilities(
            &self.identity,
            self.hardware_json.clone(),
            self.free_memory_bytes,
            &groups,
            &cached,
        )
    }

    async fn control_exchange(
        &self,
        conn: &quinn::Connection,
        peer: PeerId,
        connector: bool,
    ) -> Result<(ControlStream, Capabilities, u32)> {
        let lost = |m: &str| CommonError::new(ErrorCode::PeerLost, m);
        let (mut send, mut recv) = if connector {
            conn.open_bi().await
        } else {
            conn.accept_bi().await
        }
        .map_err(|e| map_conn_error(&e))?;
        let ours = self.hello();
        if connector {
            write_frame(&mut send, &Body::Hello(ours.clone()).into()).await?;
        }
        let theirs = match read_frame(&mut recv, MAX_CONTROL_FRAME).await? {
            Some(ControlMessage {
                body: Some(Body::Hello(h)),
            }) => h,
            Some(_) => {
                return Err(CommonError::new(
                    ErrorCode::ProtocolIncompatible,
                    "expected Hello first",
                ))
            }
            None => return Err(lost("peer closed the control stream before Hello")),
        };
        if theirs.peer_id != peer.0 {
            return Err(CommonError::new(
                ErrorCode::PeerUnauthorized,
                "Hello peer_id does not match the TLS identity",
            ));
        }
        let minor = negotiate(&ours, &theirs)?;
        if !connector {
            write_frame(&mut send, &Body::Hello(ours).into()).await?;
        }
        let mine = self.capabilities()?;
        if connector {
            write_frame(&mut send, &Body::Capabilities(mine.clone()).into()).await?;
        }
        let signed = match read_frame(&mut recv, MAX_CAPABILITIES_BYTES).await? {
            Some(ControlMessage {
                body: Some(Body::Capabilities(c)),
            }) => c,
            Some(_) => {
                return Err(CommonError::new(
                    ErrorCode::ProtocolIncompatible,
                    "expected Capabilities after Hello",
                ))
            }
            None => return Err(lost("peer closed the control stream before Capabilities")),
        };
        let caps = verify_capabilities(&signed, &peer)?;
        if !connector {
            write_frame(&mut send, &Body::Capabilities(mine).into()).await?;
        }
        Ok((ControlStream { send, recv }, caps, minor))
    }

    /// The control exchange with a deadline; on failure the connection is closed with the typed
    /// code, and a close received from the peer explains the failure instead of our read error.
    async fn finish_handshake(
        &self,
        conn: &quinn::Connection,
        peer: PeerId,
        connector: bool,
    ) -> Result<(ControlStream, Capabilities, u32)> {
        let r = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.control_exchange(conn, peer, connector))
            .await
            .unwrap_or_else(|_| Err(CommonError::new(ErrorCode::Timeout, "control handshake timed out")));
        r.map_err(|e| {
            let e = match conn.close_reason() {
                Some(reason) => map_conn_error(&reason),
                None => e,
            };
            conn.close(VarInt::from_u32(close_code(&e)), e.code.as_str().as_bytes());
            e
        })
    }

    /// The live connection to `peer` that survives duplicate resolution, if any (see `register`):
    /// the one to route new work onto. `connect` returns it without dialling.
    pub fn live(&self, peer: &PeerId) -> Option<Arc<PeerConnection>> {
        let mut g = lock(&self.conns);
        g.retain(|c| c.conn.close_reason().is_none());
        survivor(&g, peer, self.identity.id < *peer)
    }

    /// Makes the connection visible to `disconnect`, then checks the store once more: a revoke
    /// that ran between the TLS handshake and here found nothing to close (F-06-05).
    ///
    /// Duplicates (both sides dialled each other, or one side dialled twice): the connection
    /// initiated by the lower `PeerId` survives; between two from the same initiator the older.
    /// Only the initiator of the losing connection retires it, once it holds both, so the other
    /// side never sees a handshake cut short. Retired: no longer handed out here (`live`,
    /// `connect`, `connections`), still closed by `disconnect`/`shutdown`, and closed with
    /// `CLOSE_OK` once no work is in flight on it ([`PeerConnection::in_flight`]), so a request
    /// already running on it (either direction) finishes instead of failing with `peer_lost`.
    /// Returns the connection to use and whether it is `new` (false: `new` was retired).
    fn register(
        &self,
        conn: quinn::Connection,
        peer: PeerId,
        control: ControlStream,
        caps: Capabilities,
        minor: u32,
        outbound: bool,
    ) -> Result<(Arc<PeerConnection>, bool)> {
        let pc = Arc::new(PeerConnection {
            id: NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed),
            peer_id: peer,
            remote_addr: conn.remote_address(),
            opened_at_utc: utc_now(),
            opened: Instant::now(),
            conn,
            capabilities: Mutex::new(Some(caps)),
            negotiated_minor: minor,
            control: tokio::sync::Mutex::new(control),
            outbound,
            in_flight: AtomicUsize::new(0),
            retired: AtomicBool::new(false),
        });
        let me_lower = self.identity.id < peer;
        let keep = {
            let mut g = lock(&self.conns);
            g.retain(|c| c.conn.close_reason().is_none());
            g.push(pc.clone());
            let mine: Vec<Arc<PeerConnection>> = g
                .iter()
                .filter(|c| c.peer_id == peer && !c.is_retired())
                .cloned()
                .collect();
            let flags: Vec<bool> = mine.iter().map(|c| c.outbound).collect();
            for i in duplicates_to_close(me_lower, &flags) {
                mine[i].retired.store(true, Ordering::SeqCst);
                tokio::spawn(close_when_idle(mine[i].clone()));
            }
            // The survivor stays registered, so a retired duplicate's health watcher sees a live
            // connection when it closes and leaves the peer Connected. A new connection that is not
            // retired is the one returned even when it does not win (an inbound one from the
            // higher id): its initiator may already be sending work on it.
            if pc.is_retired() {
                survivor(&g, &peer, me_lower)
            } else {
                Some(pc.clone())
            }
        };
        let Some(keep) = keep else {
            return Err(CommonError::new(ErrorCode::PeerLost, "no live connection to the peer"));
        };
        // A hold placed during the handshake (E5) closes it like a revoke does.
        let refusal = match allowed(&self.peers, &peer) {
            Err(e) => Some((close_code(&e), e)),
            Ok(()) => self.refuse_held(&peer).err().map(|e| (CLOSE_HELD, e)),
        };
        if let Some((code, e)) = refusal {
            let gone: Vec<Arc<PeerConnection>> = {
                let mut g = lock(&self.conns);
                let (gone, rest) = g.drain(..).partition(|c| c.peer_id == peer);
                *g = rest;
                gone
            };
            for c in gone.iter().chain([&pc]) {
                c.conn.close(VarInt::from_u32(code), e.code.as_str().as_bytes());
            }
            return Err(e);
        }
        let is_new = Arc::ptr_eq(&keep, &pc);
        if is_new {
            self.update_health(&peer, HealthTracker::on_heartbeat);
            tokio::spawn(crate::health::watch(
                self.me.clone(),
                pc.clone(),
                Duration::from_millis(self.config.keep_alive_ms.max(100)),
            ));
        }
        Ok((keep, is_new))
    }

    pub(crate) fn update_health(&self, peer: &PeerId, f: impl FnOnce(&mut HealthTracker)) {
        f(lock(&self.health)
            .entry(*peer)
            .or_insert_with(|| HealthTracker::new(crate::health::MISS_THRESHOLD)));
    }

    /// Liveness of `peer` (F-02-05), for status endpoints and the peers list:
    /// - `Connected`: a live authenticated connection whose keep-alives are answered;
    /// - `Unhealthy`: still open but [`crate::health::MISS_THRESHOLD`] keep-alive intervals passed
    ///   without a datagram from the peer (the idle timeout closes it soon if this persists);
    /// - `Reconnecting`: the last connection was lost (idle timeout, reset) and a reconnect is
    ///   expected (`health::supervise`, or the daemon's own dial loop);
    /// - `Disconnected`: closed deliberately (revoked, refused, shut down); no reconnect follows.
    ///
    /// `None` until this node was first connected to it or asked to keep it connected
    /// (`health::supervise`). Updated within one keep-alive interval (1 s by default).
    pub fn health(&self, peer: &PeerId) -> Option<PeerHealth> {
        lock(&self.health).get(peer).map(|t| t.state)
    }

    async fn serve_incoming(self: Arc<Self>, incoming: quinn::Incoming, tx: mpsc::Sender<Arc<PeerConnection>>) {
        let addr = incoming.remote_address().to_string();
        let rejected = |peer: Option<&PeerId>, e: CommonError| {
            let Some(suppressed) = lock(&self.rejections).admit(Instant::now()) else {
                tracing::debug!(addr, code = %e.code, "peer rejected (audit line suppressed)");
                return;
            };
            tracing::info!(addr, code = %e.code, "peer rejected");
            self.audit(
                "peer_rejected",
                peer,
                serde_json::json!({ "addr": addr, "reason": e.code.as_str(), "detail": e.message, "suppressed": suppressed }),
            );
        };
        let connecting = match incoming.accept() {
            Ok(c) => c,
            Err(e) => return rejected(None, map_conn_error(&e)),
        };
        let conn = match connecting.await {
            Ok(c) => c,
            Err(e) => return rejected(None, map_conn_error(&e)),
        };
        let peer = match tls::peer_id_of(&conn) {
            Ok(p) => p,
            Err(e) => {
                conn.close(VarInt::from_u32(CLOSE_UNAUTHORIZED), b"no raw public key");
                return rejected(None, e);
            }
        };
        // The verifier decided inside the handshake; nothing (Capabilities included) is exchanged
        // with a key the store does not allow right now.
        if let Err(e) = allowed(&self.peers, &peer) {
            conn.close(VarInt::from_u32(close_code(&e)), e.code.as_str().as_bytes());
            return rejected(Some(&peer), e);
        }
        if self.is_held(&peer) {
            conn.close(VarInt::from_u32(CLOSE_HELD), b"held");
            return self.held_refused(&peer, &addr);
        }
        let registered = self
            .finish_handshake(&conn, peer, false)
            .await
            .and_then(|(control, caps, minor)| Ok((self.register(conn, peer, control, caps, minor, false)?, minor)));
        match registered {
            // An inbound connection is never retired here (only its initiator retires it), so it
            // is always handed to `accept`; a duplicate is closed by the peer once idle.
            Ok(((pc, _), minor)) => {
                self.clear_dial_failures(&peer);
                tracing::info!(peer = %peer.short_label(), addr, minor, "peer connected (inbound)");
                self.audit(
                    "peer_connected",
                    Some(&peer),
                    serde_json::json!({ "addr": addr, "direction": "inbound", "minor": minor }),
                );
                let _ = tx.send(pc).await;
            }
            Err(_) if self.is_held(&peer) => self.held_refused(&peer, &addr),
            Err(e) => rejected(Some(&peer), e),
        }
    }

    /// E5: close every connection to `peer` with [`CLOSE_HELD`] (audited `peer_disconnected`,
    /// reason `by_user`) and refuse it in both directions until [`Node::release`] or this Node
    /// stops (networking turned off or restarted, the service restarted). The hold lives in this
    /// Node's memory (owner decision 3); a process with its own Node (`common run --peer` builds a
    /// client-only one) asks the running daemon (`GET /api/peers` `held`) and holds the device in
    /// its Node before dialling, so [`Node::connect`] refuses it there too.
    pub fn hold(&self, peer: &PeerId) {
        lock(&self.held).insert(*peer, false);
        self.close_peer(peer, CLOSE_HELD, "by_user");
    }

    /// Ends a [`Node::hold`]; the next dial in either direction is admitted as usual.
    pub fn release(&self, peer: &PeerId) {
        lock(&self.held).remove(peer);
    }

    pub fn is_held(&self, peer: &PeerId) -> bool {
        lock(&self.held).contains_key(peer)
    }

    /// `PolicyDenied` while `peer` is held: no dial path of this Node (its sync loop, requests and
    /// connects through the daemon's API, the CLI's client Node) reaches it (see [`Node::hold`]).
    pub fn refuse_held(&self, peer: &PeerId) -> Result<()> {
        if !self.is_held(peer) {
            return Ok(());
        }
        Err(CommonError::new(
            ErrorCode::PolicyDenied,
            format!("{} is disconnected by you; connect it first", peer.short_label()),
        )
        .with_hint(format!("POST /api/peers/{}/connect", peer.to_hex())))
    }

    /// The `peer_rejected` line (reason `held`) for a held peer's connection, once per hold (the
    /// peer redials with backoff, and each refusal says nothing new) and within the inbound
    /// rejection cap like every other refusal.
    fn held_refused(&self, peer: &PeerId, addr: &str) {
        tracing::info!(peer = %peer.short_label(), addr, "held peer refused");
        let suppressed = {
            let mut held = lock(&self.held);
            let Some(audited) = held.get_mut(peer).filter(|a| !**a) else {
                return;
            };
            let Some(suppressed) = lock(&self.rejections).admit(Instant::now()) else {
                return;
            };
            *audited = true;
            suppressed
        };
        self.audit(
            "peer_rejected",
            Some(peer),
            serde_json::json!({ "addr": addr, "reason": "held", "detail": "disconnected by you", "suppressed": suppressed }),
        );
    }

    /// Connect to a pinned peer; Hello/Capabilities exchanged; typed errors: PeerUnauthorized,
    /// PeerRevoked, ProtocolIncompatible, PeerLost.
    ///
    /// One connection per peer: a live authenticated connection to `expected` (inbound or
    /// outbound, any address) is returned instead of dialling, and a dial that crossed the peer's
    /// own dial resolves to the connection initiated by the lower id (see `register`). So the
    /// result may be a connection `accept` also returned; callers that attach work per
    /// connection should key it by `conn.stable_id()`.
    ///
    /// A failed dial is remembered ([`Node::dial_failure`]) and audited (`peer_rejected`) once per
    /// (peer, address, code) until a connection to the peer succeeds; a refusal by `allowed` sends
    /// nothing and records nothing.
    pub async fn connect(&self, addr: SocketAddr, expected: PeerId) -> Result<Arc<PeerConnection>> {
        allowed(&self.peers, &expected)?;
        self.refuse_held(&expected)?;
        if let Some(live) = self.live(&expected) {
            return Ok(live);
        }
        match self.dial(addr, expected).await {
            Ok(pc) => {
                self.clear_dial_failures(&expected);
                Ok(pc)
            }
            Err(e) => {
                self.dial_failed(&expected, addr, &e);
                Err(e)
            }
        }
    }

    async fn dial(&self, addr: SocketAddr, expected: PeerId) -> Result<Arc<PeerConnection>> {
        // Exactly the expected key, still pinned and not revoked, checked inside the handshake.
        let store = self.peers.clone();
        let shown = Arc::new(AtomicBool::new(false));
        let seen = shown.clone();
        let check: Check = Arc::new(move |id| {
            if *id == expected {
                seen.store(true, Ordering::SeqCst);
                tls::store_verdict(&store, id)
            } else {
                Verdict::Unpinned
            }
        });
        let mut cc = tls::client_config_with(self.identity.pkcs8_der(), check, ALPN_WORKLOAD)?;
        cc.transport_config(self.transport.clone());
        let connecting = self
            .endpoint
            .connect_with(cc, addr, "common")
            .map_err(|e| CommonError::new(ErrorCode::PeerLost, format!("connect: {e}")))?;
        let conn = match connecting.await {
            Ok(c) => c,
            // The handshake never completed: nothing answered at `addr`.
            Err(ConnectionError::TimedOut) => return Err(no_answer(addr, self.config.idle_timeout_ms)),
            Err(e) => {
                return Err(unproven(
                    map_conn_error(&e),
                    shown.load(Ordering::SeqCst),
                    addr,
                    &expected,
                ))
            }
        };
        let peer = tls::peer_id_of(&conn)?;
        let registered = self
            .finish_handshake(&conn, peer, true)
            .await
            .and_then(|(control, caps, minor)| Ok((self.register(conn, peer, control, caps, minor, true)?, minor)));
        let (pc, minor) = match registered? {
            ((pc, true), minor) => (pc, minor),
            // Retired as a duplicate of the connection returned instead.
            ((pc, false), _) => return Ok(pc),
        };
        tracing::info!(peer = %peer.short_label(), %addr, minor, "peer connected (outbound)");
        self.audit(
            "peer_connected",
            Some(&peer),
            serde_json::json!({ "addr": addr.to_string(), "direction": "outbound", "minor": minor }),
        );
        Ok(pc)
    }

    /// Remembers a failed outbound dial; writes the `peer_rejected` line only when this (peer,
    /// address) had no failure with the same code since the peer's last connection.
    fn dial_failed(&self, peer: &PeerId, addr: SocketAddr, e: &CommonError) {
        let failure = DialFailure {
            addr,
            code: e.code,
            message: e.message.clone(),
            hint: e.hint.clone(),
            at_utc: utc_now(),
        };
        let repeat = {
            let mut g = lock(&self.dial_failures);
            // ponytail: cleared above 64 entries; per-peer LRU if many manual addresses ever matter
            if g.len() >= MAX_DIAL_FAILURES && !g.contains_key(&(*peer, addr)) {
                g.clear();
            }
            g.insert((*peer, addr), (failure, Instant::now()))
                .is_some_and(|(old, _)| old.code == e.code)
        };
        tracing::debug!(peer = %peer.short_label(), %addr, code = %e.code, repeat, "dial failed");
        if !repeat {
            self.audit(
                "peer_rejected",
                Some(peer),
                serde_json::json!({ "addr": addr.to_string(), "direction": "outbound", "reason": e.code.as_str() }),
            );
        }
    }

    fn clear_dial_failures(&self, peer: &PeerId) {
        lock(&self.dial_failures).retain(|(p, _), _| p != peer);
    }

    /// The most recent failed outbound dial to `peer` since its last successful connection in
    /// either direction; `None` if there was none.
    pub fn dial_failure(&self, peer: &PeerId) -> Option<DialFailure> {
        lock(&self.dial_failures)
            .iter()
            .filter(|((p, _), _)| p == peer)
            .max_by_key(|(_, (_, at))| *at)
            .map(|(_, (f, _))| f.clone())
    }

    /// Next inbound authenticated connection (after Hello/Capabilities). None when the endpoint closed.
    pub async fn accept(&self) -> Option<Arc<PeerConnection>> {
        self.inbound.lock().await.recv().await
    }

    /// Currently connected peers (closed connections are dropped from the registry here, retired
    /// duplicates are left out). Per peer, the connection to use ([`Node::live`]) comes first, so
    /// `find(|c| c.peer_id == id)` picks it.
    pub fn connections(&self) -> Vec<Arc<PeerConnection>> {
        let mut g = lock(&self.conns);
        g.retain(|c| c.conn.close_reason().is_none());
        let mut v: Vec<Arc<PeerConnection>> = g.iter().filter(|c| !c.is_retired()).cloned().collect();
        // Stable: the same order `survivor` picks by (lower id's connection first, then oldest).
        v.sort_by_key(|c| c.outbound != (self.identity.id < c.peer_id));
        v
    }

    /// Close every connection to `id` with the REVOKED close code (F-06-05, ≤ 5 s).
    pub async fn disconnect(&self, id: &PeerId, reason: &str) {
        self.close_peer(id, CLOSE_REVOKED, reason);
    }

    /// Close every connection to `id` with `code`; audited `peer_disconnected` when one was open.
    fn close_peer(&self, id: &PeerId, code: u32, reason: &str) {
        let victims: Vec<Arc<PeerConnection>> = {
            let mut g = lock(&self.conns);
            let (gone, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|c| c.peer_id == *id);
            *g = keep;
            gone
        };
        for c in &victims {
            c.conn.close(VarInt::from_u32(code), reason.as_bytes());
        }
        if !victims.is_empty() {
            self.audit(
                "peer_disconnected",
                Some(id),
                serde_json::json!({ "reason": reason, "connections": victims.len() }),
            );
        }
    }

    pub async fn shutdown(&self) {
        self.stop.cancel();
        let conns: Vec<Arc<PeerConnection>> = lock(&self.conns).drain(..).collect();
        for c in conns {
            c.conn.close(VarInt::from_u32(CLOSE_OK), b"shutdown");
        }
        self.endpoint.close(VarInt::from_u32(CLOSE_OK), b"shutdown");
        self.endpoint.wait_idle().await;
    }
}

/// How long [`Node::start`] keeps retrying a fixed port that is still held.
const REBIND_WAIT: Duration = Duration::from_secs(2);

/// Bind the peer endpoint. A restart on the same fixed port (settings change, network toggled off
/// and on) can arrive while the previous endpoint's driver is still releasing its socket, which
/// quinn does asynchronously after the last handle is dropped; retry `AddrInUse` briefly instead
/// of failing. A port held by another program still fails after [`REBIND_WAIT`] with the same error.
// ponytail: fixed 25 ms polling up to 2 s; an explicit "endpoint driver finished" signal is the upgrade.
async fn bind_endpoint(sc: quinn::ServerConfig, addr: SocketAddr) -> Result<quinn::Endpoint> {
    let deadline = Instant::now() + REBIND_WAIT;
    loop {
        match quinn::Endpoint::server(sc.clone(), addr) {
            Ok(e) => return Ok(e),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse && addr.port() != 0 && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.stop.cancel();
        self.endpoint.close(VarInt::from_u32(CLOSE_OK), b"node dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::AlertDescription as A;

    fn hello(major: u32, minor: u32) -> Hello {
        Hello {
            major,
            minor,
            common_version: "0.1.0".into(),
            ..Default::default()
        }
    }

    #[test]
    fn negotiate_major_mismatch_is_typed_and_minor_is_min() {
        let e = negotiate(&hello(1, 0), &hello(2, 0)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProtocolIncompatible);
        assert!(
            e.message.contains("2.0") && e.message.contains("1.0") && e.message.contains("update"),
            "{e}"
        );
        assert_eq!(negotiate(&hello(1, 3), &hello(1, 7)).unwrap(), 3);
        assert_eq!(negotiate(&hello(1, 7), &hello(1, 3)).unwrap(), 3);
        let mut long = hello(3, 0);
        long.common_version = "v".repeat(10_000);
        assert!(negotiate(&hello(1, 0), &long).unwrap_err().message.len() < 300);
    }

    #[test]
    fn connection_errors_map_to_stable_codes() {
        let closed = |a: A| {
            ConnectionError::ConnectionClosed(quinn::ConnectionClose {
                error_code: TransportErrorCode::crypto(a.into()),
                frame_type: None,
                reason: Default::default(),
            })
        };
        assert_eq!(
            map_conn_error(&closed(A::CertificateRevoked)).code,
            ErrorCode::PeerRevoked
        );
        assert_eq!(map_conn_error(&closed(A::UnknownCA)).code, ErrorCode::PeerUnauthorized);
        assert_eq!(
            map_conn_error(&closed(A::NoApplicationProtocol)).code,
            ErrorCode::ProtocolIncompatible
        );
        assert_eq!(map_conn_error(&closed(A::InternalError)).code, ErrorCode::PeerLost);
        assert_eq!(map_conn_error(&ConnectionError::TimedOut).code, ErrorCode::PeerLost);
        let app = |code: u32| {
            ConnectionError::ApplicationClosed(quinn::ApplicationClose {
                error_code: VarInt::from_u32(code),
                reason: b"x".as_slice().into(),
            })
        };
        assert_eq!(map_conn_error(&app(CLOSE_REVOKED)).code, ErrorCode::PeerRevoked);
        assert_eq!(
            map_conn_error(&app(CLOSE_UNAUTHORIZED)).code,
            ErrorCode::PeerUnauthorized
        );
        assert_eq!(
            map_conn_error(&app(CLOSE_PROTOCOL)).code,
            ErrorCode::ProtocolIncompatible
        );
        assert_eq!(
            map_conn_error(&app(CLOSE_FRAME_TOO_LARGE)).code,
            ErrorCode::FrameTooLarge
        );
        assert_eq!(map_conn_error(&app(0)).code, ErrorCode::PeerLost);
        // Codes round-trip through close_code.
        for (ec, code) in [
            (ErrorCode::PeerRevoked, CLOSE_REVOKED),
            (ErrorCode::PeerUnauthorized, CLOSE_UNAUTHORIZED),
            (ErrorCode::FrameTooLarge, CLOSE_FRAME_TOO_LARGE),
            (ErrorCode::ProtocolIncompatible, CLOSE_PROTOCOL),
            (ErrorCode::PeerHeld, CLOSE_HELD),
        ] {
            assert_eq!(close_code(&CommonError::new(ec, "")), code);
            assert_eq!(map_conn_error(&app(code)).code, ec);
        }
    }

    #[test]
    fn no_answer_error_names_address_and_hint() {
        let e = no_answer("10.0.0.9:8193".parse().unwrap(), 4000);
        assert_eq!(e.code, ErrorCode::PeerLost);
        assert!(e.message.contains("no answer from 10.0.0.9:8193 within 4 s"), "{e}");
        assert!(e.hint.as_deref().unwrap().contains("UDP port 8193"), "{e:?}");
        assert_eq!(map_conn_error(&ConnectionError::TimedOut).code, ErrorCode::PeerLost);
    }

    #[test]
    fn rejection_audit_lines_are_capped_per_minute() {
        let t0 = Instant::now();
        let mut log = RejectionLog::new(t0);
        for _ in 0..REJECTIONS_PER_MINUTE {
            assert_eq!(log.admit(t0), Some(0));
        }
        for _ in 0..1000 {
            assert_eq!(log.admit(t0 + Duration::from_secs(59)), None);
        }
        // The next window writes again and reports what was dropped, once.
        let t1 = t0 + Duration::from_secs(60);
        assert_eq!(log.admit(t1), Some(1000));
        assert_eq!(log.admit(t1), Some(0));
    }

    #[test]
    fn capabilities_signature_covers_the_bytes_sent() {
        let me = DeviceIdentity::generate().unwrap();
        let other = DeviceIdentity::generate().unwrap();
        let signed = sign_capabilities(&me, "{}".into(), 7, &["home".into()], &[]).unwrap();
        let caps = verify_capabilities(&signed, &me.id).unwrap();
        assert_eq!(
            (caps.free_memory_bytes, caps.groups.clone()),
            (7, vec!["home".to_string()])
        );
        // A newer minor's extra field (tag 99, varint 1) is unknown here but still verifies.
        let mut newer = caps.encode_to_vec();
        newer.extend_from_slice(&[0x98, 0x06, 0x01]);
        let newer = sign_capabilities_record(&me, newer);
        assert_eq!(verify_capabilities(&newer, &me.id).unwrap(), caps);
        let unauthorized = |s: &SignedCapabilities, id: &PeerId| verify_capabilities(s, id).unwrap_err().code;
        assert_eq!(unauthorized(&signed, &other.id), ErrorCode::PeerUnauthorized);
        let mut flipped = signed.clone();
        *flipped.record.last_mut().unwrap() ^= 1;
        assert_eq!(unauthorized(&flipped, &me.id), ErrorCode::PeerUnauthorized);
        // Signed by `other` over a record naming `me`: the signature check fails first.
        let foreign = sign_capabilities_record(&other, signed.record.clone());
        assert_eq!(unauthorized(&foreign, &me.id), ErrorCode::PeerUnauthorized);
        // Validly signed by `other`, but the record claims `me`.
        assert_eq!(unauthorized(&foreign, &other.id), ErrorCode::PeerUnauthorized);
        // A bare signature over the record without the context does not verify.
        let bare = SignedCapabilities {
            signature: me.sign(&signed.record).to_vec(),
            record: signed.record.clone(),
        };
        assert_eq!(unauthorized(&bare, &me.id), ErrorCode::PeerUnauthorized);
    }

    #[test]
    fn duplicates_resolve_to_the_lower_ids_connection() {
        const O: bool = true; // outbound: this node dialled it
        const I: bool = false; // inbound: the peer dialled it
                               // Crossed dials: the higher id closes its own outbound one, whatever the arrival order;
                               // the lower id closes nothing and waits for the peer's close of the inbound duplicate.
        assert_eq!(duplicates_to_close(false, &[O, I]), vec![0]);
        assert_eq!(duplicates_to_close(false, &[I, O]), vec![1]);
        assert!(duplicates_to_close(true, &[O, I]).is_empty());
        assert!(duplicates_to_close(true, &[I, O]).is_empty());
        // Dialled twice by this node: the newer goes, on either side of the id order.
        assert_eq!(duplicates_to_close(true, &[O, O]), vec![1]);
        assert_eq!(duplicates_to_close(false, &[O, O]), vec![1]);
        // Dialled twice by the peer: the peer decides.
        assert!(duplicates_to_close(false, &[I, I]).is_empty());
        assert!(duplicates_to_close(true, &[I, I]).is_empty());
        // Single connections are never touched.
        for me_lower in [true, false] {
            assert!(duplicates_to_close(me_lower, &[O]).is_empty());
            assert!(duplicates_to_close(me_lower, &[I]).is_empty());
        }
    }

    #[test]
    fn engines_names_the_pinned_build() {
        let e = engines();
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(e, vec!["llama.cpp b11609 macos-arm64".to_string()]);
        } else {
            assert!(e.len() <= 1);
        }
    }

    #[test]
    fn transport_config_builds_from_config() {
        let cfg = NetworkConfig {
            keep_alive_ms: 2000,
            idle_timeout_ms: 6000,
            ..NetworkConfig::default()
        };
        let _ = transport_config(&cfg);
        let _ = transport_config(&NetworkConfig {
            idle_timeout_ms: u64::MAX,
            ..cfg
        });
    }
}
