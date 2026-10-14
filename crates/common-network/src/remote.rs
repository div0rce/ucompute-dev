//! Mode B: run a whole generation on a trusted peer over the authenticated transport. STUB.
//! Client: `generate_remote`. Server: `serve_peer` loop on an accepted connection, gated by
//! `PolicyEngine::may_serve` and quotas, executed through the local `ModelManager`, audited.
use crate::transport::PeerConnection;
use common_core::Result;
use common_runtime::{GenerateRequest, GenerateStream};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct RemoteOutcome;

/// Send `req` for `model_id` to the peer; stream events back. Cancel closes the stream (the server
/// cancels its generation). Errors: PeerUnauthorized, PeerLost, ModelNotDownloaded (on the peer), …
pub async fn generate_remote(
    conn: Arc<PeerConnection>,
    model_id: &str,
    model_sha256: &str,
    req: GenerateRequest,
    privacy: &str,
    cancel: CancellationToken,
) -> Result<GenerateStream> {
    let _ = (conn, model_id, model_sha256, req, privacy, cancel);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "remote",
    ))
}

/// Serve remote requests on an accepted connection until it closes.
pub async fn serve_peer(
    conn: Arc<PeerConnection>,
    manager: Arc<common_runtime::manager::ModelManager>,
    policy: Arc<common_policy::PolicyEngine>,
    audit: Arc<common_observability::Audit>,
) -> Result<()> {
    let _ = (conn, manager, policy, audit);
    Err(common_core::CommonError::new(
        common_core::ErrorCode::NotImplemented,
        "remote",
    ))
}
