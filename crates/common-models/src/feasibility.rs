//! Memory feasibility estimates (F-01-03): weights + KV cache + compute buffers + process baseline
//! compared with available memory, with a plain-language verdict.

use crate::gguf::GgufMetadata;
use crate::human_bytes;
use crate::planning::KvType;
use serde::{Deserialize, Serialize};

/// CALIBRATION — every coefficient of the estimate lives here so the evidence phase can tune them
/// against measured RSS of the pinned `llama-server`. Nothing else in this module has a magic number.
pub mod calibration {
    /// llama-server default physical batch size (`--ubatch-size`).
    pub const N_UBATCH: u64 = 512;
    /// Per-token intermediate activations are roughly this many embedding widths wide.
    pub const COMPUTE_EMBD_FACTOR: u64 = 16;
    /// Compute buffers hold F32.
    pub const COMPUTE_BYTES_PER_ELEMENT: u64 = 4;
    /// Fixed process baseline when the Metal backend is loaded.
    pub const OVERHEAD_GPU_BYTES: u64 = 200 * 1024 * 1024;
    /// Fixed process baseline for CPU-only inference.
    pub const OVERHEAD_CPU_BYTES: u64 = 100 * 1024 * 1024;
}

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
    use calibration::*;
    let weights_bytes = file_size.saturating_sub(meta.data_offset);
    let kv_cache_bytes = (n_ctx as f64 * meta.kv_bytes_per_token(kv.bytes_per_element())).ceil() as u64;
    let per_token = (meta.embedding_length as u64)
        .saturating_mul(COMPUTE_EMBD_FACTOR)
        .saturating_add(meta.vocab_size.unwrap_or(0) as u64);
    let compute_buffer_bytes = N_UBATCH.saturating_mul(per_token).saturating_mul(COMPUTE_BYTES_PER_ELEMENT);
    // ponytail: Metal assumed on macOS; thread a device parameter through once CPU-forced runs are planned.
    let overhead_bytes = if cfg!(target_os = "macos") { OVERHEAD_GPU_BYTES } else { OVERHEAD_CPU_BYTES };
    let total_bytes = weights_bytes
        .saturating_add(kv_cache_bytes)
        .saturating_add(compute_buffer_bytes)
        .saturating_add(overhead_bytes);
    MemoryEstimate { weights_bytes, kv_cache_bytes, compute_buffer_bytes, overhead_bytes, total_bytes, n_ctx }
}

/// Bytes usable after keeping `headroom_fraction` of `available_bytes` spare.
pub(crate) fn budget_bytes(available_bytes: u64, headroom_fraction: f32) -> u64 {
    let f = if headroom_fraction.is_finite() { headroom_fraction.clamp(0.0, 1.0) } else { 0.0 };
    (available_bytes as f64 * (1.0 - f64::from(f))).round() as u64
}

/// Compare an estimate with available memory, keeping `headroom_fraction` (e.g. 0.10) spare.
pub fn check_fit(est: &MemoryEstimate, available_bytes: u64, headroom_fraction: f32) -> FitVerdict {
    if est.total_bytes <= budget_bytes(available_bytes, headroom_fraction) {
        FitVerdict::Fits { headroom_bytes: available_bytes - est.total_bytes }
    } else {
        FitVerdict::TooLarge {
            needed_bytes: est.total_bytes,
            available_bytes,
            reason: format!(
                "This model needs about {} but only {} is available.",
                human_bytes(est.total_bytes),
                human_bytes(available_bytes)
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gguf::parse_metadata;
    use crate::gguf::tests::synthetic_llama;

    fn meta() -> (GgufMetadata, u64) {
        let bytes = synthetic_llama();
        (parse_metadata(&bytes).unwrap(), bytes.len() as u64)
    }

    #[test]
    fn formula_per_component() {
        let (m, file_size) = meta();
        let e = estimate(&m, file_size, 128, KvType::F16);
        assert_eq!(e.weights_bytes, file_size - m.data_offset);
        assert_eq!(e.weights_bytes, 64);
        // 6 layers x 128 ctx x 2 (K,V) x 6 kv heads x 48 head_dim x 2 bytes
        assert_eq!(e.kv_cache_bytes, 884_736);
        assert_eq!(e.compute_buffer_bytes, 512 * (288 * 16 + 300) * 4);
        assert_eq!(e.overhead_bytes, if cfg!(target_os = "macos") { 200 << 20 } else { 100 << 20 });
        assert_eq!(e.total_bytes, e.weights_bytes + e.kv_cache_bytes + e.compute_buffer_bytes + e.overhead_bytes);
        assert_eq!(e.n_ctx, 128);
        let q8 = estimate(&m, file_size, 128, KvType::Q8_0);
        assert_eq!(q8.kv_cache_bytes, (884_736.0 * 1.0625 / 2.0_f64).ceil() as u64);
        assert!(q8.total_bytes < e.total_bytes);
        // file shorter than header never underflows
        assert_eq!(estimate(&m, 0, 128, KvType::F16).weights_bytes, 0);
    }

    #[test]
    fn fit_boundaries_and_message() {
        let est = MemoryEstimate { total_bytes: 900, ..Default::default() };
        assert_eq!(check_fit(&est, 1000, 0.10), FitVerdict::Fits { headroom_bytes: 100 });
        match check_fit(&est, 999, 0.10) {
            FitVerdict::TooLarge { needed_bytes, available_bytes, reason } => {
                assert_eq!((needed_bytes, available_bytes), (900, 999));
                assert_eq!(reason, "This model needs about 900 B but only 999 B is available.");
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert_eq!(check_fit(&est, 900, 0.0), FitVerdict::Fits { headroom_bytes: 0 });
        assert!(matches!(check_fit(&est, 899, 0.0), FitVerdict::TooLarge { .. }));
        let big = MemoryEstimate { total_bytes: 5_200_000_000, ..Default::default() };
        let FitVerdict::TooLarge { reason, .. } = check_fit(&big, 3_100_000_000, 0.1) else { panic!() };
        assert_eq!(reason, "This model needs about 5.2 GB but only 3.1 GB is available.");
        // nonsense headroom values degrade to "no headroom", never panic
        assert_eq!(budget_bytes(1000, f32::NAN), 1000);
        assert_eq!(budget_bytes(1000, 7.0), 0);
        assert_eq!(budget_bytes(1000, -1.0), 1000);
    }
}
