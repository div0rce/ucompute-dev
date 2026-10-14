//! Context-length and KV-cache planning (F-01-08). STUB — implementation pending.

use crate::gguf::GgufMetadata;
use common_core::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KvType { F16, Q8_0 }

impl KvType {
    pub fn bytes_per_element(self) -> f32 {
        match self { KvType::F16 => 2.0, KvType::Q8_0 => 1.0625 }
    }
    pub fn as_llama_arg(self) -> &'static str {
        match self { KvType::F16 => "f16", KvType::Q8_0 => "q8_0" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextPlan {
    pub n_ctx: u32,
    pub trained_context: u32,
    pub kv_type: KvType,
    pub kv_cache_bytes: u64,
    /// True when the request was reduced to fit the trained context or the memory budget.
    pub capped: bool,
    pub reason: Option<String>,
}

/// Choose `n_ctx` and KV type. `requested = None` means "as large as fits, up to trained context".
/// Requests above the trained context are refused (Err ContextTooLong) unless `allow_cap` lets
/// us cap with an explicit reason.
pub fn plan_context(meta: &GgufMetadata, requested: Option<u32>, memory_budget_bytes: u64, allow_cap: bool) -> Result<ContextPlan> {
    let _ = (meta, requested, memory_budget_bytes, allow_cap);
    Err(common_core::CommonError::new(common_core::ErrorCode::NotImplemented, "context planning not implemented"))
}
