//! Liveness (F-02-05): QUIC keep-alive 2 s / idle 6 s; state machine Connected → Unhealthy → Reconnecting. STUB.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PeerHealth {
    Connected,
    Unhealthy,
    Reconnecting,
    Disconnected,
}

/// Pure state machine over heartbeat events (unit-testable): misses counted, threshold 3.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthTracker {
    pub state: PeerHealth,
    pub misses: u32,
    pub threshold: u32,
}
impl HealthTracker {
    pub fn new(threshold: u32) -> Self {
        Self {
            state: PeerHealth::Connected,
            misses: 0,
            threshold,
        }
    }
    pub fn on_heartbeat(&mut self) {
        self.misses = 0;
        self.state = PeerHealth::Connected;
    }
    pub fn on_miss(&mut self) {
        self.misses += 1;
        if self.misses >= self.threshold {
            self.state = PeerHealth::Unhealthy;
        }
    }
    pub fn on_lost(&mut self) {
        self.state = PeerHealth::Reconnecting;
    }
    /// Backoff for reconnect attempt `n` (1,2,4,8… capped at 30 s).
    pub fn backoff_secs(n: u32) -> u64 {
        (1u64 << n.min(5)).min(30)
    }
}
