//! llama.cpp adapter: supervises a pinned `llama-server` on 127.0.0.1 (ADR-0002).
//! STUB — implementation pending.

use crate::types::*;
use common_core::{CommonError, ErrorCode, Paths, Result};
use std::path::PathBuf;
use std::sync::Arc;

pub struct LlamaCppBackend {
    pub engine_dir: PathBuf,
    pub paths: Paths,
}

impl LlamaCppBackend {
    pub fn new(engine_dir: PathBuf, paths: Paths) -> Self { Self { engine_dir, paths } }
}

#[async_trait::async_trait]
impl InferenceBackend for LlamaCppBackend {
    fn name(&self) -> &str { "llama.cpp" }
    fn version(&self) -> String { String::new() }
    fn capabilities(&self) -> Capabilities { Capabilities::default() }
    async fn load(&self, req: LoadRequest) -> Result<Arc<dyn LoadedModel>> {
        let _ = req;
        Err(CommonError::new(ErrorCode::NotImplemented, "llama.cpp adapter not implemented"))
    }
}
