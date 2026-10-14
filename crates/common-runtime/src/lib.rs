//! Inference runtime: the `InferenceBackend` / `LoadedModel` traits (F-01-06) and the llama.cpp
//! adapter (ADR-0002). STUB — adapter implementation pending.

pub mod types;
pub mod llamacpp;
pub mod conformance;

pub use types::*;

use std::sync::Arc;

/// Look up a backend adapter by name. `llama.cpp` is the only one today.
pub fn backend_by_name(name: &str, engine_dir: std::path::PathBuf, paths: common_core::Paths) -> Option<Arc<dyn InferenceBackend>> {
    match name {
        "llama.cpp" | "llamacpp" => Some(Arc::new(llamacpp::LlamaCppBackend::new(engine_dir, paths))),
        _ => None,
    }
}
