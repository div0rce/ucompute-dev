//! Real inference on the pinned llama-server b11609 with the stories15M-q4_0 fixture. Nothing here
//! is mocked: every assertion is against a live engine process on 127.0.0.1.

mod support;

use futures_util::StreamExt;
use std::time::{Duration, Instant};
use support::*;
use tokio_util::sync::CancellationToken;
use ucompute_core::ErrorCode;
use ucompute_runtime::*;

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
    assert!(!ucompute_runtime::llamacpp::process::alive(loaded.info().pid.unwrap()));
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

/// A model manager over UCOMPUTE_HOME's store and the real backend, with a probe that reports
/// `physical_cores`, CPU only, and 64 GiB available (a deliberately stale figure).
fn manager(backend: ucompute_runtime::llamacpp::LlamaCppBackend, physical_cores: u32) -> manager::ModelManager {
    let paths = ucompute_core::Paths::at(ucompute_home());
    let hardware = ucompute_hardware::HardwareReport {
        cpu: ucompute_hardware::CpuInfo {
            physical_cores,
            ..Default::default()
        },
        memory: ucompute_hardware::MemoryInfo {
            available_bytes: 64 << 30,
            ..Default::default()
        },
        backends: vec![ucompute_hardware::BackendInfo {
            name: "cpu".into(),
            available: true,
            reason: None,
        }],
        ..Default::default()
    };
    manager::ModelManager::new(
        paths.clone(),
        ucompute_artifacts::Store::new(paths),
        std::sync::Arc::new(backend),
        hardware,
    )
}

fn cpu_opts() -> manager::LoadOptions {
    manager::LoadOptions {
        device: Some(DevicePreference::Cpu),
        n_ctx: Some(128),
        ..Default::default()
    }
}

/// SIGKILL (TerminateProcess on Windows) from outside, as jetsam or the OOM killer would.
fn kill_hard(pid: u32) {
    let ok = if cfg!(windows) {
        std::process::Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .status()
    } else {
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
    };
    assert!(ok.unwrap().success(), "could not kill engine {pid}");
}

async fn golden_from(model: &dyn LoadedModel) {
    let (text, ..) = collect(model.generate(greedy(32), CancellationToken::new()).await.unwrap())
        .await
        .unwrap();
    assert_eq!(text, GOLDEN);
}

/// F-08-04, SINGLE-HOST, REAL engine: an engine killed mid-stream ends that stream with a typed
/// error naming the exit; one killed while idle is noticed without a request. Either way the slot
/// is freed, nothing is left running, and the next request starts a fresh engine that produces
/// the golden text. A third exit within the window hits the restart bound. Also checks the
/// default engine threads (the probe's physical cores, here a probe that says 3).
#[tokio::test]
async fn killed_engine_is_reported_freed_and_reloaded_single_host() {
    let Some((backend, _)) = setup() else { return };
    let m = manager(backend, 3);

    let model = m.ensure_loaded(MODEL_ID, cpu_opts()).await.unwrap();
    assert_eq!(model.info().threads, 3, "threads default to the probe's physical cores");
    let pid1 = model.info().pid.unwrap();
    #[cfg(unix)]
    {
        let argv = std::process::Command::new("ps")
            .args(["-o", "args=", "-p", &pid1.to_string()])
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&argv.stdout).contains(" -t 3 "), "engine argv");
    }
    // stories15M decodes ~2,600 tok/s on the M2's CPU: freeze the engine the moment the stream is
    // open so the kill lands mid-stream however fast it is (SIGSTOP; not available on Windows,
    // where only the idle kills below run).
    #[cfg(unix)]
    {
        let mut s = model.generate(greedy(120), CancellationToken::new()).await.unwrap();
        // SAFETY: plain syscall on the engine pid this test owns.
        assert_eq!(unsafe { libc::kill(pid1 as libc::pid_t, libc::SIGSTOP) }, 0);
        kill_hard(pid1);
        let mut events = vec![];
        while let Some(ev) = s.next().await {
            events.push(ev);
        }
        let e = match events.last() {
            Some(Err(e)) => e.clone(),
            other => panic!("the stream must end in an error, got {other:?}"),
        };
        assert_eq!(e.code, ErrorCode::BackendFailed, "{e}");
        assert!(e.message.contains("stopped unexpectedly"), "{e}");
        assert!(e.hint.as_deref().is_some_and(|h| h.contains("retry")), "{e:?}");
        eprintln!("killed mid-stream after {} events: {e}", events.len() - 1);
    }
    #[cfg(not(unix))]
    kill_hard(pid1);
    drop(model);
    let t = Instant::now();
    while m.snapshot().loaded.is_some() {
        assert!(t.elapsed() < Duration::from_secs(5), "engine death not noticed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let v = m.snapshot();
    assert!(v.loaded.is_none(), "slot freed");
    assert_eq!(v.last_exit.as_ref().and_then(|x| x.pid), Some(pid1));
    assert!(
        !ucompute_runtime::llamacpp::process::alive(pid1),
        "the dead engine was reaped"
    );

    let model = m.ensure_loaded(MODEL_ID, cpu_opts()).await.unwrap();
    let pid2 = model.info().pid.unwrap();
    assert_ne!(pid2, pid1);
    golden_from(model.as_ref()).await;
    drop(model);

    kill_hard(pid2);
    let t = Instant::now();
    while m.snapshot().loaded.is_some() {
        assert!(t.elapsed() < Duration::from_secs(5), "idle engine death not noticed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!ucompute_runtime::llamacpp::process::alive(pid2));
    let model = m.ensure_loaded(MODEL_ID, cpu_opts()).await.unwrap();
    let pid3 = model.info().pid.unwrap();
    golden_from(model.as_ref()).await;
    drop(model);

    kill_hard(pid3);
    let t = Instant::now();
    while m.snapshot().loaded.is_some() {
        assert!(t.elapsed() < Duration::from_secs(5), "idle engine death not noticed");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let e = m.ensure_loaded(MODEL_ID, cpu_opts()).await.err().unwrap();
    assert_eq!(e.code, ErrorCode::BackendFailed, "{e}");
    assert!(e.message.contains("3 times"), "{e}");
    assert!(m.status().await.is_none());
    for pid in [pid1, pid2, pid3] {
        assert!(
            !ucompute_runtime::llamacpp::process::alive(pid),
            "engine {pid} left running"
        );
    }
}

/// F-01-03: the fit check uses memory sampled when the load is planned, not the startup probe.
/// The probe here says 64 GiB; the load-time sample says 1 MB, so the load is refused before
/// anything is spawned.
#[tokio::test]
async fn fit_check_uses_memory_sampled_at_load_time() {
    let Some((backend, _)) = setup() else { return };
    let mut m = manager(backend, 4);
    m.available_memory = || Some(1_000_000);
    let e = m.ensure_loaded(MODEL_ID, cpu_opts()).await.err().unwrap();
    assert_eq!(e.code, ErrorCode::InsufficientMemory, "{e}");
    assert!(e.message.contains(" MB"), "{e}");
    assert!(m.status().await.is_none() && m.snapshot().loading.is_none());
    assert_eq!(m.available_memory_now(), 1_000_000);
    m.available_memory = || None;
    assert_eq!(
        m.available_memory_now(),
        64 << 30,
        "an unreadable sample falls back to the probe"
    );
}

/// The guard itself: with `UCOMPUTE_REQUIRE_FIXTURES=1` and no assets, a fixture test FAILS rather
/// than printing SKIP and passing; without it, the same test skips loudly. Re-runs this test binary
/// on `cpu_load_golden_text_and_conformance` with UCOMPUTE_HOME pointing at an empty location.
#[test]
fn require_fixtures_turns_a_missing_asset_into_a_failure() {
    let empty = std::env::temp_dir().join(format!("ucompute-no-fixtures-{}", std::process::id()));
    let run = |require: &str| {
        let o = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cpu_load_golden_text_and_conformance",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("UCOMPUTE_HOME", &empty)
            .env("UCOMPUTE_REQUIRE_FIXTURES", require)
            .output()
            .unwrap();
        let out = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        (o.status.success(), out)
    };
    let (ok, out) = run("1");
    assert!(
        !ok && out.contains("UCOMPUTE_REQUIRE_FIXTURES=1: pinned llama-server missing"),
        "{out}"
    );
    let (ok, out) = run("0");
    assert!(ok && out.contains("SKIP: pinned llama-server missing"), "{out}");
}
