//! Mode B remote execution between two in-process nodes on loopback. **SINGLE-HOST**: both nodes
//! run on this one Mac; nothing here satisfies a two-machine criterion. The serving node runs the
//! real pinned llama-server on the stories15M fixture from COMMON_HOME (skips loudly when absent;
//! `COMMON_REQUIRE_FIXTURES=1` fails instead). Nothing is mocked.
mod common;

use common::{audit_kinds, pair, Peer};
use common_artifacts::Store;
use common_core::{ErrorCode, Paths};
use common_identity::{Group, GroupPolicy};
use common_network::proto::{control_message::Body, encode_frame, RemoteChunk, RemoteRequest};
use common_network::remote::{generate_remote, serve_peer};
use common_network::scan_for_plaintext;
use common_network::transport::{Node, PeerConnection};
use common_policy::{ExecutionTarget, PolicyConfig, PolicyEngine, PrivacyMode, WorkloadKind};
use common_runtime::manager::{LoadOptions, ModelManager};
use common_runtime::{collect, GenerateEvent, GenerateRequest, PromptInput};
use futures_util::StreamExt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const MODEL_ID: &str = "stories15M-q4_0";
const MODEL_SHA: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

/// Relayed datagrams: (client→server?, payload).
type Capture = Arc<Mutex<Vec<(bool, Vec<u8>)>>>;

fn common_home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

/// The real engine and fixture, or None after printing why; `COMMON_REQUIRE_FIXTURES=1` makes a
/// missing asset a failure so a green run can never mean "nothing was generated".
fn fixtures() -> Option<Paths> {
    let paths = Paths::at(common_home());
    let engine_dir = paths.backends().join("llama.cpp").join("b11609");
    let blob = paths.model_blobs().join(format!("sha256-{MODEL_SHA}"));
    let manifest = paths.model_manifests().join(format!("{MODEL_ID}.json"));
    let missing = if !common_artifacts::backend::binary(&engine_dir, "llama-server").is_file() {
        format!("pinned llama-server missing at {}", engine_dir.display())
    } else if !blob.is_file() {
        format!("fixture model missing at {}", blob.display())
    } else if !manifest.is_file() {
        format!("fixture manifest missing at {}", manifest.display())
    } else {
        return Some(paths);
    };
    assert!(
        std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "COMMON_REQUIRE_FIXTURES=1: {missing}"
    );
    eprintln!("SKIP: {missing}");
    None
}

/// The serving node's manager over the real COMMON_HOME store (its identity and audit stay in a
/// temp dir, see `Peer`).
fn manager(paths: &Paths) -> Arc<ModelManager> {
    paths.ensure_dirs().unwrap();
    let engine_dir = paths.backends().join("llama.cpp").join("b11609");
    let backend = common_runtime::backend_by_name("llama.cpp", engine_dir, paths.clone()).unwrap();
    Arc::new(ModelManager::new(
        paths.clone(),
        Store::new(paths.clone()),
        backend,
        common_hardware::probe().unwrap(),
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

fn find_audit(p: &Peer, kind: &str) -> Option<common_observability::AuditEvent> {
    p.audit.tail(50).unwrap().into_iter().find(|e| e.kind == kind)
}

/// Polls until `f` holds or `bound` passes; the time it took, or None.
async fn poll_until(bound: Duration, mut f: impl FnMut() -> bool) -> Option<Duration> {
    let t = Instant::now();
    while t.elapsed() < bound {
        if f() {
            return Some(t.elapsed());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    None
}

#[tokio::test]
async fn remote_golden_text_single_host() {
    let Some(home) = fixtures() else { return };
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
    let m = manager(&Paths::at(common_home()));
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

/// TEST-ONLY UDP relay: forwards every datagram between the client endpoint and the server
/// endpoint and records every payload. No such socket exists in `src/` (tests/invariants.rs).
async fn relay(r_in: UdpSocket, r_out: UdpSocket, server: SocketAddr, log: Capture) {
    let (mut bi, mut bo) = (vec![0u8; 65535], vec![0u8; 65535]);
    let mut client: Option<SocketAddr> = None;
    loop {
        tokio::select! {
            r = r_in.recv_from(&mut bi) => {
                let (n, from) = r.unwrap();
                client = Some(from);
                log.lock().unwrap().push((true, bi[..n].to_vec()));
                r_out.send_to(&bi[..n], server).await.unwrap();
            }
            r = r_out.recv_from(&mut bo) => {
                let (n, _) = r.unwrap();
                log.lock().unwrap().push((false, bo[..n].to_vec()));
                if let Some(c) = client {
                    r_in.send_to(&bo[..n], c).await.unwrap();
                }
            }
        }
    }
}

#[tokio::test]
async fn packet_capture_contains_no_plaintext_prompt_single_host() {
    let Some(home) = fixtures() else { return };
    let (a, b) = pair();
    let policy = serving_policy(&b, &a, true);
    let nb = b.start().await;
    let r_in = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let r_out = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (relay_addr, relay_out_addr) = (r_in.local_addr().unwrap(), r_out.local_addr().unwrap());
    let log: Capture = Arc::default();
    let relay_task = tokio::spawn(relay(r_in, r_out, nb.local_addr().unwrap(), log.clone()));
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
    // Reference completion: the same greedy request on the same engine, locally.
    let local = m.ensure_loaded(MODEL_ID, LoadOptions::default()).await.unwrap();
    let (expected, ..) = collect(
        local
            .generate(greedy(&prompt, 32), CancellationToken::new())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    let expected = expected.trim().to_string();
    assert!(expected.len() >= 8, "completion too short to search for: {expected:?}");

    let (text, _, usage, ..) = collect(
        generate_remote(
            out.clone(),
            MODEL_ID,
            MODEL_SHA,
            greedy(&prompt, 32),
            "trusted_group",
            CancellationToken::new(),
        )
        .await
        .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(text.trim(), expected);
    assert_eq!(usage.completion_tokens, 32);
    tokio::time::sleep(Duration::from_millis(200)).await; // final ACKs
    relay_task.abort();
    let captured = log.lock().unwrap().clone();
    let datagrams: Vec<Vec<u8>> = captured.iter().map(|(_, d)| d.clone()).collect();
    assert!(datagrams.len() > 10, "{} datagrams relayed", datagrams.len());

    // The completion as the peer's JSON would carry it (escaped), and as raw text.
    let expected_json = serde_json::to_string(&expected).unwrap().trim_matches('"').to_string();
    let needles = [
        canary.as_str(),
        prompt.as_str(),
        PROMPT,
        expected.as_str(),
        expected_json.as_str(),
    ];
    let hits = scan_for_plaintext(&datagrams, &needles);
    assert!(hits.is_empty(), "plaintext in relayed datagrams {hits:?}");

    // Positive control: the same scanner finds the canary in the plaintext request frame and the
    // completion in a plaintext chunk frame, so an empty result above means something.
    let req_frame = encode_frame(
        &Body::RemoteRequest(RemoteRequest {
            request_json: serde_json::to_string(&greedy(&prompt, 32)).unwrap(),
            ..Default::default()
        })
        .into(),
    );
    assert_eq!(
        scan_for_plaintext(std::slice::from_ref(&req_frame), &[canary.as_str()]),
        vec![0]
    );
    assert_eq!(scan_for_plaintext(&[req_frame], &[PROMPT]), vec![0]);
    let chunk_frame = encode_frame(
        &Body::RemoteChunk(RemoteChunk {
            event_json: serde_json::to_string(&GenerateEvent::Token {
                text: expected.clone(),
                token_ids: vec![],
            })
            .unwrap(),
            ..Default::default()
        })
        .into(),
    );
    assert_eq!(scan_for_plaintext(&[chunk_frame], &[expected_json.as_str()]), vec![0]);

    let (to_server, to_client): (Vec<_>, Vec<_>) = captured.iter().partition(|(c2s, _)| *c2s);
    let bytes = |v: &[&(bool, Vec<u8>)]| v.iter().map(|(_, d)| d.len() as u64).sum::<u64>();
    eprintln!(
        "bench:transport-overhead SINGLE-HOST relay capture, whole session incl. handshake, 32 tokens: client→server {} B in {} datagrams, server→client {} B in {} datagrams ({:.0} B/token server→client); 0 plaintext hits over {} needles",
        bytes(&to_server),
        to_server.len(),
        bytes(&to_client),
        to_client.len(),
        bytes(&to_client) as f64 / 32.0,
        needles.len(),
    );
}
