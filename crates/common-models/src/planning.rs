//! Context-length and KV-cache planning (F-01-08).

use crate::feasibility::estimate;
use crate::gguf::GgufMetadata;
use crate::human_bytes;
use common_core::{CommonError, ErrorCode, Result};
use serde::{Deserialize, Serialize};

/// Granularity of context reductions and the smallest context we will plan (unless the model's
/// trained context is smaller still).
pub const CTX_STEP: u32 = 256;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum KvType {
    F16,
    Q8_0,
}

impl KvType {
    pub fn bytes_per_element(self) -> f32 {
        match self {
            KvType::F16 => 2.0,
            KvType::Q8_0 => 1.0625,
        }
    }
    pub fn as_llama_arg(self) -> &'static str {
        match self {
            KvType::F16 => "f16",
            KvType::Q8_0 => "q8_0",
        }
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
/// us cap with an explicit reason. `n_ctx` is then lowered in steps of [`CTX_STEP`] until
/// weights (`file_size` minus header) + KV cache + compute buffers fit `memory_budget_bytes`;
/// F16 KV is preferred and Q8_0 is used only when it is the difference between fitting and not.
pub fn plan_context(
    meta: &GgufMetadata,
    file_size: u64,
    requested: Option<u32>,
    memory_budget_bytes: u64,
    allow_cap: bool,
) -> Result<ContextPlan> {
    let trained = meta.context_length;
    if trained == 0 {
        return Err(CommonError::new(
            ErrorCode::ModelFileInvalid,
            format!("model does not declare {}.context_length", meta.architecture),
        ));
    }
    let mut reasons = Vec::new();
    let start = match requested {
        None => trained,
        Some(0) => return Err(CommonError::invalid("context length must be at least 1 token")),
        Some(r) if r > trained => {
            let msg =
                format!("Requested context of {r} tokens exceeds the model's trained context of {trained} tokens");
            if !allow_cap {
                return Err(CommonError::new(ErrorCode::ContextTooLong, format!("{msg}.")));
            }
            reasons.push(format!("{msg}; capped to {trained}."));
            trained
        }
        Some(r) => r,
    };

    let min_ctx = CTX_STEP.min(trained);
    for kv_type in [KvType::F16, KvType::Q8_0] {
        let mut n_ctx = start;
        loop {
            let est = estimate(meta, file_size, n_ctx, kv_type);
            if est.total_bytes <= memory_budget_bytes {
                let budget = human_bytes(memory_budget_bytes);
                if n_ctx < start {
                    reasons.push(format!(
                        "Context reduced from {start} to {n_ctx} tokens so the model fits in {budget}."
                    ));
                }
                if kv_type == KvType::Q8_0 {
                    reasons.push(format!(
                        "KV cache stored as Q8_0 because it does not fit as F16 even at {min_ctx} tokens in {budget}."
                    ));
                }
                return Ok(ContextPlan {
                    n_ctx,
                    trained_context: trained,
                    kv_type,
                    kv_cache_bytes: est.kv_cache_bytes,
                    capped: n_ctx < requested.unwrap_or(n_ctx),
                    reason: (!reasons.is_empty()).then(|| reasons.join(" ")),
                });
            }
            if n_ctx <= min_ctx {
                break;
            }
            // round down to the next multiple of CTX_STEP, never below the minimum
            n_ctx = ((n_ctx - 1) / CTX_STEP * CTX_STEP).max(min_ctx);
        }
    }
    let est = estimate(meta, file_size, min_ctx, KvType::Q8_0);
    Err(CommonError::new(
        ErrorCode::InsufficientMemory,
        format!(
            "Even at the minimum context of {min_ctx} tokens this model needs about {} but only {} is available.",
            human_bytes(est.total_bytes),
            human_bytes(memory_budget_bytes)
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feasibility::calibration::*;
    use crate::gguf::parse_metadata;
    use crate::gguf::tests::synthetic_llama;

    fn meta() -> (GgufMetadata, u64) {
        let bytes = synthetic_llama();
        (parse_metadata(&bytes).unwrap(), bytes.len() as u64)
    }
    fn big() -> (GgufMetadata, u64) {
        let (mut m, file) = meta();
        m.context_length = 4096;
        (m, file)
    }
    /// What the engine reserves for `n_ctx` tokens: the same formula the planner budgets against.
    fn cost(m: &GgufMetadata, file: u64, n_ctx: u32, kv: KvType) -> u64 {
        estimate(m, file, n_ctx, kv).total_bytes
    }

    #[test]
    fn kv_bytes_formula_matches_hand_computation() {
        let (m, _) = meta();
        // 6 layers x 2 (K,V) x 6 kv heads x 48 head_dim x 2 bytes = 6912 per token; the engine
        // allocates whole blocks of 256 cells, so 128 requested tokens reserve 256 x 6912
        assert_eq!(m.kv_bytes_per_token(2.0), 6912.0);
        assert_eq!(m.kv_bytes_per_token(2.0) as u64 * 256, 1_769_472);
        let plan = plan_context(&m, 64, Some(128), u64::MAX, false).unwrap();
        assert_eq!(plan.kv_cache_bytes, 1_769_472);
        assert_eq!(plan.kv_type, KvType::F16);
        assert!(!plan.capped);
        assert_eq!(plan.reason, None);
    }

    #[test]
    fn requested_above_trained_is_refused_or_capped() {
        let (m, file) = meta();
        let e = plan_context(&m, file, Some(4096), u64::MAX, false).unwrap_err();
        assert_eq!(e.code, ErrorCode::ContextTooLong);
        assert!(e.message.contains("4096") && e.message.contains("128"), "{}", e.message);
        let p = plan_context(&m, file, Some(4096), u64::MAX, true).unwrap();
        assert_eq!((p.n_ctx, p.trained_context, p.capped), (128, 128, true));
        assert!(p.reason.as_deref().unwrap().contains("capped to 128"));
        // None means "trained context"
        assert_eq!(plan_context(&m, file, None, u64::MAX, false).unwrap().n_ctx, 128);
        assert_eq!(
            plan_context(&m, file, Some(0), u64::MAX, false).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn budget_driven_reduction_prefers_f16_then_q8() {
        let (m, file) = big();
        let (f16, q8) = (KvType::F16, KvType::Q8_0);
        // budget holds exactly 2048 tokens of F16 KV: 4096 is reduced to 2048
        let p = plan_context(&m, file, Some(4096), cost(&m, file, 2048, f16), false).unwrap();
        assert_eq!((p.n_ctx, p.kv_type, p.capped), (2048, f16, true));
        assert!(p.reason.as_deref().unwrap().contains("reduced from 4096 to 2048"));
        assert_eq!(p.kv_cache_bytes, 2048 * 6912);
        // F16 does not fit even at 256 tokens but Q8_0 at 256 does: Q8_0 is used only as the last
        // resort, with both reasons spelled out
        let p = plan_context(&m, file, Some(4096), cost(&m, file, 256, q8), false).unwrap();
        assert_eq!((p.n_ctx, p.kv_type, p.capped), (256, q8, true));
        let reason = p.reason.as_deref().unwrap();
        assert!(
            reason.contains("reduced from 4096 to 256") && reason.contains("Q8_0"),
            "{reason}"
        );
        assert_eq!(p.kv_cache_bytes, 940_032);
        // when F16 fits at a smaller context, it is chosen over Q8_0 at a larger one
        let p = plan_context(&m, file, Some(4096), cost(&m, file, 2048, f16) + 1, false).unwrap();
        assert_eq!(p.kv_type, f16);
        // an odd request costs the same as the next block of 256 cells and is kept as asked when it fits
        assert_eq!(
            plan_context(&m, file, Some(1000), cost(&m, file, 1024, f16), false)
                .unwrap()
                .n_ctx,
            1000
        );
        // ... and rounds down to a multiple of 256 when it does not
        let p = plan_context(&m, file, Some(1000), cost(&m, file, 1024, f16) - 1, false).unwrap();
        assert_eq!((p.n_ctx, p.capped), (768, true));
        // nothing fits: typed InsufficientMemory with sizes in the message
        let e = plan_context(&m, file, Some(4096), cost(&m, file, 256, q8) - 1, false).unwrap_err();
        assert_eq!(e.code, ErrorCode::InsufficientMemory);
        assert!(e.message.contains("minimum context of 256"), "{}", e.message);
        // the floor is the trained context when that is below 256
        let (small, file) = meta();
        let e = plan_context(&small, file, None, 0, false).unwrap_err();
        assert!(e.message.contains("minimum context of 128"), "{}", e.message);
        assert!(
            cost(&m, file, 0, f16) > OVERHEAD_CPU_BYTES,
            "sanity: fixed cost includes the baseline"
        );
    }
}
