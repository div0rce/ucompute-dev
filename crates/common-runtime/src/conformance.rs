//! Backend conformance suite (F-01-06): the same checks run against every adapter.
//! STUB — implementation pending. Each check returns Ok(()) or a descriptive error.

use crate::types::*;
use common_core::Result;
use std::sync::Arc;

pub struct ConformanceReport { pub passed: Vec<String>, pub failed: Vec<(String, String)> }

/// Run every conformance check against `backend` using `model_path` (a small real GGUF).
pub async fn run(backend: Arc<dyn InferenceBackend>, req: LoadRequest) -> Result<ConformanceReport> {
    let _ = (backend, req);
    Ok(ConformanceReport { passed: vec![], failed: vec![("suite".into(), "not implemented".into())] })
}
