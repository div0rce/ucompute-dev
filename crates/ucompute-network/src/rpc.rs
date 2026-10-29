//! WP-04 split inference carriage (ADR-0005 D3-D7): llama.cpp RPC between this device's engine
//! and one peer's `ggml-rpc-server`, carried only inside the authenticated QUIC connection.
//!
//! **Roles.** The head H runs its split engine with `--rpc 127.0.0.1:<bridge>`.
//! [`SplitSession::open`] asks the stage host R for a session on a new stream (`RpcSessionOpen`,
//! answered by `RpcSessionAccept` or `ErrorMsg`) and then listens on the bridge port; each TCP
//! connection of H's engine to it becomes one tunnel stream (`RpcTunnelOpen`, then raw RPC bytes
//! both ways; FIN maps to FIN, an abort to TCP RST and a QUIC reset with [`RPC_RESET`]). R
//! ([`Stage`], reached through `remote::serve_peer_with`) runs one supervised rpc-server per
//! session (C3) and connects each tunnel to it.
//!
//! **Sockets.** The only module of this crate with TCP (`tests/invariants.rs`): one listener on
//! `127.0.0.1:0` (H) and connects to `127.0.0.1:<rpc-server>` (R), nothing else.
//!
//! **Owner checks.** H carries only connections whose client end the split engine's pid owns
//! (`Split::engine_pid`, set by the runtime at each start); any other is reset and audited
//! `rpc_bridge_refused`. R connects a tunnel only while every client of its rpc-server is one of
//! this daemon's own sockets, and re-checks on every tick.
//!
//! **Limits** ([`Limits`]): one session per R, [`MAX_TUNNELS`] tunnels open at once, the TTL, the
//! idle close, H's `T_rpc` silence watchdog and per-load byte cap ([`byte_cap`], `rpc_meter`), R's
//! per-session byte cap of two loads ([`stage_byte_cap`]) and address-space cap within what
//! admission found free, H's bound on concurrent owner checks, R's bounded writes to H, the
//! command allowlist and graph-op check on both sides (R enforces them against H: a hostile head
//! cannot send `COMM_INIT`), and the policy and ownership tick on both sides.
//!
//! **Teardown.** Any end resets every tunnel and the session stream; R kills rpc-server, checks
//! that no listener is left and frees its slot before it tells H why. The reason ([`SplitSession::failure`]) is this side's own when it
//! knows one (policy, hold, cap, watchdog), R's when R says why, otherwise `peer_lost`. Nothing
//! falls back to local (D7): the caller unloads the split engine and fails its requests with that
//! reason (C6).
//!
//! Audit lines carry ids, counts, bytes and durations, never RPC payload.
use crate::proto::{
    check_rpc_frame, control_message::Body, read_frame, write_frame, ControlMessage, ErrorMsg, RpcSessionAccept,
    RpcSessionOpen, RpcTunnelOpen,
};
use crate::remote::{parse_code, refuse, short};
use crate::rpc_meter::{self, Counters, Dir, Trip, Tunnel};
use crate::transport::{map_conn_error, InFlight, Node, PeerConnection};
use crate::{MAX_CONTROL_FRAME, RPC_SESSION_MINOR};
use quinn::{RecvStream, SendStream, VarInt};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use ucompute_core::{ErrorCode, Result, UComputeError};
use ucompute_identity::PeerId;
use ucompute_observability::Audit;
use ucompute_policy::{ExecutionTarget, PolicyEngine, PrivacyMode, WorkloadKind};
use ucompute_runtime::llamacpp::{process, rpc_server};
use ucompute_runtime::EnginePid;

/// `'RP'`: the one stream reset and stop code of RPC tunnels and sessions; the reason is in the
/// audit line, never on the wire.
pub const RPC_RESET: u32 = 0x5250;
/// Tunnels open at once per session (ADR-0005 D6): llama-server opens about 12 sequential
/// connections per load, one at a time.
pub const MAX_TUNNELS: usize = 8;
/// `UCOMPUTE_TEST_SINGLE_HOST_POOL=1` lets a non-Linux device host a stage, and only for a peer whose
/// address is loopback ([`single_host_pool_test`]).
pub const TEST_POOL_ENV: &str = rpc_server::TEST_POOL_ENV;
const ACCEPT_WAIT: Duration = Duration::from_secs(30);
/// R's writes on a session stream (the accept, the closing reason): a head that grants no
/// flow-control credit is a lost head, and cannot hold rpc-server or the slot.
const WRITE_WAIT: Duration = Duration::from_secs(5);
const CHUNK: usize = 64 << 10;
const SESSION_ID_BYTES: usize = 16;
/// Memory R keeps for itself at admission (D6).
const MEMORY_RESERVE: u64 = 2 << 30;

/// Byte cap per direction and load on H: `2 × est_bytes + 1 GiB`.
// ponytail: recalibrate from P1 (lab C5-NUMBERS.md: the head→R cap binds after about 500-700
// requests of R1a/R1b, before the 30 min TTL; scale it with the TTL if that is wrong for users).
pub fn byte_cap(est_bytes: u64) -> u64 {
    est_bytes.saturating_mul(2).saturating_add(1 << 30)
}

/// R's byte cap per direction and session: two loads' worth of H's cap. R sees no engine starts,
/// so it cannot count per load; two loads allow the runtime's one retry after a lost port race.
pub fn stage_byte_cap(granted: u64) -> u64 {
    byte_cap(granted).saturating_mul(2)
}

/// R is Linux only in this build (ADR-0005 D3), except a SINGLE-HOST test with a loopback peer.
fn may_host(peer: &SocketAddr) -> Result<()> {
    if cfg!(target_os = "linux") || single_host_pool_test(peer) {
        return Ok(());
    }
    Err(UComputeError::new(
        ErrorCode::PolicyDenied,
        "hosting a pooled stage is not available on this platform yet",
    ))
}

/// The test flag is set and `peer` is on loopback: a SINGLE-HOST pool test, never a real pair.
pub fn single_host_pool_test(peer: &SocketAddr) -> bool {
    peer.ip().is_loopback() && std::env::var_os(TEST_POOL_ENV).is_some_and(|v| v == "1")
}

/// Session time limits (ADR-0005 D6; `T_rpc` from the lab's `C5-NUMBERS.md`, pre-registered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub ttl: Duration,
    /// Close after this long with no tunnel open, once one has been.
    pub idle: Duration,
    /// Close when no tunnel has opened this long after the session started: the head checks the
    /// model's hash, plans and starts its engine first, which takes more than `idle` for a large
    /// model (C6: 56 s for qwen2.5-0.5b's hash in a debug build on an 8 GB Mac under memory
    /// pressure). The head's death or loss ends the session sooner, through its stream.
    pub first_idle: Duration,
    /// H: reset when R sends nothing this long while a compute or readback command waits on it.
    /// Armed only by those commands, never by the weight upload, which the load timeout bounds.
    pub t_rpc: Duration,
    /// Policy, ownership, TTL, idle and watchdog checks.
    pub tick: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(30 * 60),
            idle: Duration::from_secs(10),
            // ponytail: the split load's base timeout (C6 `SPLIT_LOAD_BASE`); tune from P1
            first_idle: Duration::from_secs(300),
            t_rpc: Duration::from_secs(10),
            tick: Duration::from_secs(1),
        }
    }
}

static BYTES_TO_R: AtomicU64 = AtomicU64::new(0);
static BYTES_TO_H: AtomicU64 = AtomicU64::new(0);

/// Plaintext RPC bytes pumped by this process, head→stage and stage→head, both roles
/// (`ucompute_rpc_tunnel_bytes_total{dir}`).
pub fn tunnel_bytes_total() -> (u64, u64) {
    (BYTES_TO_R.load(Ordering::Relaxed), BYTES_TO_H.load(Ordering::Relaxed))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn record(audit: &Audit, kind: &str, peer: &PeerId, fields: serde_json::Value) {
    if let Err(e) = audit.record(kind, Some(&peer.to_hex()), fields) {
        tracing::warn!(kind, error = %e, "audit record not written");
    }
}

/// R's reason to H, verbatim: this module's own messages and the policy's name no local path.
/// Errors from starting rpc-server can, so they go through `remote::refuse` instead.
async fn tell(send: &mut SendStream, e: &UComputeError) {
    let msg = Body::Error(ErrorMsg {
        code: e.code.as_str().into(),
        message: short(&e.message),
    })
    .into();
    if write_bounded(send, &msg).await {
        let _ = send.finish();
    }
}

/// One write of R to H within [`WRITE_WAIT`]; past it the head is lost and the stream is reset.
async fn write_bounded(send: &mut SendStream, msg: &ControlMessage) -> bool {
    let r = tokio::time::timeout(WRITE_WAIT, write_frame(send, msg)).await;
    if r.is_err() {
        let _ = send.reset(reset_code());
    }
    matches!(r, Ok(Ok(())))
}

fn reset_code() -> VarInt {
    VarInt::from_u32(RPC_RESET)
}

// ---------------------------------------------------------------- both sides

/// One session's state, shared by its loop and its tunnels.
struct Shared {
    id: Vec<u8>,
    meter: rpc_meter::Session,
    ended: CancellationToken,
    reason: OnceLock<UComputeError>,
    tunnels: Mutex<Vec<Weak<Mutex<Tunnel>>>>,
    opened: AtomicU64,
    /// When the last tunnel opened: llama-server's short connections can open and close between
    /// two ticks, and each one is activity for the idle close.
    last_open: Mutex<Option<Instant>>,
    /// H: the engine start whose connections were carried last (`EnginePid` count).
    load: AtomicU32,
    /// H: connections the bridge refused, and (when, how many) at the last `rpc_bridge_refused`
    /// line: at most one line a second, so a local flood cannot fill the audit log.
    refused: AtomicU64,
    refused_line: Mutex<(Option<Instant>, u64)>,
    started: Instant,
}

impl Shared {
    fn new(id: Vec<u8>, cap: u64) -> Arc<Self> {
        Arc::new(Self {
            id,
            meter: rpc_meter::Session::new(cap),
            ended: CancellationToken::new(),
            reason: OnceLock::new(),
            tunnels: Mutex::new(Vec::new()),
            opened: AtomicU64::new(0),
            last_open: Mutex::new(None),
            load: AtomicU32::new(0),
            refused: AtomicU64::new(0),
            refused_line: Mutex::new((None, 0)),
            started: Instant::now(),
        })
    }

    /// The first reason wins.
    fn end(&self, e: UComputeError) {
        let _ = self.reason.set(e);
        self.ended.cancel();
    }

    fn hex(&self) -> String {
        hex::encode(&self.id)
    }

    fn live(&self) -> Vec<Arc<Mutex<Tunnel>>> {
        let mut t = lock(&self.tunnels);
        t.retain(|w| w.strong_count() > 0);
        t.iter().filter_map(Weak::upgrade).collect()
    }

    /// A new tunnel's meter, or None at [`MAX_TUNNELS`].
    fn add_tunnel(&self) -> Option<(u64, Arc<Mutex<Tunnel>>)> {
        let mut t = lock(&self.tunnels);
        t.retain(|w| w.strong_count() > 0);
        if t.len() >= MAX_TUNNELS {
            return None;
        }
        let tunnel = Arc::new(Mutex::new(Tunnel::default()));
        t.push(Arc::downgrade(&tunnel));
        *lock(&self.last_open) = Some(Instant::now());
        Some((self.opened.fetch_add(1, Ordering::Relaxed) + 1, tunnel))
    }

    /// TTL, idle and (H) the silence watchdog.
    fn limits(&self, limits: &Limits, idle_since: &mut Option<Instant>, watchdog: bool) -> Option<UComputeError> {
        let now = Instant::now();
        if now >= self.started + limits.ttl {
            return Some(UComputeError::new(
                ErrorCode::Timeout,
                format!("the pooled session reached its limit of {} s", limits.ttl.as_secs()),
            ));
        }
        let live = self.live();
        let bound = if self.opened.load(Ordering::Relaxed) == 0 {
            limits.first_idle
        } else {
            limits.idle
        };
        let since = idle_since.get_or_insert(now);
        if let Some(t) = *lock(&self.last_open) {
            *since = (*since).max(t);
        }
        if !live.is_empty() {
            *idle_since = None;
        } else if now.saturating_duration_since(*since) >= bound {
            return Some(UComputeError::new(
                ErrorCode::Timeout,
                format!("the pooled session had no connection for {} s", bound.as_secs()),
            ));
        }
        let silent = live.iter().filter_map(|t| lock(t).silence(now)).max();
        match silent {
            Some(s) if watchdog && s > limits.t_rpc => Some(UComputeError::new(
                ErrorCode::Timeout,
                format!(
                    "the other device sent nothing for {} s while a pooled step waited on it",
                    s.as_secs()
                ),
            )),
            _ => None,
        }
    }

    fn close_fields(&self, role: &str, reason: &UComputeError) -> serde_json::Value {
        let c = self.meter.counters();
        serde_json::json!({
            "role": role,
            "session": self.hex(),
            "reason": reason.code.as_str(),
            "to_r": c.to_r,
            "to_h": c.to_h,
            "switches": c.switches,
            "tunnels": self.opened.load(Ordering::Relaxed),
            "refused": self.refused.load(Ordering::Relaxed),
            "duration_ms": self.started.elapsed().as_millis() as u64,
        })
    }
}

fn trip_error(t: Trip) -> UComputeError {
    match t {
        Trip::ByteCap { dir, total, cap } => UComputeError::new(
            ErrorCode::Overloaded,
            format!("pooled traffic {dir:?} reached {total} bytes, over the {cap}-byte cap"),
        ),
        Trip::Command { cmd } => UComputeError::new(
            ErrorCode::PolicyDenied,
            format!("RPC command {cmd} is not allowed in a pooled session"),
        ),
        Trip::GraphOp { op } => UComputeError::new(
            ErrorCode::PolicyDenied,
            format!("graph op {op} is not allowed in a pooled session"),
        ),
        Trip::GraphTooLarge { size } => UComputeError::new(
            ErrorCode::FrameTooLarge,
            format!("an RPC graph of {size} bytes is over the pooled session's bound"),
        ),
    }
}

/// The tunnel line's outcome for a trip: which check refused, never what the bytes were.
fn trip_outcome(t: Trip) -> &'static str {
    match t {
        Trip::ByteCap { .. } => "byte_cap",
        Trip::Command { .. } => "refused_command",
        Trip::GraphOp { .. } => "refused_graph_op",
        Trip::GraphTooLarge { .. } => "refused_graph_size",
    }
}

enum Abort {
    Trip(Trip),
    /// An I/O error or reset on either side.
    Broken,
    /// The session ended.
    Ended,
}

/// One direction: each chunk read from `from` is metered, then written to `to`; EOF finishes `to`.
async fn carry<R, W>(
    from: &mut R,
    to: &mut W,
    dir: Dir,
    tunnel: &Mutex<Tunnel>,
    shared: &Shared,
    bytes: &AtomicU64,
) -> std::result::Result<(), Abort>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = from.read(&mut buf).await.map_err(|_| Abort::Broken)?;
        if n == 0 {
            let _ = to.shutdown().await;
            return Ok(());
        }
        lock(tunnel)
            .on_chunk(&shared.meter, dir, &buf[..n], Instant::now())
            .map_err(Abort::Trip)?;
        bytes.fetch_add(n as u64, Ordering::Relaxed);
        match dir {
            Dir::ToR => &BYTES_TO_R,
            Dir::ToH => &BYTES_TO_H,
        }
        .fetch_add(n as u64, Ordering::Relaxed);
        to.write_all(&buf[..n]).await.map_err(|_| Abort::Broken)?;
    }
}

/// Which end of the tunnel the TCP connection is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    /// H: TCP is the engine (head side), QUIC goes to the stage.
    Head,
    /// R: QUIC comes from the head, TCP is rpc-server.
    Stage,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Head => "head",
            Role::Stage => "stage",
        }
    }
}

/// Pump one tunnel until both directions end, a trip, a break or the session's end. A clean end
/// propagates FIN both ways; anything else resets the QUIC stream and the TCP connection (RST).
/// Returns the outcome for the audit line and the bytes each way.
async fn run_tunnel(
    role: Role,
    tcp: TcpStream,
    mut send: SendStream,
    mut recv: RecvStream,
    tunnel: Arc<Mutex<Tunnel>>,
    shared: &Shared,
) -> (&'static str, u64, u64) {
    let _ = tcp.set_nodelay(true);
    let (mut tr, mut tw) = tcp.into_split();
    let (up, down) = (AtomicU64::new(0), AtomicU64::new(0));
    let pumped = async {
        match role {
            Role::Head => tokio::try_join!(
                carry(&mut tr, &mut send, Dir::ToR, &tunnel, shared, &up),
                carry(&mut recv, &mut tw, Dir::ToH, &tunnel, shared, &down),
            ),
            Role::Stage => tokio::try_join!(
                carry(&mut recv, &mut tw, Dir::ToR, &tunnel, shared, &up),
                carry(&mut tr, &mut send, Dir::ToH, &tunnel, shared, &down),
            ),
        }
    };
    let r = tokio::select! {
        r = pumped => r.map(|_| ()),
        _ = shared.ended.cancelled() => Err(Abort::Ended),
    };
    let outcome = match r {
        Ok(()) => "closed",
        Err(a) => {
            let _ = tw.as_ref().set_zero_linger();
            let _ = send.reset(reset_code());
            let _ = recv.stop(reset_code());
            match a {
                Abort::Trip(t) => {
                    shared.end(trip_error(t));
                    trip_outcome(t)
                }
                Abort::Broken => "reset",
                Abort::Ended => "session_ended",
            }
        }
    };
    (outcome, up.into_inner(), down.into_inner())
}

/// Reads the session stream to its end: R's reason when R sent one, None on FIN, reset or loss.
async fn session_end(recv: &mut RecvStream) -> Option<UComputeError> {
    loop {
        match read_frame(recv, MAX_CONTROL_FRAME).await {
            Ok(Some(ControlMessage {
                body: Some(Body::Error(e)),
            })) => return Some(UComputeError::new(parse_code(&e.code), short(&e.message))),
            // ponytail: nothing else is defined on a session stream; later minors may add messages
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return None,
        }
    }
}

// ---------------------------------------------------------------- H: the head

/// What H asks R to host.
#[derive(Debug, Clone, Default)]
pub struct SplitSpec {
    pub model_sha256: String,
    /// The pinned llama.cpp release on H; R refuses any other.
    pub engine_release: String,
    /// R's blocks `[layer_start, layer_end)` of `n_layer`.
    pub layer_start: u32,
    pub layer_end: u32,
    pub n_layer: u32,
    /// The bytes R will hold (its range's weights).
    pub est_bytes: u64,
    /// This device's privacy mode as it is now, read again on every tick: a change to `local_only`
    /// mid-session ends it (ADR-0005 D5).
    pub privacy: PrivacySource,
    /// The split engine whose connections the bridge carries: the same cell as the load's
    /// `Split::engine_pid`.
    pub engine: EnginePid,
    /// Per direction and load; None is [`byte_cap`]`(est_bytes)`.
    pub byte_cap: Option<u64>,
    pub limits: Limits,
}

/// Where H reads its privacy mode. `From<PrivacyMode>` gives a fixed one; the daemon passes its
/// settings.
#[derive(Clone)]
pub struct PrivacySource(Arc<dyn Fn() -> PrivacyMode + Send + Sync>);

impl PrivacySource {
    pub fn new(f: impl Fn() -> PrivacyMode + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn get(&self) -> PrivacyMode {
        (self.0)()
    }
}

impl From<PrivacyMode> for PrivacySource {
    fn from(p: PrivacyMode) -> Self {
        Self::new(move || p)
    }
}

impl Default for PrivacySource {
    fn default() -> Self {
        PrivacyMode::default().into()
    }
}

impl std::fmt::Debug for PrivacySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PrivacySource({:?})", self.get())
    }
}

/// H's side of a pooled session: the bridge listener and the session stream. Dropping it, or
/// [`SplitSession::close`], ends the session.
pub struct SplitSession {
    shared: Arc<Shared>,
    port: u16,
    accept: RpcSessionAccept,
}

impl std::fmt::Debug for SplitSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitSession")
            .field("session", &self.shared.hex())
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// H's own reason to refuse or end a session with `peer`: the policy (privacy, networking, pinned,
/// not revoked, a group with `allow_pooled_inference`) and this device's hold.
fn head_reason(node: &Node, peer: &PeerId, privacy: PrivacyMode) -> Option<UComputeError> {
    node.policy
        .evaluate(privacy, ExecutionTarget::Peer(*peer), WorkloadKind::PooledInference)
        .into_result()
        .and_then(|()| node.refuse_held(peer))
        .err()
}

/// Why the session stream or connection went away: this side's reason first, then the
/// connection's close reason, then `peer_lost`.
fn head_lost(node: &Weak<Node>, conn: &PeerConnection, privacy: PrivacyMode) -> UComputeError {
    let lost = || {
        UComputeError::new(
            ErrorCode::PeerLost,
            format!("device {} ended the pooled session", conn.peer_id.short_label()),
        )
    };
    match node.upgrade() {
        Some(n) => head_reason(&n, &conn.peer_id, privacy),
        None => None,
    }
    .or_else(|| conn.conn.close_reason().map(|r| map_conn_error(&r)))
    .unwrap_or_else(lost)
}

impl SplitSession {
    /// Ask `conn`'s peer to host blocks `[layer_start, layer_end)`, then listen on the bridge.
    /// Before any stream: the policy for pooled inference with that peer (default deny), this
    /// device's hold, a live connection, and the RPC minor on it.
    pub async fn open(node: &Arc<Node>, conn: Arc<PeerConnection>, spec: SplitSpec) -> Result<SplitSession> {
        let peer = conn.peer_id;
        if let Some(e) = head_reason(node, &peer, spec.privacy.get()) {
            return Err(e);
        }
        if let Some(r) = conn.conn.close_reason() {
            return Err(map_conn_error(&r));
        }
        if conn.negotiated_minor < RPC_SESSION_MINOR {
            return Err(UComputeError::new(
                ErrorCode::ProtocolIncompatible,
                format!(
                    "device {} runs a UCompute without pooled inference (protocol minor {}, needs {RPC_SESSION_MINOR})",
                    peer.short_label(),
                    conn.negotiated_minor
                ),
            )
            .with_hint("update UCompute on that device"));
        }
        let mut id = vec![0u8; SESSION_ID_BYTES];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut id)
            .map_err(|_| UComputeError::internal("no randomness for a session id"))?;
        let in_flight = conn.in_flight();
        let (mut send, mut recv) = conn.conn.open_bi().await.map_err(|e| map_conn_error(&e))?;
        let open = RpcSessionOpen {
            session_id: id.clone(),
            model_sha256: spec.model_sha256.clone(),
            engine_release: spec.engine_release.clone(),
            layer_start: spec.layer_start,
            layer_end: spec.layer_end,
            n_layer: spec.n_layer,
            est_bytes: spec.est_bytes,
            ttl_s: spec.limits.ttl.as_secs().min(u64::from(u32::MAX)) as u32,
        };
        let msg = Body::RpcSessionOpen(open).into();
        check_rpc_frame(&msg)?;
        write_frame(&mut send, &msg).await?;
        let label = peer.short_label();
        let answer = match tokio::time::timeout(ACCEPT_WAIT, read_frame(&mut recv, MAX_CONTROL_FRAME)).await {
            Err(_) => Err(UComputeError::new(
                ErrorCode::Timeout,
                format!(
                    "device {label} did not answer the pooled session within {} s",
                    ACCEPT_WAIT.as_secs()
                ),
            )),
            Ok(Ok(Some(m))) => check_rpc_frame(&m).and_then(|()| match m.body {
                Some(Body::RpcSessionAccept(a)) if a.session_id == id => Ok(a),
                Some(Body::Error(e)) => Err(UComputeError::new(
                    parse_code(&e.code),
                    format!("device {label}: {}", short(&e.message)),
                )),
                _ => Err(UComputeError::new(
                    ErrorCode::ProtocolIncompatible,
                    "unexpected answer to a pooled session request",
                )),
            }),
            Ok(Ok(None)) | Ok(Err(_)) => Err(head_lost(&Arc::downgrade(node), &conn, spec.privacy.get())),
        };
        let accept = match answer {
            Ok(a) => a,
            Err(e) => {
                let _ = send.reset(reset_code());
                return Err(e);
            }
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let shared = Shared::new(id, spec.byte_cap.unwrap_or_else(|| byte_cap(spec.est_bytes)));
        record(
            &node.audit,
            "session_open",
            &peer,
            serde_json::json!({
                "role": "head",
                "session": shared.hex(),
                "layer_start": spec.layer_start,
                "layer_end": spec.layer_end,
                "n_layer": spec.n_layer,
                "est_bytes": spec.est_bytes,
                "granted_bytes": accept.granted_bytes,
                "threads": accept.threads,
            }),
        );
        tokio::spawn(run_head(
            Arc::downgrade(node),
            node.audit.clone(),
            conn,
            in_flight,
            (send, recv),
            listener,
            shared.clone(),
            spec,
        ));
        Ok(SplitSession { shared, port, accept })
    }

    /// The bridge's loopback port: the split engine's `--rpc 127.0.0.1:<port>`.
    pub fn rpc_port(&self) -> u16 {
        self.port
    }

    pub fn session_id(&self) -> String {
        self.shared.hex()
    }

    /// R's grant: bytes, threads and the device its stage runs on.
    pub fn accepted(&self) -> &RpcSessionAccept {
        &self.accept
    }

    /// Plaintext RPC bytes each way and direction switches, summed over the session's tunnels
    /// (lab `SCORING-INPUT.md`: the counters P1 reads at its checkpoints).
    pub fn counters(&self) -> Counters {
        self.shared.meter.counters()
    }

    /// Why the session ended; None while it is live.
    pub fn failure(&self) -> Option<UComputeError> {
        self.shared.ended.is_cancelled().then(|| self.reason())
    }

    /// Waits for the end; the reason.
    pub async fn ended(&self) -> UComputeError {
        self.shared.ended.cancelled().await;
        self.reason()
    }

    pub fn close(&self) {
        self.shared.end(UComputeError::new(
            ErrorCode::Cancelled,
            "the pooled session was closed on this device",
        ));
    }

    fn reason(&self) -> UComputeError {
        self.shared
            .reason
            .get()
            .cloned()
            .unwrap_or_else(|| UComputeError::new(ErrorCode::PeerLost, "the pooled session ended"))
    }
}

impl Drop for SplitSession {
    fn drop(&mut self) {
        self.close();
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_head(
    node: Weak<Node>,
    audit: Arc<Audit>,
    conn: Arc<PeerConnection>,
    _in_flight: InFlight,
    (mut send, mut recv): (SendStream, RecvStream),
    listener: TcpListener,
    shared: Arc<Shared>,
    spec: SplitSpec,
) {
    let peer = conn.peer_id;
    let bridge = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let mut tick = tokio::time::interval(spec.limits.tick);
    let mut idle_since = Some(Instant::now());
    // owner checks at once (each one a `lsof` child on macOS, a /proc scan on Linux)
    let checks = Arc::new(tokio::sync::Semaphore::new(MAX_TUNNELS));
    {
        let r_end = session_end(&mut recv);
        tokio::pin!(r_end);
        loop {
            tokio::select! {
                _ = shared.ended.cancelled() => break,
                r = &mut r_end => {
                    shared.end(r.unwrap_or_else(|| head_lost(&node, &conn, spec.privacy.get())));
                    break;
                }
                a = listener.accept() => if let Ok((tcp, from)) = a {
                    match checks.clone().try_acquire_owned() {
                        Ok(check) => {
                            tokio::spawn(head_tunnel(conn.clone(), audit.clone(), tcp, bridge, from, shared.clone(), spec.engine.clone(), check));
                        }
                        Err(_) => bridge_refused(&audit, &peer, &shared, bridge, &tcp, "owner_checks_busy"),
                    }
                },
                _ = tick.tick() => {
                    let reason = match node.upgrade() {
                        None => Some(UComputeError::new(ErrorCode::PeerLost, "peer networking stopped on this device")),
                        Some(n) => head_reason(&n, &peer, spec.privacy.get()),
                    }
                    .or_else(|| conn.conn.close_reason().map(|r| map_conn_error(&r)))
                    .or_else(|| shared.limits(&spec.limits, &mut idle_since, true));
                    if let Some(e) = reason {
                        shared.end(e);
                        break;
                    }
                }
            }
        }
    }
    drop(listener);
    let _ = send.reset(reset_code());
    let _ = recv.stop(reset_code());
    let reason = shared
        .reason
        .get()
        .cloned()
        .unwrap_or_else(|| head_lost(&node, &conn, spec.privacy.get()));
    tracing::info!(peer = %peer.short_label(), session = %shared.hex(), code = %reason.code, "pooled session ended");
    record(&audit, "session_close", &peer, shared.close_fields("head", &reason));
}

/// Whether [`engine_owns`] can prove anything on this platform; a head elsewhere refuses a split
/// before any connection (`ucompute-api` `split::ensure`).
pub const HEAD_OWNER_CHECK: bool = cfg!(any(target_os = "linux", target_os = "macos"));

/// The client end `client` -> 127.0.0.1:`bridge` belongs to the split engine: its start count, else
/// None. The whole accepted address is matched, not its port: another local process can reuse the
/// engine's client port from 127.0.0.2 or a LAN address. Fails closed where no owner check exists.
fn engine_owns(engine: &EnginePid, bridge: u16, client: SocketAddr) -> Option<u32> {
    let (pid, load) = engine.get()?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let owned = ucompute_runtime::llamacpp::sockets::live::conn_peer_owned_by(pid, bridge, client).unwrap_or(false);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let owned = {
        let _ = (pid, bridge, client);
        false
    };
    owned.then_some(load)
}

/// Resets a bridge connection (RST) and audits it, one line a second at most: the line says how
/// many refusals it stands for, and the session's close line has the total.
fn bridge_refused(audit: &Audit, peer: &PeerId, shared: &Shared, bridge: u16, tcp: &TcpStream, reason: &str) {
    let _ = tcp.set_zero_linger();
    let client = tcp.peer_addr().map_or(0, |a| a.port());
    let total = shared.refused.fetch_add(1, Ordering::Relaxed) + 1;
    let now = Instant::now();
    let mut line = lock(&shared.refused_line);
    if line.0.is_some_and(|t| now < t + Duration::from_secs(1)) {
        return;
    }
    let refusals = total - line.1;
    *line = (Some(now), total);
    tracing::info!(peer = %peer.short_label(), client, reason, refusals, "pooled bridge refused a connection");
    record(
        audit,
        "rpc_bridge_refused",
        peer,
        serde_json::json!({ "session": shared.hex(), "bridge_port": bridge, "client_port": client, "reason": reason, "refusals": refusals }),
    );
}

#[allow(clippy::too_many_arguments)]
async fn head_tunnel(
    conn: Arc<PeerConnection>,
    audit: Arc<Audit>,
    tcp: TcpStream,
    bridge: u16,
    from: SocketAddr,
    shared: Arc<Shared>,
    engine: EnginePid,
    check: tokio::sync::OwnedSemaphorePermit,
) {
    let peer = conn.peer_id;
    let refused = |reason: &str| bridge_refused(&audit, &peer, &shared, bridge, &tcp, reason);
    let owner = tokio::task::spawn_blocking(move || engine_owns(&engine, bridge, from))
        .await
        .ok()
        .flatten();
    drop(check);
    let Some(load) = owner else {
        return refused("not_the_engine");
    };
    if shared.ended.is_cancelled() {
        return refused("session_ended");
    }
    let Some((n, tunnel)) = shared.add_tunnel() else {
        return refused("tunnel_cap");
    };
    // a new engine start is a new load: its weight upload gets the whole byte cap
    if shared.load.swap(load, Ordering::SeqCst) != load {
        shared.meter.new_load();
    }
    let (mut send, recv) = match conn.conn.open_bi().await {
        Ok(s) => s,
        Err(_) => return refused("stream_failed"),
    };
    let msg = Body::RpcTunnelOpen(RpcTunnelOpen {
        session_id: shared.id.clone(),
    })
    .into();
    if write_frame(&mut send, &msg).await.is_err() {
        return refused("stream_failed");
    }
    tunnel_lines(&audit, &peer, &shared, n, Role::Head, async {
        run_tunnel(Role::Head, tcp, send, recv, tunnel, &shared).await
    })
    .await;
}

/// `rpc_tunnel_open`, the pump, `rpc_tunnel_close` with its bytes and duration.
async fn tunnel_lines(
    audit: &Audit,
    peer: &PeerId,
    shared: &Shared,
    n: u64,
    role: Role,
    pump: impl std::future::Future<Output = (&'static str, u64, u64)>,
) {
    let started = Instant::now();
    let session = shared.hex();
    let open = serde_json::json!({ "role": role.as_str(), "session": session, "tunnel": n });
    record(audit, "rpc_tunnel_open", peer, open);
    let (outcome, up, down) = pump.await;
    record(
        audit,
        "rpc_tunnel_close",
        peer,
        serde_json::json!({
            "role": role.as_str(),
            "session": session,
            "tunnel": n,
            "outcome": outcome,
            "to_r": up,
            "to_h": down,
            "duration_ms": started.elapsed().as_millis() as u64,
        }),
    );
}

// ---------------------------------------------------------------- R: the stage host

/// This device as a stage host: one rpc-server per session, one session at a time. Build one per
/// daemon and share it through `ServeOptions::rpc`.
pub struct Stage {
    pub engine_dir: PathBuf,
    pub logs: PathBuf,
    /// This device's pinned llama.cpp release; a head with another is refused.
    pub release: String,
    /// rpc-server's `-t` (`rpc_server::stage_threads`).
    pub threads: u32,
    /// Bytes available now (`ucompute_hardware::available_memory`).
    pub available_memory: fn() -> Option<u64>,
    pub limits: Limits,
    /// Per direction and session; None is [`stage_byte_cap`]`(granted)`.
    pub byte_cap: Option<u64>,
    busy: AtomicBool,
    session: Mutex<Option<Arc<StageSession>>>,
}

struct StageSession {
    shared: Arc<Shared>,
    conn: usize,
    peer: PeerId,
    port: u16,
    pid: u32,
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("release", &self.release)
            .field("threads", &self.threads)
            .finish_non_exhaustive()
    }
}

/// Frees the one session slot on drop.
struct Slot<'a>(&'a Stage);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *lock(&self.0.session) = None;
        self.0.busy.store(false, Ordering::SeqCst);
    }
}

impl Stage {
    pub fn new(engine_dir: PathBuf, logs: PathBuf, release: String, threads: u32) -> Self {
        Self {
            engine_dir,
            logs,
            release,
            threads,
            available_memory: ucompute_hardware::available_memory,
            limits: Limits::default(),
            byte_cap: None,
            busy: AtomicBool::new(false),
            session: Mutex::new(None),
        }
    }

    /// The live session's rpc-server: (pid, port).
    pub fn session(&self) -> Option<(u32, u16)> {
        lock(&self.session).as_ref().map(|s| (s.pid, s.port))
    }

    /// Admission in ADR-0005's order; the session slot and rpc-server's address-space cap on success.
    fn admit(&self, conn: &PeerConnection, open: &RpcSessionOpen) -> Result<(Slot<'_>, u64)> {
        if open.session_id.len() != SESSION_ID_BYTES
            || open.layer_start >= open.layer_end
            || open.layer_end > open.n_layer
        {
            return Err(UComputeError::invalid(
                "a pooled session needs a 16-byte id and a block range inside the model",
            ));
        }
        may_host(&conn.remote_addr)?;
        if open.engine_release != self.release {
            return Err(UComputeError::new(
                ErrorCode::ProtocolIncompatible,
                format!(
                    "this device runs llama.cpp {}, the head runs {}; pooling needs the same release",
                    self.release,
                    short(&open.engine_release)
                ),
            ));
        }
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(UComputeError::new(
                ErrorCode::Overloaded,
                "this device already hosts a pooled stage; retry later",
            ));
        }
        let slot = Slot(self);
        let cap = memory_grant(open.est_bytes, (self.available_memory)().unwrap_or(0))?;
        Ok((slot, cap))
    }

    fn lookup(&self, id: &[u8], conn: usize, peer: &PeerId) -> Option<Arc<StageSession>> {
        lock(&self.session)
            .as_ref()
            .filter(|s| s.shared.id == id && s.conn == conn && s.peer == *peer)
            .cloned()
    }
}

/// R's address-space cap for its rpc-server.
// ponytail: granted × 1.5 + 1 GiB; C6 replaces it with granted + the range's largest tensor (held
// twice during SET_TENSOR) + the measured idle baseline and thread stacks (ADR-0005 D3).
fn mem_cap(granted: u64) -> u64 {
    granted.saturating_add(granted / 2).saturating_add(1 << 30)
}

/// Admission's memory rule and rpc-server's cap from the same figure (D6): `est × 1.1` must fit in
/// `available` less [`MEMORY_RESERVE`], and the cap is [`mem_cap`] clamped to that room, so no
/// `est_bytes` a head claims lets rpc-server commit into the reserve.
fn memory_grant(est: u64, available: u64) -> Result<u64> {
    let need = est.saturating_add(est / 10);
    let have = available.saturating_sub(MEMORY_RESERVE);
    if need > have {
        return Err(UComputeError::new(
            ErrorCode::InsufficientMemory,
            format!(
                "the stage needs {} MiB and this device can give {} MiB while keeping 2 GiB free",
                need >> 20,
                have >> 20
            ),
        ));
    }
    Ok(mem_cap(est).min(have))
}

/// Clients of the stage's port that are not this daemon's sockets (ADR-0005 D3). An error counts as
/// foreign: the check fails closed.
async fn foreign_clients(port: u16) -> bool {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let check = move || {
        ucompute_runtime::llamacpp::sockets::live::foreign_clients_of(std::process::id(), port)
            .map_or(true, |f| !f.is_empty())
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let check = move || {
        let _ = port;
        true
    };
    tokio::task::spawn_blocking(check).await.unwrap_or(true)
}

/// Any listener left on `port` after rpc-server stopped.
async fn listener_left(port: u16) -> bool {
    use ucompute_runtime::llamacpp::sockets;
    let check = move || {
        #[cfg(target_os = "linux")]
        let socks = sockets::live::sockets();
        #[cfg(target_os = "macos")]
        let socks = sockets::live::sockets_on_port(port);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let socks: std::io::Result<Vec<sockets::Sock>> = Ok(Vec::new());
        socks.map_or(true, |s| s.iter().any(|s| s.listening && s.local.port() == port))
    };
    tokio::task::spawn_blocking(check).await.unwrap_or(true)
}

/// A first frame from `remote::serve_peer_with` that is an RPC session or tunnel.
pub(crate) async fn serve(
    stage: Option<Arc<Stage>>,
    conn: Arc<PeerConnection>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
    first: Body,
    send: SendStream,
    recv: RecvStream,
) {
    match first {
        Body::RpcSessionOpen(open) => serve_session(stage, conn, policy, audit, open, send, recv).await,
        Body::RpcTunnelOpen(t) => serve_tunnel(stage, conn, audit, t, send, recv).await,
        _ => {}
    }
}

/// Policy and hold on R: `may_serve` for pooled inference. A hold closes the connection
/// (`Node::hold`), which ends the session through the session stream.
fn stage_reason(policy: &PolicyEngine, peer: &PeerId) -> Option<UComputeError> {
    policy
        .may_serve(peer, WorkloadKind::PooledInference)
        .into_result()
        .err()
}

async fn serve_session(
    stage: Option<Arc<Stage>>,
    conn: Arc<PeerConnection>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
    open: RpcSessionOpen,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let peer = conn.peer_id;
    let rejected = |e: &UComputeError| {
        tracing::info!(peer = %peer.short_label(), code = %e.code, "pooled session refused");
        record(
            &audit,
            "peer_rejected",
            &peer,
            serde_json::json!({ "reason": e.code.as_str(), "detail": e.message, "session": hex::encode(&open.session_id) }),
        );
    };
    // default deny first: nothing is checked, spawned or answered beyond the refusal
    let admitted = match stage_reason(&policy, &peer) {
        Some(e) => Err(e),
        None if conn.negotiated_minor < RPC_SESSION_MINOR => Err(UComputeError::new(
            ErrorCode::ProtocolIncompatible,
            "pooled sessions need protocol minor 2 on both devices",
        )),
        None => match &stage {
            None => Err(UComputeError::new(
                ErrorCode::PolicyDenied,
                "this device does not host pooled stages",
            )),
            Some(s) => s.admit(&conn, &open).map(|slot| (s, slot)),
        },
    };
    let (stage, (slot, cap)) = match admitted {
        Ok((s, admitted)) => (s.clone(), admitted),
        Err(e) => {
            rejected(&e);
            return tell(&mut send, &e).await;
        }
    };
    let granted = open.est_bytes;
    let cap = cfg!(target_os = "linux").then_some(cap);
    let started = rpc_server::start(&stage.engine_dir, &stage.logs, stage.threads, cap)
        .await
        .and_then(|s| match s.child.id() {
            Some(pid) => Ok((s, pid)),
            None => Err(UComputeError::new(ErrorCode::BackendFailed, "rpc-server exited")),
        });
    let (server, pid) = match started {
        Ok(s) => s,
        Err(e) => {
            rejected(&e);
            drop(slot);
            if tokio::time::timeout(WRITE_WAIT, refuse(&mut send, &e)).await.is_err() {
                let _ = send.reset(reset_code());
            }
            return;
        }
    };
    let (port, device, log) = (server.port, server.device.clone(), server.log_path.clone());
    let mut child = server.child;
    // R meters the command allowlist and its own byte cap per session; H's cap is per load
    let shared = Shared::new(
        open.session_id.clone(),
        stage.byte_cap.unwrap_or_else(|| stage_byte_cap(granted)),
    );
    *lock(&stage.session) = Some(Arc::new(StageSession {
        shared: shared.clone(),
        conn: conn.conn.stable_id(),
        peer,
        port,
        pid,
    }));
    let _in_flight = conn.in_flight();
    record(
        &audit,
        "session_open",
        &peer,
        serde_json::json!({
            "role": "stage",
            "session": shared.hex(),
            "layer_start": open.layer_start,
            "layer_end": open.layer_end,
            "n_layer": open.n_layer,
            "est_bytes": open.est_bytes,
            "granted_bytes": granted,
            "threads": stage.threads,
            "device": device,
        }),
    );
    let accept = Body::RpcSessionAccept(RpcSessionAccept {
        session_id: open.session_id.clone(),
        granted_bytes: granted,
        threads: stage.threads,
        device,
    })
    .into();
    if !write_bounded(&mut send, &accept).await {
        shared.end(UComputeError::new(
            ErrorCode::PeerLost,
            "the head left before the session started",
        ));
    }
    let mut limits = stage.limits;
    if open.ttl_s > 0 {
        limits.ttl = limits.ttl.min(Duration::from_secs(u64::from(open.ttl_s)));
    }
    let mut tick = tokio::time::interval(limits.tick);
    let mut idle_since = Some(Instant::now());
    // whether R tells H why (its own reasons) or H ended it
    let mut tell_h = true;
    {
        let h_end = session_end(&mut recv);
        tokio::pin!(h_end);
        loop {
            tokio::select! {
                _ = shared.ended.cancelled() => break,
                _ = &mut h_end => {
                    tell_h = false;
                    let lost = conn.conn.close_reason().map(|r| map_conn_error(&r));
                    shared.end(lost.unwrap_or_else(|| UComputeError::new(ErrorCode::PeerLost, "the head ended the pooled session")));
                    break;
                }
                _ = tick.tick() => {
                    let reason = stage_reason(&policy, &peer)
                        .or_else(|| match child.try_wait() {
                            Ok(None) => None,
                            _ => Some(UComputeError::new(ErrorCode::BackendFailed, "the stage's rpc-server exited")),
                        })
                        .or_else(|| shared.limits(&limits, &mut idle_since, false));
                    let reason = match reason {
                        None if foreign_clients(port).await => Some(UComputeError::new(
                            ErrorCode::PolicyDenied,
                            "a process that is not this device's bridge connected to the pooled stage",
                        )),
                        r => r,
                    };
                    if let Some(e) = reason {
                        shared.end(e);
                        break;
                    }
                }
            }
        }
    }
    shared.ended.cancel();
    let reason = shared
        .reason
        .get()
        .cloned()
        .unwrap_or_else(|| UComputeError::new(ErrorCode::PeerLost, "ended"));
    // rpc-server and the slot go first: telling H waits on H's flow control (bounded)
    let _ = recv.stop(reset_code());
    process::stop(child).await;
    let left = listener_left(port).await;
    if left {
        tracing::warn!(
            port,
            "a listener is still on the pooled stage's port after rpc-server stopped"
        );
    }
    // like llama-server's: a clean end leaves nothing to diagnose
    if reason.code != ErrorCode::BackendFailed {
        let _ = std::fs::remove_file(&log);
    }
    tracing::info!(peer = %peer.short_label(), session = %shared.hex(), code = %reason.code, "pooled stage ended");
    let mut fields = shared.close_fields("stage", &reason);
    fields["listener_left"] = left.into();
    record(&audit, "session_close", &peer, fields);
    drop(slot);
    if tell_h {
        tell(&mut send, &reason).await;
    } else {
        let _ = send.reset(reset_code());
    }
}

async fn serve_tunnel(
    stage: Option<Arc<Stage>>,
    conn: Arc<PeerConnection>,
    audit: Arc<Audit>,
    t: RpcTunnelOpen,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let peer = conn.peer_id;
    let mut refused = |reason: &str| {
        let _ = send.reset(reset_code());
        let _ = recv.stop(reset_code());
        tracing::info!(peer = %peer.short_label(), reason, "pooled tunnel refused");
        record(
            &audit,
            "rpc_tunnel_close",
            &peer,
            serde_json::json!({ "role": "stage", "session": hex::encode(&t.session_id), "outcome": reason }),
        );
    };
    // the session must be on this connection, from this peer
    let Some(s) = stage
        .as_ref()
        .and_then(|st| st.lookup(&t.session_id, conn.conn.stable_id(), &peer))
    else {
        return refused("unknown_session");
    };
    if s.shared.ended.is_cancelled() {
        return refused("session_ended");
    }
    let Some((n, tunnel)) = s.shared.add_tunnel() else {
        return refused("tunnel_cap");
    };
    tunnel_lines(
        &audit,
        &peer,
        &s.shared,
        n,
        Role::Stage,
        stage_tunnel(&s, send, recv, tunnel),
    )
    .await;
}

/// R's tunnel: rpc-server is dialled at the head's first byte, so an open but silent tunnel holds
/// no place in rpc-server's accept backlog (one connection is served at a time; llama-server's
/// connections are sequential). That byte passes the allowlist before rpc-server sees anything,
/// and every client of rpc-server must be ours before the dial.
async fn stage_tunnel(
    s: &StageSession,
    mut send: SendStream,
    mut recv: RecvStream,
    tunnel: Arc<Mutex<Tunnel>>,
) -> (&'static str, u64, u64) {
    let shared = &s.shared;
    fn abort(send: &mut SendStream, recv: &mut RecvStream, outcome: &'static str) -> (&'static str, u64, u64) {
        let _ = send.reset(reset_code());
        let _ = recv.stop(reset_code());
        (outcome, 0, 0)
    }
    let mut first = vec![0u8; CHUNK];
    let n = tokio::select! {
        r = recv.read(&mut first) => match r {
            Ok(Some(n)) => n,
            Ok(None) => {
                let _ = send.finish();
                return ("closed", 0, 0);
            }
            Err(_) => return abort(&mut send, &mut recv, "reset"),
        },
        _ = shared.ended.cancelled() => return abort(&mut send, &mut recv, "session_ended"),
    };
    let first = &first[..n];
    if let Err(t) = lock(&tunnel).on_chunk(&shared.meter, Dir::ToR, first, Instant::now()) {
        shared.end(trip_error(t));
        return abort(&mut send, &mut recv, trip_outcome(t));
    }
    if foreign_clients(s.port).await {
        shared.end(UComputeError::new(
            ErrorCode::PolicyDenied,
            "a process that is not this device's bridge connected to the pooled stage",
        ));
        return abort(&mut send, &mut recv, "foreign_client");
    }
    let mut tcp = match TcpStream::connect((Ipv4Addr::LOCALHOST, s.port)).await {
        Ok(t) => t,
        Err(_) => return abort(&mut send, &mut recv, "connect_failed"),
    };
    if tcp.write_all(first).await.is_err() {
        return abort(&mut send, &mut recv, "connect_failed");
    }
    BYTES_TO_R.fetch_add(n as u64, Ordering::Relaxed);
    let (outcome, up, down) = run_tunnel(Role::Stage, tcp, send, recv, tunnel, shared).await;
    (outcome, up + n as u64, down)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_cap_is_two_estimates_plus_a_gib() {
        assert_eq!(byte_cap(0), 1 << 30);
        assert_eq!(byte_cap(190_200_000), 2 * 190_200_000 + (1 << 30));
        assert_eq!(byte_cap(u64::MAX), u64::MAX);
    }

    #[test]
    fn stage_byte_cap_is_two_loads() {
        assert_eq!(stage_byte_cap(0), 2 << 30);
        assert_eq!(stage_byte_cap(190_200_000), 2 * byte_cap(190_200_000));
        assert_eq!(stage_byte_cap(u64::MAX), u64::MAX);
    }

    /// Admission and the address-space cap agree (review rlimit-cap-exceeds-admission): for every
    /// admitted estimate the cap fits in what admission found free less the 2 GiB reserve, and
    /// still holds the grant and its 10 %.
    #[test]
    fn rpc_server_cap_never_exceeds_what_admission_found_free() {
        let gib = 1u64 << 30;
        let mut ests: Vec<u64> = vec![0, 1, 9, 10, 11, u64::MAX / 2, u64::MAX - 1, u64::MAX];
        ests.extend((0..64).map(|i| 1u64 << i));
        ests.extend((1..=400).map(|i| i * (64 << 20) + 12_345));
        let avails = [
            0,
            MEMORY_RESERVE,
            3 * gib,
            6 * gib,
            8 * gib,
            13 * gib,
            64 * gib,
            u64::MAX,
        ];
        let mut admitted = 0;
        for &available in &avails {
            let have = available.saturating_sub(MEMORY_RESERVE);
            for &est in &ests {
                let need = est.saturating_add(est / 10);
                match memory_grant(est, available) {
                    Ok(cap) => {
                        admitted += 1;
                        assert!(cap <= have, "est {est}, available {available}: cap {cap} > {have}");
                        assert!(cap >= need, "est {est}: cap {cap} below the grant's {need}");
                        assert!(cap <= mem_cap(est));
                    }
                    Err(e) => {
                        assert!(need > have, "est {est} fits in {have} but was refused: {e}");
                        assert_eq!(e.code, ErrorCode::InsufficientMemory);
                    }
                }
            }
        }
        assert!(admitted > 1000, "the grid admits most estimates: {admitted}");
        // the reviewer's case: 4 GiB claimed on 8 GiB free; mem_cap alone would be 7 GiB
        assert_eq!(memory_grant(4 * gib, 8 * gib).unwrap(), 6 * gib);
        assert_eq!(memory_grant(gib, 64 * gib).unwrap(), mem_cap(gib));
    }

    /// The test flag lifts the Linux-only stage rule for a loopback peer and never for a LAN one.
    /// The flag is process-wide, so this test sets it (and the lib tests never rely on its absence).
    #[test]
    fn test_flag_needs_a_loopback_peer() {
        let lan: SocketAddr = "192.168.1.167:8193".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:8193".parse().unwrap();
        std::env::set_var(TEST_POOL_ENV, "1");
        assert!(single_host_pool_test(&lo));
        assert!(!single_host_pool_test(&lan));
        assert!(may_host(&lo).is_ok());
        let lan_host = may_host(&lan);
        if cfg!(target_os = "linux") {
            assert!(lan_host.is_ok());
        } else {
            let e = lan_host.unwrap_err();
            assert_eq!(e.code, ErrorCode::PolicyDenied);
            assert!(e.message.contains("not available on this platform"), "{e}");
        }
    }

    #[test]
    fn tunnel_cap_and_idle_and_ttl() {
        let s = Shared::new(vec![1; 16], u64::MAX);
        // before the first tunnel only `first_idle` counts
        let first = Limits {
            idle: Duration::ZERO,
            ..Limits::default()
        };
        assert!(s.limits(&first, &mut None, true).is_none(), "the head is still loading");
        let e = s
            .limits(
                &Limits {
                    first_idle: Duration::ZERO,
                    ..first
                },
                &mut None,
                true,
            )
            .unwrap();
        assert!(e.message.contains("no connection for 0 s"), "{e}");
        // a tunnel that opened and closed between two ticks is activity (C6: llama-server's
        // short connections last about 25 ms; the idle close took the session's start instead)
        let limits10 = Limits {
            idle: Duration::from_secs(10),
            ..Limits::default()
        };
        let mut long_ago = Instant::now().checked_sub(Duration::from_secs(60));
        drop(s.add_tunnel());
        assert!(s.limits(&limits10, &mut long_ago, true).is_none());
        let held: Vec<_> = (0..MAX_TUNNELS).map(|_| s.add_tunnel().unwrap()).collect();
        assert!(s.add_tunnel().is_none(), "the ninth at once is refused");
        assert_eq!(held.last().unwrap().0, MAX_TUNNELS as u64 + 1);
        drop(held);
        assert!(s.add_tunnel().is_some(), "closed tunnels free their places");
        let limits = Limits {
            idle: Duration::ZERO,
            ..Limits::default()
        };
        let mut idle = None;
        assert_eq!(s.limits(&limits, &mut idle, true).unwrap().code, ErrorCode::Timeout);
        let ttl = Limits {
            ttl: Duration::ZERO,
            ..Limits::default()
        };
        let e = s.limits(&ttl, &mut None, true).unwrap();
        assert!(e.message.contains("reached its limit"), "{e}");
    }
}
