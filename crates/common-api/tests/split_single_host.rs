//! WP-04 C6: the `split:<peer>` target end to end, REAL and SINGLE-HOST. Two daemons (head H and
//! stage host R) run in this one process on this one machine, with separate state directories and
//! identities, loopback QUIC between them, the real pinned llama-server on H and the real pinned
//! ggml-rpc-server on R, carried through Common's tunnel (`common_network::rpc`). Nothing here
//! satisfies a two-machine criterion (F-03-01, F-03-02, Gate D) or claims a speedup.
//!
//! `COMMON_TEST_SINGLE_HOST_POOL=1` is set by every test in this binary: it lets this Mac host a
//! stage and lifts the licence gate for stories15M (unknown licence, B-011), and only because the
//! peer is on loopback. qwen2.5-0.5b (Apache-2.0) is used where a generation must last seconds.
//! On this Mac R's rpc-server runs on Metal, so `same_backend` here is a CPU head with a Metal
//! stage, not ggml-cpu on both stages.

mod common;

use common::*;
use common_api::ApiConfig;
use common_identity::{PeerId, PeerRecord};
use common_network::NetworkConfig;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// One pair of daemons at a time: each runs a real engine and an rpc-server.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn cfg(max_concurrent: usize) -> ApiConfig {
    ApiConfig {
        max_concurrent,
        network: NetworkConfig {
            enabled: false,
            listen_port: 0,
            bind_addr: "127.0.0.1".into(),
            mdns: false,
            ..NetworkConfig::default()
        },
        ..ApiConfig::default()
    }
}

async fn call(srv: &Server, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
    let r = client()
        .request(method, srv.url(path))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn get(srv: &Server, path: &str) -> Value {
    call(srv, reqwest::Method::GET, path, Value::Null).await.1
}

/// R: [`serving_app`] whose memory sample is a fixed 6 GiB. This 8 GB Mac rarely has the 2 GiB
/// R keeps free at admission; that rule has its own test (`rpc_session_refused_when_memory_short_
/// single_host`, common-network). R's rpc-server uses what it really needs.
fn stage_app(home: &std::path::Path, dir: &std::path::Path) -> std::sync::Arc<common_api::AppState> {
    use common_core::Paths;
    let real = Paths::at(home);
    let backend = common_runtime::backend_by_name("llama.cpp", engine_dir(&real), real.clone()).unwrap();
    let mut manager = common_runtime::manager::ModelManager::new(
        real.clone(),
        common_artifacts::Store::new(real),
        backend,
        common_hardware::probe().unwrap(),
    );
    manager.available_memory = || Some(6 << 30);
    app_with(cfg(2), Paths::at(dir), std::sync::Arc::new(manager))
}

struct Pool {
    /// `pooled` (configuration P) when H has a GPU, else `same_backend` (C).
    mode: &'static str,
    h: Server,
    r: Server,
    idr: PeerId,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    _turn: tokio::sync::MutexGuard<'static, ()>,
}

fn pin(srv: &Server, other: &Server, name: &str, addr: &str) {
    srv.state
        .peers
        .pin(PeerRecord {
            id: other.state.identity.id,
            name: name.into(),
            paired_at_utc: common_identity::utc_now(),
            last_seen_utc: None,
            last_addr: Some(addr.into()),
        })
        .unwrap();
}

/// H and R pinned to each other (as pairing leaves them), both in a group allowing pooled
/// inference only, H in `trusted_group`, R serving peers. None without the fixtures.
async fn pool() -> Option<Pool> {
    let home = fixture_home()?;
    let turn = TURN.lock().await;
    std::env::set_var(common_network::rpc::TEST_POOL_ENV, "1");
    let dirs = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let h = Server::start(serving_app(&home, dirs.0.path(), cfg(1))).await;
    let r = Server::start(stage_app(&home, dirs.1.path())).await;
    let mut addrs = vec![];
    for (srv, serve) in [(&h, false), (&r, true)] {
        let (status, v) = call(
            srv,
            reqwest::Method::PUT,
            "/api/network",
            json!({ "enabled": true, "serve_peers": serve }),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        addrs.push(v["listen_addr"].as_str().unwrap().to_string());
    }
    pin(&h, &r, "R", &addrs[1]);
    pin(&r, &h, "H", &addrs[0]);
    for (srv, other) in [(&h, &r), (&r, &h)] {
        let (status, v) = call(
            srv,
            reqwest::Method::PUT,
            "/api/groups",
            json!({ "name": "pool", "members": [other.state.identity.id.to_hex()], "policy": { "allow_remote_inference": false, "allow_pooled_inference": true } }),
        )
        .await;
        assert_eq!(status, 200, "{v}");
    }
    let (status, v) = call(
        &h,
        reqwest::Method::PUT,
        "/api/privacy",
        json!({ "mode": "trusted_group", "acknowledged": true }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let idr = r.state.identity.id;
    let gpu = h
        .state
        .manager
        .hardware
        .backends
        .iter()
        .any(|b| b.name == "metal" && b.available);
    Some(Pool {
        mode: if gpu { "pooled" } else { "same_backend" },
        h,
        r,
        idr,
        _dirs: dirs,
        _turn: turn,
    })
}

impl Pool {
    fn body(&self, model: &str, prompt: &str, n: u32, split_at: u32, mode: &str) -> Value {
        json!({ "model": model, "prompt": prompt, "max_tokens": n, "temperature": 0, "seed": 42,
                "target": format!("split:{}", self.idr.to_hex()), "split_at": split_at, "split_mode": mode, "acknowledged": true })
    }

    /// R's rpc-server (pid, port) while a session is open.
    fn stage(&self) -> Option<(u32, u16)> {
        self.r.state.stage().and_then(|s| s.session())
    }

    async fn stop(self) {
        self.h.stop().await;
        self.r.stop().await;
    }
}

fn alive(pid: u32) -> bool {
    // signal 0: does the process exist (and is it not a reaped zombie of ours)
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

async fn within<T>(what: &str, bound: Duration, mut f: impl FnMut() -> Option<T>) -> (T, Duration) {
    let t0 = Instant::now();
    loop {
        if let Some(v) = f() {
            return (v, t0.elapsed());
        }
        assert!(t0.elapsed() < bound, "{what} not within {bound:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn audit(srv: &Server, kind: &str) -> Vec<Value> {
    srv.state
        .audit
        .tail(10_000)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
}

/// The greedy 64-token completion of a split equals this device's own run of the whole model, in
/// configuration P (R holds blocks [0,3) on its Metal; H's Metal [3,6) and the output) and C
/// (H's CPU [0,3); R [3,6) and the output). The placement comes from the engine log.
#[tokio::test(flavor = "multi_thread")]
async fn split_completion_golden_64_tokens_single_host() {
    let Some(p) = pool().await else { return };
    let local = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 64, "temperature": 0, "seed": 42 });
    let (status, reference) = call(&p.h, reqwest::Method::POST, "/v1/completions", local).await;
    assert_eq!(status, 200, "{reference}");
    let want = reference["choices"][0]["text"].as_str().unwrap().to_string();
    assert!(
        want.starts_with(GOLDEN),
        "the local reference is the known greedy text: {want:?}"
    );
    assert_eq!(reference["usage"]["completion_tokens"], 64);

    // configuration P needs this device's GPU (sentinel's CPU-only build has none: refused, typed)
    let gpu =
        p.h.state
            .manager
            .hardware
            .backends
            .iter()
            .any(|b| b.name == "metal" && b.available);
    let mut modes = vec![("same_backend", ["CPU", "RPC0"])];
    if gpu {
        modes.insert(0, ("pooled", ["RPC0", "MTL0"]));
    } else {
        let (status, v) = call(
            &p.h,
            reqwest::Method::POST,
            "/v1/completions",
            p.body(MODEL_ID, PROMPT, 64, 3, "pooled"),
        )
        .await;
        assert_eq!(
            (status, v["error"]["code"].as_str()),
            (503, Some("gpu_unavailable")),
            "{v}"
        );
    }
    for (mode, devices) in modes {
        let t0 = Instant::now();
        let (status, v) = call(
            &p.h,
            reqwest::Method::POST,
            "/v1/completions",
            p.body(MODEL_ID, PROMPT, 64, 3, mode),
        )
        .await;
        eprintln!("SINGLE-HOST split {mode}: {:?} (load and 64 tokens)", t0.elapsed());
        assert_eq!(status, 200, "{mode}: {v}");
        assert_eq!(v["target"], format!("split:{}", p.idr.to_hex()));
        assert_eq!(v["usage"]["completion_tokens"], 64, "{v}");
        let got = v["choices"][0]["text"].as_str().unwrap();
        let agree = got.chars().zip(want.chars()).take_while(|(a, b)| a == b).count();
        eprintln!(
            "SINGLE-HOST split {mode}: {agree} of {} characters equal the local run",
            want.len()
        );
        assert_eq!(got, want, "{mode}: split text differs from the local run");
        let stages = &v["placement"]["stages"];
        assert_eq!(stages[0]["device"], devices[0], "{v}");
        assert_eq!(
            (stages[0]["first_block"].as_u64(), stages[0]["end_block"].as_u64()),
            (Some(0), Some(3))
        );
        assert_eq!(stages[1]["device"], devices[1], "{v}");
        assert_eq!(
            (stages[1]["first_block"].as_u64(), stages[1]["end_block"].as_u64()),
            (Some(3), Some(6))
        );
        assert!(
            v["placement"]["devices"].as_str().unwrap().contains("RPC0 (127.0.0.1:"),
            "{v}"
        );
        let status = get(&p.h, "/api/status").await;
        assert_eq!(status["loaded"]["split"], *stages, "{status}");
        assert_eq!(status["split"]["split_mode"], mode);
        assert!(status["split"]["to_r"].as_u64().unwrap() > 0 && status["split"]["to_h"].as_u64().unwrap() > 0);
        assert!(p.stage().is_some(), "R hosts the stage");
    }
    let r_opens = audit(&p.r, "session_open");
    assert_eq!(
        r_opens.len(),
        if gpu { 2 } else { 1 },
        "one session per configuration: {r_opens:?}"
    );
    assert!(r_opens.iter().all(|e| e["fields"]["role"] == "stage"));
    p.stop().await;
}

/// ADR-0005 D7: R goes away mid-generation (its peer endpoint stops, as when its daemon dies):
/// the request fails with `peer_lost` within 10 s, nothing reruns locally, H's split engine stops
/// and R's rpc-server is gone. qwen2.5-0.5b (Apache-2.0), a long greedy generation.
#[tokio::test(flavor = "multi_thread")]
async fn split_peer_killed_mid_generation_is_peer_lost_within_10s_single_host() {
    let Some(p) = pool().await else { return };
    if !has_model(&common_home(), CHAT_MODEL_ID) {
        return eprintln!("SKIP: {CHAT_MODEL_ID} not downloaded");
    }
    let mut body = p.body(
        CHAT_MODEL_ID,
        "Count from one to one thousand in words:",
        3000,
        12,
        p.mode,
    );
    body["stream"] = json!(true);
    let mut r = client()
        .post(p.h.url("/v1/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    if r.status() != 200 {
        panic!("{}: {}", r.status(), r.text().await.unwrap());
    }
    let (mut text, mut chunks) = (String::new(), 0);
    while chunks < 20 {
        text.push_str(&String::from_utf8_lossy(
            &r.chunk().await.unwrap().expect("stream ended early"),
        ));
        chunks = text.matches("data: ").count();
    }
    let (pid, _) = p.stage().expect("a stage is open");
    let killed = Instant::now();
    p.r.state.stop_network().await;
    while let Some(c) = r.chunk().await.unwrap() {
        text.push_str(&String::from_utf8_lossy(&c));
    }
    let ended = killed.elapsed();
    eprintln!("SINGLE-HOST peer lost: the stream ended {ended:?} after R's endpoint stopped");
    assert!(ended < Duration::from_secs(10), "{ended:?}");
    let last_err = sse_data(&text)
        .iter()
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .find(|v| v.get("error").is_some())
        .unwrap_or_else(|| panic!("no error event: {text}"));
    assert_eq!(last_err["error"]["code"], "peer_lost", "{last_err}");
    within("H's split engine stopped", Duration::from_secs(5), || {
        p.h.state.manager.snapshot().loaded.is_none().then_some(())
    })
    .await;
    within("R's rpc-server gone", Duration::from_secs(6), || {
        (!alive(pid)).then_some(())
    })
    .await;
    let jobs = get(&p.h, "/api/jobs").await;
    assert!(jobs.to_string().contains("peer_lost"), "{jobs}");
    p.stop().await;
}

/// ADR-0005 D5 and D7: turning pooling off in H's group mid-session ends the session within 5 s
/// (the 1 s policy tick), and H stops the split engine itself: an idle engine never learns that its
/// RPC connections were reset, so nothing else would. The next split request is refused by the
/// policy before any connection.
#[tokio::test(flavor = "multi_thread")]
async fn group_pooling_off_mid_session_stops_the_split_engine_single_host() {
    let Some(p) = pool().await else { return };
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(MODEL_ID, PROMPT, 4, 3, p.mode),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let engine = get(&p.h, "/api/status").await["loaded"]["pid"].as_u64().unwrap() as u32;
    let (rpc_pid, _) = p.stage().expect("a stage is open");
    let off = Instant::now();
    let (status, v) = call(
        &p.h,
        reqwest::Method::PUT,
        "/api/groups",
        json!({ "name": "pool", "members": [p.idr.to_hex()], "policy": { "allow_remote_inference": false, "allow_pooled_inference": false } }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    within("H's split engine stopped", Duration::from_secs(5), || {
        (p.h.state.manager.snapshot().loaded.is_none() && !alive(engine)).then_some(())
    })
    .await;
    within("R's rpc-server stopped", Duration::from_secs(6), || {
        (!alive(rpc_pid)).then_some(())
    })
    .await;
    eprintln!(
        "SINGLE-HOST pooling off: both ends stopped {:?} after the group change",
        off.elapsed()
    );
    let status = get(&p.h, "/api/status").await;
    assert_eq!(status["split"]["ended"], "peer_unauthorized", "{status}");
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(MODEL_ID, PROMPT, 4, 3, p.mode),
    )
    .await;
    assert_eq!(
        (status, v["error"]["code"].as_str()),
        (403, Some("peer_unauthorized")),
        "{v}"
    );
    p.stop().await;
}

/// `local_only` set mid-session ends it on H's next tick (ADR-0005 D5): H reads its privacy as it
/// is now, not as it was when the session opened. Both ends stop; nothing reruns locally.
#[tokio::test(flavor = "multi_thread")]
async fn privacy_local_only_mid_session_stops_the_split_engine_single_host() {
    let Some(p) = pool().await else { return };
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(MODEL_ID, PROMPT, 4, 3, p.mode),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let engine = get(&p.h, "/api/status").await["loaded"]["pid"].as_u64().unwrap() as u32;
    let (rpc_pid, _) = p.stage().expect("a stage is open");
    let off = Instant::now();
    let (status, v) = call(
        &p.h,
        reqwest::Method::PUT,
        "/api/privacy",
        json!({ "mode": "local_only" }),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    within("H's split engine stopped", Duration::from_secs(5), || {
        (p.h.state.manager.snapshot().loaded.is_none() && !alive(engine)).then_some(())
    })
    .await;
    within("R's rpc-server stopped", Duration::from_secs(6), || {
        (!alive(rpc_pid)).then_some(())
    })
    .await;
    eprintln!(
        "SINGLE-HOST privacy local_only: both ends stopped {:?} after the change",
        off.elapsed()
    );
    let status = get(&p.h, "/api/status").await;
    assert_eq!(status["split"]["ended"], "policy_denied", "{status}");
    p.stop().await;
}

/// A client that drops a split stream mid-generation cancels it (F-08-02); the session stays and
/// the next split request reuses it.
#[tokio::test(flavor = "multi_thread")]
async fn split_cancel_mid_generation_single_host() {
    let Some(p) = pool().await else { return };
    if !has_model(&common_home(), CHAT_MODEL_ID) {
        return eprintln!("SKIP: {CHAT_MODEL_ID} not downloaded");
    }
    let mut body = p.body(
        CHAT_MODEL_ID,
        "Count from one to one thousand in words:",
        3000,
        12,
        p.mode,
    );
    body["stream"] = json!(true);
    let mut r = client()
        .post(p.h.url("/v1/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    if r.status() != 200 {
        panic!("{}: {}", r.status(), r.text().await.unwrap());
    }
    let mut seen = String::new();
    while seen.matches("data: ").count() < 10 {
        seen.push_str(&String::from_utf8_lossy(&r.chunk().await.unwrap().unwrap()));
    }
    let session = get(&p.h, "/api/status").await["split"]["session"].clone();
    drop(r);
    let (_, took) = within(
        "the cancelled generation released the engine",
        Duration::from_secs(2),
        || {
            p.h.state
                .manager
                .snapshot()
                .loaded
                .filter(|l| l.in_flight == 0)
                .map(|_| ())
        },
    )
    .await;
    eprintln!("SINGLE-HOST split cancel: engine idle {took:?} after the client left");
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(CHAT_MODEL_ID, "Hello", 4, 12, p.mode),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        v["placement"]["session"], session,
        "the same session serves the next request"
    );
    p.stop().await;
}

/// `POST /api/unload` on H ends the session, so R's rpc-server stops at once (not after the idle
/// close) and R's slot is free again.
#[tokio::test(flavor = "multi_thread")]
async fn unload_stops_remote_rpc_server_single_host() {
    let Some(p) = pool().await else { return };
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(MODEL_ID, PROMPT, 4, 3, p.mode),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (pid, port) = p.stage().expect("a stage is open");
    assert!(alive(pid));
    let (status, _) = call(&p.h, reqwest::Method::POST, "/api/unload", Value::Null).await;
    assert_eq!(status, 204);
    let (_, took) = within("R's rpc-server stopped", Duration::from_secs(5), || {
        (p.stage().is_none() && !alive(pid)).then_some(())
    })
    .await;
    eprintln!("SINGLE-HOST unload: R's rpc-server (port {port}) gone {took:?} after H's unload");
    assert!(get(&p.h, "/api/status").await["split"].is_null());
    let closes = audit(&p.r, "session_close");
    assert_eq!(closes.len(), 1, "{closes:?}");
    assert_eq!(closes[0]["fields"]["listener_left"], false);
    p.stop().await;
}

/// H's split engine dies (SIGKILL): its RPC connections close, the session idles out, and R stops
/// its rpc-server, so the stage's memory goes with it; H's slot is freed too.
#[tokio::test(flavor = "multi_thread")]
async fn remote_memory_released_when_head_engine_stops_single_host() {
    let Some(p) = pool().await else { return };
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(MODEL_ID, PROMPT, 4, 3, p.mode),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (rpc_pid, _) = p.stage().expect("a stage is open");
    let engine = get(&p.h, "/api/status").await["loaded"]["pid"].as_u64().unwrap();
    let killed = Instant::now();
    std::process::Command::new("kill")
        .args(["-9", &engine.to_string()])
        .status()
        .unwrap();
    within("R's rpc-server stopped", Duration::from_secs(15), || {
        (p.stage().is_none() && !alive(rpc_pid)).then_some(())
    })
    .await;
    eprintln!(
        "SINGLE-HOST head engine killed: R's rpc-server gone {:?} later",
        killed.elapsed()
    );
    let status = get(&p.h, "/api/status").await;
    assert!(status["loaded"].is_null(), "{status}");
    assert!(
        status["split"].is_null() || !status["split"]["ended"].is_null(),
        "{status}"
    );
    p.stop().await;
}

/// I-05 with a split loaded: a local request reloads the model locally and nothing of it crosses
/// to R (the session's byte counters grow only by the split engine's teardown).
#[tokio::test(flavor = "multi_thread")]
async fn local_request_never_uses_the_split_engine_single_host() {
    let Some(p) = pool().await else { return };
    let (status, v) = call(
        &p.h,
        reqwest::Method::POST,
        "/v1/completions",
        p.body(MODEL_ID, PROMPT, 8, 3, p.mode),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let before = get(&p.h, "/api/status").await["split"].clone();
    let local = json!({ "model": MODEL_ID, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "target": "local" });
    let (status, v) = call(&p.h, reqwest::Method::POST, "/v1/completions", local).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["choices"][0]["text"], GOLDEN);
    assert!(v.get("placement").is_none(), "{v}");
    let status = get(&p.h, "/api/status").await;
    assert!(status["loaded"]["split"].is_null(), "the local engine: {status}");
    // The split engine's shutdown frees its remote buffers (a few 17-byte FREE_BUFFER commands);
    // a generation through R would carry three 288-float activations per token, over 3 KB each.
    for k in ["to_r", "to_h"] {
        let delta = status["split"][k].as_u64().unwrap() - before[k].as_u64().unwrap();
        eprintln!("SINGLE-HOST local request with a split loaded: {k} grew by {delta} B");
        assert!(delta < 1024, "{k}: {delta} B crossed to R during a local request");
    }
    // the unused session idles out and R's rpc-server stops
    within("the idle session closed", Duration::from_secs(15), || {
        p.stage().is_none().then_some(())
    })
    .await;
    p.stop().await;
}
