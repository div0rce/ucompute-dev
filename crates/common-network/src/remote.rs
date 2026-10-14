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
use crate::proto::{
    control_message::Body, read_frame, write_frame, Cancel, ControlMessage, ErrorMsg, RemoteChunk, RemoteRequest,
};
use crate::transport::{map_conn_error, PeerConnection};
use crate::MAX_CONTROL_FRAME;
use common_core::{CommonError, ErrorCode, Result, Timeouts};
use common_identity::PeerId;
use common_observability::Audit;
use common_policy::{PolicyEngine, PrivacyMode, QuotaConfig, Quotas, WorkloadKind};
use common_runtime::manager::{LoadOptions, ModelManager};
use common_runtime::{GenerateEvent, GenerateRequest, GenerateStream, Usage};
use futures_util::StreamExt;
use quinn::{ConnectionError, RecvStream, SendStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

pub struct RemoteOutcome;

/// Request streams one connection may have open at once (SD-12). The next one is answered with
/// `overloaded` and closed without being read.
pub const MAX_STREAMS_PER_CONNECTION: usize = 8;

/// Longest peer-supplied string that is echoed into an error message.
const MAX_ECHO: usize = 200;

fn short(s: &str) -> String {
    s.chars().take(MAX_ECHO).collect()
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
fn parse_code(code: &str) -> ErrorCode {
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
        let mut ev: GenerateEvent = serde_json::from_str(&c.event_json)
            .map_err(|_| CommonError::new(ErrorCode::ProtocolIncompatible, "undecodable event from peer"))?;
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
    check_privacy(privacy)?;
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
        request_id,
        cancel,
        timeouts: req.timeouts,
        started: Instant::now(),
        first_token: None,
        done: false,
    };
    // Dropping the stream drops `client`: its send side finishes and its recv side stops, which
    // the server reads as a cancel.
    Ok(futures_util::stream::unfold(client, |mut c| async move { c.step().await.map(|item| (item, c)) }).boxed())
}

// ---------------------------------------------------------------- server

fn quotas() -> &'static Quotas {
    // ponytail: process-wide default QuotaConfig; thread the daemon's configured Quotas through
    // serve_peer once [network.quotas] exists in the config file.
    static QUOTAS: OnceLock<Quotas> = OnceLock::new();
    QUOTAS.get_or_init(|| Quotas::new(QuotaConfig::default()))
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

async fn refuse(send: &mut SendStream, e: &CommonError) {
    let msg = Body::Error(ErrorMsg {
        code: e.code.as_str().into(),
        message: e.message.clone(),
    })
    .into();
    let _ = write_frame(send, &msg).await;
    let _ = send.finish();
}

fn record(audit: &Audit, kind: &str, peer: &PeerId, fields: serde_json::Value) {
    if let Err(e) = audit.record(kind, Some(&peer.to_hex()), fields) {
        tracing::debug!(kind, error = %e, "audit record not written");
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

/// Quotas, integrity, load, generate, forward. `Ok((usage, outcome))` once the stream is finished;
/// `Err` before anything was streamed (the caller sends it as the `Error` frame).
async fn execute(
    manager: &ModelManager,
    peer: &PeerId,
    req: RemoteRequest,
    send: &mut SendStream,
    recv: RecvStream,
) -> Result<(Usage, &'static str)> {
    let _slot = admit(quotas(), peer)?;
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
    let model = manager.ensure_loaded(&req.model_id, LoadOptions::default()).await?;
    let have = &model.info().model_sha256;
    if *have != req.model_sha256 {
        return Err(integrity(&req.model_id, &req.model_sha256, have));
    }
    // ponytail: the whole max_tokens is charged up front (refused as Overloaded when the per-minute
    // budget cannot cover it); refund the unused part if quotas ever bite on LAN use.
    quotas().charge_tokens(peer, u64::from(greq.max_tokens))?;
    let cancel = CancellationToken::new();
    let _watcher = AbortOnDrop(tokio::spawn(watch_cancel(recv, cancel.clone())));
    let mut stream = model.generate(greq, cancel.clone()).await?;
    let mut usage = Usage::default();
    let outcome = loop {
        let Some(ev) = stream.next().await else { break "ended" };
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

async fn handle_stream(
    conn: Arc<PeerConnection>,
    manager: Arc<ModelManager>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let peer = conn.peer_id;
    let started = Instant::now();
    let req = match read_frame(&mut recv, MAX_CONTROL_FRAME).await {
        Ok(Some(ControlMessage {
            body: Some(Body::RemoteRequest(r)),
        })) => r,
        Ok(Some(_)) => return refuse(&mut send, &CommonError::invalid("expected RemoteRequest")).await,
        Ok(None) => return,
        Err(e) => return refuse(&mut send, &e).await,
    };
    if !is_request_id(&req.request_id) {
        return refuse(
            &mut send,
            &CommonError::invalid("request_id must be 1..=64 safe characters"),
        )
        .await;
    }
    let request_id = req.request_id.clone();
    let model_id = short(&req.model_id);
    // Policy first: nothing is parsed or loaded for a peer this device does not serve (default deny).
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
    let result = execute(&manager, &peer, req, &mut send, recv).await;
    let (usage, outcome) = match &result {
        Ok((usage, outcome)) => (usage.clone(), *outcome),
        Err(e) => {
            refuse(&mut send, e).await;
            (Usage::default(), e.code.as_str())
        }
    };
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

/// Serve remote requests on an accepted connection until it closes. `Ok` when the connection
/// closed cleanly (either side), the typed loss otherwise. Call only with connections from
/// `Node::accept`/`Node::connect` (authenticated); `policy.may_serve` is applied per request.
pub async fn serve_peer(
    conn: Arc<PeerConnection>,
    manager: Arc<ModelManager>,
    policy: Arc<PolicyEngine>,
    audit: Arc<Audit>,
) -> Result<()> {
    let limit = Arc::new(Semaphore::new(MAX_STREAMS_PER_CONNECTION));
    loop {
        let (mut send, recv) = match conn.conn.accept_bi().await {
            Ok(s) => s,
            Err(ConnectionError::ApplicationClosed(_)) | Err(ConnectionError::LocallyClosed) => return Ok(()),
            Err(e) => return Err(map_conn_error(&e)),
        };
        let Ok(permit) = limit.clone().try_acquire_owned() else {
            let e = CommonError::new(
                ErrorCode::Overloaded,
                format!("more than {MAX_STREAMS_PER_CONNECTION} requests in flight on one connection; retry later"),
            );
            tokio::spawn(async move { refuse(&mut send, &e).await });
            continue;
        };
        let (conn, manager, policy, audit) = (conn.clone(), manager.clone(), policy.clone(), audit.clone());
        tokio::spawn(async move {
            let _permit = permit;
            handle_stream(conn, manager, policy, audit, send, recv).await;
        });
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
    fn integrity_message_names_both_hashes_without_echoing_junk() {
        let e = integrity(&"m".repeat(5_000), &"a".repeat(5_000), "bb");
        assert_eq!(e.code, ErrorCode::IntegrityMismatch);
        assert!(e.message.contains("no substitution") && e.message.contains("bb"));
        assert!(e.message.len() < 2 * MAX_ECHO + 100);
    }
}
