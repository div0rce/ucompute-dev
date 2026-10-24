//! F-01-04 fallback: COMMON_TEST_FORCE_GPU_FAIL=1 makes the GPU attempt pass a device that does not
//! exist, so llama-server really exits (code 1). Own test binary because the hook is process-wide.

mod common;

use common::*;
use common_core::ErrorCode;
use common_runtime::llamacpp::process::FORCE_GPU_FAIL_ENV;
use common_runtime::manager::{LoadOptions, ModelManager};
use common_runtime::*;

#[tokio::test]
async fn auto_falls_back_to_cpu_with_a_reason() {
    std::env::set_var(FORCE_GPU_FAIL_ENV, "1");
    let Some((backend, model)) = setup() else { return };
    let loaded = backend.load(load_req(&model, DevicePreference::Auto)).await.unwrap();
    let info = loaded.info();
    assert_eq!(info.device_actual, "cpu");
    assert_eq!(info.offloaded_layers, 0);
    assert_eq!(info.device_requested, "auto");
    let reason = info
        .fallback_reason
        .as_deref()
        .expect("fallback must be surfaced, never silent");
    assert!(reason.contains("GPU engine start failed"), "{reason}");
    assert!(!reason.contains("--api-key"), "no secrets in the reason: {reason}");
    let (text, ..) = collect(
        loaded
            .generate(greedy(32), tokio_util::sync::CancellationToken::new())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(text, GOLDEN);
    loaded.unload().await.unwrap();
}

#[tokio::test]
async fn gpu_preference_is_a_typed_error() {
    std::env::set_var(FORCE_GPU_FAIL_ENV, "1");
    let Some((backend, model)) = setup() else { return };
    let e = backend
        .load(load_req(&model, DevicePreference::Gpu))
        .await
        .err()
        .unwrap();
    assert_eq!(e.code, ErrorCode::GpuUnavailable, "{e}");
    assert!(e.message.contains("exited"), "{e}");
}

/// Round 6b regression (F-08-04), real engine: three failed `--device gpu` starts refuse a 4th GPU
/// start, and a CPU load of the same model right after still loads and generates the golden text.
#[tokio::test]
async fn failed_gpu_starts_do_not_refuse_a_cpu_load() {
    std::env::set_var(FORCE_GPU_FAIL_ENV, "1");
    let Some((backend, _)) = setup() else { return };
    let paths = common_core::Paths::at(common_home());
    let m = ModelManager::new(
        paths.clone(),
        common_artifacts::Store::new(paths),
        std::sync::Arc::new(backend),
        Default::default(),
    );
    let on = |d| LoadOptions {
        device: Some(d),
        n_ctx: Some(128),
        threads: Some(4),
        ..Default::default()
    };
    for n in 1..=manager::MAX_RESTARTS {
        let e = m
            .ensure_loaded(MODEL_ID, on(DevicePreference::Gpu))
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::GpuUnavailable, "start {n}: {e}");
        assert!(e.message.contains("exited"), "start {n}: {e}");
    }
    let e = m
        .ensure_loaded(MODEL_ID, on(DevicePreference::Gpu))
        .await
        .err()
        .unwrap();
    assert!(e.message.contains("3 times in the last 60 s"), "{e}");
    let exit = m.snapshot().last_exit.expect("an exit during load is shown");
    assert!(exit.reason.contains("exit status"), "{}", exit.reason);
    let loaded = m
        .ensure_loaded(MODEL_ID, on(DevicePreference::Cpu))
        .await
        .expect("a CPU load after three failed GPU starts");
    assert_eq!(loaded.info().device_actual, "cpu");
    let (text, ..) = collect(
        loaded
            .generate(greedy(32), tokio_util::sync::CancellationToken::new())
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(text, GOLDEN);
    drop(loaded);
    m.unload().await.unwrap();
}
