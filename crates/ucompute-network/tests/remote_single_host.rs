//! Mode B remote execution between two in-process nodes on loopback. **SINGLE-HOST**: both nodes
//! run on this one Mac; nothing here satisfies a two-machine criterion. The serving node runs the
//! real pinned llama-server on the stories15M fixture from UCOMPUTE_HOME (skips loudly when absent;
//! `UCOMPUTE_REQUIRE_FIXTURES=1` fails instead). Nothing is mocked.
mod support;

use futures_util::StreamExt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::{audit_kinds, pair, poll_until, start_relay, Peer};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use ucompute_artifacts::Store;
use ucompute_core::{ErrorCode, Paths};
use ucompute_identity::{Group, GroupPolicy};
use ucompute_network::proto::{control_message::Body, encode_frame, RemoteChunk, RemoteRequest};
use ucompute_network::remote::{generate_remote, serve_peer, serve_peer_with, ServeOptions};
use ucompute_network::scan_for_plaintext;
use ucompute_network::transport::{Node, PeerConnection};
use ucompute_policy::{ExecutionTarget, PolicyConfig, PolicyEngine, PrivacyMode, QuotaConfig, Quotas, WorkloadKind};
use ucompute_runtime::manager::{LoadOptions, ModelManager};
use ucompute_runtime::{collect, GenerateEvent, GenerateRequest, GenerateStream, PromptInput};

const MODEL_ID: &str = "stories15M-q4_0";
const MODEL_SHA: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

fn ucompute_home() -> PathBuf {
    std::env::var_os("UCOMPUTE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.ucompute"))
}

/// The real engine and fixture, or None after printing why; `UCOMPUTE_REQUIRE_FIXTURES=1` makes a
/// missing asset a failure so a green run can never mean "nothing was generated".
fn fixtures() -> Option<Paths> {
    let paths = Paths::at(ucompute_home());
    let engine_dir = paths.backends().join("llama.cpp").join("b11609");
    let blob = paths.model_blobs().join(format!("sha256-{MODEL_SHA}"));
    let manifest = paths.model_manifests().join(format!("{MODEL_ID}.json"));
    let missing = if !ucompute_artifacts::backend::binary(&engine_dir, "llama-server").is_file() {
        format!("pinned llama-server missing at {}", engine_dir.display())
    } else if !blob.is_file() {
        format!("fixture model missing at {}", blob.display())
    } else if !manifest.is_file() {
        format!("fixture manifest missing at {}", manifest.display())
    } else {
        return Some(paths);
    };
    assert!(
        std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "UCOMPUTE_REQUIRE_FIXTURES=1: {missing}"
    );
    eprintln!("SKIP: {missing}");
    None
}

/// Tests that run the engine take turns: the timing ones (TPOT, flood) must not measure another
/// test's llama-server, and two engines at once on an 8 GB Mac only add noise.
async fn engine_turn() -> tokio::sync::MutexGuard<'static, ()> {
    static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    TURN.lock().await
}

/// The serving node's manager over the real UCOMPUTE_HOME store (its identity and audit stay in a
/// temp dir, see `Peer`).
fn manager(paths: &Paths) -> Arc<ModelManager> {
    paths.ensure_dirs().unwrap();
    let engine_dir = paths.backends().join("llama.cpp").join("b11609");
    let backend = ucompute_runtime::backend_by_name("llama.cpp", engine_dir, paths.clone()).unwrap();
    Arc::new(ModelManager::new(
        paths.clone(),
        Store::new(paths.clone()),
        backend,
        ucompute_hardware::probe().unwrap(),
    ))
}

fn greedy(prompt: &str, max_tokens: u32) -> GenerateRequest {
    GenerateRequest {
        input: PromptInput::Raw(prompt.into()),
        max_tokens,
        temperature: 0.0,
        seed: Some(42),
        return_token_ids: true,
        ..Default::default()
    }
}

/// `host` serves remote inference to `guest` iff `allow` (group policy); networking and serving on.
fn serving_policy(host: &Peer, guest: &Peer, allow: bool) -> Arc<PolicyEngine> {
    host.peers
        .upsert_group(Group {
            name: "home".into(),
            members: vec![guest.id()],
            policy: GroupPolicy {
                allow_remote_inference: allow,
                allow_pooled_inference: false,
            },
        })
        .unwrap();
    Arc::new(PolicyEngine::new(
        PolicyConfig {
            default_privacy: PrivacyMode::TrustedGroup,
            network_enabled: true,
            serve_peers: true,
            swarm_enabled: false,
        },
        host.peers.clone(),
    ))
}

struct Link {
    na: Arc<Node>,
    nb: Arc<Node>,
    /// a → b, as seen by a.
    out: Arc<PeerConnection>,
    /// a → b, as seen by b.
    inn: Arc<PeerConnection>,
}

async fn link_to(a: &Peer, b: &Peer, addr: Option<SocketAddr>) -> Link {
    let (na, nb) = (a.start().await, b.start().await);
    let addr = addr.unwrap_or_else(|| nb.local_addr().unwrap());
    let (out, inn) = tokio::join!(na.connect(addr, b.id()), async { nb.accept().await.unwrap() });
    Link {
        na,
        nb,
        out: out.unwrap(),
        inn,
    }
}

async fn link(a: &Peer, b: &Peer) -> Link {
    link_to(a, b, None).await
}

fn find_audit(p: &Peer, kind: &str) -> Option<ucompute_observability::AuditEvent> {
    p.audit.tail(50).unwrap().into_iter().find(|e| e.kind == kind)
}

#[tokio::test]
async fn remote_golden_text_single_host() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&home);
    let serve = tokio::spawn(serve_peer(l.inn.clone(), m.clone(), policy, b.audit.clone()));

    let s0 = l.out.conn.stats();
    let t0 = Instant::now();
    let stream = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        greedy(PROMPT, 32),
        "trusted_group",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let (text, ids, usage, timings, finish) = collect(stream).await.unwrap();
    let total = t0.elapsed();
    let s1 = l.out.conn.stats();
    assert_eq!(text, GOLDEN);
    assert_eq!(ids.len(), 32);
    assert_eq!(usage.completion_tokens, 32);
    assert_eq!(usage.prompt_tokens, 10);
    assert_eq!(finish, "length");
    let ttft = timings.time_to_first_token_ms.unwrap();
    assert!(
        ttft > 0.0 && ttft <= total.as_secs_f64() * 1000.0,
        "client-side ttft {ttft} ms"
    );

    // The same request through the same manager locally: identical text and token ids.
    let local = m.ensure_loaded(MODEL_ID, LoadOptions::default()).await.unwrap();
    let (ltext, lids, ..) = collect(
        local
            .generate(greedy(PROMPT, 32), CancellationToken::new())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(ltext, text);
    assert_eq!(lids, ids);

    // Audited on the serving side with ids and counts; never the text.
    let entry = poll_until(Duration::from_secs(2), || find_audit(&b, "remote_request").is_some())
        .await
        .and_then(|_| find_audit(&b, "remote_request"))
        .expect("remote_request audited");
    assert_eq!(entry.peer_id.as_deref(), Some(a.id().to_hex().as_str()));
    assert_eq!(entry.fields["outcome"], "done");
    assert_eq!(entry.fields["prompt_tokens"], 10);
    assert_eq!(entry.fields["completion_tokens"], 32);
    assert_eq!(entry.fields["model_id"], MODEL_ID);
    assert!(entry.fields["request_id"].as_str().unwrap().len() == 16);
    assert!(entry.fields["duration_ms"].as_u64().is_some());
    let raw = std::fs::read_to_string(&b.audit.path).unwrap();
    assert!(
        !raw.contains(PROMPT) && !raw.contains(GOLDEN),
        "text leaked into audit.jsonl"
    );

    let (tx, rx) = (s1.udp_tx.bytes - s0.udp_tx.bytes, s1.udp_rx.bytes - s0.udp_rx.bytes);
    eprintln!(
        "bench:transport-overhead SINGLE-HOST loopback QUIC, 32 tokens: client tx {tx} B ({} datagrams), rx {rx} B ({} datagrams), {:.0} B/token rx, {:.0} B/token tx, client ttft {ttft:.1} ms, total {total:?}",
        s1.udp_tx.datagrams - s0.udp_tx.datagrams,
        s1.udp_rx.datagrams - s0.udp_rx.datagrams,
        rx as f64 / 32.0,
        tx as f64 / 32.0,
    );
    l.nb.shutdown().await;
    assert!(serve.await.unwrap().is_ok(), "serve_peer ends cleanly on shutdown");
}

#[tokio::test]
async fn remote_streaming_delivers_before_done() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&home);
    tokio::spawn(serve_peer(l.inn.clone(), m, policy, b.audit.clone()));
    let mut stream = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        greedy(PROMPT, 32),
        "trusted_group",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let t0 = Instant::now();
    let (mut tokens, mut first, mut done_at, mut ttft) = (0, None, None, None);
    while let Some(ev) = stream.next().await {
        match ev.unwrap() {
            GenerateEvent::Token { .. } => {
                assert!(done_at.is_none(), "token after Done");
                tokens += 1;
                first.get_or_insert(t0.elapsed());
            }
            GenerateEvent::Done { timings, .. } => {
                done_at = Some(t0.elapsed());
                ttft = timings.time_to_first_token_ms;
            }
            GenerateEvent::ToolCallDelta { .. } => {}
        }
    }
    let (first, done_at) = (first.unwrap(), done_at.unwrap());
    assert!(tokens >= 2, "{tokens} token events");
    assert!(
        first < done_at,
        "first token {first:?} must arrive before Done {done_at:?}"
    );
    let ttft = ttft.unwrap();
    assert!(ttft > 0.0 && ttft <= done_at.as_secs_f64() * 1000.0, "{ttft}");
    eprintln!("SINGLE-HOST streaming: {tokens} token events, first at {first:?}, Done at {done_at:?}");
}

#[tokio::test]
async fn unauthorized_peer_is_refused() {
    // Pinned, but the group policy does not allow remote inference: refused before anything runs.
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, false);
    let l = link(&a, &b).await;
    let m = manager(&Paths::at(ucompute_home()));
    tokio::spawn(serve_peer(l.inn.clone(), m.clone(), policy, b.audit.clone()));
    let stream = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        greedy(PROMPT, 8),
        "trusted_group",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let items: Vec<_> = timeout(Duration::from_secs(5), stream.collect()).await.unwrap();
    assert_eq!(items.len(), 1, "one error item then end: {items:?}");
    let e = items[0].as_ref().unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{e}");
    assert!(e.message.contains("trusted group"), "{e}");
    assert!(m.status().await.is_none(), "nothing was loaded");
    let rejected = poll_until(Duration::from_secs(2), || find_audit(&b, "peer_rejected").is_some())
        .await
        .and_then(|_| find_audit(&b, "peer_rejected"))
        .expect("peer_rejected audited");
    assert_eq!(rejected.fields["reason"], "peer_unauthorized");
    assert_eq!(rejected.fields["request_id"].as_str().unwrap().len(), 16);
    assert!(!audit_kinds(&b).contains(&"remote_request".to_string()));
}

#[tokio::test]
async fn local_only_privacy_never_leaves_device() {
    let (a, b) = pair();
    // Even with networking on and b in an allowing group, local_only is denied by the engine …
    let a_policy = {
        b.peers
            .upsert_group(Group {
                name: "home".into(),
                members: vec![b.id()],
                policy: GroupPolicy {
                    allow_remote_inference: true,
                    allow_pooled_inference: false,
                },
            })
            .unwrap();
        PolicyEngine::new(
            PolicyConfig {
                network_enabled: true,
                ..PolicyConfig::default()
            },
            a.peers.clone(),
        )
    };
    let d = a_policy.evaluate(
        PrivacyMode::LocalOnly,
        ExecutionTarget::Peer(b.id()),
        WorkloadKind::RemoteInference,
    );
    let e = d.into_result().unwrap_err();
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("local execution"), "{e}");

    // … and generate_remote refuses before opening any stream: b sees none.
    let l = link(&a, &b).await;
    let inn = l.inn.clone();
    let no_stream =
        tokio::spawn(async move { timeout(Duration::from_millis(700), inn.conn.accept_bi()).await.is_err() });
    let e = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        greedy(PROMPT, 8),
        "local_only",
        CancellationToken::new(),
    )
    .await
    .err()
    .expect("denied");
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("local execution"), "{e}");
    assert!(no_stream.await.unwrap(), "zero bidirectional streams were opened");
    assert!(!audit_kinds(&b).contains(&"remote_request".to_string()));
    assert_eq!(
        generate_remote(
            l.out.clone(),
            MODEL_ID,
            MODEL_SHA,
            greedy(PROMPT, 8),
            "swarm_allowed",
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap()
        .code,
        ErrorCode::PolicyDenied
    );
}

#[tokio::test]
async fn remote_cancel_stops_server_generation_within_500ms() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&home);
    // Pre-load so the measurement is about cancel, not the load.
    let model = m.ensure_loaded(MODEL_ID, LoadOptions::default()).await.unwrap();
    tokio::spawn(serve_peer(l.inn.clone(), m.clone(), policy, b.audit.clone()));
    let cancel = CancellationToken::new();
    let mut stream = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        greedy(PROMPT, 128),
        "trusted_group",
        cancel.clone(),
    )
    .await
    .unwrap();
    assert!(matches!(stream.next().await, Some(Ok(GenerateEvent::Token { .. }))));
    assert_eq!(model.in_flight(), 1, "the server's generation holds the model");
    cancel.cancel();
    let t = Instant::now();
    let tail: Vec<_> = stream.collect().await;
    let stream_end = t.elapsed();
    assert!(
        matches!(tail.last(), Some(Err(e)) if e.code == ErrorCode::Cancelled),
        "{tail:?}"
    );
    let released = poll_until(Duration::from_millis(500), || model.in_flight() == 0)
        .await
        .expect("server generation still running 500 ms after cancel");
    eprintln!(
        "SINGLE-HOST cancel latency: client stream ended {stream_end:?}, server released the model {released:?} after cancel (bound 500 ms)"
    );
    let entry = poll_until(Duration::from_secs(2), || find_audit(&b, "remote_request").is_some())
        .await
        .and_then(|_| find_audit(&b, "remote_request"))
        .expect("audited");
    assert_eq!(entry.fields["outcome"], "cancelled");
    assert!(entry.fields["completion_tokens"].as_u64().unwrap() < 128);
}

#[tokio::test]
async fn peer_lost_mid_stream_is_typed() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&home);
    let model = m.ensure_loaded(MODEL_ID, LoadOptions::default()).await.unwrap();
    let serve = tokio::spawn(serve_peer(l.inn.clone(), m.clone(), policy, b.audit.clone()));
    let mut stream = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        greedy(PROMPT, 128),
        "trusted_group",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(matches!(stream.next().await, Some(Ok(GenerateEvent::Token { .. }))));
    // The serving endpoint goes away mid-generation.
    let t = Instant::now();
    l.nb.shutdown().await;
    let tail: Vec<_> = timeout(Duration::from_secs(7), stream.collect())
        .await
        .expect("loss must surface within idle 6 s + 1 s");
    let took = t.elapsed();
    let last = match tail.last() {
        Some(Err(e)) => e,
        other => panic!("expected a typed error last, got {other:?}"),
    };
    assert_eq!(last.code, ErrorCode::PeerLost, "{last}");
    assert!(serve.await.unwrap().is_ok());
    poll_until(Duration::from_secs(1), || model.in_flight() == 0)
        .await
        .expect("the server stopped its generation when the client vanished");
    assert!(l.na.connections().is_empty());
    eprintln!("SINGLE-HOST: peer loss mid-stream surfaced as PeerLost after {took:?}");
}

/// Every event of a stream with its arrival time since `t0`.
async fn timed(mut stream: GenerateStream, t0: Instant) -> Vec<(Duration, GenerateEvent)> {
    let mut out = Vec::new();
    while let Some(ev) = stream.next().await {
        out.push((t0.elapsed(), ev.unwrap()));
    }
    out
}

/// Client-observed decode time per output token: (last token − first token) / (tokens − 1).
fn tpot(events: &[(Duration, GenerateEvent)]) -> Duration {
    let at: Vec<Duration> = events
        .iter()
        .filter(|(_, e)| matches!(e, GenerateEvent::Token { .. }))
        .map(|(t, _)| *t)
        .collect();
    assert!(at.len() >= 2, "{} token events", at.len());
    (at[at.len() - 1] - at[0]) / (at.len() as u32 - 1)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn p95(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[((v.len() * 95).div_ceil(100)).saturating_sub(1)]
}

/// Tokens generated in the capture test. The prompt carries a random canary, so the completion
/// varies run to run; 96 tokens reliably yields at least 3 distinct words of 4+ letters to search for
/// (32 sometimes did not). Needles stay at 4+ alphanumeric chars: a 3-char needle would match random
/// ciphertext by chance in ~24 KB of datagrams.
const CAPTURE_TOKENS: u32 = 96;

#[tokio::test]
async fn packet_capture_contains_no_plaintext_prompt_single_host() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let nb = b.start().await;
    let (relay_addr, relay_out_addr, log, _, relay_task) = start_relay(nb.local_addr().unwrap()).await;
    let na = a.start().await;
    let (out, inn) = tokio::join!(na.connect(relay_addr, b.id()), async { nb.accept().await.unwrap() });
    let out = out.unwrap();
    assert_eq!(
        inn.remote_addr, relay_out_addr,
        "the server only ever talked to the relay"
    );
    let m = manager(&home);
    tokio::spawn(serve_peer(inn, m.clone(), policy, b.audit.clone()));

    let mut rnd = [0u8; 8];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut rnd).unwrap();
    let canary = format!("CANARY-{}", hex::encode(rnd));
    let prompt = format!("{PROMPT} {canary}");
    // Reference completion: the same greedy request on the same engine, locally, event by event.
    // The server sends one RemoteChunk per event, so the needles are the individual token texts
    // (what a plaintext wire would carry contiguously), not the whole completion.
    let local = m.ensure_loaded(MODEL_ID, LoadOptions::default()).await.unwrap();
    let reference = timed(
        local
            .generate(greedy(&prompt, CAPTURE_TOKENS), CancellationToken::new())
            .await
            .unwrap(),
        Instant::now(),
    )
    .await;
    let reference: Vec<GenerateEvent> = reference.into_iter().map(|(_, e)| e).collect();
    let expected: String = reference
        .iter()
        .filter_map(|e| match e {
            GenerateEvent::Token { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let expected = expected.trim().to_string();
    let mut token_needles: Vec<String> = reference
        .iter()
        .filter_map(|e| match e {
            GenerateEvent::Token { text, .. } if text.trim().chars().filter(|c| c.is_alphanumeric()).count() >= 4 => {
                Some(text.trim().to_string())
            }
            _ => None,
        })
        .collect();
    token_needles.sort();
    token_needles.dedup();
    assert!(
        token_needles.len() >= 3,
        "too few distinctive tokens to search for: {token_needles:?}"
    );

    let (text, _, usage, ..) = collect(
        generate_remote(
            out.clone(),
            MODEL_ID,
            MODEL_SHA,
            greedy(&prompt, CAPTURE_TOKENS),
            "trusted_group",
            CancellationToken::new(),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(text.trim(), expected);
    assert_eq!(usage.completion_tokens, CAPTURE_TOKENS);
    tokio::time::sleep(Duration::from_millis(200)).await; // final ACKs
    relay_task.abort();
    let captured = log.lock().unwrap().clone();
    let datagrams: Vec<Vec<u8>> = captured.iter().map(|(_, d)| d.clone()).collect();
    assert!(datagrams.len() > 10, "{} datagrams relayed", datagrams.len());

    let mut needles: Vec<&str> = vec![canary.as_str(), prompt.as_str(), PROMPT];
    needles.extend(token_needles.iter().map(String::as_str));
    let hits = scan_for_plaintext(&datagrams, &needles);
    assert!(hits.is_empty(), "plaintext in relayed datagrams {hits:?}");

    // Positive control: the same scanner finds the canary in the plaintext request frame and the
    // completion in a plaintext chunk frame, so an empty result above means something.
    let req_frame = encode_frame(
        &Body::RemoteRequest(RemoteRequest {
            request_json: serde_json::to_string(&greedy(&prompt, CAPTURE_TOKENS)).unwrap(),
            ..Default::default()
        })
        .into(),
    );
    assert_eq!(
        scan_for_plaintext(std::slice::from_ref(&req_frame), &[canary.as_str()]),
        vec![0]
    );
    assert_eq!(scan_for_plaintext(&[req_frame], &[PROMPT]), vec![0]);
    // The real per-token wire sequence in plaintext: one RemoteChunk frame per event, exactly as
    // the server frames them. Every token needle must be found there, so the empty result above
    // is not an artefact of the needles never being contiguous on the wire.
    let plain_frames: Vec<Vec<u8>> = reference
        .iter()
        .map(|ev| {
            encode_frame(
                &Body::RemoteChunk(RemoteChunk {
                    request_id: "0123456789abcdef".into(),
                    event_json: serde_json::to_string(ev).unwrap(),
                    done: matches!(ev, GenerateEvent::Done { .. }),
                })
                .into(),
            )
        })
        .collect();
    for n in &token_needles {
        assert!(
            !scan_for_plaintext(&plain_frames, &[n.as_str()]).is_empty(),
            "positive control misses token {n:?}"
        );
    }

    let (to_server, to_client): (Vec<_>, Vec<_>) = captured.iter().partition(|(c2s, _)| *c2s);
    let bytes = |v: &[&(bool, Vec<u8>)]| v.iter().map(|(_, d)| d.len() as u64).sum::<u64>();
    eprintln!(
        "bench:transport-overhead SINGLE-HOST relay capture, whole session incl. handshake, {} tokens: client→server {} B in {} datagrams, server→client {} B in {} datagrams ({:.0} B/token server→client); 0 plaintext hits over {} needles ({} per-token: {:?})",
        CAPTURE_TOKENS,
        bytes(&to_server),
        to_server.len(),
        bytes(&to_client),
        to_client.len(),
        bytes(&to_client) as f64 / CAPTURE_TOKENS as f64,
        needles.len(),
        token_needles.len(),
        token_needles,
    );
}

/// F-02-03 "transport overhead on decode TPOT measured": the same greedy 64-token request on the
/// same loaded engine, local stream vs through the peer transport, alternating, 5 runs each;
/// client-observed time per output token. SINGLE-HOST loopback QUIC: this is the CPU cost of the
/// QUIC + TLS + framing hop, a lower bound for H1 (<= 5 % on a real LAN), not the LAN number.
#[tokio::test]
async fn decode_tpot_overhead_measured_single_host() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&home);
    let model = m.ensure_loaded(MODEL_ID, LoadOptions::default()).await.unwrap();
    tokio::spawn(serve_peer(l.inn.clone(), m.clone(), policy, b.audit.clone()));
    let (mut local, mut remote) = (Vec::new(), Vec::new());
    for run in 0..6 {
        let t0 = Instant::now();
        let ev = timed(
            model
                .generate(greedy(PROMPT, 64), CancellationToken::new())
                .await
                .unwrap(),
            t0,
        )
        .await;
        let lt = tpot(&ev);
        let t0 = Instant::now();
        let stream = generate_remote(
            l.out.clone(),
            MODEL_ID,
            MODEL_SHA,
            greedy(PROMPT, 64),
            "trusted_group",
            CancellationToken::new(),
        )
        .await
        .unwrap();
        let rt = tpot(&timed(stream, t0).await);
        if run > 0 {
            // run 0 warms caches on both paths
            local.push(lt);
            remote.push(rt);
        }
    }
    let (lt, rt) = (median(local.clone()), median(remote.clone()));
    let ratio = rt.as_secs_f64() / lt.as_secs_f64();
    eprintln!(
        "bench:transport-overhead SINGLE-HOST loopback decode TPOT (median of 5, 64 tokens, stories15M-q4_0): local {:.3} ms, remote {:.3} ms, ratio {ratio:.3} (overhead {:+.1} %); local runs {local:?}, remote runs {remote:?}",
        lt.as_secs_f64() * 1000.0,
        rt.as_secs_f64() * 1000.0,
        (ratio - 1.0) * 100.0,
    );
    // A gross-regression guard only; the H1 claim needs the two-machine LAN measurement (B-002).
    assert!(ratio < 1.5, "remote decode is {ratio:.2}x local");
}

/// A peer's request is held to the local API's bounds: a raw GBNF grammar is refused before
/// anything is loaded (so this runs without the fixture too).
#[tokio::test]
async fn peer_supplied_grammar_refused_before_load() {
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&Paths::at(ucompute_home()));
    tokio::spawn(serve_peer(l.inn.clone(), m.clone(), policy, b.audit.clone()));
    let req = GenerateRequest {
        grammar: Some("root ::= (\"a\" root) | \"a\"".into()),
        ..greedy(PROMPT, 8)
    };
    let stream = generate_remote(
        l.out.clone(),
        MODEL_ID,
        MODEL_SHA,
        req,
        "trusted_group",
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let items: Vec<_> = timeout(Duration::from_secs(5), stream.collect()).await.unwrap();
    let e = items[0].as_ref().unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidRequest, "{e}");
    assert!(e.message.contains("grammar"), "{e}");
    assert!(m.status().await.is_none(), "nothing was loaded");
}

/// A request over the peer's quota is refused at once and audited, never a job row: refusals cost
/// the peer nothing, so as rows they could push this computer's own history out of the 256 kept.
/// SINGLE-HOST, no engine: the one admitted request fails at the load (the model is not in this
/// store), which still makes it a row.
#[tokio::test]
async fn quota_refusals_are_audited_but_not_job_rows_single_host() {
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let empty = tempfile::tempdir().unwrap();
    let jobs = ucompute_observability::jobs::Jobs::new();
    let opts = ServeOptions {
        quotas: Arc::new(Quotas::new(QuotaConfig {
            requests_per_minute: 1,
            ..QuotaConfig::default()
        })),
        jobs: jobs.clone(),
        ..ServeOptions::standalone()
    };
    tokio::spawn(serve_peer_with(
        l.inn.clone(),
        manager(&Paths::at(empty.path().to_path_buf())),
        policy,
        b.audit.clone(),
        opts,
    ));
    let mut codes = vec![];
    for _ in 0..3 {
        let r = match generate_remote(
            l.out.clone(),
            MODEL_ID,
            MODEL_SHA,
            greedy(PROMPT, 4),
            "trusted_group",
            CancellationToken::new(),
        )
        .await
        {
            Ok(s) => collect(s).await.map(|_| ()),
            Err(e) => Err(e),
        };
        codes.push(r.unwrap_err().code);
    }
    assert_eq!(codes[1..], [ErrorCode::Overloaded, ErrorCode::Overloaded], "{codes:?}");
    assert_ne!(codes[0], ErrorCode::Overloaded, "{codes:?}");
    poll_until(Duration::from_secs(5), || jobs.list().1.len() == 1)
        .await
        .expect("the admitted request ends as a row");
    let refused = poll_until(Duration::from_secs(5), || {
        let tail = b.audit.tail(50).unwrap();
        tail.iter()
            .filter(|e| e.kind == "remote_request" && e.fields["outcome"] == "overloaded")
            .count()
            == 2
    })
    .await;
    assert!(refused.is_some(), "both refusals are audited: {:?}", audit_kinds(&b));
    let (active, recent) = jobs.list();
    assert!(active.is_empty(), "{active:?}");
    assert_eq!(recent.len(), 1, "only the admitted request is a job: {recent:?}");
    assert_ne!(recent[0].error_code.as_deref(), Some("overloaded"));
}

/// F-06-04 `peer_flood_throttled` + `bench:abuse-flood`: a peer floods 8 concurrent requests at a
/// time while this device serves its own (local) requests. The serving side shares its engine as
/// the daemon would (`serve_peer_with`: the local load options, the local admission's engine-slot
/// semaphore, at most one peer generation at a time, per-peer quotas). The peer gets typed
/// `overloaded` errors; local p95 latency must degrade by less than 20 %. SINGLE-HOST: peer and
/// local work share this Mac's CPU/GPU, which makes the bound harder, not easier.
#[tokio::test]
async fn peer_flood_throttled_local_p95_within_20pct_single_host() {
    let Some(home) = fixtures() else { return };
    let _turn = engine_turn().await;
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let l = link(&a, &b).await;
    let m = manager(&home);
    let slots = 4;
    let load = LoadOptions {
        parallel_slots: Some(slots),
        ..Default::default()
    };
    let engine = Arc::new(Semaphore::new(slots as usize));
    let peer_share = Arc::new(Semaphore::new(1));
    let opts = ServeOptions {
        load: load.clone(),
        quotas: Arc::new(Quotas::new(QuotaConfig {
            requests_per_minute: 100_000,
            max_concurrent: 2,
            ..QuotaConfig::default()
        })),
        engine_slots: Some(engine.clone()),
        peer_slots: peer_share.clone(),
        ..ServeOptions::standalone()
    };
    tokio::spawn(serve_peer_with(l.inn.clone(), m.clone(), policy, b.audit.clone(), opts));
    let model = m.ensure_loaded(MODEL_ID, load).await.unwrap();
    // One local request as the API runs it: wait for an engine slot (admission), then generate.
    let local_once = || {
        let (engine, model) = (engine.clone(), model.clone());
        async move {
            let t0 = Instant::now();
            let _slot = engine.acquire_owned().await.unwrap();
            let s = model
                .generate(greedy(PROMPT, 16), CancellationToken::new())
                .await
                .unwrap();
            collect(s).await.unwrap();
            t0.elapsed()
        }
    };
    for _ in 0..3 {
        local_once().await;
    }
    // Flood on and off in alternating phases, so drift on this shared machine hits both buckets.
    let (stop, on) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let (out, flag, flooding) = (l.out.clone(), stop.clone(), on.clone());
    let flood = tokio::spawn(async move {
        let (mut ok, mut overloaded, mut other) = (0u32, 0u32, Vec::new());
        while !flag.load(Ordering::SeqCst) {
            if !flooding.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
            let wave = (0..8).map(|_| {
                let out = out.clone();
                async move {
                    let s = generate_remote(
                        out,
                        MODEL_ID,
                        MODEL_SHA,
                        greedy(PROMPT, 32),
                        "trusted_group",
                        CancellationToken::new(),
                    )
                    .await?;
                    collect(s).await
                }
            });
            for r in futures_util::future::join_all(wave).await {
                match r {
                    Ok(_) => ok += 1,
                    Err(e) if e.code == ErrorCode::Overloaded => overloaded += 1,
                    Err(e) => other.push(e.code),
                }
            }
        }
        (ok, overloaded, other)
    });
    let (mut baseline, mut flooded) = (Vec::new(), Vec::new());
    for phase in 0..8 {
        let flood_phase = phase % 2 == 1;
        on.store(flood_phase, Ordering::SeqCst);
        if flood_phase {
            // Measure only once a peer generation really holds the engine.
            poll_until(Duration::from_secs(5), || peer_share.available_permits() == 0)
                .await
                .expect("the flood reaches the engine");
        } else {
            // No peer generation left over from the last flood phase in the baseline.
            poll_until(Duration::from_secs(5), || peer_share.available_permits() == 1)
                .await
                .expect("peer generations drain");
        }
        for _ in 0..10 {
            let t = local_once().await;
            if flood_phase {
                flooded.push(t);
            } else {
                baseline.push(t);
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    let (ok, overloaded, other) = flood.await.unwrap();
    let (base, under) = (p95(baseline.clone()), p95(flooded.clone()));
    let ratio = under.as_secs_f64() / base.as_secs_f64();
    eprintln!(
        "bench:abuse-flood SINGLE-HOST: local p95 {:.1} ms baseline vs {:.1} ms under a peer flood (ratio {ratio:.3}, {:+.1} %); peer requests served {ok}, refused overloaded {overloaded}, other {other:?}; local median {:.1} -> {:.1} ms",
        base.as_secs_f64() * 1000.0,
        under.as_secs_f64() * 1000.0,
        (ratio - 1.0) * 100.0,
        median(baseline).as_secs_f64() * 1000.0,
        median(flooded).as_secs_f64() * 1000.0,
    );
    assert!(overloaded > 0, "the flood must be throttled with typed errors");
    assert!(other.is_empty(), "every refusal is typed overloaded: {other:?}");
    assert!(ok > 0, "the peer is still served within its share");
    assert!(
        ratio < 1.2,
        "local p95 degraded {:.1} % under the flood",
        (ratio - 1.0) * 100.0
    );
}
