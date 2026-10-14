//! Pinned inference-engine provisioning (ADR-0002). Pins live in `catalog/backends.json`.
//! STUB — implementation pending.

use common_core::{Paths, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const BACKENDS_JSON: &str = include_str!("../../../catalog/backends.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendPin {
    pub name: String,
    pub release: String,
    pub source_commit: String,
    /// `macos-arm64`, `linux-x64`, `win-x64`...
    pub platform: String,
    pub archive: String,
    /// Directory inside the archive that holds the binaries.
    pub directory: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub license: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BackendPins { pub schema_version: u32, pub backends: Vec<BackendPin> }

impl BackendPins {
    pub fn builtin() -> Result<Self> { Ok(serde_json::from_str(BACKENDS_JSON)?) }
    /// The pin for this OS/arch, if one exists.
    pub fn for_current_platform(&self, name: &str) -> Option<&BackendPin> {
        let plat = current_platform();
        self.backends.iter().find(|b| b.name == name && b.platform == plat)
    }
}

pub fn current_platform() -> String {
    let os = match std::env::consts::OS { "macos" => "macos", "linux" => "linux", "windows" => "win", o => o };
    let arch = match std::env::consts::ARCH { "aarch64" => "arm64", "x86_64" => "x64", a => a };
    format!("{os}-{arch}")
}

/// Directory where the verified binaries for `pin` live.
pub fn backend_dir(paths: &Paths, pin: &BackendPin) -> PathBuf {
    paths.backends().join(&pin.name).join(&pin.release)
}

/// Download (if needed), verify the archive hash, extract safely, verify every extracted file
/// against the archive. Returns the binary directory.
pub async fn ensure_backend(paths: &Paths, pin: &BackendPin, progress: &mut dyn FnMut(crate::download::Progress)) -> Result<PathBuf> {
    let _ = (paths, pin, progress);
    Err(common_core::CommonError::new(common_core::ErrorCode::NotImplemented, "ensure_backend"))
}

/// Verify an already-extracted backend directory against its pinned archive without network.
pub fn verify_backend(paths: &Paths, pin: &BackendPin) -> Result<PathBuf> {
    let _ = (paths, pin);
    Err(common_core::CommonError::new(common_core::ErrorCode::NotImplemented, "verify_backend"))
}

/// Path of a named binary inside a backend dir (adds `.exe` on Windows).
pub fn binary(dir: &Path, name: &str) -> PathBuf {
    if cfg!(windows) { dir.join(format!("{name}.exe")) } else { dir.join(name) }
}
