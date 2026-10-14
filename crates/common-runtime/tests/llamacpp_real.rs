//! Real inference on the pinned llama-server b11609 with the stories15M-q4_0 fixture. Nothing here
//! is mocked: every assertion is against a live engine process on 127.0.0.1.

mod common;

use common::*;
use common_core::ErrorCode;
use common_runtime::*;
use futures_util::StreamExt;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn cpu_load_golden_text_and_conformance() {
    let Some((backend, model)) = setup() else { return };
    let loaded = backend
        .load_llama(load_req(&model, DevicePreference::Cpu))
        .await
        .unwrap();
    let info = loaded.info();
    assert_eq!(info.device_actual, "cpu");
    assert_eq!(info.offloaded_layers, 0);
    assert_eq!(info.total_layers, 6);
    assert_eq!(info.n_ctx, 128);
    assert!(info.backend_version.contains("11609"), "{}", info.backend_version);
    assert!(info.fallback_reason.is_none());
    assert!(info.pid.is_some() && loaded.memory_rss_bytes().unwrap() > 1_000_000);

    let (text, ids, usage, timings, finish) =
        collect(loaded.generate(greedy(32), CancellationToken::new()).await.unwrap())
            .await
            .unwrap();
    assert_eq!(text, GOLDEN);
    assert_eq!(ids.len(), 32);
    assert_eq!(
        usage,
        Usage {
            prompt_tokens: 10,
            completion_tokens: 32
        }
    );
    assert_eq!(finish, "length");
    assert!(timings.decode_tokens_per_second.unwrap() > 0.0);
    assert!(timings.time_to_first_token_ms.unwrap() > 0.0);

    // F-06-06: no prompt or completion text in the engine log at -lv 4, and the per-process key is in
    // neither the log nor the process table (it went through a 0600 file that is gone once the engine is up).
    let key = loaded.api_key();
    assert_eq!(key.len(), 36);
    let log = std::fs::read_to_string(&loaded.log_path).unwrap();
    assert!(log.contains("offloaded 0/"), "the log is the real engine log");
    assert!(
        !log.contains(PROMPT) && !log.contains(GOLDEN),
        "prompt or completion text leaked into the engine log"
    );
    assert!(!log.contains(key), "api key leaked into the engine log");
    let argv = std::process::Command::new("ps")
        .args(["-o", "args=", "-p", &info.pid.unwrap().to_string()])
        .output()
        .unwrap();
    let argv = String::from_utf8_lossy(&argv.stdout);
    assert!(
        argv.contains("llama-server") && argv.contains("--api-key-file"),
        "{argv}"
    );
    assert!(!argv.contains(key), "api key visible in the process table: {argv}");
    let key_file = loaded.log_path.with_extension("key");
    assert!(!key_file.exists(), "key file must be deleted once the engine is up");
    loaded.unload().await.unwrap();
    assert!(!loaded.log_path.exists(), "a clean unload removes the engine log");

    let report = conformance::run(std::sync::Arc::new(backend), load_req(&model, DevicePreference::Cpu))
        .await
        .unwrap();
    assert!(report.ok(), "{:?}", report.failed);
    assert_eq!(report.passed.len(), 7);
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[tokio::test]
async fn metal_load_golden_text_and_conformance() {
    let Some((backend, model)) = setup() else { return };
    let loaded = backend
        .load_llama(load_req(&model, DevicePreference::Gpu))
        .await
        .unwrap();
    let info = loaded.info();
    assert!(info.device_actual.contains("Apple"), "{}", info.device_actual);
    assert!(info.offloaded_layers > 0);
    assert_eq!(info.total_layers, 6);
    assert!(info.fallback_reason.is_none());

    let (text, ids, usage, timings, _) = collect(loaded.generate(greedy(32), CancellationToken::new()).await.unwrap())
        .await
        .unwrap();
    assert_eq!(text, GOLDEN, "Metal greedy output must equal the CPU golden text");
    assert_eq!(ids.len(), 32);
    assert_eq!(usage.completion_tokens, 32);
    let tps = timings.decode_tokens_per_second.unwrap();
    assert!(tps > 0.0);
    eprintln!(
        "metal decode tok/s = {tps:.1}, ttft = {:.1} ms",
        timings.time_to_first_token_ms.unwrap()
    );
    loaded.unload().await.unwrap();

    let auto = backend
        .load_llama(load_req(&model, DevicePreference::Auto))
        .await
        .unwrap();
    assert!(auto.info().offloaded_layers > 0, "Auto picks the GPU on a Metal host");
    assert_eq!(auto.info().device_requested, "auto");
    auto.unload().await.unwrap();

    let report = conformance::run(std::sync::Arc::new(backend), load_req(&model, DevicePreference::Gpu))
        .await
        .unwrap();
    assert!(report.ok(), "{:?}", report.failed);
}

#[tokio::test]
async fn corrupt_file_is_rejected_before_any_spawn() {
    let Some((backend, _)) = setup() else { return };
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("junk.gguf");
    std::fs::write(&junk, [0x5a_u8; 1024]).unwrap();
    let logs_before = std::fs::read_dir(backend.paths.logs()).map(|d| d.count()).unwrap_or(0);
    let e = backend
        .load(load_req(&junk, DevicePreference::Cpu))
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::ModelFileInvalid, "{e}");
    let logs_after = std::fs::read_dir(backend.paths.logs()).map(|d| d.count()).unwrap_or(0);
    assert_eq!(logs_before, logs_after, "no engine log means no process was spawned");
    let e = backend
        .load(load_req(&dir.path().join("missing.gguf"), DevicePreference::Cpu))
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::Io);
}

/// A prompt longer than n_ctx is refused by the engine with HTTP 400 `exceed_context_size_error`
/// before any token is produced; the adapter maps it to ContextTooLong from `generate()` itself.
#[tokio::test]
async fn prompt_longer_than_context_is_context_too_long() {
    let Some((backend, model)) = setup() else { return };
    let loaded = backend.load(load_req(&model, DevicePreference::Cpu)).await.unwrap();
    let long = "word ".repeat(300);
    assert!(loaded.tokenize(&long).await.unwrap().len() > 128);
    let req = GenerateRequest {
        input: PromptInput::Raw(long),
        max_tokens: 8,
        ..Default::default()
    };
    let e = loaded
        .generate(req, CancellationToken::new())
        .await
        .err()
        .expect("typed error, not a stream");
    assert_eq!(e.code, ErrorCode::ContextTooLong, "{e}");
    assert!(e.message.contains("context"));
    let e = loaded.embed(&["x".into()]).await.err().unwrap();
    assert_eq!(
        e.code,
        ErrorCode::InvalidRequest,
        "embeddings need embeddings=true: {e}"
    );
    let e = loaded
        .generate(
            GenerateRequest {
                max_tokens: 0,
                ..greedy(1)
            },
            CancellationToken::new(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::InvalidRequest);
    loaded.unload().await.unwrap();
}

/// F-08-02: cancel after the first token; the stream ends within 200 ms and `/slots` shows the
/// engine idle within 500 ms (client disconnect is the cancel mechanism, see llamacpp.rs docs).
#[tokio::test]
async fn cancel_mid_generation_releases_the_engine_slot() {
    let Some((backend, model)) = setup() else { return };
    let loaded = backend
        .load_llama(load_req(&model, DevicePreference::Cpu))
        .await
        .unwrap();
    let cancel = CancellationToken::new();
    let mut s = loaded.generate(greedy(120), cancel.clone()).await.unwrap();
    assert!(matches!(s.next().await, Some(Ok(GenerateEvent::Token { .. }))));
    cancel.cancel();
    let t = Instant::now();
    let mut tail = vec![];
    while let Some(ev) = s.next().await {
        tail.push(ev);
    }
    let stream_end = t.elapsed();
    assert!(
        stream_end <= Duration::from_millis(200),
        "stream ended after {stream_end:?}"
    );
    assert!(
        matches!(tail.last(), Some(Err(e)) if e.code == ErrorCode::Cancelled),
        "{tail:?}"
    );
    let mut idle_after = None;
    while t.elapsed() < Duration::from_millis(500) {
        if loaded.slots_processing().await.unwrap() == 0 {
            idle_after = Some(t.elapsed());
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let idle_after = idle_after.expect("engine slot still processing 500 ms after cancel");
    eprintln!("cancel: stream ended after {stream_end:?}, engine idle after {idle_after:?}");
    loaded.unload().await.unwrap();
    assert!(!common_runtime::llamacpp::process::alive(loaded.info().pid.unwrap()));
}

#[tokio::test]
async fn chat_endpoint_streams_with_usage() {
    let Some((backend, model)) = setup() else { return };
    let loaded = backend.load(load_req(&model, DevicePreference::Cpu)).await.unwrap();
    // stories15M has no chat template; llama-server falls back to a default one, which is enough
    // to exercise the OpenAI delta parsing against a live engine.
    let req = GenerateRequest {
        input: PromptInput::Chat(vec![ChatMessage {
            role: "user".into(),
            content: "Tell me a story.".into(),
        }]),
        max_tokens: 8,
        temperature: 0.0,
        ..Default::default()
    };
    let (text, ids, usage, timings, finish) = collect(loaded.generate(req, CancellationToken::new()).await.unwrap())
        .await
        .unwrap();
    assert!(!text.is_empty());
    assert!(ids.is_empty(), "the OpenAI endpoint carries no token ids");
    assert_eq!(usage.completion_tokens, 8);
    assert_eq!(finish, "length");
    assert!(timings.decode_tokens_per_second.unwrap() > 0.0);
    loaded.unload().await.unwrap();
}
