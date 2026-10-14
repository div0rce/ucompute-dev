//! rustls configs with RFC 7250 raw public keys (ADR-0003 D3). STUB.
//! Server: TLS 1.3 only, `AlwaysResolvesServerRawPublicKeys`, client auth mandatory via `PinnedPeerVerifier`.
//! Client: `AlwaysResolvesClientRawPublicKeys`, server verified by the same pinned verifier.
//! Pairing endpoint uses `AcceptAnyPeer` (records the peer id; no trust).
use common_core::Result;
use common_identity::{DeviceIdentity, PeerId, PeerStore};
use std::sync::Arc;

/// Which keys a handshake may complete with.
pub enum PeerPolicy {
    /// Workload endpoint: pinned and not revoked, checked inside the handshake.
    Pinned(Arc<PeerStore>),
    /// Pairing endpoint: any Ed25519 key (the pairing protocol then authenticates with the code).
    AcceptAny,
}

pub fn server_config(identity: &DeviceIdentity, policy: PeerPolicy, alpn: &[u8]) -> Result<quinn::ServerConfig> {
    let _ = (identity, policy, alpn);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "tls",
    ))
}
pub fn client_config(identity: &DeviceIdentity, policy: PeerPolicy, alpn: &[u8]) -> Result<quinn::ClientConfig> {
    let _ = (identity, policy, alpn);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "tls",
    ))
}
/// The authenticated peer id of a connection (from its raw public key).
pub fn peer_id_of(conn: &quinn::Connection) -> Result<PeerId> {
    let _ = conn;
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "tls",
    ))
}
