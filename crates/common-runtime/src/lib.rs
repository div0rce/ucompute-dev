//! Inference runtime: the `InferenceBackend` / `LoadedModel` traits (F-01-06), the llama.cpp
//! adapter (ADR-0002), the single-slot `ModelManager` and the backend conformance suite.
//!
//! Test hook: `COMMON_TEST_FORCE_GPU_FAIL=1` makes every GPU attempt fail for real (see
//! `llamacpp::process::FORCE_GPU_FAIL_ENV`) so the Auto→CPU fallback can be exercised anywhere.

pub mod types;
pub mod llamacpp;
pub mod conformance;
pub mod manager;

pub use types::*;

use std::sync::Arc;

/// Look up a backend adapter by name. `llama.cpp` is the only one today.
pub fn backend_by_name(name: &str, engine_dir: std::path::PathBuf, paths: common_core::Paths) -> Option<Arc<dyn InferenceBackend>> {
    match name {
        "llama.cpp" | "llamacpp" => Some(Arc::new(llamacpp::LlamaCppBackend::new(engine_dir, paths))),
        _ => None,
    }
}
