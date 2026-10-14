//! Quantization recommendation (F-01-07). STUB — implementation pending.

use crate::gguf::QuantType;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuantCandidate {
    pub id: String,
    pub quant: QuantType,
    pub file_size: u64,
    pub estimated_total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Recommendation {
    /// Highest-quality candidate that fits with the required headroom, if any.
    pub recommended: Option<QuantCandidate>,
    /// All candidates ordered best-quality-first with their fit status.
    pub ranked: Vec<(QuantCandidate, bool)>,
    /// Always true: a recommendation is advisory and must be confirmed by the user.
    pub requires_confirmation: bool,
}

pub fn recommend(candidates: Vec<QuantCandidate>, available_bytes: u64, headroom_fraction: f32) -> Recommendation {
    let _ = (available_bytes, headroom_fraction);
    Recommendation { recommended: None, ranked: candidates.into_iter().map(|c| (c, false)).collect(), requires_confirmation: true }
}
