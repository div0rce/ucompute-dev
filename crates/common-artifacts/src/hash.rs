//! SHA-256 helpers. STUB — implementation pending.
use common_core::Result;
use std::path::Path;

pub fn sha256_file(path: &Path) -> Result<String> {
    let _ = path;
    Err(common_core::CommonError::new(common_core::ErrorCode::NotImplemented, "sha256_file"))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
