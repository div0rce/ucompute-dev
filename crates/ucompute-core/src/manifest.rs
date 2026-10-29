//! Reproducible execution manifests (X-15).
//!
//! Every `ucompute run` and every API request that completes writes one of these (without the
//! prompt text unless the caller opts in) so the exact run can be replayed.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HostSummary {
    pub os: String,
    pub os_version: String,
    pub arch: String,
    pub chip: String,
    pub memory_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionManifest {
    pub schema_version: u32,
    pub ucompute_version: String,
    pub timestamp_utc: String,
    pub host: HostSummary,
    /// Catalog id or user-supplied name.
    pub model_id: String,
    /// SHA-256 of the exact GGUF file that ran.
    pub model_sha256: String,
    pub backend_name: String,
    pub backend_version: String,
    /// `cpu` or `metal`/`cuda`/`vulkan`; what was *requested*.
    pub device_requested: String,
    /// What actually ran, with proof fields.
    pub device_actual: String,
    pub offloaded_layers: u32,
    pub total_layers: u32,
    pub n_ctx: u32,
    pub threads: u32,
    pub seed: u64,
    pub temperature: f32,
    pub max_tokens: u32,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// SHA-256 of the prompt text; the text itself is stored only when `prompt` is Some.
    pub prompt_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Generated token ids when available; used by the replay test.
    #[serde(default)]
    pub completion_token_ids: Vec<u32>,
    pub completion_sha256: String,
    pub metrics: RunMetrics,
    /// `single_physical_host` for Mode A; distributed modes add peer records later.
    pub scope: String,
    pub physical_device_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RunMetrics {
    pub load_ms: Option<f64>,
    pub time_to_first_token_ms: Option<f64>,
    pub prefill_tokens_per_second: Option<f64>,
    pub decode_tokens_per_second: Option<f64>,
    pub total_ms: f64,
    pub peak_rss_bytes: Option<u64>,
}

impl ExecutionManifest {
    pub const SCHEMA_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let m = ExecutionManifest {
            schema_version: ExecutionManifest::SCHEMA_VERSION,
            ucompute_version: "0.1.0".into(),
            timestamp_utc: "2026-10-14T00:00:00Z".into(),
            host: HostSummary::default(),
            model_id: "stories15M-q4_0".into(),
            model_sha256: "66".into(),
            backend_name: "llama.cpp".into(),
            backend_version: "b11609".into(),
            device_requested: "metal".into(),
            device_actual: "Metal0 (Apple M2)".into(),
            offloaded_layers: 7,
            total_layers: 6,
            n_ctx: 128,
            threads: 4,
            seed: 42,
            temperature: 0.0,
            max_tokens: 32,
            prompt_tokens: 10,
            completion_tokens: 32,
            prompt_sha256: "ab".into(),
            prompt: None,
            completion_token_ids: vec![1, 2],
            completion_sha256: "cd".into(),
            metrics: RunMetrics::default(),
            scope: "single_physical_host".into(),
            physical_device_count: 1,
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: ExecutionManifest = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }
}
