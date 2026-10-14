//! Verified, resumable downloads (F-01-05, F-05-02). STUB — implementation pending.

use crate::catalog::ModelEntry;
use crate::store::{ModelManifest, Store};
use common_core::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Progress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    /// Bytes fetched over the network during this call (0 on cache hit).
    pub network_bytes: u64,
    pub resumed_from: u64,
    pub done: bool,
}

/// Download `entry` into the store. Resumes a partial file via HTTP Range, verifies SHA-256,
/// deletes the file on mismatch, and is a no-op (zero network bytes) when the blob is already
/// present and valid. Requires `https://` URLs.
pub async fn pull(store: &Store, entry: &ModelEntry, progress: &mut dyn FnMut(Progress)) -> Result<ModelManifest> {
    let _ = (store, entry, progress);
    Err(common_core::CommonError::new(common_core::ErrorCode::NotImplemented, "pull"))
}
