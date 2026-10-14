//! Local HTTP API. STUB — implementation pending.
//!
//! Secure defaults: binds `127.0.0.1` unless `--host` is given; a non-loopback bind without an
//! API key is refused (PolicyDenied). The engine's own port is never exposed.

use common_core::{CommonError, ErrorCode, Paths, Result};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    /// Max in-flight generations; excess waits up to `queue_timeout_ms` then gets 429.
    pub max_concurrent: usize,
    pub max_queue: usize,
    pub queue_timeout_ms: u64,
    pub default_device: common_runtime::DevicePreference,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { host: "127.0.0.1".into(), port: 8192, api_key: None, max_concurrent: 4, max_queue: 32, queue_timeout_ms: 30_000, default_device: common_runtime::DevicePreference::Auto }
    }
}

pub struct AppState {
    pub config: ApiConfig,
    pub paths: Paths,
    pub store: common_artifacts::Store,
    pub backend: Arc<dyn common_runtime::InferenceBackend>,
}

/// Build the axum router (exposed for in-process tests).
pub fn router(state: Arc<AppState>) -> axum::Router {
    let _ = state;
    axum::Router::new()
}

/// Bind and serve until `shutdown` resolves.
pub async fn serve(state: Arc<AppState>, shutdown: impl std::future::Future<Output = ()> + Send + 'static) -> Result<()> {
    let _ = (state, shutdown);
    Err(CommonError::new(ErrorCode::NotImplemented, "api serve"))
}
