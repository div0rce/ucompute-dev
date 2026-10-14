//! Quantization recommendation (F-01-07): advisory only, always confirmed by the user.

use crate::feasibility::budget_bytes;
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

/// Rank by bits per weight (descending, ties broken by larger file) and recommend the first
/// candidate whose estimate fits `available_bytes` after keeping `headroom_fraction` spare.
pub fn recommend(mut candidates: Vec<QuantCandidate>, available_bytes: u64, headroom_fraction: f32) -> Recommendation {
    let budget = budget_bytes(available_bytes, headroom_fraction);
    candidates.sort_by(|a, b| {
        b.quant.bits_per_weight().total_cmp(&a.quant.bits_per_weight()).then(b.file_size.cmp(&a.file_size))
    });
    let ranked: Vec<(QuantCandidate, bool)> =
        candidates.into_iter().map(|c| { let fits = c.estimated_total_bytes <= budget; (c, fits) }).collect();
    let recommended = ranked.iter().find(|(_, fits)| *fits).map(|(c, _)| c.clone());
    Recommendation { recommended, ranked, requires_confirmation: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: &str, quant: QuantType, file_size: u64, total: u64) -> QuantCandidate {
        QuantCandidate { id: id.into(), quant, file_size, estimated_total_bytes: total }
    }
    fn three() -> Vec<QuantCandidate> {
        vec![
            c("q4", QuantType::Q4KM, 400, 700),
            c("q8", QuantType::Q8_0, 800, 1100),
            c("q2", QuantType::Q2K, 200, 500),
        ]
    }

    #[test]
    fn picks_highest_bits_that_fits_with_headroom() {
        let r = recommend(three(), 1000, 0.10); // budget 900
        let ids: Vec<&str> = r.ranked.iter().map(|(c, _)| c.id.as_str()).collect();
        assert_eq!(ids, ["q8", "q4", "q2"]);
        assert_eq!(r.ranked.iter().map(|(_, f)| *f).collect::<Vec<_>>(), [false, true, true]);
        assert_eq!(r.recommended.as_ref().map(|c| c.id.as_str()), Some("q4"));
        assert!(r.requires_confirmation);
        // 1100 fits in 1300 only without headroom: 1300 * 0.9 = 1170 >= 1100
        assert_eq!(recommend(three(), 1300, 0.10).recommended.unwrap().id, "q8");
        assert_eq!(recommend(three(), 1200, 0.10).recommended.unwrap().id, "q4");
    }

    #[test]
    fn none_fits_and_ties_break_by_file_size() {
        let r = recommend(three(), 400, 0.10);
        assert_eq!(r.recommended, None);
        assert!(r.ranked.iter().all(|(_, f)| !f));
        assert!(r.requires_confirmation);
        let r = recommend(vec![c("small", QuantType::Q4KM, 100, 100), c("large", QuantType::Q4KM, 200, 200)], 10_000, 0.0);
        assert_eq!(r.ranked[0].0.id, "large");
        assert_eq!(r.recommended.unwrap().id, "large");
        assert_eq!(recommend(vec![], 10_000, 0.1).recommended, None);
    }
}
