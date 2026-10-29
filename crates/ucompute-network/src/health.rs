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
use quinn::ConnectionError;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, Weak};
use std::time::Duration;
use ucompute_core::ErrorCode;
use ucompute_identity::PeerId;

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

/// Why a connection this device did not close went away, for the `peer_lost` audit line; `None`
/// when this device closed it (networking off, shutdown, revoke, unpair, a hold: `LocallyClosed`).
// ponytail: a local QUIC protocol-violation close (`TransportError`) and the other rare quinn
// errors are not recorded; map them here if they ever show up in the field.
fn lost_reason(reason: &ConnectionError) -> Option<&'static str> {
    match reason {
        ConnectionError::TimedOut => Some("timed_out"),
        ConnectionError::Reset => Some("reset"),
        ConnectionError::ApplicationClosed(_) | ConnectionError::ConnectionClosed(_) => Some("closed_by_peer"),
        _ => None,
    }
}

/// Samples `pc` every `every` until it closes; drives the node's tracker for its peer. Holds only a
/// `Weak` node so it never keeps a dropped node alive. A loss this device did not cause, of the
/// peer's last live connection, is audited `peer_lost` (`reason`, `addr`, `connected_s`); a retired
/// duplicate's close is not a loss.
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
    if let Some(why) = lost_reason(&reason).filter(|_| !pc.is_retired()) {
        node.audit(
            "peer_lost",
            Some(&peer),
            serde_json::json!({ "reason": why, "addr": pc.remote_addr.to_string(), "connected_s": pc.connected_s() }),
        );
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
/// meanwhile. Ends when the node stops, this device's own store refuses the peer (revoked,
/// unpinned), or the peer refuses this device for good after proving its key (revoked,
/// incompatible); health is then `Disconnected`. A refusal the peer did not prove reads
/// `peer_unauthorized` (`transport::unproven`: anything on the path can forge it) and is retried.
///
/// Both sides supervising each other is fine: crossed dials resolve to the one connection
/// initiated by the lower id (`Node::connect`), and `on_connect` may then see a connection that
/// `Node::accept` also returned.
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
                        if matches!(e.code, ErrorCode::PeerRevoked | ErrorCode::ProtocolIncompatible)
                            || crate::tls::store_verdict(&node.peers, &peer) != crate::tls::Verdict::Allowed =>
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
    fn only_losses_this_device_did_not_cause_are_peer_lost() {
        assert_eq!(lost_reason(&ConnectionError::TimedOut), Some("timed_out"));
        assert_eq!(lost_reason(&ConnectionError::Reset), Some("reset"));
        let held = ConnectionError::ApplicationClosed(quinn::ApplicationClose {
            error_code: quinn::VarInt::from_u32(crate::transport::CLOSE_HELD),
            reason: Default::default(),
        });
        assert_eq!(lost_reason(&held), Some("closed_by_peer"));
        assert_eq!(
            lost_reason(&ConnectionError::LocallyClosed),
            None,
            "stop, revoke, unpair, hold"
        );
    }

    #[test]
    fn backoff_doubles_and_caps_at_30s() {
        let seq: Vec<u64> = (0..8).map(HealthTracker::backoff_secs).collect();
        assert_eq!(seq, [1, 2, 4, 8, 16, 30, 30, 30]);
    }
}
