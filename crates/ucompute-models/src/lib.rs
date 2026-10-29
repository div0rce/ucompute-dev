//! Model understanding without running anything: GGUF metadata (F-01-02), memory feasibility
//! (F-01-03), context/KV planning (F-01-08), quantization recommendation (F-01-07).

pub mod feasibility;
pub mod gguf;
pub mod planning;
pub mod quant;

pub use feasibility::{check_fit, estimate, estimate_stage, FitVerdict, MemoryEstimate, StageRange};
pub use gguf::{parse_metadata, read_metadata, GgufMetadata, GgufValue, LayerBytes, QuantType, TensorInfo};
pub use planning::{plan_context, plan_context_with, ContextPlan, KvType};
pub use quant::{recommend, QuantCandidate, Recommendation};

/// Human-readable decimal size for messages: `"19.1 MB"`, `"5.2 GB"`, `"512 B"`.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut unit = 0;
    while v >= 1000.0 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::human_bytes;

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1000), "1.0 KB");
        assert_eq!(human_bytes(19_077_344), "19.1 MB");
        assert_eq!(human_bytes(5_200_000_000), "5.2 GB");
        assert_eq!(human_bytes(u64::MAX), "18446744.1 TB");
    }
}
