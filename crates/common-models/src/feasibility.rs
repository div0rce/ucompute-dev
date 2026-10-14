//! Memory feasibility estimates (F-01-03). STUB — implementation pending.

use crate::gguf::GgufMetadata;
use crate::planning::KvType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MemoryEstimate {
    pub weights_bytes: u64,
    pub kv_cache_bytes: u64,
    pub compute_buffer_bytes: u64,
    pub overhead_bytes: u64,
    pub total_bytes: u64,
    pub n_ctx: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum FitVerdict {
    Fits { headroom_bytes: u64 },
    TooLarge { needed_bytes: u64, available_bytes: u64, reason: String },
}

/// Estimate peak memory for loading `meta` (file of `file_size` bytes) with `n_ctx` tokens.
pub fn estimate(meta: &GgufMetadata, file_size: u64, n_ctx: u32, kv: KvType) -> MemoryEstimate {
    let _ = (meta, file_size, n_ctx, kv);
    MemoryEstimate::default()
}

/// Compare an estimate with available memory, keeping `headroom_fraction` (e.g. 0.10) spare.
pub fn check_fit(est: &MemoryEstimate, available_bytes: u64, headroom_fraction: f32) -> FitVerdict {
    let _ = (est, available_bytes, headroom_fraction);
    FitVerdict::TooLarge { needed_bytes: 0, available_bytes: 0, reason: "not implemented".into() }
}
