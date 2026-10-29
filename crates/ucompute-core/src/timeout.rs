//! Timeout and retry policy (F-08-01).
//!
//! Retries are only ever applied to idempotent operations (downloads, health probes). Generation
//! is never retried automatically.

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Timeouts {
    /// Engine start + model load until `/health` is ok.
    pub load: Duration,
    /// From request submit to the first generated token.
    pub first_token: Duration,
    /// Maximum gap between consecutive tokens.
    pub inter_token: Duration,
    /// Whole-request ceiling.
    pub total: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            load: Duration::from_secs(300),
            first_token: Duration::from_secs(120),
            inter_token: Duration::from_secs(30),
            total: Duration::from_secs(600),
        }
    }
}

/// Bounded exponential backoff for idempotent operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial: Duration,
    pub max: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            initial: Duration::from_millis(250),
            max: Duration::from_secs(8),
        }
    }
}

impl RetryPolicy {
    /// Delay before attempt `n` (0-based). Returns None once attempts are exhausted.
    pub fn delay_for(&self, attempt: u32) -> Option<Duration> {
        if attempt + 1 >= self.max_attempts {
            return None;
        }
        let d = self.initial.saturating_mul(1u32 << attempt.min(16));
        Some(d.min(self.max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_bounded() {
        let p = RetryPolicy::default();
        assert_eq!(p.delay_for(0), Some(Duration::from_millis(250)));
        assert_eq!(p.delay_for(1), Some(Duration::from_millis(500)));
        assert_eq!(p.delay_for(2), Some(Duration::from_secs(1)));
        assert_eq!(p.delay_for(3), None);
        let big = RetryPolicy { max_attempts: 100, ..p };
        assert_eq!(big.delay_for(40), Some(Duration::from_secs(8)));
    }
}
