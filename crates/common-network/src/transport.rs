//! The node: one quinn Endpoint for workloads (+ a temporary one for pairing). STUB.
use crate::proto::Capabilities;
use crate::NetworkConfig;
use common_core::Result;
use common_identity::{DeviceIdentity, PeerId, PeerStore};
use common_observability::Audit;
use common_policy::PolicyEngine;
use std::net::SocketAddr;
use std::sync::Arc;

/// An authenticated connection to a pinned peer.
pub struct PeerConnection {
    pub peer_id: PeerId,
    pub remote_addr: SocketAddr,
    pub conn: quinn::Connection,
    /// Received right after Hello; refreshed by Status messages.
    pub capabilities: std::sync::Mutex<Option<Capabilities>>,
    pub negotiated_minor: u32,
}

pub struct Node {
    pub config: NetworkConfig,
    pub identity: Arc<DeviceIdentity>,
    pub peers: Arc<PeerStore>,
    pub policy: Arc<PolicyEngine>,
    pub audit: Arc<Audit>,
    pub endpoint: quinn::Endpoint,
}

impl Node {
    /// Bind the workload endpoint (refuses when `config.enabled == false`), start accepting.
    pub async fn start(
        config: NetworkConfig,
        identity: Arc<DeviceIdentity>,
        peers: Arc<PeerStore>,
        policy: Arc<PolicyEngine>,
        audit: Arc<Audit>,
    ) -> Result<Arc<Node>> {
        let _ = (config, identity, peers, policy, audit);
        Err(common_core::CommonError::new(
            common_core::ErrorCode::NotImplemented,
            "transport",
        ))
    }
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }
    /// Connect to a pinned peer; Hello/Capabilities exchanged; typed errors: PeerUnauthorized,
    /// PeerRevoked, ProtocolIncompatible, PeerLost.
    pub async fn connect(&self, addr: SocketAddr, expected: PeerId) -> Result<Arc<PeerConnection>> {
        let _ = (addr, expected);
        Err(common_core::CommonError::new(
            common_core::ErrorCode::NotImplemented,
            "transport",
        ))
    }
    /// Next inbound authenticated connection (after Hello/Capabilities). None when the endpoint closed.
    pub async fn accept(&self) -> Option<Arc<PeerConnection>> {
        None
    }
    /// Currently connected peers.
    pub fn connections(&self) -> Vec<Arc<PeerConnection>> {
        vec![]
    }
    /// Close every connection to `id` with the REVOKED close code (F-06-05, ≤ 5 s).
    pub async fn disconnect(&self, id: &PeerId, reason: &str) {
        let _ = (id, reason);
    }
    pub async fn shutdown(&self) {}
}

/// Build this device's signed Capabilities record from `common hardware --json` and the store.
pub fn local_capabilities(
    identity: &DeviceIdentity,
    groups: &[String],
    cached_models: &[String],
) -> Result<Capabilities> {
    let _ = (identity, groups, cached_models);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "transport",
    ))
}
