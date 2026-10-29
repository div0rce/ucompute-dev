//! Mode B, **remote execution**: one whole generation forwarded to one trusted peer over the
//! authenticated transport (F-06-02, F-06-07, F-06-10, F-08-02 remote half; spec section 7 B).
//! This is not pooled inference: the peer runs the entire request on its own engine and sees the
//! text in plaintext inside its own process (THREAT_MODEL §4); only the wire is protected.
//!
//! Wire: the client opens one bidirectional stream per request and writes a `RemoteRequest`
//! frame, keeping its send side open. The server answers with `RemoteChunk` frames (one per
//! `GenerateEvent`, the last marked `done`) or one `Error` frame, then finishes. The server
//! cancels its generation when it reads a `Cancel` frame **or** when the client's send side ends
//! for any reason (FIN, reset, connection loss; dropping the client's stream finishes it), so a
//! client that stopped waiting never leaves an engine slot busy.
//!
//! Text travels inside TLS only: `request_json` and `event_json` are never logged or audited.
//! Audit entries carry ids, counts, codes and durations.
//!
//! The serving side treats the decoded request as untrusted input with the local API's bounds
//! (`sanitize`: no raw grammar, bounded JSON schema, sampling ranges, this device's timeouts), and
//! shares the engine with local work through [`ServeOptions`]: the daemon's load options (no
//! reload, no `Overloaded` for local requests), its engine-slot semaphore (a peer takes a free
//! slot or is refused, it never queues ahead of local work) and a process-wide cap on peer
//! generations. Error frames carry codes and this module's own messages, never local paths.
use crate::proto::{
    control_message::Body, read_frame, write_frame, Cancel, ControlMessage, ErrorMsg, RemoteChunk, RemoteRequest,
};
use crate::transport::{map_conn_error, InFlight, PeerConnection};
use crate::MAX_CONTROL_FRAME;
use common_core::{CommonError, ErrorCode, Result, Timeouts};
use common_identity::PeerId;
use common_observability::jobs::{Finished, JobHandle, Jobs};
use common_observability::Audit;
use common_policy::{PolicyEngine, PrivacyMode, QuotaConfig, Quotas, WorkloadKind};
use common_runtime::manager::{LoadOptions, ModelManager};
use common_runtime::{GenerateEvent, GenerateRequest, GenerateStream, Usage};
use futures_util::StreamExt;
use quinn::{ConnectionError, RecvStream, SendStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

pub struct RemoteOutcome;

/// Mode B request streams one connection may have open at once (SD-12). The next `RemoteRequest`
/// is answered with `overloaded`. Pooled-session streams have their own budget (`rpc`: one session
/// per device, `rpc::MAX_TUNNELS` tunnels in it), and QUIC caps the streams open at once.
pub const MAX_STREAMS_PER_CONNECTION: usize = 8;

/// Longest peer-supplied string that is echoed into an error message.
const MAX_ECHO: usize = 200;

pub(crate) fn short(s: &str) -> String {
    s.chars().take(MAX_ECHO).collect()
}

/// Longest `finish_reason` kept from a peer ("stop", "length", "tool_calls" ...).
const MAX_FINISH_REASON: usize = 32;

/// A peer's generation event as this device keeps it: its `finish_reason` is peer text that lands
/// in job rows (served on every `GET /api/jobs`) and the caller's response, so it is capped like
/// other peer text.
fn peer_event(json: &str) -> Result<GenerateEvent> {
    let mut ev: GenerateEvent = serde_json::from_str(json)
        .map_err(|_| CommonError::new(ErrorCode::ProtocolIncompatible, "undecodable event from peer"))?;
    if let GenerateEvent::Done { finish_reason, .. } = &mut ev {
        *finish_reason = finish_reason.chars().take(MAX_FINISH_REASON).collect();
    }
    Ok(ev)
}

/// The privacy gate both sides apply: only `trusted_group` may leave the device. The routing
/// decision proper (`PolicyEngine::evaluate`, I-05) belongs to the caller; this is the last line,
/// so a `local_only` request never opens a stream even if a caller forgot to ask the engine.
fn check_privacy(privacy: &str) -> Result<()> {
    let mode: PrivacyMode = serde_json::from_value(serde_json::Value::String(privacy.into()))
        .map_err(|_| CommonError::invalid(format!("unknown privacy mode `{}`", short(privacy))))?;
    match mode {
        PrivacyMode::TrustedGroup => Ok(()),
        PrivacyMode::LocalOnly => Err(CommonError::new(
            ErrorCode::PolicyDenied,
            "your privacy settings require local execution",
        )),
        PrivacyMode::SwarmAllowed => Err(CommonError::new(
            ErrorCode::PolicyDenied,
            "public swarm execution is disabled until it passes its security gate",
        )),
    }
}

/// The stable code string of an `ErrorMsg` back to a typed code; unknown strings are `Internal`.
pub(crate) fn parse_code(code: &str) -> ErrorCode {
    serde_json::from_value(serde_json::Value::String(code.into())).unwrap_or(ErrorCode::Internal)
}

fn new_request_id() -> String {
    let mut b = [0u8; 8];
    let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
    hex::encode(b)
}

fn is_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && common_core::paths::safe_name(id)
}

// ---------------------------------------------------------------- client

struct Client {
    conn: quinn::Connection,
    peer: String,
    send: SendStream,
    recv: RecvStream,
    request_id: String,
    cancel: CancellationToken,
    timeouts: Timeouts,
    started: Instant,
    first_token: Option<Duration>,
    done: bool,
    /// Keeps a retired duplicate connection open until this request ends (see `Node::register`).
    // ponytail: held until the stream is dropped, not until Done; a caller that keeps a finished
    // stream delays that close (never a request). Release it on `done` if that ever matters.
    _in_flight: InFlight,
}

impl Client {
    /// Upgrades a stream-level loss to the connection's close reason when there is one
    /// (`PeerLost` on idle timeout, typed codes on an application close).
    fn lost(&self, e: CommonError) -> CommonError {
        match self.conn.close_reason() {
            Some(reason) => map_conn_error(&reason),
            None => e,
        }
    }

    fn chunk(&mut self, c: RemoteChunk) -> Result<GenerateEvent> {
        if c.request_id != self.request_id {
            return Err(CommonError::new(
                ErrorCode::ProtocolIncompatible,
                "peer answered with a chunk for another request",
            ));
        }
        let mut ev = peer_event(&c.event_json)?;
        match &mut ev {
            GenerateEvent::Token { .. } if self.first_token.is_none() => {
                let ttft = self.started.elapsed();
                self.first_token = Some(ttft);
                tracing::debug!(
                    request_id = self.request_id,
                    ttft_ms = ttft.as_millis() as u64,
                    "remote first token"
                );
            }
            // The client-side wall clock (request written → first token) replaces the peer engine's
            // number: it is what the caller experiences and includes the network and any load.
            GenerateEvent::Done { timings, .. } => {
                timings.time_to_first_token_ms = self
                    .first_token
                    .map(|d| d.as_secs_f64() * 1000.0)
                    .or(timings.time_to_first_token_ms);
            }
            _ => {}
        }
        if c.done {
            self.done = true;
        }
        Ok(ev)
    }

    async fn step(&mut self) -> Option<Result<GenerateEvent>> {
        if self.done {
            return None;
        }
        let deadline = if self.first_token.is_none() {
            self.timeouts.load + self.timeouts.first_token
        } else {
            self.timeouts.inter_token
        };
        let cancel = self.cancel.clone();
        let frame = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.done = true;
                let msg = Body::Cancel(Cancel { request_id: self.request_id.clone() }).into();
                let _ = write_frame(&mut self.send, &msg).await;
                let _ = self.send.finish();
                return Some(Err(CommonError::new(ErrorCode::Cancelled, "remote generation cancelled")));
            }
            r = tokio::time::timeout(deadline, read_frame(&mut self.recv, MAX_CONTROL_FRAME)) => match r {
                Ok(r) => r,
                Err(_) => Err(CommonError::new(
                    ErrorCode::Timeout,
                    format!("device {} sent nothing for {} s", self.peer, deadline.as_secs()),
                )),
            },
        };
        let item = match frame {
            Ok(Some(ControlMessage {
                body: Some(Body::RemoteChunk(c)),
            })) => self.chunk(c),
            Ok(Some(ControlMessage {
                body: Some(Body::Error(e)),
            })) => Err(CommonError::new(
                parse_code(&e.code),
                format!("device {}: {}", self.peer, short(&e.message)),
            )),
            Ok(Some(_)) => Err(CommonError::new(
                ErrorCode::ProtocolIncompatible,
                "unexpected message on a request stream",
            )),
            Ok(None) => Err(self.lost(CommonError::new(
                ErrorCode::PeerLost,
                "peer ended the request stream before Done",
            ))),
            Err(e) if e.code == ErrorCode::PeerLost => Err(self.lost(e)),
            Err(e) => Err(e),
        };
        if item.is_err() {
            self.done = true;
        }
        Some(item)
    }
}

/// Send `req` for `model_id` to the peer; stream events back. Cancel (token or dropping the
/// stream) makes the peer cancel its generation. Items: the peer's `GenerateEvent`s, or one typed
/// error then end: PolicyDenied (privacy, before any stream), PeerUnauthorized, Overloaded,
/// ModelNotDownloaded / IntegrityMismatch (on the peer), Cancelled, Timeout, PeerLost.
/// `Done.timings.time_to_first_token_ms` is measured here, on the client.
pub async fn generate_remote(
    conn: Arc<PeerConnection>,
    model_id: &str,
    model_sha256: &str,
    req: GenerateRequest,
    privacy: &str,
    cancel: CancellationToken,
) -> Result<GenerateStream> {
    generate_remote_with_id(conn, model_id, model_sha256, req, privacy, cancel)
        .await
        .map(|(_, stream)| stream)
}

/// [`generate_remote`], also returning the `request_id` it put in the `RemoteRequest`: the id the
/// serving device writes into its `remote_request` audit entry, so both sides' entries join.
pub async fn generate_remote_with_id(
    conn: Arc<PeerConnection>,
    model_id: &str,
    model_sha256: &str,
    req: GenerateRequest,
    privacy: &str,
    cancel: CancellationToken,
) -> Result<(String, GenerateStream)> {
    check_privacy(privacy)?;
    let in_flight = conn.in_flight();
    let (mut send, recv) = conn.conn.open_bi().await.map_err(|e| map_conn_error(&e))?;
    let request_id = new_request_id();
    let msg = RemoteRequest {
        request_id: request_id.clone(),
        model_id: model_id.into(),
        model_sha256: model_sha256.into(),
        request_json: serde_json::to_string(&req)?,
        privacy: privacy.into(),
    };
    write_frame(&mut send, &Body::RemoteRequest(msg).into()).await?;
    let client = Client {
        conn: conn.conn.clone(),
        peer: conn.peer_id.short_label(),
        send,
        recv,
        request_id: request_id.clone(),
        cancel,
        timeouts: req.timeouts,
        started: Instant::now(),
        first_token: None,
        done: false,
        _in_flight: in_flight,
    };
    // Dropping the stream drops `client`: its send side finishes and its recv side stops, which
    // the server reads as a cancel.
    let stream =
        futures_util::stream::unfold(client, |mut c| async move { c.step().await.map(|item| (item, c)) }).boxed();
    Ok((request_id, stream))
}

// ---------------------------------------------------------------- server

/// How a serving device shares itself with peers. Build one per daemon and clone it into every
/// `serve_peer_with` call: the semaphores and quotas are shared across all peer connections.
#[derive(Clone)]
pub struct ServeOptions {
    /// The load options local requests use (the API's `AppState::load_options`), so a peer request
    /// reuses the engine local work loads and never forces an incompatible reload.
    pub load: LoadOptions,
    /// Per-peer token buckets (F-06-04).
    pub quotas: Arc<Quotas>,
    /// The local admission's engine-slot semaphore (one permit per engine slot). A peer request
    /// takes a permit only if one is free at that instant, else `Overloaded`; it never waits in
    /// the local queue. `None`: no shared accounting (tests, CLI one-shots).
    pub engine_slots: Option<Arc<Semaphore>>,
    /// Peer generations running at once across all peers; the rest of the engine's slots stay
    /// with local work. Exhausted → `Overloaded`.
    pub peer_slots: Arc<Semaphore>,
    /// Where each served request is listed (`kind: served_for_peer`); the daemon's `/api/jobs`.
    pub jobs: Arc<Jobs>,
    /// This device as a pooled-inference stage host (WP-04); None refuses every pooled session
    /// after the policy check.
    pub rpc: Option<Arc<crate::rpc::Stage>>,
}

impl ServeOptions {
    /// Defaults: default load options, default quotas, no shared engine slots, peer generations
    /// capped at the per-peer concurrency of `QuotaConfig::default()`.
    pub fn standalone() -> Self {
        let cfg = QuotaConfig::default();
        Self {
            load: LoadOptions::default(),
            peer_slots: Arc::new(Semaphore::new(cfg.max_concurrent.max(1) as usize)),
            quotas: Arc::new(Quotas::new(cfg)),
            engine_slots: None,
            jobs: Jobs::new(),
            rpc: None,
        }
    }
}

/// The process-wide options behind [`serve_peer`], so quotas hold across a peer's connections.
fn process_defaults() -> &'static ServeOptions {
    static OPTS: OnceLock<ServeOptions> = OnceLock::new();
    OPTS.get_or_init(ServeOptions::standalone)
}

fn overloaded(m: &str) -> CommonError {
    CommonError::new(ErrorCode::Overloaded, format!("{m}; retry later"))
}

/// The engine capacity a peer request holds while it runs: one of the peer share and, when the
/// daemon shares its admission, one engine slot. Neither ever waits.
fn engine_permits(opts: &ServeOptions) -> Result<(OwnedSemaphorePermit, Option<OwnedSemaphorePermit>)> {
    let share = opts
        .peer_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| overloaded("this device is serving as many peer requests as it allows"))?;
    let slot = match &opts.engine_slots {
        Some(s) => Some(
            s.clone()
                .try_acquire_owned()
                .map_err(|_| overloaded("this device's engine is busy with local work"))?,
        ),
        None => None,
    };
    Ok((share, slot))
}

const MAX_STOP: usize = 16;
const MAX_STOP_BYTES: usize = 256;

/// A peer's request with the local API's bounds applied (a buggy or compromised group member
/// must not be able to stall this device's engine): no raw GBNF grammar, a bounded JSON schema,
/// sampling in range, bounded stop strings, and this device's own timeouts.
fn sanitize(mut g: GenerateRequest) -> Result<GenerateRequest> {
    if g.grammar.is_some() {
        return Err(CommonError::invalid(
            "raw grammars are not accepted from peers; send a json_schema",
        ));
    }
    if g.max_tokens == 0 {
        return Err(CommonError::invalid("max_tokens must be >= 1"));
    }
    if !(0.0..=2.0).contains(&g.temperature) || !(0.0..=1.0).contains(&g.top_p) {
        return Err(CommonError::invalid(
            "temperature must be in [0, 2] and top_p in [0, 1]",
        ));
    }
    if g.stop.len() > MAX_STOP || g.stop.iter().any(|s| s.len() > MAX_STOP_BYTES) {
        return Err(CommonError::invalid(format!(
            "at most {MAX_STOP} stop strings of at most {MAX_STOP_BYTES} bytes"
        )));
    }
    if let Some(schema) = &g.json_schema {
        common_runtime::schema::check_schema(schema)?;
    }
    g.timeouts = Timeouts::default();
    Ok(g)
}

/// An admitted request; released on drop so every exit path frees the concurrency slot.
struct Admitted<'a> {
    quotas: &'a Quotas,
    peer: PeerId,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        self.quotas.release(&self.peer);
    }
}

fn admit<'a>(quotas: &'a Quotas, peer: &PeerId) -> Result<Admitted<'a>> {
    quotas.admit_request(peer)?;
    Ok(Admitted { quotas, peer: *peer })
}

/// Aborts the task on drop.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What a peer may read of an error: the message for codes this module or the policy produce
/// (bounded, path-free); a generic text for codes whose messages can name local paths, binaries
/// or the account (engine, filesystem, model files).
fn peer_message(e: &CommonError) -> String {
    match e.code {
        ErrorCode::InvalidRequest
        | ErrorCode::PolicyDenied
        | ErrorCode::PeerUnauthorized
        | ErrorCode::Overloaded
        | ErrorCode::IntegrityMismatch
        | ErrorCode::ContextTooLong
        | ErrorCode::Cancelled
        | ErrorCode::Timeout
        | ErrorCode::FrameTooLarge
        | ErrorCode::ProtocolIncompatible => short(&e.message),
        code => format!(
            "the serving device could not run this request ({}); details are in its own log",
            code.as_str()
        ),
    }
}

pub(crate) async fn refuse(send: &mut SendStream, e: &CommonError) {
    let msg = Body::Error(ErrorMsg {
        code: e.code.as_str().into(),
        message: peer_message(e),
    })
    .into();
    let _ = write_frame(send, &msg).await;
    let _ = send.finish();
}

fn record(audit: &Audit, kind: &str, peer: &PeerId, fields: serde_json::Value) {
    if let Err(e) = audit.record(kind, Some(&peer.to_hex()), fields) {
        tracing::warn!(kind, error = %e, "audit record not written");
    }
}

/// Cancels when the client says so or its send side ends (FIN, reset, connection loss): a client
/// that stopped waiting must never leave an engine slot busy (F-08-02).
async fn watch_cancel(mut recv: RecvStream, cancel: CancellationToken) {
    loop {
        match read_frame(&mut recv, MAX_CONTROL_FRAME).await {
            Ok(Some(ControlMessage {
                body: Some(Body::Cancel(_)),
            }))
            | Ok(None)
            | Err(_) => break,
            // ponytail: anything else on a request stream is ignored; later minors may add messages.
            Ok(Some(_)) => {}
        }
    }
    cancel.cancel();
}

fn integrity(model_id: &str, asked: &str, have: &str) -> CommonError {
    CommonError::new(
        ErrorCode::IntegrityMismatch,
        format!(
            "peer asked for {} with sha256 {}, this device has {}; no substitution",
            short(model_id),
            short(asked),
            have
        ),
    )
}

/// Moves `job` on for one generation event: a streamed piece, the engine's final numbers, or the
/// error that ended it. Shared by the daemon's own requests and the ones served here.
pub fn job_event(job: &JobHandle, ev: &Result<GenerateEvent>) {
    match ev {
        Ok(GenerateEvent::Token { .. } | GenerateEvent::ToolCallDelta { .. }) => job.token(),
        Ok(GenerateEvent::Done {
            finish_reason,
            usage,
            timings,
        }) => job.done(Finished {
            finish_reason: Some(finish_reason.clone()),
            prompt_tokens: Some(usage.prompt_tokens),
            completion_tokens: Some(usage.completion_tokens),
            prompt_ms: timings.prompt_ms,
            decode_ms: timings.decode_ms,
            decode_tokens_per_second: timings.decode_tokens_per_second,
        }),
        Err(e) => job.fail(e.code.as_str()),
    }
}

/// Integrity, load, generate, forward, for a request already admitted (`_slot`, held until the
/// end). `Ok((usage, outcome))` once the stream is finished; `Err` before anything was streamed
/// (the caller sends it as the `Error` frame). `job` follows along: admitted, loaded, each piece,
/// and the end (`done`, or the outcome as its code).
#[allow(clippy::too_many_arguments)]
async fn execute(
    manager: &ModelManager,
    opts: &ServeOptions,
    peer: &PeerId,
    req: RemoteRequest,
    send: &mut SendStream,
    recv: RecvStream,
    job: &JobHandle,
    _slot: Admitted<'_>,
) -> Result<(Usage, &'static str)> {
    if !common_core::paths::safe_name(&req.model_id) {
        return Err(CommonError::invalid(
            "model id may only contain letters, digits, '-', '_' and '.'",
        ));
    }
    // The hash the peer expects must be the hash this device has, checked before any load and
    // again on the loaded model: a different file is refused, never substituted (F-02-10).
    if let Some(m) = manager.store.get(&req.model_id).ok().flatten() {
        if m.sha256 != req.model_sha256 {
            return Err(integrity(&req.model_id, &req.model_sha256, &m.sha256));
        }
    }
    let greq: GenerateRequest = serde_json::from_str(&req.request_json)
        .map_err(|_| CommonError::invalid("request_json is not a GenerateRequest"))?;
    let greq = sanitize(greq)?;
    // Held until the stream ends: the engine slot this generation occupies.
    let _engine = engine_permits(opts)?;
    job.started();
    let model = manager
        .ensure_loaded_for(&req.model_id, opts.load.clone(), &format!("peer:{}", peer.to_hex()))
        .await?;
    job.loaded(&model.info().device_actual);
    let have = &model.info().model_sha256;
    if *have != req.model_sha256 {
        return Err(integrity(&req.model_id, &req.model_sha256, have));
    }
    // ponytail: the whole max_tokens is charged up front (refused as Overloaded when the per-minute
    // budget cannot cover it); refund the unused part if quotas ever bite on LAN use.
    opts.quotas.charge_tokens(peer, u64::from(greq.max_tokens))?;
    let cancel = CancellationToken::new();
    let _watcher = AbortOnDrop(tokio::spawn(watch_cancel(recv, cancel.clone())));
    let mut stream = model.generate(greq, cancel.clone()).await?;
    let mut usage = Usage::default();
    let outcome = loop {
        let Some(ev) = stream.next().await else { break "ended" };
        job_event(job, &ev);
        match ev {
            Ok(ev) => {
                let done = matches!(ev, GenerateEvent::Done { .. });
                if let GenerateEvent::Done { usage: u, .. } = &ev {
                    usage = u.clone();
                }
                let chunk = RemoteChunk {
                    request_id: req.request_id.clone(),
                    event_json: serde_json::to_string(&ev)?,
                    done,
                };
                if write_frame(send, &Body::RemoteChunk(chunk).into()).await.is_err() {
                    break "client_gone";
                }
                if done {
                    break "done";
                }
            }
            // The engine's own error (Cancelled, Timeout, BackendFailed …) is the stream's last frame.
            Err(e) => {
                refuse(send, &e).await;
                break e.code.as_str();
            }
        }
    };
    // Stops the engine when the client went away mid-stream; a no-op after Done.
    cancel.cancel();
    drop(stream);
    let _ = send.finish();
    Ok((usage, outcome))
}

#[allow(clippy::too_many_arguments)]
async fn handle_stream(
    conn: Arc<PeerConnection>,
    manager: Arc<ModelManager>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
    opts: ServeOptions,
    req: RemoteRequest,
    mut send: SendStream,
    recv: RecvStream,
) {
    let peer = conn.peer_id;
    let started = Instant::now();
    if !is_request_id(&req.request_id) {
        return refuse(
            &mut send,
            &CommonError::invalid("request_id must be 1..=64 safe characters"),
        )
        .await;
    }
    let request_id = req.request_id.clone();
    let model_id = short(&req.model_id);
    // Policy first: nothing is parsed or loaded for a peer this device does not serve (default deny),
    // and a refused request is a `peer_rejected` line, not a job.
    let allowed = policy
        .may_serve(&peer, WorkloadKind::RemoteInference)
        .into_result()
        .and_then(|_| check_privacy(&req.privacy));
    if let Err(e) = allowed {
        tracing::info!(peer = %peer.short_label(), code = %e.code, "remote request refused");
        record(
            &audit,
            "peer_rejected",
            &peer,
            serde_json::json!({ "reason": e.code.as_str(), "detail": e.message, "request_id": request_id }),
        );
        return refuse(&mut send, &e).await;
    }
    // Quota before the row: a request over the peer's quota is refused at once and audited below
    // (`remote_request`, outcome `overloaded`), never a job. Refusals cost the peer nothing, so as
    // rows they could push this computer's own history out of the 256 finished jobs kept.
    let mut job = None;
    let result = match admit(&opts.quotas, &peer) {
        Err(e) => Err(e),
        Ok(slot) => {
            let j = job.insert(opts.jobs.start(
                request_id.clone(),
                "served_for_peer",
                &model_id,
                "local".into(),
                format!("peer:{}", peer.to_hex()),
                None,
            ));
            j.set_request_id(&request_id);
            execute(&manager, &opts, &peer, req, &mut send, recv, j, slot).await
        }
    };
    let (usage, outcome) = match &result {
        Ok((usage, outcome)) => (usage.clone(), *outcome),
        Err(e) => {
            // The detail stays here; the peer gets `peer_message`.
            tracing::info!(peer = %peer.short_label(), request_id, code = %e.code, detail = %e.message, "remote request failed");
            refuse(&mut send, e).await;
            (Usage::default(), e.code.as_str())
        }
    };
    // Done and the engine's errors already ended the row (`job_event`); this ends the rest:
    // `ended` (no Done) reads as done, `client_gone` and failures before the stream by their code.
    match (outcome, &job) {
        (_, None) => {}
        ("ended", Some(job)) => job.done(Finished::default()),
        (other, Some(job)) => job.fail(other),
    }
    tracing::info!(peer = %peer.short_label(), request_id, outcome, "remote request finished");
    record(
        &audit,
        "remote_request",
        &peer,
        serde_json::json!({
            "request_id": request_id,
            "peer": peer.short_label(),
            "model_id": model_id,
            "outcome": outcome,
            "prompt_tokens": usage.prompt_tokens,
            "completion_tokens": usage.completion_tokens,
            "duration_ms": started.elapsed().as_millis() as u64,
        }),
    );
}

/// [`serve_peer_with`] using process-wide [`ServeOptions::standalone`] options (default load
/// options, default quotas, no engine-slot sharing). A daemon that also serves local requests
/// must use `serve_peer_with` with its own load options and admission semaphore.
pub async fn serve_peer(
    conn: Arc<PeerConnection>,
    manager: Arc<ModelManager>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
) -> Result<()> {
    serve_peer_with(conn, manager, policy, audit, process_defaults().clone()).await
}

/// Serve remote requests on an accepted connection until it closes. `Ok` when the connection
/// closed cleanly (either side), the typed loss otherwise. Call only with connections from
/// `Node::accept`/`Node::connect` (authenticated); `policy.may_serve` is applied per request.
pub async fn serve_peer_with(
    conn: Arc<PeerConnection>,
    manager: Arc<ModelManager>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
    opts: ServeOptions,
) -> Result<()> {
    let limit = Arc::new(Semaphore::new(MAX_STREAMS_PER_CONNECTION));
    loop {
        let (send, recv) = match conn.conn.accept_bi().await {
            Ok(s) => s,
            Err(ConnectionError::ApplicationClosed(_)) | Err(ConnectionError::LocallyClosed) => return Ok(()),
            Err(e) => return Err(map_conn_error(&e)),
        };
        let in_flight = conn.in_flight();
        let (conn, manager, policy, audit, opts, limit) = (
            conn.clone(),
            manager.clone(),
            policy.clone(),
            audit.clone(),
            opts.clone(),
            limit.clone(),
        );
        tokio::spawn(async move {
            let _held = in_flight;
            dispatch(conn, manager, policy, audit, opts, limit, send, recv).await;
        });
    }
}

/// The first frame decides what a stream is: a Mode B request (within the connection's
/// [`MAX_STREAMS_PER_CONNECTION`]) or a pooled session or tunnel (`rpc`, bounded to
/// `MAX_RPC_FRAME`).
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    conn: Arc<PeerConnection>,
    manager: Arc<ModelManager>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
    opts: ServeOptions,
    limit: Arc<Semaphore>,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let first = match read_frame(&mut recv, MAX_CONTROL_FRAME).await {
        Ok(Some(m)) => m,
        Ok(None) => return,
        Err(e) => return refuse(&mut send, &e).await,
    };
    if let Err(e) = crate::proto::check_rpc_frame(&first) {
        return refuse(&mut send, &e).await;
    }
    match first.body {
        Some(Body::RemoteRequest(req)) => {
            let Ok(_permit) = limit.try_acquire_owned() else {
                let e = CommonError::new(
                    ErrorCode::Overloaded,
                    format!("more than {MAX_STREAMS_PER_CONNECTION} requests in flight on one connection; retry later"),
                );
                return refuse(&mut send, &e).await;
            };
            handle_stream(conn, manager, policy, audit, opts, req, send, recv).await
        }
        Some(b @ (Body::RpcSessionOpen(_) | Body::RpcTunnelOpen(_))) => {
            crate::rpc::serve(opts.rpc.clone(), conn, policy, audit, b, send, recv).await
        }
        _ => refuse(&mut send, &CommonError::invalid("expected RemoteRequest")).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privacy_gate_lets_only_trusted_group_leave_the_device() {
        check_privacy("trusted_group").unwrap();
        let e = check_privacy("local_only").unwrap_err();
        assert_eq!(e.code, ErrorCode::PolicyDenied);
        assert!(e.message.contains("local execution"), "{e}");
        assert_eq!(
            check_privacy("swarm_allowed").unwrap_err().code,
            ErrorCode::PolicyDenied
        );
        let e = check_privacy(&"x".repeat(10_000)).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidRequest);
        assert!(e.message.len() < 300);
    }

    #[test]
    fn error_codes_round_trip_through_their_strings() {
        for code in ErrorCode::ALL {
            assert_eq!(parse_code(code.as_str()), *code);
        }
        assert_eq!(parse_code("no_such_code"), ErrorCode::Internal);
        assert_eq!(parse_code(""), ErrorCode::Internal);
    }

    #[test]
    fn request_ids_are_fresh_and_validated() {
        let (a, b) = (new_request_id(), new_request_id());
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert!(is_request_id(&a));
        assert!(is_request_id("req-1_a.b"));
        assert!(!is_request_id(""));
        assert!(!is_request_id("a/b"));
        assert!(!is_request_id("a b"));
        assert!(!is_request_id(&"x".repeat(65)));
    }

    #[test]
    fn admission_guard_releases_its_slot_on_drop() {
        let q = Quotas::new(QuotaConfig {
            max_concurrent: 2,
            ..QuotaConfig::default()
        });
        let peer = PeerId([7; 32]);
        let first = admit(&q, &peer).unwrap();
        let _second = admit(&q, &peer).unwrap();
        assert_eq!(admit(&q, &peer).err().unwrap().code, ErrorCode::Overloaded);
        drop(first);
        let _third = admit(&q, &peer).unwrap();
        assert_eq!(admit(&q, &peer).err().unwrap().code, ErrorCode::Overloaded);
    }

    #[test]
    fn peer_requests_get_the_local_api_bounds() {
        let ok = GenerateRequest {
            json_schema: Some(
                serde_json::json!({"type": "object", "properties": {"a": {"type": "string", "maxLength": 10}}}),
            ),
            timeouts: Timeouts {
                first_token: Duration::from_secs(86_400),
                ..Timeouts::default()
            },
            ..GenerateRequest::default()
        };
        let clean = sanitize(ok.clone()).unwrap();
        assert_eq!(clean.timeouts, Timeouts::default(), "the peer's timeouts are replaced");
        assert_eq!(clean.json_schema, ok.json_schema);
        let refused = |g: GenerateRequest| sanitize(g).unwrap_err().code;
        let with = |f: fn(&mut GenerateRequest)| {
            let mut g = ok.clone();
            f(&mut g);
            g
        };
        for g in [
            with(|g| g.grammar = Some("root ::= \"a\"".into())),
            with(|g| g.max_tokens = 0),
            with(|g| g.temperature = 2.5),
            with(|g| g.temperature = f32::NAN),
            with(|g| g.top_p = -0.1),
            with(|g| g.stop = vec!["x".into(); 17]),
            with(|g| g.stop = vec!["x".repeat(257)]),
            with(|g| g.json_schema = Some(serde_json::json!({"type": "string", "pattern": "a{100000}"}))),
            with(|g| g.json_schema = Some(serde_json::json!({"type": "array", "minItems": 5000}))),
            with(|g| g.json_schema = Some(serde_json::json!({"$ref": "https://example.com/s.json"}))),
            with(|g| g.json_schema = Some(serde_json::json!({"description": "x".repeat(70_000)}))),
        ] {
            assert_eq!(refused(g), ErrorCode::InvalidRequest);
        }
        let mut deep = serde_json::json!({"type": "string"});
        for _ in 0..40 {
            deep = serde_json::json!({"type": "array", "items": deep});
        }
        let deep = GenerateRequest {
            json_schema: Some(deep),
            ..ok
        };
        assert_eq!(refused(deep), ErrorCode::InvalidRequest);
    }

    #[test]
    fn peers_never_see_local_paths() {
        for code in [
            ErrorCode::Io,
            ErrorCode::Internal,
            ErrorCode::BackendUnavailable,
            ErrorCode::BackendFailed,
            ErrorCode::ModelFileInvalid,
        ] {
            let e = CommonError::new(
                code,
                "model file not found: /Users/someone/.common/models/blobs/sha256-x",
            );
            let m = peer_message(&e);
            assert!(!m.contains("/Users") && m.contains(code.as_str()), "{m}");
        }
        let e = CommonError::new(ErrorCode::Overloaded, "device x exceeded its limit; retry later");
        assert_eq!(peer_message(&e), e.message);
        assert!(peer_message(&CommonError::invalid("y".repeat(5_000))).len() <= MAX_ECHO);
    }

    #[test]
    fn peer_share_and_engine_slots_never_wait() {
        let opts = ServeOptions {
            engine_slots: Some(Arc::new(Semaphore::new(2))),
            peer_slots: Arc::new(Semaphore::new(1)),
            ..ServeOptions::standalone()
        };
        let held = engine_permits(&opts).unwrap();
        assert!(held.1.is_some());
        // The peer share (1) is used up although an engine slot is still free.
        assert_eq!(engine_permits(&opts).unwrap_err().code, ErrorCode::Overloaded);
        drop(held);
        // Local work holds every engine slot: a peer is refused, not queued.
        let local = opts.engine_slots.clone().unwrap().try_acquire_many_owned(2).unwrap();
        let e = engine_permits(&opts).unwrap_err();
        assert_eq!(e.code, ErrorCode::Overloaded);
        assert!(e.message.contains("local work"), "{e}");
        assert_eq!(
            opts.peer_slots.available_permits(),
            1,
            "the share is returned on refusal"
        );
        drop(local);
        assert!(engine_permits(&opts).is_ok());
    }

    #[test]
    fn integrity_message_names_both_hashes_without_echoing_junk() {
        let e = integrity(&"m".repeat(5_000), &"a".repeat(5_000), "bb");
        assert_eq!(e.code, ErrorCode::IntegrityMismatch);
        assert!(e.message.contains("no substitution") && e.message.contains("bb"));
        assert!(e.message.len() < 2 * MAX_ECHO + 100);
    }

    /// A peer's `finish_reason` is kept at most [`MAX_FINISH_REASON`] characters: a peer cannot
    /// park text (a completion, a megabyte) in every job row it answers.
    #[test]
    fn peer_finish_reason_is_capped() {
        let done = |reason: &str| {
            serde_json::to_string(&GenerateEvent::Done {
                finish_reason: reason.into(),
                usage: Usage::default(),
                timings: Default::default(),
            })
            .unwrap()
        };
        let reason = |json: &str| match peer_event(json).unwrap() {
            GenerateEvent::Done { finish_reason, .. } => finish_reason,
            other => panic!("{other:?}"),
        };
        assert_eq!(reason(&done("stop")), "stop");
        assert_eq!(reason(&done(&"é".repeat(1 << 19))), "é".repeat(MAX_FINISH_REASON));
        assert_eq!(
            peer_event("not json").unwrap_err().code,
            ErrorCode::ProtocolIncompatible
        );
    }
}
