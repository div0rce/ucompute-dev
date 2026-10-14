//! SPAKE2 pairing over a temporary accept-any QUIC endpoint (ADR-0003 D2). STUB.
use crate::transport::Node;
use common_core::Result;
use common_identity::{PeerId, PeerRecord};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct PairingSession {
    /// 6 decimal digits, single use, valid for the window.
    pub code: String,
    pub port: u16,
    pub expires_at_unix: u64,
    /// QR payload: `common-pair:v1:<peer hex>:<ip:port>:<128-bit secret hex>`.
    pub qr_payload: String,
}

/// Initiator: open the pairing endpoint, return the code. Completes at most one pairing; >5 failed
/// confirmations in 60 s locks pairing (PairingLocked) and closes the endpoint.
pub async fn start(
    node: Arc<Node>,
    device_name: &str,
) -> Result<(PairingSession, tokio::task::JoinHandle<Result<PeerRecord>>)> {
    let _ = (node, device_name);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "pairing",
    ))
}

/// Joiner: connect to `addr` (pairing endpoint), run SPAKE2 with `code`, verify HMAC confirmation
/// bound to the TLS exporter and both PeerIds, pin the initiator. Wrong code / MITM → PairingFailed.
pub async fn join(node: Arc<Node>, addr: SocketAddr, code: &str, device_name: &str) -> Result<PeerRecord> {
    let _ = (node, addr, code, device_name);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "pairing",
    ))
}

pub fn expected_peer_from_qr(payload: &str) -> Result<(PeerId, SocketAddr, String)> {
    let _ = payload;
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "pairing",
    ))
}
