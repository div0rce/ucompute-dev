//! Liveness (F-02-05): QUIC keep-alive 1 s / idle 4 s; state machine Connected → Unhealthy →
//! Reconnecting → Connected.
//!
//! Detection: every registered connection gets a [`watch`]er that samples the connection's
//! received-datagram counter once per keep-alive interval. A healthy peer answers every keep-alive
//! (it ACKs our PING), so an interval without a datagram is a miss; [`MISS_THRESHOLD`] misses mark
//! the peer `Unhealthy`, and quinn's idle timeout (4 s) then closes the connection with
//! `TimedOut` → `Reconnecting`. Worst case from the peer's death to the close is idle + one
//! keep-alive interval (our own PING restarts the idle timer once, RFC 9000 §10.1) = 5 s, inside the
//! 6 s criterion. The 2 s / 6 s of ADR-0003 D7 gave 8 s worst case (6.5 s was measured), so the
//! defaults moved. Recovery: [`supervise`] reconnects with 1, 2, 4 … 30 s backoff.
use crate::transport::{Node, PeerConnection};
use common_core::ErrorCode;
use common_identity::PeerId;
use quinn::ConnectionError;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// Missed keep-alive intervals before a peer is `Unhealthy`.
pub const MISS_THRESHOLD: u32 = 3;

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

/// What a connection's end means for the peer: lost (it may come back) or deliberately closed.
fn after_close(reason: &ConnectionError) -> PeerHealth {
    match reason {
        ConnectionError::TimedOut | ConnectionError::Reset => PeerHealth::Reconnecting,
        _ => PeerHealth::Disconnected,
    }
}

/// Samples `pc` every `every` until it closes; drives the node's tracker for its peer. Holds only a
/// `Weak` node so it never keeps a dropped node alive.
pub(crate) async fn watch(node: Weak<Node>, pc: Arc<PeerConnection>, every: Duration) {
    let peer = pc.peer_id;
    let mut last = pc.conn.stats().udp_rx.datagrams;
    let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let closed = pc.conn.closed();
    tokio::pin!(closed);
    let reason = loop {
        tokio::select! {
            reason = &mut closed => break reason,
            _ = tick.tick() => {
                let now = pc.conn.stats().udp_rx.datagrams;
                let Some(node) = node.upgrade() else { return };
                if now == last {
                    node.update_health(&peer, HealthTracker::on_miss);
                } else {
                    node.update_health(&peer, HealthTracker::on_heartbeat);
                }
                last = now;
            }
        }
    };
    let Some(node) = node.upgrade() else { return };
    // Another live connection to the same peer keeps it Connected.
    if node.connections().iter().any(|c| c.peer_id == peer) {
        return;
    }
    let state = after_close(&reason);
    node.update_health(&peer, |t| match state {
        PeerHealth::Reconnecting => t.on_lost(),
        other => t.state = other,
    });
}

/// Keeps `peer` (pinned, reachable at `addr`) connected: connects when no live connection exists,
/// calls `on_connect` for every new connection (the daemon attaches its serving and control tasks
/// there), waits for it to end and reconnects with backoff 1, 2, 4 … 30 s, health `Reconnecting`
/// meanwhile. Ends when the node stops or the peer refuses this device for good (revoked,
/// unpinned, incompatible); health is then `Disconnected`.
// ponytail: both sides supervising each other can briefly hold two connections (one per
// direction); each is authenticated and served, so it costs a few streams. Tie-break by id if it matters.
pub fn supervise<F>(node: &Arc<Node>, peer: PeerId, addr: SocketAddr, on_connect: F) -> tokio::task::JoinHandle<()>
where
    F: Fn(Arc<PeerConnection>) + Send + 'static,
{
    let (weak, stop) = (Arc::downgrade(node), node.stop.clone());
    let live = node.connections().iter().any(|c| c.peer_id == peer);
    node.update_health(&peer, |t| {
        if !live {
            t.on_lost();
        }
    });
    tokio::spawn(async move {
        let mut attempt = 0u32;
        loop {
            let Some(node) = weak.upgrade() else { return };
            let live = node.connections().into_iter().find(|c| c.peer_id == peer);
            let conn = match live {
                Some(c) => Some(c),
                None => match node.connect(addr, peer).await {
                    Ok(c) => {
                        attempt = 0;
                        on_connect(c.clone());
                        Some(c)
                    }
                    Err(e)
                        if matches!(
                            e.code,
                            ErrorCode::PeerRevoked | ErrorCode::PeerUnauthorized | ErrorCode::ProtocolIncompatible
                        ) =>
                    {
                        tracing::info!(peer = %peer.short_label(), code = %e.code, "peer refuses this device; not reconnecting");
                        node.update_health(&peer, |t| t.state = PeerHealth::Disconnected);
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(peer = %peer.short_label(), code = %e.code, attempt, "reconnect failed");
                        node.update_health(&peer, HealthTracker::on_lost);
                        None
                    }
                },
            };
            drop(node);
            if let Some(c) = conn {
                tokio::select! {
                    _ = stop.cancelled() => return,
                    _ = c.conn.closed() => {}
                }
            }
            let wait = Duration::from_secs(HealthTracker::backoff_secs(attempt));
            attempt = attempt.saturating_add(1);
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    })
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
    fn loss_reconnects_deliberate_close_does_not() {
        assert_eq!(after_close(&ConnectionError::TimedOut), PeerHealth::Reconnecting);
        assert_eq!(after_close(&ConnectionError::Reset), PeerHealth::Reconnecting);
        assert_eq!(after_close(&ConnectionError::LocallyClosed), PeerHealth::Disconnected);
        let revoked = ConnectionError::ApplicationClosed(quinn::ApplicationClose {
            error_code: quinn::VarInt::from_u32(crate::transport::CLOSE_REVOKED),
            reason: Default::default(),
        });
        assert_eq!(after_close(&revoked), PeerHealth::Disconnected);
    }

    #[test]
    fn backoff_doubles_and_caps_at_30s() {
        let seq: Vec<u64> = (0..8).map(HealthTracker::backoff_secs).collect();
        assert_eq!(seq, [1, 2, 4, 8, 16, 30, 30, 30]);
    }
}
