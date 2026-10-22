//! API contract tests that need no inference engine: route inventory vs. the OpenAPI document,
//! secure bind defaults, error shape, auth, admission control, metrics and SSE framing. Where a
//! model is needed the FAKE `LoadedModel` double from `common/mod.rs` is seeded into the slot.

mod common;

use common::*;
use common_api::{bind_addr, serve, serve_on, ApiConfig, ROUTES, SHUTDOWN_GRACE};
use common_core::ErrorCode;
use common_hardware::HardwareReport;
use common_runtime::manager::LoadOptions;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

fn cfg() -> ApiConfig {
    ApiConfig::default()
}

/// Server over an empty temporary COMMON_HOME (catalog only, nothing downloaded, nothing loaded).
async fn empty_server(config: ApiConfig) -> (Server, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let srv = Server::start(app(config, dir.path().to_path_buf(), HardwareReport::default())).await;
    (srv, dir)
}

#[tokio::test(flavor = "multi_thread")]
async fn route_inventory_matches_openapi_document() {
    let (srv, _dir) = empty_server(cfg()).await;
    let resp = client().get(srv.url("/openapi.json")).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("application/json"));
    let doc: Value = resp.json().await.unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["info"]["version"], "1.0.0");
    let documented: BTreeSet<(String, String)> = doc["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, ops)| {
            ops.as_object()
                .unwrap()
                .keys()
                .map(move |m| (m.to_uppercase(), path.clone()))
        })
        .collect();
    let served: BTreeSet<(String, String)> = ROUTES.iter().map(|(m, p)| (m.to_string(), p.to_string())).collect();
    assert_eq!(
        served, documented,
        "every served route must be documented and vice versa"
    );
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn default_bind_is_loopback_and_public_bind_without_key_is_refused() {
    let addr = bind_addr(&cfg()).unwrap();
    assert!(addr.ip().is_loopback(), "{addr}");
    assert_eq!(addr.port(), 8192);
    let dir = tempfile::tempdir().unwrap();
    let state = app(
        ApiConfig {
            host: "0.0.0.0".into(),
            ..cfg()
        },
        dir.path().to_path_buf(),
        HardwareReport::default(),
    );
    let e = serve(state, std::future::pending()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PolicyDenied);
    assert!(e.message.contains("0.0.0.0") && e.message.contains("--api-key"), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn errors_have_openai_shape_and_stable_codes() {
    let (srv, _dir) = empty_server(cfg()).await;
    let c = client();
    let post = |body: Value| c.post(srv.url("/v1/chat/completions")).json(&body).send();

    let r = post(chat_body("no-such-model", 4, json!({}))).await.unwrap();
    assert_eq!(r.status(), 404);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "model_not_found");
    assert_eq!(v["error"]["type"], "not_found_error");
    assert!(v["error"]["param"].is_null() && v["error"]["message"].is_string());

    let r = post(chat_body(MODEL_ID, 4, json!({}))).await.unwrap();
    assert_eq!(r.status(), 404);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "model_not_downloaded");
    assert_eq!(v["error"]["hint"], format!("common models pull {MODEL_ID}"));

    let r = post(json!({"messages": [{"role": "user", "content": "x"}]}))
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "invalid_request");

    let r = post(json!({"model": "m", "messages": []})).await.unwrap();
    assert_eq!(r.status(), 400);

    let r = c
        .post(srv.url("/v1/chat/completions"))
        .header("content-type", "application/json")
        .body("{\"model\": ")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["error"]["code"], "invalid_request");
    assert!(v["error"]["message"].as_str().unwrap().contains("JSON"), "{v}");

    let r = c.post(srv.url("/v1/chat/completions")).body("{}").send().await.unwrap();
    assert_eq!(r.status(), 400, "missing content-type");

    // Over the 2 MiB body limit. The server rejects as soon as the limit is crossed and closes the
    // connection, so a client still writing may see a reset instead of the 413 (either proves the limit).
    let big = format!(
        "{{\"model\": \"m\", \"pad\": \"{}\"}}",
        "x".repeat(common_api::BODY_LIMIT + 64 * 1024)
    );
    match c
        .post(srv.url("/v1/chat/completions"))
        .header("content-type", "application/json")
        .body(big)
        .send()
        .await
    {
        Ok(r) => {
            assert_eq!(r.status(), 413);
            assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "invalid_request");
        }
        Err(e) => assert!(
            e.is_request() && !e.is_timeout(),
            "unexpected error for an oversized body: {e}"
        ),
    }

    let r = c.get(srv.url("/no/such/route")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "invalid_request");

    let r = c.get(srv.url("/v1/models/no-such-model")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "model_not_found");

    let r = c.get(srv.url("/v1/models")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["object"], "list");
    let data = v["data"].as_array().unwrap();
    assert!(data.len() >= 8, "{}", data.len());
    let fixture = data.iter().find(|m| m["id"] == MODEL_ID).expect("catalog entry listed");
    assert_eq!(fixture["object"], "model");
    assert_eq!(fixture["common"]["downloaded"], false);
    assert_eq!(fixture["common"]["loaded"], false);
    assert!(fixture["common"]["license"].is_string() && fixture["common"]["bytes"].as_u64().unwrap() > 0);

    let r = c.get(srv.url(&format!("/v1/models/{MODEL_ID}"))).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["common"]["downloaded"], false);
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_required_on_every_route_except_health() {
    let (srv, _dir) = empty_server(ApiConfig {
        api_key: Some("secret-key".into()),
        ..cfg()
    })
    .await;
    let c = client();
    assert_eq!(c.get(srv.url("/health")).send().await.unwrap().status(), 200);
    for path in [
        "/ready",
        "/metrics",
        "/openapi.json",
        "/v1/models",
        "/api/status",
        "/api/models",
        "/api/hardware",
        "/api/network",
        "/api/peers",
        "/api/pair/status",
        "/api/groups",
        "/api/privacy",
        "/api/audit",
    ] {
        let r = c.get(srv.url(path)).send().await.unwrap();
        assert_eq!(r.status(), 401, "{path}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["error"]["code"], "unauthorized", "{path}");
        assert_eq!(v["error"]["type"], "authentication_error");
        assert_eq!(
            c.get(srv.url(path)).bearer_auth("wrong").send().await.unwrap().status(),
            401,
            "{path}"
        );
    }
    let r = c
        .post(srv.url("/v1/chat/completions"))
        .json(&chat_body(MODEL_ID, 1, json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401, "auth runs before body parsing");
    assert_eq!(
        c.get(srv.url("/v1/models"))
            .bearer_auth("secret-key")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        c.get(srv.url("/api/status"))
            .bearer_auth("secret-key")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    srv.stop().await;
}

/// F-08-08 with the FAKE double: max_concurrent=1, max_queue=1, five concurrent requests.
#[tokio::test(flavor = "multi_thread")]
async fn admission_control_rejects_with_429_and_503() {
    let (srv, _dir) = empty_server(ApiConfig {
        max_concurrent: 1,
        max_queue: 1,
        queue_timeout_ms: 5_000,
        ..cfg()
    })
    .await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(3, Duration::from_millis(150))).await;
    let c = client();
    let body = json!({"model": MODEL_ID, "prompt": "hello there", "max_tokens": 3});
    let mut tasks = vec![];
    for _ in 0..5 {
        let (c, url, body) = (c.clone(), srv.url("/v1/completions"), body.clone());
        tasks.push(tokio::spawn(async move {
            let r = c.post(url).json(&body).send().await.unwrap();
            let status = r.status();
            let retry_after = r.headers().get("retry-after").map(|v| v.to_str().unwrap().to_string());
            (status, retry_after, r.json::<Value>().await.unwrap())
        }));
    }
    let mut ok = 0;
    let mut rejected = 0;
    for t in tasks {
        let (status, retry_after, v) = t.await.unwrap();
        match status.as_u16() {
            200 => {
                ok += 1;
                assert_eq!(v["choices"][0]["text"], "tok0 tok1 tok2 ");
                assert_eq!(v["choices"][0]["finish_reason"], "length");
            }
            429 | 503 => {
                rejected += 1;
                assert_eq!(v["error"]["code"], "overloaded", "{v}");
                assert_eq!(retry_after.as_deref(), Some("1"), "Retry-After on {status}");
            }
            other => panic!("unexpected status {other}: {v}"),
        }
    }
    assert_eq!(ok + rejected, 5);
    assert!(rejected >= 3 && ok >= 1, "ok={ok} rejected={rejected}");
    let text = c.get(srv.url("/metrics")).send().await.unwrap().text().await.unwrap();
    assert!(
        metric(&text, "common_requests_rejected_total{reason=\"queue_full\"}").unwrap() >= 3.0,
        "{text}"
    );
    assert_eq!(metric(&text, "common_inflight").unwrap(), 0.0);
    assert_eq!(metric(&text, "common_queue_depth").unwrap(), 0.0);
    srv.stop().await;

    // Queue wait longer than queue_timeout_ms: the waiter gets 503, the admitted request completes.
    let (srv, _dir) = empty_server(ApiConfig {
        max_concurrent: 1,
        max_queue: 4,
        queue_timeout_ms: 50,
        ..cfg()
    })
    .await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(3, Duration::from_millis(200))).await;
    let a = tokio::spawn({
        let (c, url, body) = (c.clone(), srv.url("/v1/completions"), body.clone());
        async move { c.post(url).json(&body).send().await.unwrap().status() }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    let b = c.post(srv.url("/v1/completions")).json(&body).send().await.unwrap();
    assert_eq!(b.status(), 503);
    assert_eq!(b.json::<Value>().await.unwrap()["error"]["code"], "overloaded");
    assert_eq!(a.await.unwrap(), 200);
    let text = c.get(srv.url("/metrics")).send().await.unwrap().text().await.unwrap();
    assert_eq!(
        metric(&text, "common_requests_rejected_total{reason=\"queue_timeout\"}").unwrap(),
        1.0
    );
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn metrics_are_prometheus_text_and_never_contain_prompt() {
    let (srv, _dir) = empty_server(cfg()).await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(5, Duration::from_millis(1))).await;
    let c = client();
    let secret = "ZEBRA_SECRET_PROMPT_7731";
    let r = c
        .post(srv.url("/v1/completions"))
        .json(&json!({"model": MODEL_ID, "prompt": format!("{secret} and more words"), "max_tokens": 5}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(
        v["usage"],
        json!({"prompt_tokens": 4, "completion_tokens": 5, "total_tokens": 9})
    );
    assert!(v["timings"].is_object());

    let r = c.get(srv.url("/metrics")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
    let text = r.text().await.unwrap();
    check_prometheus_text(&text);
    promtool_check(&text);
    assert!(
        !text.contains(secret) && !text.contains("more words"),
        "prompt text leaked into metrics"
    );
    assert_eq!(
        metric(&text, "common_requests_total{route=\"/v1/completions\",status=\"200\"}").unwrap(),
        1.0
    );
    assert_eq!(metric(&text, "common_tokens_generated_total").unwrap(), 5.0);
    assert_eq!(metric(&text, "common_prompt_tokens_total").unwrap(), 4.0);
    assert_eq!(metric(&text, "common_ttft_seconds_count").unwrap(), 1.0);
    assert_eq!(metric(&text, "common_decode_tokens_per_second").unwrap(), 100.0);
    assert_eq!(metric(&text, "common_inflight").unwrap(), 0.0);
    assert_eq!(metric(&text, "common_queue_depth").unwrap(), 0.0);
    for name in ["common_requests_rejected_total", "common_ttft_seconds_bucket"] {
        assert!(text.contains(name), "missing {name}");
    }
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sse_framing_with_fake_model() {
    let (srv, _dir) = empty_server(cfg()).await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(10, Duration::from_millis(1))).await;
    let body = json!({"model": MODEL_ID, "prompt": "hi", "max_tokens": 4, "stream": true, "stream_options": {"include_usage": true}});
    let r = client()
        .post(srv.url("/v1/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let data = sse_data(&r.text().await.unwrap());
    assert_eq!(data.last().map(String::as_str), Some("[DONE]"));
    let chunks: Vec<Value> = data[..data.len() - 1]
        .iter()
        .map(|d| serde_json::from_str(d).unwrap_or_else(|e| panic!("chunk {d:?} is not JSON: {e}")))
        .collect();
    let tokens: Vec<&Value> = chunks
        .iter()
        .filter(|c| c["choices"][0]["text"].as_str().is_some_and(|t| !t.is_empty()))
        .collect();
    assert_eq!(tokens.len(), 4);
    assert!(tokens.len() > 1, "more than one chunk before completion");
    assert!(chunks
        .iter()
        .all(|c| c["object"] == "text_completion" && c["id"].as_str().unwrap().starts_with("cmpl-")));
    let finish: Vec<&Value> = chunks
        .iter()
        .filter(|c| c["choices"][0]["finish_reason"].is_string())
        .collect();
    assert_eq!(finish.len(), 1);
    assert_eq!(finish[0]["choices"][0]["finish_reason"], "length");
    let usage = chunks.last().unwrap();
    assert_eq!(usage["choices"], json!([]));
    assert_eq!(usage["usage"]["completion_tokens"], 4);
    srv.stop().await;
}

/// F-09-02: dropping the response mid-stream cancels the runtime token and frees the slot.
#[tokio::test(flavor = "multi_thread")]
async fn client_disconnect_cancels_generation_with_fake_model() {
    let (srv, _dir) = empty_server(cfg()).await;
    let fake = FakeModel::new(1000, Duration::from_millis(20));
    seed_slot(&srv.state, MODEL_ID, fake.clone()).await;
    let body = json!({"model": MODEL_ID, "prompt": "hi", "max_tokens": 1000, "stream": true});
    let r = client()
        .post(srv.url("/v1/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let mut stream = r.bytes_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert!(std::str::from_utf8(&first).unwrap().starts_with("data: "));
    drop(stream);
    let t = Instant::now();
    let cancel = fake.last_cancel().expect("generate was called");
    while !cancel.is_cancelled() && t.elapsed() < Duration::from_millis(500) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        cancel.is_cancelled(),
        "runtime token not cancelled within 500 ms of the disconnect"
    );
    while srv.state.admission.inflight() != 0 && t.elapsed() < Duration::from_millis(500) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        srv.state.admission.inflight(),
        0,
        "permit not released after disconnect"
    );
    eprintln!("fake disconnect: cancelled after {:?}", t.elapsed());
    srv.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_ready_models_and_admin_validation() {
    let (srv, _dir) = empty_server(cfg()).await;
    let c = client();
    let v: Value = c.get(srv.url("/health")).send().await.unwrap().json().await.unwrap();
    assert_eq!(v, json!({"status": "ok", "version": common_core::VERSION}));

    let r = c.get(srv.url("/ready")).send().await.unwrap();
    assert_eq!(r.status(), 200, "nothing loaded = ready to accept a load");
    assert_eq!(
        r.json::<Value>().await.unwrap(),
        json!({"status": "ready", "model": null})
    );

    let v: Value = c
        .get(srv.url("/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["version"], common_core::VERSION);
    assert!(v["loaded"].is_null() && v["uptime_seconds"].is_number());
    assert_eq!(
        v["queue"],
        json!({"inflight": 0, "queued": 0, "max_concurrent": 1, "max_queue": 32})
    );

    let v: Value = c
        .get(srv.url("/api/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let models = v["models"].as_array().unwrap();
    assert!(models.len() >= 8);
    for m in models {
        assert_eq!(m["downloaded"], false);
        assert_eq!(m["estimate_basis"], "file_size");
        assert!(
            matches!(m["fit"]["verdict"].as_str(), Some("fits" | "too_large")),
            "{m}"
        );
        assert!(m["estimate"]["total_bytes"].as_u64().unwrap() > m["bytes"].as_u64().unwrap());
    }

    seed_slot(&srv.state, MODEL_ID, FakeModel::new(1, Duration::ZERO)).await;
    let v: Value = c.get(srv.url("/ready")).send().await.unwrap().json().await.unwrap();
    assert_eq!(v, json!({"status": "ready", "model": MODEL_ID}));
    let v: Value = c
        .get(srv.url("/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["loaded"]["model_id"], MODEL_ID);
    let v: Value = c
        .get(srv.url(&format!("/v1/models/{MODEL_ID}")))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["common"]["loaded"], true);

    // A generation-mode slot cannot serve embeddings: the manager would reload (404 here, nothing downloaded).
    let r = c
        .post(srv.url("/v1/embeddings"))
        .json(&json!({"model": MODEL_ID, "input": "a"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    let opts = LoadOptions {
        embeddings: true,
        ..srv.state.load_options()
    };
    *srv.state.manager.loaded.lock().await = Some(slot(MODEL_ID, opts, FakeModel::new(1, Duration::ZERO)));
    let r = c
        .post(srv.url("/v1/embeddings"))
        .json(&json!({"model": MODEL_ID, "input": ["a b", "c"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["object"], "list");
    assert_eq!(v["data"].as_array().unwrap().len(), 2);
    assert_eq!(v["data"][1]["index"], 1);
    let vec: Vec<f32> = v["data"][0]["embedding"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap() as f32)
        .collect();
    assert_eq!(vec, [0.1, 0.2, 0.3]);
    assert_eq!(v["usage"]["prompt_tokens"], 3);
    let r = c
        .post(srv.url("/v1/embeddings"))
        .json(&json!({"model": MODEL_ID, "input": "a", "encoding_format": "base64"}))
        .send()
        .await
        .unwrap();
    assert!(r.json::<Value>().await.unwrap()["data"][0]["embedding"].is_string());

    assert_eq!(c.post(srv.url("/api/unload")).send().await.unwrap().status(), 204);
    let v: Value = c
        .get(srv.url("/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(v["loaded"].is_null());

    let r = c
        .post(srv.url("/api/models/..%2Fetc/load"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "invalid_request");
    let r = c.delete(srv.url("/api/models/no-such-model")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = c.post(srv.url("/api/models/no-such-model/pull")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = c
        .post(srv.url("/api/models/import"))
        .json(&json!({"path": "relative/x.gguf"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = c
        .post(srv.url("/api/models/import"))
        .json(&json!({"path": "/no/such/file.gguf"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400);
    let r = c
        .post(srv.url("/api/models/no-such-model/load"))
        .json(&json!({"device": "cpu"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "model_not_found");
    srv.stop().await;
}

/// The engine-free tests above build the router through `serve_on`; `serve()` itself must reject a
/// loopback-less host before touching the network and otherwise bind the configured port.
#[tokio::test(flavor = "multi_thread")]
async fn serve_binds_configured_loopback_port() {
    let dir = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let state = app(
        ApiConfig { port, ..cfg() },
        dir.path().to_path_buf(),
        HardwareReport::default(),
    );
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(serve(state, async {
        let _ = rx.await;
    }));
    let c = client();
    let mut last = None;
    for _ in 0..50 {
        match c.get(format!("http://127.0.0.1:{port}/health")).send().await {
            Ok(r) => {
                last = Some(r.status());
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    assert_eq!(last, Some(StatusCode::OK));
    let _ = tx.send(());
    assert!(tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap()
        .is_ok());
}

/// F-06-06: CORS is enforced at request time. A web page the user visits cannot drive the daemon
/// with a "simple" cross-origin POST, a no-cors cross-site GET, or a DNS-rebound Host.
#[tokio::test(flavor = "multi_thread")]
async fn cross_origin_and_foreign_host_requests_are_refused() {
    let (srv, _dir) = empty_server(cfg()).await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(1, Duration::ZERO)).await;
    let c = client();
    let r = c
        .post(srv.url("/api/unload"))
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "policy_denied");
    assert!(
        srv.state.manager.status().await.is_some(),
        "the handler must not have run"
    );
    assert_eq!(
        c.post(srv.url("/api/unload"))
            .header("origin", "null")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        c.post(srv.url(&format!("/api/models/{MODEL_ID}/pull")))
            .header("origin", "https://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let r = c
        .get(srv.url("/api/status"))
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "no-cors cross-site GET");
    let r = c
        .request(reqwest::Method::OPTIONS, srv.url("/api/unload"))
        .header("origin", "https://evil.example")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    assert!(r.headers().get("access-control-allow-origin").is_none());
    let r = c
        .get(srv.url("/api/status"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403, "DNS rebinding: foreign Host without an API key");
    assert_eq!(
        c.get(srv.url("/api/status"))
            .header("host", format!("localhost:{}", srv.addr.port()))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );

    // The desktop webview's origin is allow-listed: preflight answered, request served, CORS headers set.
    let r = c
        .request(reqwest::Method::OPTIONS, srv.url("/api/unload"))
        .header("origin", "tauri://localhost")
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(r.headers()["access-control-allow-origin"], "tauri://localhost");
    // E2/E3: the app's own request id may ride on a chat (preflighted because it is a custom header).
    let r = c
        .request(reqwest::Method::OPTIONS, srv.url("/v1/chat/completions"))
        .header("origin", "tauri://localhost")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type, x-common-client-ref")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let allowed = r.headers()["access-control-allow-headers"]
        .to_str()
        .unwrap()
        .to_string();
    for h in ["content-type", "x-common-client-ref"] {
        assert!(allowed.split(", ").any(|a| a == h), "{h} not in {allowed}");
    }
    let r = c
        .post(srv.url("/api/unload"))
        .header("origin", "tauri://localhost")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    assert_eq!(r.headers()["access-control-allow-origin"], "tauri://localhost");
    assert!(srv.state.manager.status().await.is_none());
    assert_eq!(
        c.get(srv.url("/api/status")).send().await.unwrap().status(),
        200,
        "non-browser clients are unaffected"
    );
    srv.stop().await;

    // With an API key the key is the boundary: a LAN client sends its own IP as Host. The origin
    // check still holds (a page cannot read the key, but must not be able to send it either).
    let (srv, _dir) = empty_server(ApiConfig {
        api_key: Some("k".into()),
        ..cfg()
    })
    .await;
    assert_eq!(
        c.get(srv.url("/api/status"))
            .header("host", "192.168.1.5:8192")
            .bearer_auth("k")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        c.post(srv.url("/api/unload"))
            .header("origin", "https://evil.example")
            .bearer_auth("k")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    srv.stop().await;
}

/// The API import shares the CLI's trust boundary: a readable file that is not a GGUF is refused
/// and nothing is written to the store.
#[tokio::test(flavor = "multi_thread")]
async fn import_refuses_non_gguf_files() {
    let (srv, _dir) = empty_server(cfg()).await;
    let junk = tempfile::tempdir().unwrap();
    let path = junk.path().join("hosts.txt");
    std::fs::write(&path, b"127.0.0.1 localhost\n").unwrap();
    let r = client()
        .post(srv.url("/api/models/import"))
        .json(&json!({"path": path, "id": "hosts-test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 422);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "model_file_invalid");
    assert!(
        srv.state.store.list().unwrap().is_empty(),
        "nothing linked into the store"
    );
    srv.stop().await;
}

/// F-01-12 over HTTP: a grammar bomb in `response_format` is a 400 in well under a second and
/// never reaches the model.
#[tokio::test(flavor = "multi_thread")]
async fn schema_bombs_are_400_before_generation() {
    let (srv, _dir) = empty_server(cfg()).await;
    let fake = FakeModel::new(1, Duration::ZERO);
    seed_slot(&srv.state, MODEL_ID, fake.clone()).await;
    let mut deep = json!({"type": "string"});
    for _ in 0..64 {
        deep = json!({"type": "object", "properties": {"a": deep}});
    }
    for schema in [deep, json!({"type": "string", "pattern": "^(a{1,100000})$"})] {
        let body = chat_body(
            MODEL_ID,
            4,
            json!({"response_format": {"type": "json_schema", "json_schema": {"name": "bomb", "schema": schema}}}),
        );
        let t = Instant::now();
        let r = client()
            .post(srv.url("/v1/chat/completions"))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
        assert!(t.elapsed() < Duration::from_secs(1));
        assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "invalid_request");
    }
    assert!(fake.last_cancel().is_none(), "generate must not have been called");
    srv.stop().await;
}

/// Shutdown while a client streams: the generation is cancelled, the stream ends with a
/// `cancelled` error and `[DONE]`, the model is unloaded, and `serve_on` returns long before
/// `SHUTDOWN_GRACE` (previously it waited for the whole generation, up to max_tokens).
#[tokio::test(flavor = "multi_thread")]
async fn shutdown_cancels_in_flight_streams_and_returns() {
    let dir = tempfile::tempdir().unwrap();
    let state = app(cfg(), dir.path().to_path_buf(), HardwareReport::default());
    let fake = FakeModel::new(1000, Duration::from_millis(20));
    seed_slot(&state, MODEL_ID, fake.clone()).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(serve_on(state.clone(), listener, async {
        let _ = rx.await;
    }));
    let body = json!({"model": MODEL_ID, "prompt": "hi", "max_tokens": 1000, "stream": true});
    let r = client()
        .post(format!("http://{addr}/v1/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let mut stream = r.bytes_stream();
    let first = stream.next().await.unwrap().unwrap();
    assert!(std::str::from_utf8(&first).unwrap().starts_with("data: "));
    let t = Instant::now();
    tx.send(()).unwrap();
    let mut tail = Vec::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(b) => tail.extend_from_slice(&b),
            Err(e) => panic!("stream did not end cleanly: {e}"),
        }
    }
    let data = sse_data(std::str::from_utf8(&tail).unwrap());
    assert_eq!(data.last().map(String::as_str), Some("[DONE]"), "{data:?}");
    assert!(
        data.iter()
            .any(|d| serde_json::from_str::<Value>(d).is_ok_and(|v| v["error"]["code"] == "cancelled")),
        "{data:?}"
    );
    let r = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("serve_on returned within 2 s")
        .unwrap();
    assert!(r.is_ok(), "{r:?}");
    assert!(t.elapsed() < SHUTDOWN_GRACE, "{:?}", t.elapsed());
    assert!(fake.last_cancel().unwrap().is_cancelled());
    assert!(state.manager.status().await.is_none(), "model unloaded at shutdown");
    assert!(state.shutdown.is_cancelled());
    eprintln!("shutdown with a live stream: returned after {:?}", t.elapsed());
}

/// F-08-08 at 10x capacity with the FAKE double: 4 slots + 8 queue places, 40 concurrent
/// requests. Exactly 12 are served, 28 get 429 at once, and the process does not grow by more
/// than a couple of MiB per refused request.
#[tokio::test(flavor = "multi_thread")]
async fn overload_at_10x_capacity_is_bounded() {
    let (srv, _dir) = empty_server(ApiConfig {
        max_concurrent: 4,
        max_queue: 8,
        queue_timeout_ms: 20_000,
        ..cfg()
    })
    .await;
    seed_slot(&srv.state, MODEL_ID, FakeModel::new(5, Duration::from_millis(200))).await;
    let c = client();
    let body = json!({"model": MODEL_ID, "prompt": "hi", "max_tokens": 5});
    let rss_before = rss_kib();
    let t = Instant::now();
    let tasks: Vec<_> = (0..40)
        .map(|_| {
            let (c, url, body) = (c.clone(), srv.url("/v1/completions"), body.clone());
            tokio::spawn(async move { c.post(url).json(&body).send().await.unwrap().status().as_u16() })
        })
        .collect();
    let mut counts = std::collections::BTreeMap::new();
    for task in tasks {
        *counts.entry(task.await.unwrap()).or_insert(0) += 1;
    }
    assert_eq!(
        counts,
        [(200, 12), (429, 28)].into_iter().collect(),
        "4 running + 8 queued served, the rest refused"
    );
    let rss_after = rss_kib();
    let grew_mib = rss_after.saturating_sub(rss_before) / 1024;
    eprintln!(
        "10x overload: {:?} in {:?}, rss {} -> {} KiB (+{grew_mib} MiB)",
        counts,
        t.elapsed(),
        rss_before,
        rss_after
    );
    assert!(
        grew_mib < 56,
        "process grew {grew_mib} MiB during a 40-request burst (28 refused)"
    );
    assert_eq!(srv.state.admission.inflight(), 0);
    assert_eq!(srv.state.admission.queued(), 0);
    let text = c.get(srv.url("/metrics")).send().await.unwrap().text().await.unwrap();
    assert_eq!(
        metric(&text, "common_requests_rejected_total{reason=\"queue_full\"}").unwrap(),
        28.0
    );
    srv.stop().await;
}

/// E2 with the FAKE double in the slot: status and both model lists answer while something holds
/// the slot (a load holds it for the whole hash check and engine start; before E2 these requests
/// waited that long). The loaded model stays visible from the last moment the slot was free.
#[tokio::test(flavor = "multi_thread")]
async fn status_and_model_lists_answer_while_the_slot_is_held_fake() {
    let (srv, _dir) = empty_server(cfg()).await;
    let c = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let get = |path: &'static str| {
        let (c, url) = (c.clone(), srv.url(path));
        async move {
            let r = c.get(url).send().await.unwrap_or_else(|e| panic!("GET {path}: {e}"));
            assert_eq!(r.status(), 200, "{path}");
            r.json::<Value>().await.unwrap()
        }
    };
    let v = get("/api/status").await;
    assert_eq!(
        v["engine"],
        json!({"state": "idle", "loading_model_id": null, "loading_for_ms": null})
    );
    assert!(v["loaded"].is_null(), "{v}");

    seed_slot(&srv.state, MODEL_ID, FakeModel::new(1, Duration::ZERO)).await;
    let v = get("/api/status").await;
    assert_eq!(v["engine"]["state"], "ready");
    assert_eq!(v["loaded"]["model_id"], MODEL_ID);
    assert_eq!(v["loaded"]["loaded_for"], "local");
    assert_eq!(v["loaded"]["in_flight"], 0);
    assert!(
        v["loaded"]["loaded_at_utc"].is_string() && v["loaded"].get("context_note").is_some(),
        "{v}"
    );

    let held = srv.state.manager.loaded.lock().await;
    let t0 = Instant::now();
    let v = get("/api/status").await;
    assert_eq!(
        (v["engine"]["state"].as_str(), v["loaded"]["model_id"].as_str()),
        (Some("ready"), Some(MODEL_ID))
    );
    // (Not downloaded here, so its catalog row says loaded: false; the point is that it answers.)
    assert!(get("/api/models").await["models"].is_array());
    let list = get("/v1/models").await;
    let m = list["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == MODEL_ID)
        .unwrap();
    assert_eq!(m["common"]["loaded"], true);
    assert_eq!(get("/v1/models/stories15M-q4_0").await["common"]["loaded"], true);
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    drop(held);
    srv.stop().await;
}
