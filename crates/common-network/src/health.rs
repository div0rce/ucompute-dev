//! Liveness (F-02-05): QUIC keep-alive 2 s / idle 6 s; state machine Connected → Unhealthy → Reconnecting.
//!
//! The transport itself detects a dead peer: quinn's idle timeout (6 s, three missed 2 s
//! keep-alives) closes the `Connection` with `ConnectionError::TimedOut`
//! (`tests/peer_transport.rs::kill_peer_detected_within_bound_single_host`). This module is the
//! pure state machine a reconnect loop drives from those events.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_misses_then_unhealthy_then_recovers() {
        let mut t = HealthTracker::new(3);
        t.on_miss();
        t.on_miss();
        assert_eq!(t.state, PeerHealth::Connected);
        t.on_miss();
        assert_eq!(t.state, PeerHealth::Unhealthy);
        assert_eq!(t.misses, 3);
        t.on_heartbeat();
        assert_eq!((t.state, t.misses), (PeerHealth::Connected, 0));
    }

    #[test]
    fn lost_means_reconnecting_until_heartbeat() {
        let mut t = HealthTracker::new(3);
        t.on_lost();
        assert_eq!(t.state, PeerHealth::Reconnecting);
        t.on_heartbeat();
        assert_eq!(t.state, PeerHealth::Connected);
    }

    #[test]
    fn backoff_doubles_and_caps_at_30s() {
        let seq: Vec<u64> = (0..8).map(HealthTracker::backoff_secs).collect();
        assert_eq!(seq, [1, 2, 4, 8, 16, 30, 30, 30]);
    }
}
