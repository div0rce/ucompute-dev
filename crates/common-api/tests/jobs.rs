//! `GET /api/jobs` (desktop UI 2.0, E3) with the FAKE `LoadedModel` double from `common/mod.rs` in
//! the slot: queue mechanics (a queued row with its client_ref, a request abandoned while queued,
//! a full queue) need deterministic timing, not an engine. Rows of real generations are checked in
//! `real_engine.rs`, rows of requests to and from a paired device in `mesh.rs` (SINGLE-HOST).

mod common;

use common::*;
use common_api::ApiConfig;
use common_hardware::HardwareReport;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

async fn jobs(srv: &Server) -> Value {
    client()
        .get(srv.url("/api/jobs"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

/// The row with `client_ref` in `list` (`active` or `recent`).
fn by_ref(v: &Value, list: &str, client_ref: &str) -> Option<Value> {
    v[list]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["client_ref"] == client_ref)
        .cloned()
}

async fn wait_for<T, F, Fut>(what: &str, mut f: F) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<T>>,
{
    let t0 = Instant::now();
    loop {
        if let Some(v) = f().await {
            return v;
        }
        assert!(t0.elapsed() < Duration::from_secs(10), "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn jobs_show_running_queued_abandoned_and_refused_requests_fake() {
    let dir = tempfile::tempdir().unwrap();
    let config = ApiConfig {
        max_concurrent: 1,
        max_queue: 1,
        queue_timeout_ms: 20_000,
        ..ApiConfig::default()
    };
    let srv = Server::start(app(config, dir.path().to_path_buf(), HardwareReport::default())).await;
    // 60 pieces 50 ms apart: A holds the only slot for about 3 s.
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(60, Duration::from_millis(50))).await;
    let canary = "ZEBRA_CANARY_PROMPT_4471";
    let body = |stream: bool| json!({ "model": MODEL_ID, "prompt": format!("{canary} once upon"), "max_tokens": 60, "stream": stream, "stream_options": { "include_usage": true } });
    let post = |client_ref: &'static str, body: Value| {
        client()
            .post(srv.url("/v1/completions"))
            .header("x-common-client-ref", client_ref)
            .json(&body)
            .send()
    };

    let mut a = post("ref-a", body(true)).await.unwrap();
    assert_eq!(a.status(), 200);
    let mut a_text = String::from_utf8(a.chunk().await.unwrap().unwrap().to_vec()).unwrap();
    let row = by_ref(&jobs(&srv).await, "active", "ref-a").expect("A is active");
    assert_eq!(
        (row["state"].as_str(), row["target"].as_str(), row["origin"].as_str()),
        (Some("running"), Some("local"), Some("local"))
    );
    assert_eq!(
        (
            row["kind"].as_str(),
            row["model_id"].as_str(),
            row["device_actual"].as_str()
        ),
        (Some("completion"), Some(MODEL_ID), Some("cpu"))
    );
    assert!(row["id"].as_str().unwrap().starts_with("cmpl-"), "{row}");
    assert!(
        row["queue_ms"].is_u64() && row["load_ms"].is_u64() && row["ttft_ms"].is_u64(),
        "{row}"
    );

    // B waits for the slot; its row is there, queued, with its ref, before any load or token.
    let b = tokio::spawn(post("ref-b", body(false)));
    let row = wait_for("B queued", || async { by_ref(&jobs(&srv).await, "active", "ref-b") }).await;
    assert_eq!(row["state"], "queued", "{row}");
    assert!(
        row["started_at_utc"].is_null() && row["completion_tokens"].is_null(),
        "{row}"
    );

    // A's count is live: it rises between two polls.
    let n = |v: &Value| {
        by_ref(v, "active", "ref-a").unwrap()["completion_tokens"]
            .as_u64()
            .unwrap()
    };
    let first = n(&jobs(&srv).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = n(&jobs(&srv).await);
    assert!(second > first, "completion_tokens {first} -> {second}");

    // C finds the queue full: refused at once, listed as an error.
    let c = post("ref-c", body(false)).await.unwrap();
    assert_eq!(c.status(), 429);
    let row = by_ref(&jobs(&srv).await, "recent", "ref-c").expect("C is listed");
    assert_eq!(
        (row["state"].as_str(), row["error_code"].as_str()),
        (Some("error"), Some("overloaded")),
        "{row}"
    );

    // B's client goes away while B still waits: the server drops the request, its row ends
    // `cancelled` and it leaves the queue, while A is still streaming.
    b.abort();
    let row = wait_for("B cancelled", || async { by_ref(&jobs(&srv).await, "recent", "ref-b") }).await;
    assert_eq!(
        (row["state"].as_str(), row["error_code"].as_str()),
        (Some("cancelled"), Some("cancelled")),
        "{row}"
    );
    assert!(row["started_at_utc"].is_null(), "never admitted: {row}");
    let status: Value = client()
        .get(srv.url("/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["queue"]["queued"], 0,
        "the abandoned request left the queue: {status}"
    );
    assert!(by_ref(&jobs(&srv).await, "active", "ref-a").is_some(), "A still runs");

    // A finishes: its row moves to recent with the engine's count, equal to the usage it sent.
    while let Some(c) = a.chunk().await.unwrap() {
        a_text.push_str(std::str::from_utf8(&c).unwrap());
    }
    let usage = sse_data(&a_text)
        .iter()
        .filter_map(|d| serde_json::from_str::<Value>(d).ok())
        .find_map(|v| v.get("usage").cloned())
        .expect("usage chunk");
    let v = jobs(&srv).await;
    assert!(v["active"].as_array().unwrap().is_empty(), "{v}");
    let row = by_ref(&v, "recent", "ref-a").unwrap();
    assert_eq!(
        (row["state"].as_str(), row["finish_reason"].as_str()),
        (Some("done"), Some("length"))
    );
    assert_eq!(row["completion_tokens"], usage["completion_tokens"]);
    assert_eq!(row["completion_tokens"], 60);
    assert!(
        row["duration_ms"].as_u64().unwrap() >= row["ttft_ms"].as_u64().unwrap(),
        "{row}"
    );
    assert_eq!(v["recent"][0]["client_ref"], "ref-a", "newest first");

    // Never any text: not the prompt, not a generated piece.
    let text = client()
        .get(srv.url("/api/jobs"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !text.contains(canary) && !text.contains("once upon") && !text.contains("tok1"),
        "{text}"
    );
    srv.stop().await;
}

/// Embeddings are jobs too; a request without the header (or with an invalid one) has no ref.
#[tokio::test(flavor = "multi_thread")]
async fn embeddings_and_unreferenced_requests_are_listed_fake() {
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(app(
        ApiConfig::default(),
        dir.path().to_path_buf(),
        HardwareReport::default(),
    ))
    .await;
    let opts = common_runtime::manager::LoadOptions {
        embeddings: true,
        ..srv.state.load_options()
    };
    *srv.state.manager.loaded.lock().await = Some(slot(MODEL_ID, opts, FakeModel::new(1, Duration::ZERO)));
    let r = client()
        .post(srv.url("/v1/embeddings"))
        .header("x-common-client-ref", "not a valid ref")
        .json(&json!({ "model": MODEL_ID, "input": ["a b", "c"] }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v = jobs(&srv).await;
    let row = &v["recent"][0];
    assert_eq!(
        (row["kind"].as_str(), row["state"].as_str()),
        (Some("embeddings"), Some("done")),
        "{row}"
    );
    assert_eq!(row["prompt_tokens"], 3);
    assert!(
        row["client_ref"].is_null() && row["id"].as_str().unwrap().starts_with("embd-"),
        "{row}"
    );
    srv.stop().await;
}
