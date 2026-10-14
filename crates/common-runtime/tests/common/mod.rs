//! Shared setup for the real-engine tests. Everything comes from COMMON_HOME (default: the repo's
//! `.common`); tests skip loudly when the pinned engine or the fixture model is missing.
#![allow(dead_code)]

use common_core::{Paths, Timeouts};
use common_runtime::llamacpp::LlamaCppBackend;
use common_runtime::*;
use std::path::{Path, PathBuf};

pub const MODEL_ID: &str = "stories15M-q4_0";
pub const MODEL_SHA: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
pub const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
pub const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

pub fn common_home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

/// Backend + fixture path, or None (with a printed reason) when the assets are not on this machine.
/// With `COMMON_REQUIRE_FIXTURES=1` (the evidence/CI job) a missing asset fails the test instead,
/// so a green run can never mean "nothing was generated".
pub fn setup() -> Option<(LlamaCppBackend, PathBuf)> {
    let paths = Paths::at(common_home());
    let engine_dir = paths.backends().join("llama.cpp").join("b11609");
    let model = paths.model_blobs().join(format!("sha256-{MODEL_SHA}"));
    let missing = if !common_artifacts::backend::binary(&engine_dir, "llama-server").is_file() {
        format!("pinned llama-server missing at {}", engine_dir.display())
    } else if !model.is_file() {
        format!("fixture model missing at {}", model.display())
    } else {
        return Some((LlamaCppBackend::new(engine_dir, paths), model));
    };
    assert!(
        std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "COMMON_REQUIRE_FIXTURES=1: {missing}"
    );
    eprintln!("SKIP: {missing}");
    None
}

pub fn load_req(model: &Path, device: DevicePreference) -> LoadRequest {
    LoadRequest {
        model_id: MODEL_ID.into(),
        model_path: model.to_path_buf(),
        model_sha256: MODEL_SHA.into(),
        device,
        n_ctx: 128,
        n_gpu_layers: None,
        threads: Some(4),
        parallel_slots: 1,
        kv_type: common_models::KvType::F16,
        embeddings: false,
        timeouts: Timeouts {
            load: std::time::Duration::from_secs(60),
            ..Timeouts::default()
        },
        extra_args: vec![],
    }
}

pub fn greedy(max_tokens: u32) -> GenerateRequest {
    GenerateRequest {
        input: PromptInput::Raw(PROMPT.into()),
        max_tokens,
        temperature: 0.0,
        seed: Some(42),
        return_token_ids: true,
        ..Default::default()
    }
}
