//! Memory feasibility estimates (F-01-03): weights + KV cache + compute buffers + process baseline
//! compared with available memory, with a plain-language verdict.

use crate::gguf::GgufMetadata;
use crate::human_bytes;
use crate::planning::KvType;
use serde::{Deserialize, Serialize};

/// CALIBRATION — every coefficient of the estimate lives here. Values were measured against the
/// pinned `llama-server` b11609 on an Apple M2 (`tests/estimate_vs_measured.rs` re-checks them):
/// the engine's own `model buffer size` / `llama_kv_cache: size` / `compute buffer size` log lines
/// and the kernel's peak physical footprint with `--load-mode none` and a filled context.
/// Nothing else in this module has a magic number.
pub mod calibration {
    /// llama-server default physical batch size (`--ubatch-size`); the engine caps it at the context size.
    pub const N_UBATCH: u64 = 512;
    /// The engine rounds the KV cache up to whole blocks of this many cells (`-c 128` allocates 256).
    pub const KV_CELL_PADDING: u64 = 256;
    /// Compute buffer per ubatch token, in bytes per feed-forward element (Metal: qwen2.5 18.0, smollm2 ~24).
    pub const COMPUTE_FF_BYTES: u64 = 18;
    /// Compute buffer per ubatch token per KV cell: the f16 KQ mask and friends (Metal 4, CPU 2).
    pub const COMPUTE_CTX_BYTES: u64 = 4;
    /// Tokenizer tables per vocabulary entry: BPE models (`gpt2`-style: qwen2, smollm2) keep token
    /// and merge maps, ~1.1 KiB each; SentencePiece (`llama`) ~320 B. Qwen2.5's 151,936 tokens
    /// are 160 MiB, more than its KV cache at 4096.
    ///
    /// Linux CPU measures far less against VmHWM (llama.cpp b11609 Ubuntu CPU build on sentinel,
    /// 2026-10-24): the whole overhead was 28 MiB for stories15M and 72–76 MiB for every qwen2.5 row,
    /// so the BPE rate there is 390 B (with the 20 MiB baseline below, all six measured rows land
    /// 1–3% above the peak). ponytail: Windows is unmeasured and keeps the macOS figures (larger, so
    /// safe); measure it on the laptop and add its own row.
    pub const TOKENIZER_BYTES_PER_TOKEN_BPE: u64 = if cfg!(target_os = "linux") { 390 } else { 1100 };
    pub const TOKENIZER_BYTES_PER_TOKEN_SPM: u64 = 320;
    /// Fixed process baseline without a vocabulary (dylibs, Metal device, server): 45–48 MiB
    /// measured with stories260K (512 tokens) on both devices.
    pub const OVERHEAD_GPU_BYTES: u64 = 46 * 1024 * 1024;
    /// Same baseline on CPU (20 MiB on Linux, from the rows above). ponytail: on Apple Silicon the CPU
    /// path also repacks Q4_0/Q8_0/Q4_K matmul weights while loading (+44 MiB footprint on
    /// qwen2.5-0.5b, more with mmap); upgrade by summing repackable 2-D tensors from `meta.tensors`
    /// when a CPU-only plan is made.
    pub const OVERHEAD_CPU_BYTES: u64 = if cfg!(target_os = "linux") { 20 } else { 46 } * 1024 * 1024;
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
    Fits {
        headroom_bytes: u64,
    },
    TooLarge {
        needed_bytes: u64,
        available_bytes: u64,
        reason: String,
    },
}

/// Estimate peak memory for loading `meta` (file of `file_size` bytes) with `n_ctx` tokens:
/// weights (file minus header) + KV cache for the engine's padded cell count + compute buffers +
/// process baseline and tokenizer tables. Reports what the engine reserves, so a context that is
/// never filled shows a smaller footprint.
pub fn estimate(meta: &GgufMetadata, file_size: u64, n_ctx: u32, kv: KvType) -> MemoryEstimate {
    use calibration::*;
    let data_bytes = file_size.saturating_sub(meta.data_offset);
    // Tied embeddings (no output.weight): the engine keeps token_embd on the CPU for lookups and
    // loads a second copy on the compute device for the output matmul (smollm2: +47.8 MiB).
    let tied = !meta.tensors.is_empty() && !meta.tensors.iter().any(|t| t.name == "output.weight");
    let weights_bytes = data_bytes.saturating_add(if tied {
        tensor_bytes(meta, data_bytes, "token_embd.weight")
    } else {
        0
    });
    let cells = u64::from(n_ctx).div_ceil(KV_CELL_PADDING) * KV_CELL_PADDING;
    let kv_cache_bytes = (cells as f64 * meta.kv_bytes_per_token(kv.bytes_per_element())).ceil() as u64;
    let n_ff = if meta.feed_forward_length > 0 {
        meta.feed_forward_length
    } else {
        meta.embedding_length.saturating_mul(4)
    };
    // the engine caps the batch at the requested context, so tiny contexts get a tiny ubatch
    let n_ubatch = N_UBATCH.min(u64::from(n_ctx));
    let compute_buffer_bytes = n_ubatch * (COMPUTE_FF_BYTES * u64::from(n_ff) + COMPUTE_CTX_BYTES * cells);
    // ponytail: Metal assumed on macOS; thread a device parameter through once CPU-forced runs are planned.
    let baseline = if cfg!(target_os = "macos") {
        OVERHEAD_GPU_BYTES
    } else {
        OVERHEAD_CPU_BYTES
    };
    let per_token = if meta.tokenizer_model.as_deref() == Some("llama") {
        TOKENIZER_BYTES_PER_TOKEN_SPM
    } else {
        TOKENIZER_BYTES_PER_TOKEN_BPE
    };
    let overhead_bytes = baseline.saturating_add(per_token * u64::from(meta.vocab_size.unwrap_or(0)));
    let total_bytes = weights_bytes
        .saturating_add(kv_cache_bytes)
        .saturating_add(compute_buffer_bytes)
        .saturating_add(overhead_bytes);
    MemoryEstimate {
        weights_bytes,
        kv_cache_bytes,
        compute_buffer_bytes,
        overhead_bytes,
        total_bytes,
        n_ctx,
    }
}

/// Size of tensor `name` in the file: the gap to the next tensor offset (or to the end of the
/// data section), so no per-type block-size table is needed. 0 when the tensor is absent.
fn tensor_bytes(meta: &GgufMetadata, data_bytes: u64, name: &str) -> u64 {
    let Some(t) = meta.tensors.iter().find(|t| t.name == name) else {
        return 0;
    };
    let end = meta
        .tensors
        .iter()
        .map(|o| o.offset)
        .filter(|&o| o > t.offset)
        .min()
        .unwrap_or(data_bytes);
    end.min(data_bytes).saturating_sub(t.offset)
}

/// Bytes usable after keeping `headroom_fraction` of `available_bytes` spare.
pub(crate) fn budget_bytes(available_bytes: u64, headroom_fraction: f32) -> u64 {
    let f = if headroom_fraction.is_finite() {
        headroom_fraction.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (available_bytes as f64 * (1.0 - f64::from(f))).round() as u64
}

/// Compare an estimate with available memory, keeping `headroom_fraction` (e.g. 0.10) spare.
pub fn check_fit(est: &MemoryEstimate, available_bytes: u64, headroom_fraction: f32) -> FitVerdict {
    if est.total_bytes <= budget_bytes(available_bytes, headroom_fraction) {
        FitVerdict::Fits {
            headroom_bytes: available_bytes - est.total_bytes,
        }
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
        // the synthetic file has no output.weight, so its 64-byte token_embd is counted twice
        assert_eq!(e.weights_bytes, file_size - m.data_offset + 64);
        assert_eq!(e.weights_bytes, 128);
        let mut untied = m.clone();
        untied.tensors.push(crate::gguf::TensorInfo {
            name: "output.weight".into(),
            dims: vec![288, 300],
            ggml_type: 2,
            offset: 64,
        });
        assert_eq!(estimate(&untied, file_size, 128, KvType::F16).weights_bytes, 64);
        // the engine pads 128 to 256 cells: 256 x 6 layers x 2 (K,V) x 6 kv heads x 48 head_dim x 2 bytes
        assert_eq!(e.kv_cache_bytes, 1_769_472);
        // ubatch is capped at the requested 128 tokens: 128 x (18 x 768 ff + 4 x 256 cells)
        assert_eq!(e.compute_buffer_bytes, 128 * (18 * 768 + 4 * 256));
        // baseline plus 300 SentencePiece tokens; an unknown or BPE tokenizer costs the BPE rate
        // (Linux CPU is calibrated on its own measurements: 20 MiB and 390 B per BPE token)
        let (base, bpe_rate) = if cfg!(target_os = "linux") {
            (20 << 20, 390)
        } else {
            (46 << 20, 1100)
        };
        assert_eq!(e.overhead_bytes, base + 300 * 320);
        let mut bpe = m.clone();
        bpe.tokenizer_model = Some("gpt2".into());
        assert_eq!(
            estimate(&bpe, file_size, 128, KvType::F16).overhead_bytes,
            base + 300 * bpe_rate
        );
        bpe.tokenizer_model = None;
        assert_eq!(
            estimate(&bpe, file_size, 128, KvType::F16).overhead_bytes,
            base + 300 * bpe_rate
        );
        assert_eq!(
            e.total_bytes,
            e.weights_bytes + e.kv_cache_bytes + e.compute_buffer_bytes + e.overhead_bytes
        );
        assert_eq!(e.n_ctx, 128);
        let q8 = estimate(&m, file_size, 128, KvType::Q8_0);
        assert_eq!(q8.kv_cache_bytes, (1_769_472.0 * 1.0625 / 2.0_f64).ceil() as u64);
        assert!(q8.total_bytes < e.total_bytes);
        // a full ubatch once the context allows it; the mask term grows with the cells
        let big = estimate(&m, file_size, 4096, KvType::F16);
        assert_eq!(big.kv_cache_bytes, 4096 * 6912);
        assert_eq!(big.compute_buffer_bytes, 512 * (18 * 768 + 4 * 4096));
        // odd contexts round up to the next block of cells
        assert_eq!(estimate(&m, file_size, 1000, KvType::F16).kv_cache_bytes, 1024 * 6912);
        // no feed_forward_length in the header: assume 4x the embedding width
        let mut no_ff = m.clone();
        no_ff.feed_forward_length = 0;
        assert_eq!(
            estimate(&no_ff, file_size, 128, KvType::F16).compute_buffer_bytes,
            128 * (18 * 4 * 288 + 4 * 256)
        );
        // zero context reserves nothing but weights and baseline; a file shorter than its header never underflows
        let zero = estimate(&m, 0, 0, KvType::F16);
        assert_eq!(
            (zero.weights_bytes, zero.kv_cache_bytes, zero.compute_buffer_bytes),
            (0, 0, 0)
        );
    }

    #[test]
    fn fit_boundaries_and_message() {
        let est = MemoryEstimate {
            total_bytes: 900,
            ..Default::default()
        };
        assert_eq!(check_fit(&est, 1000, 0.10), FitVerdict::Fits { headroom_bytes: 100 });
        match check_fit(&est, 999, 0.10) {
            FitVerdict::TooLarge {
                needed_bytes,
                available_bytes,
                reason,
            } => {
                assert_eq!((needed_bytes, available_bytes), (900, 999));
                assert_eq!(reason, "This model needs about 900 B but only 999 B is available.");
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
        assert_eq!(check_fit(&est, 900, 0.0), FitVerdict::Fits { headroom_bytes: 0 });
        assert!(matches!(check_fit(&est, 899, 0.0), FitVerdict::TooLarge { .. }));
        let big = MemoryEstimate {
            total_bytes: 5_200_000_000,
            ..Default::default()
        };
        let FitVerdict::TooLarge { reason, .. } = check_fit(&big, 3_100_000_000, 0.1) else {
            panic!()
        };
        assert_eq!(reason, "This model needs about 5.2 GB but only 3.1 GB is available.");
        // nonsense headroom values degrade to "no headroom", never panic
        assert_eq!(budget_bytes(1000, f32::NAN), 1000);
        assert_eq!(budget_bytes(1000, 7.0), 0);
        assert_eq!(budget_bytes(1000, -1.0), 1000);
    }
}
