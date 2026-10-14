//! Artifact plane: catalog (F-10-02 data), manifests + checksums (F-05-01), license metadata
//! (F-05-04), download + integrity + cache (F-01-05), resumable downloads (F-05-02), pinned
//! backend provisioning (ADR-0002).

pub mod backend;
pub mod catalog;
pub mod download;
pub mod hash;
pub mod store;

pub use backend::{ensure_backend, verify_backend, BackendPin};
pub use catalog::{Catalog, ModelEntry};
pub use download::{pull, Progress};
pub use store::{ModelManifest, Store};
