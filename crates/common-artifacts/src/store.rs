//! Content-addressed model store: `models/blobs/sha256-<hex>` + `models/manifests/<id>.json`.
//! STUB — implementation pending.

use common_core::{Paths, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelManifest {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub file_name: String,
    pub sha256: String,
    pub bytes: u64,
    pub license: String,
    #[serde(default)]
    pub license_url: Option<String>,
    pub source_url: String,
    #[serde(default = "unknown")]
    pub redistribution: String,
    pub imported_at_utc: String,
    #[serde(default)]
    pub chat: bool,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn unknown() -> String { "unknown".into() }

#[derive(Debug, Clone)]
pub struct Store {
    pub paths: Paths,
}

impl Store {
    pub fn new(paths: Paths) -> Self { Self { paths } }
    pub fn blob_path(&self, sha256: &str) -> PathBuf { self.paths.model_blobs().join(format!("sha256-{sha256}")) }
    pub fn manifest_path(&self, id: &str) -> PathBuf { self.paths.model_manifests().join(format!("{id}.json")) }

    pub fn list(&self) -> Result<Vec<ModelManifest>> { Err(todo()) }
    pub fn get(&self, id: &str) -> Result<Option<ModelManifest>> { let _ = id; Err(todo()) }
    /// Hash `path`, link or copy it into the blob store, write a manifest. Does not move the original.
    pub fn import(&self, path: &Path, id: Option<&str>, license: Option<&str>) -> Result<ModelManifest> { let _ = (path, id, license); Err(todo()) }
    pub fn write_manifest(&self, m: &ModelManifest) -> Result<()> { let _ = m; Err(todo()) }
    pub fn remove(&self, id: &str) -> Result<()> { let _ = id; Err(todo()) }
    /// Re-hash the blob and compare with the manifest (F-05-01).
    pub fn verify(&self, id: &str) -> Result<()> { let _ = id; Err(todo()) }
}

fn todo() -> common_core::CommonError { common_core::CommonError::new(common_core::ErrorCode::NotImplemented, "store") }
