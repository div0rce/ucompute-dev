//! Real inference through the API on the pinned llama-server b11609 with the fixture models from
//! COMMON_HOME (default: the repo's `.common`). Nothing is mocked; every test skips loudly when the
//! engine or a fixture is missing. Each test owns its own server and engine process; they take
//! turns (`fixture_home_server`).

mod common;

use common::*;
use common_api::ApiConfig;
use common_core::{Paths, Timeouts};
use common_runtime::llamacpp::LlamaCppBackend;
use common_runtime::*;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

async fn fixture_server(config: ApiConfig) -> Option<Server> {
    Some(fixture_home_server(config, &fixture_home()?).await)
}

#[tokio::test(flavor = "multi_thread")]
async fn completions_golden_text_chat_fallback_and_metrics() {
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let c = client();
    let body = json!({"model": MODEL_ID, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "seed": 42});
    let r = c.post(srv.url("/v1/completions")).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["object"], "text_completion");
    assert!(v["id"].as_str().unwrap().starts_with("cmpl-"));
    assert_eq!(v["model"], MODEL_ID);
    assert_eq!(v["choices"][0]["text"], GOLDEN);
    assert_eq!(v["choices"][0]["finish_reason"], "length");
    assert_eq!(
        v["usage"],
        json!({"prompt_tokens": 10, "completion_tokens": 32, "total_tokens": 42})
    );
    assert!(v["timings"]["decode_tokens_per_second"].as_f64().unwrap() > 0.0);
    assert!(v["timings"]["time_to_first_token_ms"].as_f64().unwrap() > 0.0);

    // Array-form prompt and the same deterministic result.
    let r = c
        .post(srv.url("/v1/completions"))
        .json(&json!({"model": MODEL_ID, "prompt": [PROMPT], "max_tokens": 32, "temperature": 0, "seed": 42}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.json::<Value>().await.unwrap()["choices"][0]["text"], GOLDEN);

    // stories15M has no chat template: the API prompts with `role: content` lines and raw generation.
    let r = c
        .post(srv.url("/v1/chat/completions"))
        .json(&chat_body(MODEL_ID, 8, json!({})))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["object"], "chat.completion");
    assert!(v["id"].as_str().unwrap().starts_with("chatcmpl-"));
    assert_eq!(v["choices"][0]["message"]["role"], "assistant");
    assert!(!v["choices"][0]["message"]["content"].as_str().unwrap().is_empty());
    assert_eq!(v["choices"][0]["finish_reason"], "length");
    assert_eq!(v["usage"]["completion_tokens"], 8);

    let v: Value = c
        .get(srv.url("/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(v["loaded"]["model_id"], MODEL_ID);
    assert!(v["loaded"]["backend_version"].as_str().unwrap().contains("11609"));
    assert_eq!(v["loaded"]["n_ctx"], 128);
    let r = c.get(srv.url("/ready")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap()["model"], MODEL_ID);
    let v: Value = c.get(srv.url("/v1/models")).send().await.unwrap().json().await.unwrap();
    let m = v["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == MODEL_ID)
        .unwrap();
    assert_eq!(m["common"]["downloaded"], true);
    assert_eq!(m["common"]["loaded"], true);
    let v: Value = c
        .get(srv.url("/api/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m = v["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == MODEL_ID)
        .unwrap();
    assert_eq!(m["estimate_basis"], "gguf");
    assert_eq!(m["fit"]["verdict"], "fits");
    assert_eq!(m["estimate"]["n_ctx"], 128);

    let text = c.get(srv.url("/metrics")).send().await.unwrap().text().await.unwrap();
    check_prometheus_text(&text);
    assert!(
        !text.contains("rabbit") && !text.contains("Benny") && !text.contains("story"),
        "prompt/completion text leaked into metrics"
    );
    assert_eq!(metric(&text, "common_tokens_generated_total").unwrap(), 72.0);
    assert!(metric(&text, "common_prompt_tokens_total").unwrap() >= 20.0);
    assert_eq!(metric(&text, "common_ttft_seconds_count").unwrap(), 3.0);
    assert!(metric(&text, "common_decode_tokens_per_second").unwrap() > 0.0);
    assert_eq!(
        metric(&text, "common_requests_total{route=\"/v1/completions\",status=\"200\"}").unwrap(),
        2.0
    );
    srv.stop().await;
}

/// F-09-02: OpenAI chat chunk framing from a live engine.
#[tokio::test(flavor = "multi_thread")]
async fn sse_stream_framing_on_fixture() {
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let body = chat_body(
        MODEL_ID,
        8,
        json!({"stream": true, "stream_options": {"include_usage": true}}),
    );
    let r = client()
        .post(srv.url("/v1/chat/completions"))
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
    let id = chunks[0]["id"].as_str().unwrap();
    assert!(id.starts_with("chatcmpl-"), "{id}");
    assert!(chunks
        .iter()
        .all(|c| c["object"] == "chat.completion.chunk" && c["id"] == id && c["model"] == MODEL_ID));
    let content: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert!(!content.is_empty());
    let token_chunks = chunks
        .iter()
        .filter(|c| {
            c["choices"][0]["delta"]["content"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        })
        .count();
    assert!(token_chunks > 1, "{token_chunks} chunks before completion");
    let finish: Vec<&Value> = chunks
        .iter()
        .filter(|c| c["choices"][0]["finish_reason"].is_string())
        .collect();
    assert_eq!(finish.len(), 1);
    assert_eq!(finish[0]["choices"][0]["finish_reason"], "length");
    let usage = chunks.last().unwrap();
    assert_eq!(usage["choices"], json!([]));
    assert_eq!(usage["usage"]["completion_tokens"], 8);
    assert!(usage["usage"]["prompt_tokens"].as_u64().unwrap() > 0);
    srv.stop().await;
}

/// F-08-08 against the real engine: five concurrent chat requests, one slot, one queue place.
#[tokio::test(flavor = "multi_thread")]
async fn admission_control_on_fixture() {
    let Some(srv) = fixture_server(ApiConfig {
        max_concurrent: 1,
        max_queue: 1,
        queue_timeout_ms: 10_000,
        ..ApiConfig::default()
    })
    .await
    else {
        return;
    };
    let c = client();
    let r = c
        .post(srv.url(&format!("/api/models/{MODEL_ID}/load")))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let info: Value = r.json().await.unwrap();
    assert_eq!(info["model_id"], MODEL_ID);
    assert!(info["device_actual"].is_string() && info["pid"].is_number(), "{info}");

    let body = chat_body(MODEL_ID, 100, json!({}));
    let mut tasks = vec![];
    for _ in 0..5 {
        let (c, url, body) = (c.clone(), srv.url("/v1/chat/completions"), body.clone());
        tasks.push(tokio::spawn(async move {
            let r = c.post(url).json(&body).send().await.unwrap();
            (
                r.status().as_u16(),
                r.headers().contains_key("retry-after"),
                r.json::<Value>().await.unwrap(),
            )
        }));
    }
    let (mut ok, mut rejected) = (0, 0);
    for t in tasks {
        let (status, retry_after, v) = t.await.unwrap();
        match status {
            200 => {
                ok += 1;
                assert!(!v["choices"][0]["message"]["content"].as_str().unwrap().is_empty());
            }
            429 | 503 => {
                rejected += 1;
                assert!(retry_after);
                assert_eq!(v["error"]["code"], "overloaded");
            }
            other => panic!("unexpected status {other}: {v}"),
        }
    }
    assert_eq!(ok + rejected, 5);
    assert!(rejected >= 3 && ok >= 1, "ok={ok} rejected={rejected}");
    let text = c.get(srv.url("/metrics")).send().await.unwrap().text().await.unwrap();
    assert!(metric(&text, "common_requests_rejected_total{reason=\"queue_full\"}").unwrap() >= 3.0);
    assert_eq!(metric(&text, "common_inflight").unwrap(), 0.0);
    srv.stop().await;
}

/// F-09-02 / F-08-02: a client that disconnects mid-stream frees the engine slot within 500 ms.
/// The engine is loaded directly (n_ctx 2048, max_tokens 1500 = seconds of work) and seeded into
/// the manager so `/slots` can be read through `LoadedLlama::slots_processing`.
#[tokio::test(flavor = "multi_thread")]
async fn client_disconnect_cancels_engine_on_fixture() {
    let Some(home) = fixture_home() else { return };
    let paths = Paths::at(&home);
    let backend = LlamaCppBackend::new(engine_dir(&paths), paths.clone());
    let (_, blob) = common_artifacts::Store::new(paths.clone()).resolve(MODEL_ID).unwrap();
    let req = LoadRequest {
        model_id: MODEL_ID.into(),
        model_path: blob,
        model_sha256: MODEL_SHA.into(),
        device: DevicePreference::Cpu,
        n_ctx: 2048,
        n_gpu_layers: None,
        threads: Some(4),
        parallel_slots: 1,
        kv_type: common_models::KvType::F16,
        embeddings: false,
        timeouts: Timeouts {
            load: Duration::from_secs(60),
            ..Timeouts::default()
        },
        extra_args: vec![],
        split: None,
    };
    let srv = fixture_home_server(ApiConfig::default(), &home).await;
    let loaded = backend.load_llama(req).await.unwrap();
    seed_slot(&srv.state, MODEL_ID, loaded.clone()).await;

    let body = json!({"model": MODEL_ID, "prompt": PROMPT, "max_tokens": 1500, "stream": true});
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
    assert_eq!(
        loaded.slots_processing().await.unwrap(),
        1,
        "engine is busy before the disconnect"
    );
    drop(stream);
    let t = Instant::now();
    let mut idle_after = None;
    while t.elapsed() < Duration::from_millis(500) {
        if loaded.slots_processing().await.unwrap() == 0 {
            idle_after = Some(t.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let idle_after = idle_after.expect("engine slot still processing 500 ms after the client disconnected");
    while srv.state.admission.inflight() != 0 && t.elapsed() < Duration::from_millis(500) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(srv.state.admission.inflight(), 0, "admission permit not released");
    eprintln!("disconnect: engine idle after {idle_after:?}");
    // The slot is free: a follow-up request is served immediately.
    let r = client()
        .post(srv.url("/v1/completions"))
        .json(&json!({"model": MODEL_ID, "prompt": PROMPT, "max_tokens": 1, "temperature": 0}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    srv.stop().await;
}

/// F-09-01 / F-01-12 on a model with a chat template: the engine applies qwen2.5's template and
/// `response_format: json_schema` constrains the output.
#[tokio::test(flavor = "multi_thread")]
async fn qwen_chat_template_and_json_schema() {
    let Some(home) = fixture_home() else { return };
    if !has_model(&home, CHAT_MODEL_ID) {
        eprintln!("SKIP: {CHAT_MODEL_ID} not downloaded under {}", home.display());
        return;
    }
    let srv = fixture_home_server(ApiConfig::default(), &home).await;
    let c = client();
    let body = json!({
        "model": CHAT_MODEL_ID,
        "messages": [{"role": "system", "content": "Answer with one word."}, {"role": "user", "content": "What is the capital of France?"}],
        "max_tokens": 8, "temperature": 0, "seed": 42
    });
    let r = c
        .post(srv.url("/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status();
    let v: Value = r.json().await.unwrap();
    if status == 422 && v["error"]["code"] == "insufficient_memory" {
        eprintln!(
            "SKIP: {CHAT_MODEL_ID} does not fit in the memory available right now: {}",
            v["error"]["message"]
        );
        srv.stop().await;
        return;
    }
    assert_eq!(status, 200, "{v}");
    let content = v["choices"][0]["message"]["content"].as_str().unwrap().to_string();
    eprintln!("qwen chat answer: {content:?}");
    assert!(content.to_lowercase().contains("paris"), "{content:?}");
    assert!(matches!(
        v["choices"][0]["finish_reason"].as_str(),
        Some("stop" | "length")
    ));

    let body = json!({
        "model": CHAT_MODEL_ID,
        "messages": [{"role": "user", "content": "Give the capital of France as JSON."}],
        "max_tokens": 32, "temperature": 0,
        "response_format": {"type": "json_schema", "json_schema": {"name": "capital", "schema": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"], "additionalProperties": false}}}
    });
    let r = c
        .post(srv.url("/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let content = v["choices"][0]["message"]["content"].as_str().unwrap();
    eprintln!("qwen json_schema answer: {content:?}");
    let parsed: Value =
        serde_json::from_str(content).unwrap_or_else(|e| panic!("constrained output is not JSON: {e}: {content:?}"));
    assert!(parsed["city"].is_string(), "{parsed}");

    let body = json!({"model": CHAT_MODEL_ID, "messages": [{"role": "user", "content": "Reply with a JSON object with one key \"ok\" whose value is true."}], "max_tokens": 64, "temperature": 0, "response_format": {"type": "json_object"}});
    let r = c
        .post(srv.url("/v1/chat/completions"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let content = v["choices"][0]["message"]["content"].as_str().unwrap();
    eprintln!(
        "qwen json_object answer: {content:?} ({})",
        v["choices"][0]["finish_reason"]
    );
    assert!(
        content.trim_start().starts_with('{'),
        "json_object grammar not applied: {content:?}"
    );
    if v["choices"][0]["finish_reason"] == "stop" {
        assert!(
            serde_json::from_str::<Value>(content).is_ok_and(|j| j.is_object()),
            "json_object output: {content:?}"
        );
    }
    srv.stop().await;
}

fn python() -> Option<PathBuf> {
    let venv = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.venv/bin/python");
    let candidates = [venv.clone(), PathBuf::from("python3")];
    for py in candidates {
        let ok = std::process::Command::new(&py)
            .args(["-I", "-c", "import openai"])
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            return Some(py);
        }
    }
    eprintln!("SKIP: no python with the openai package; create it with `uv venv .venv && uv pip install --python .venv/bin/python openai` ({} missing)", venv.display());
    None
}

/// F-09-01: the official OpenAI Python client against the API with only base_url changed.
#[tokio::test(flavor = "multi_thread")]
async fn openai_python_client_smoke() {
    let Some(py) = python() else { return };
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/openai_client_smoke.py");
    let out = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new(&py)
            .arg("-I")
            .arg(&script)
            .arg(srv.url("/v1"))
            .arg(MODEL_ID)
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .output(),
    )
    .await
    .expect("python client finished within 180 s")
    .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    eprintln!("--- openai client stdout ---\n{stdout}--- stderr ---\n{stderr}");
    assert!(out.status.success(), "openai client smoke failed ({})", out.status);
    assert!(stdout.contains("ALL CHECKS PASSED"));
    srv.stop().await;
}

/// F-01-12 on the real engine: 100 constrained generations with varied prompts, every output
/// must satisfy the schema (object with a bounded string `city` and nothing else). The fixture
/// model is tiny, so the content is nonsense; the grammar, not the model, is under test.
#[tokio::test(flavor = "multi_thread")]
async fn json_schema_holds_100_of_100_on_fixture() {
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let c = client();
    let schema = json!({"type": "object", "properties": {"city": {"type": "string", "maxLength": 16}}, "required": ["city"], "additionalProperties": false});
    // The fixture's context is 128 tokens and the JSON grammar allows whitespace runs between
    // tokens, so the budget is the whole context: prompt (<= 8 tokens) + 110 generated.
    let t = Instant::now();
    for i in 0..100 {
        let prompt = format!("{} {i}:", ["Story", "Tale", "Once", "Rabbit", "City"][i % 5]);
        let body = json!({"model": MODEL_ID, "prompt": prompt, "max_tokens": 110, "temperature": 0.7, "seed": i, "response_format": {"type": "json_schema", "json_schema": {"name": "capital", "schema": schema}}});
        let r = c.post(srv.url("/v1/completions")).json(&body).send().await.unwrap();
        assert_eq!(r.status(), 200);
        let v: Value = r.json().await.unwrap();
        let text = v["choices"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap_or_else(|e| {
            panic!(
                "iteration {i}: not JSON ({e}): {text:?} finish={}",
                v["choices"][0]["finish_reason"]
            )
        });
        let obj = parsed
            .as_object()
            .unwrap_or_else(|| panic!("iteration {i}: not an object: {text:?}"));
        assert_eq!(obj.len(), 1, "iteration {i}: extra keys in {text:?}");
        assert!(
            obj["city"].as_str().is_some_and(|s| s.chars().count() <= 16),
            "iteration {i}: {text:?}"
        );
        assert_eq!(v["choices"][0]["finish_reason"], "stop", "iteration {i}: {text:?}");
    }
    eprintln!("json_schema: 100/100 valid in {:?}", t.elapsed());
    let text = c.get(srv.url("/metrics")).send().await.unwrap().text().await.unwrap();
    check_prometheus_text(&text);
    promtool_check(&text);
    srv.stop().await;
}

fn node_openai() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/clients/openai-js");
    let have_node = std::process::Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if have_node && dir.join("node_modules/openai/package.json").is_file() {
        return Some(dir);
    }
    eprintln!(
        "SKIP: node with the openai package not found; run `npm install` in {}",
        dir.display()
    );
    None
}

/// F-09-01: the official OpenAI JS client against the API with only baseURL changed.
#[tokio::test(flavor = "multi_thread")]
async fn openai_js_client_smoke() {
    let Some(dir) = node_openai() else { return };
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/openai_client_smoke.mjs");
    let out = tokio::time::timeout(
        Duration::from_secs(180),
        tokio::process::Command::new("node")
            .arg(&script)
            .arg(srv.url("/v1"))
            .arg(MODEL_ID)
            .arg(&dir)
            .env("NO_PROXY", "127.0.0.1")
            .env("no_proxy", "127.0.0.1")
            .output(),
    )
    .await
    .expect("node client finished within 180 s")
    .unwrap();
    let (stdout, stderr) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    eprintln!("--- openai js client stdout ---\n{stdout}--- stderr ---\n{stderr}");
    assert!(out.status.success(), "openai js client smoke failed ({})", out.status);
    assert!(stdout.contains("ALL CHECKS PASSED"));
    srv.stop().await;
}

/// The fixture servers keep their own settings: a fixture home whose `config.json` says networking
/// is on (an acceptance daemon left sentinel's so) with the peer port taken must not delay HTTP.
/// Sentinel, before: every real-engine server waited out the 2 s rebind wait on the fixed peer port
/// before serving, and the load-in-progress test's first `/api/status` took 2.01 s. No engine needed.
#[tokio::test(flavor = "multi_thread")]
async fn fixture_servers_ignore_the_home_settings_and_answer_at_once() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("config.json"), r#"{"network_enabled": true}"#).unwrap();
    let held = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let config = ApiConfig {
        network: common_network::NetworkConfig {
            listen_port: held.local_addr().unwrap().port(),
            bind_addr: "127.0.0.1".into(),
            mdns: false,
            ..Default::default()
        },
        ..ApiConfig::default()
    };
    let srv = fixture_home_server(config, home.path()).await;
    let t = Instant::now();
    let c = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let status = c.get(srv.url("/api/status")).send().await.expect("status within 1 s");
    assert_eq!(status.status(), 200);
    let net: Value = c
        .get(srv.url("/api/network"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(net["enabled"], false, "{net}");
    assert!(t.elapsed() < Duration::from_secs(1), "{:?}", t.elapsed());
    srv.stop().await;
}

/// E2 on the real engine: `/api/status` answers within its 1 s budget throughout a real load
/// (hash check, engine start) and shows it as `loading` with the model id, then `ready`, loaded
/// for this computer.
#[tokio::test(flavor = "multi_thread")]
async fn engine_state_shows_a_real_load_in_progress_on_fixture() {
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let c = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(1))
        .build()
        .unwrap();
    let load = tokio::spawn({
        let url = srv.url(&format!("/api/models/{MODEL_ID}/load"));
        async move { client().post(url).json(&json!({})).send().await.unwrap().status() }
    });
    let mut loading = None;
    while !load.is_finished() {
        let v: Value = c
            .get(srv.url("/api/status"))
            .send()
            .await
            .expect("status within 1 s")
            .json()
            .await
            .unwrap();
        if v["engine"]["state"] == "loading" {
            loading = Some(v);
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(load.await.unwrap(), 200);
    let loading = loading.expect("a status poll during the load saw it");
    assert_eq!(loading["engine"]["loading_model_id"], MODEL_ID, "{loading}");
    assert!(loading["engine"]["loading_for_ms"].is_u64(), "{loading}");
    let v: Value = c
        .get(srv.url("/api/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        v["engine"],
        json!({"state": "ready", "loading_model_id": null, "loading_for_ms": null, "last_exit": null})
    );
    assert_eq!(
        (v["loaded"]["model_id"].as_str(), v["loaded"]["loaded_for"].as_str()),
        (Some(MODEL_ID), Some("local"))
    );
    assert_eq!(v["loaded"]["in_flight"], 0);
    assert!(
        v["loaded"]["loaded_at_utc"].is_string() && !v["loaded"]["device_actual"].as_str().unwrap().is_empty(),
        "{v}"
    );
    srv.stop().await;
}

/// E3 on the real engine: a streaming chat is one active row (running, this computer) whose
/// `completion_tokens` rises while it streams; once done it is in `recent` with the engine's
/// usage, timings and device, and the registry never holds the prompt or the reply.
#[tokio::test(flavor = "multi_thread")]
async fn jobs_list_a_real_streaming_chat_with_live_tokens_on_fixture() {
    let Some(srv) = fixture_server(ApiConfig::default()).await else {
        return;
    };
    let r = client()
        .post(srv.url(&format!("/api/models/{MODEL_ID}/load")))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let canary = "ZEBRA_CANARY_REAL_9182";
    let poller = tokio::spawn({
        let url = srv.url("/api/jobs");
        async move {
            let mut seen: Vec<(String, String, u64)> = vec![];
            let t0 = Instant::now();
            loop {
                let v: Value = client().get(&url).send().await.unwrap().json().await.unwrap();
                let mine = |list: &str| {
                    v[list]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|j| j["client_ref"] == "real-ref")
                        .cloned()
                };
                if let Some(j) = mine("active") {
                    let n = j["completion_tokens"].as_u64().unwrap_or(0);
                    seen.push((
                        j["state"].as_str().unwrap().into(),
                        j["target"].as_str().unwrap().into(),
                        n,
                    ));
                }
                if let Some(done) = mine("recent") {
                    return (seen, done, v.to_string());
                }
                assert!(
                    t0.elapsed() < Duration::from_secs(30),
                    "the chat never finished: {seen:?}"
                );
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        }
    });
    let body = json!({"model": MODEL_ID, "messages": [{"role": "user", "content": format!("{canary} Tell me a story.")}], "max_tokens": 100, "temperature": 0, "stream": true, "stream_options": {"include_usage": true}});
    let r = client()
        .post(srv.url("/v1/chat/completions"))
        .header("x-common-client-ref", "real-ref")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let chunks: Vec<Value> = sse_data(&r.text().await.unwrap())
        .iter()
        .filter_map(|d| serde_json::from_str(d).ok())
        .collect();
    let usage = chunks
        .iter()
        .find_map(|v| v.get("usage").cloned())
        .expect("usage chunk");
    let reply: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    let (seen, done, jobs_text) = poller.await.unwrap();

    let running: Vec<u64> = seen
        .iter()
        .filter(|(state, target, _)| state == "running" && target == "local")
        .map(|(_, _, n)| *n)
        .collect();
    assert!(
        running.windows(2).all(|w| w[0] <= w[1]),
        "the live count never goes back: {seen:?}"
    );
    let distinct: std::collections::BTreeSet<u64> = running.iter().copied().collect();
    assert!(
        distinct.len() >= 2,
        "completion_tokens rose across polls while running: {seen:?}"
    );
    eprintln!("live completion_tokens seen while streaming: {distinct:?}");

    assert_eq!(
        (done["state"].as_str(), done["kind"].as_str()),
        (Some("done"), Some("chat")),
        "{done}"
    );
    assert_eq!(done["completion_tokens"], usage["completion_tokens"], "{done}");
    assert_eq!(done["prompt_tokens"], usage["prompt_tokens"], "{done}");
    assert_eq!(done["id"], chunks[0]["id"], "the job id is the response id");
    assert!(
        done["ttft_ms"].is_u64() && done["decode_tokens_per_second"].as_f64().unwrap() > 0.0,
        "{done}"
    );
    assert!(!done["device_actual"].as_str().unwrap().is_empty(), "{done}");
    assert!(reply.len() > 20, "{reply}");
    assert!(
        !jobs_text.contains(canary) && !jobs_text.contains("Tell me a story") && !jobs_text.contains(&reply[..20]),
        "{jobs_text}"
    );
    srv.stop().await;
}
