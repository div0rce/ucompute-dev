//! Content-addressed model store: `models/blobs/sha256-<hex>` + `models/manifests/<id>.json`.
//!
//! The blob name is the hash, so one blob can back several manifests (F-05-01). Every manifest
//! carries a `license` string, `"unknown"` when not established (F-05-04).

use crate::hash::sha256_file;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use ucompute_core::paths::safe_name;
use ucompute_core::{ErrorCode, Paths, Result, UComputeError};

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

impl ModelManifest {
    pub const SCHEMA_VERSION: u32 = 1;
}

fn unknown() -> String {
    "unknown".into()
}

#[derive(Debug, Clone)]
pub struct Store {
    pub paths: Paths,
}

impl Store {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
    pub fn blob_path(&self, sha256: &str) -> PathBuf {
        self.paths.model_blobs().join(format!("sha256-{sha256}"))
    }
    pub fn manifest_path(&self, id: &str) -> PathBuf {
        self.paths.model_manifests().join(format!("{id}.json"))
    }

    /// All manifests, sorted by id. An absent manifests directory is an empty store.
    pub fn list(&self) -> Result<Vec<ModelManifest>> {
        let dir = self.paths.model_manifests();
        let mut out = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(read_manifest(&path)?);
            }
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<ModelManifest>> {
        check_id(id)?;
        let path = self.manifest_path(id);
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(read_manifest(&path)?))
    }

    /// Manifest plus the blob path, for loading. `ModelNotFound` without a manifest,
    /// `ModelNotDownloaded` when the blob is missing.
    pub fn resolve(&self, id: &str) -> Result<(ModelManifest, PathBuf)> {
        let m = self
            .get(id)?
            .ok_or_else(|| UComputeError::new(ErrorCode::ModelNotFound, format!("no manifest for model '{id}'")))?;
        let blob = self.blob_path(&m.sha256);
        if !blob.is_file() {
            return Err(UComputeError::new(
                ErrorCode::ModelNotDownloaded,
                format!("blob for model '{id}' is missing"),
            )
            .with_hint(format!("ucompute models pull {id}")));
        }
        Ok((m, blob))
    }

    /// Hash `path`, link or copy it into the blob store, write a manifest. Does not move the original.
    /// `id` defaults to the file stem, `license` to `"unknown"`.
    pub fn import(&self, path: &Path, id: Option<&str>, license: Option<&str>) -> Result<ModelManifest> {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| UComputeError::invalid("path has no file name"))?
            .to_string();
        let id = match id {
            Some(i) => i.to_string(),
            None => path
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string(),
        };
        check_id(&id)?;
        let path = path.canonicalize()?;
        let bytes = std::fs::metadata(&path)?.len();
        let sha256 = sha256_file(&path)?;
        if let Some(existing) = self.get(&id)? {
            if existing.sha256 != sha256 {
                return Err(UComputeError::invalid(format!(
                    "model id '{id}' already exists with different content; remove it first"
                )));
            }
        }
        let blob = self.blob_path(&sha256);
        if !blob.exists() {
            std::fs::create_dir_all(self.paths.model_blobs())?;
            if std::fs::hard_link(&path, &blob).is_err() {
                // Cross-volume (EXDEV) or a filesystem without hard links: copy via the partial dir.
                std::fs::create_dir_all(self.paths.partial())?;
                let tmp = self.paths.partial().join(format!("{sha256}.part"));
                std::fs::copy(&path, &tmp)?;
                std::fs::rename(&tmp, &blob)?;
            }
        }
        let m = ModelManifest {
            schema_version: ModelManifest::SCHEMA_VERSION,
            id,
            name: file_name.clone(),
            file_name,
            sha256,
            bytes,
            license: license.unwrap_or("unknown").to_string(),
            license_url: None,
            source_url: format!("file://{}", path.display()),
            redistribution: "unknown".into(),
            imported_at_utc: utc_now(),
            chat: false,
            tags: Vec::new(),
        };
        self.write_manifest(&m)?;
        Ok(m)
    }

    /// Atomic write (temp file + rename) of `models/manifests/<id>.json`.
    pub fn write_manifest(&self, m: &ModelManifest) -> Result<()> {
        check_id(&m.id)?;
        let dir = self.paths.model_manifests();
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join(format!("{}.json.tmp", m.id));
        std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
        std::fs::rename(&tmp, self.manifest_path(&m.id))?;
        Ok(())
    }

    /// Delete the manifest, and the blob when no other manifest references its hash.
    pub fn remove(&self, id: &str) -> Result<()> {
        let m = self
            .get(id)?
            .ok_or_else(|| UComputeError::new(ErrorCode::ModelNotFound, format!("no manifest for model '{id}'")))?;
        std::fs::remove_file(self.manifest_path(id))?;
        if !self.list()?.iter().any(|o| o.sha256 == m.sha256) {
            match std::fs::remove_file(self.blob_path(&m.sha256)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// Re-hash the blob and compare with the manifest (F-05-01).
    pub fn verify(&self, id: &str) -> Result<()> {
        let (m, blob) = self.resolve(id)?;
        let actual = sha256_file(&blob)?;
        if actual != m.sha256 {
            return Err(UComputeError::new(
                ErrorCode::IntegrityMismatch,
                format!(
                    "model '{id}': blob sha256 {actual} does not match manifest {}",
                    m.sha256
                ),
            ));
        }
        Ok(())
    }
}

fn check_id(id: &str) -> Result<()> {
    if safe_name(id) {
        Ok(())
    } else {
        Err(UComputeError::invalid(format!("invalid model id {id:?}")))
    }
}

fn read_manifest(path: &Path) -> Result<ModelManifest> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| UComputeError::io(format!("manifest {} is not valid: {e}", path.display())))
}

/// Current time as `YYYY-MM-DDTHH:MM:SSZ` from `std::time` (no chrono).
pub fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// Civil date from Unix seconds (Howard Hinnant's `civil_from_days`).
pub fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::sha256_bytes;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(Paths::at(dir.path().join("home")));
        (dir, s)
    }

    #[test]
    fn date_formatting() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_utc(4_102_444_799), "2099-12-31T23:59:59Z");
        assert!(utc_now().starts_with("20"));
    }

    #[test]
    fn empty_store_lists_nothing() {
        let (_d, s) = store();
        assert!(s.list().unwrap().is_empty());
        assert!(s.get("nope").unwrap().is_none());
        assert_eq!(s.verify("nope").unwrap_err().code, ErrorCode::ModelNotFound);
        assert_eq!(s.remove("nope").unwrap_err().code, ErrorCode::ModelNotFound);
    }

    #[test]
    fn unsafe_ids_rejected() {
        let (_d, s) = store();
        for id in ["../x", "a/b", ".hidden", ""] {
            assert_eq!(s.get(id).unwrap_err().code, ErrorCode::InvalidRequest, "{id}");
        }
        let f = _d.path().join("..weird.gguf");
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(s.import(&f, None, None).unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn import_list_get_resolve_verify_remove() {
        let (d, s) = store();
        let f = d.path().join("tiny-model.gguf");
        std::fs::write(&f, b"GGUF fake weights").unwrap();
        let m = s.import(&f, None, None).unwrap();
        assert_eq!(m.id, "tiny-model");
        assert_eq!(m.license, "unknown");
        assert_eq!(m.redistribution, "unknown");
        assert_eq!(m.bytes, 17);
        assert_eq!(m.sha256, sha256_bytes(b"GGUF fake weights"));
        assert!(m.imported_at_utc.ends_with('Z') && m.imported_at_utc.len() == 20);
        assert!(s.blob_path(&m.sha256).is_file());
        assert!(s.manifest_path("tiny-model").is_file());

        // original untouched
        assert_eq!(std::fs::read(&f).unwrap(), b"GGUF fake weights");

        let m2 = s.import(&f, Some("alias"), Some("MIT")).unwrap();
        assert_eq!(m2.license, "MIT");
        assert_eq!(m2.sha256, m.sha256);
        let ids: Vec<_> = s.list().unwrap().into_iter().map(|m| m.id).collect();
        assert_eq!(ids, ["alias", "tiny-model"]);
        assert_eq!(s.get("alias").unwrap().unwrap(), m2);

        let (rm, blob) = s.resolve("alias").unwrap();
        assert_eq!(rm, m2);
        assert_eq!(blob, s.blob_path(&m.sha256));
        s.verify("alias").unwrap();
        s.verify("tiny-model").unwrap();

        // same id, different content -> refused
        let g = d.path().join("other.gguf");
        std::fs::write(&g, b"different").unwrap();
        assert_eq!(
            s.import(&g, Some("alias"), None).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        // re-import same content under same id is fine
        s.import(&f, Some("alias"), None).unwrap();

        // blob shared by two manifests survives removing one
        s.remove("alias").unwrap();
        assert!(s.blob_path(&m.sha256).is_file());
        assert!(s.get("alias").unwrap().is_none());
        s.remove("tiny-model").unwrap();
        assert!(!s.blob_path(&m.sha256).exists());
        assert!(s.list().unwrap().is_empty());
    }

    #[test]
    fn verify_detects_corruption_and_missing_blob() {
        let (d, s) = store();
        let f = d.path().join("m.gguf");
        std::fs::write(&f, b"original").unwrap();
        let m = s.import(&f, None, None).unwrap();
        // flip one byte in the blob (the blob is a hard link; break the link first)
        let blob = s.blob_path(&m.sha256);
        std::fs::remove_file(&blob).unwrap();
        std::fs::write(&blob, b"originaL").unwrap();
        assert_eq!(s.verify("m").unwrap_err().code, ErrorCode::IntegrityMismatch);
        std::fs::remove_file(&blob).unwrap();
        assert_eq!(s.verify("m").unwrap_err().code, ErrorCode::ModelNotDownloaded);
        assert_eq!(s.resolve("m").unwrap_err().code, ErrorCode::ModelNotDownloaded);
    }

    #[test]
    fn corrupt_manifest_is_an_io_error() {
        let (_d, s) = store();
        std::fs::create_dir_all(s.paths.model_manifests()).unwrap();
        std::fs::write(s.manifest_path("bad"), b"{not json").unwrap();
        std::fs::write(s.paths.model_manifests().join("notes.txt"), b"ignored").unwrap();
        assert_eq!(s.get("bad").unwrap_err().code, ErrorCode::Io);
        assert_eq!(s.list().unwrap_err().code, ErrorCode::Io);
    }

    #[test]
    fn manifest_roundtrip_defaults() {
        let v = serde_json::json!({
            "schema_version": 1, "id": "x", "name": "x", "file_name": "x.gguf",
            "sha256": "00", "bytes": 1, "license": "unknown", "source_url": "https://e", "imported_at_utc": "2026-10-14T00:00:00Z"
        });
        let m: ModelManifest = serde_json::from_value(v).unwrap();
        assert_eq!(m.redistribution, "unknown");
        assert!(!m.chat && m.tags.is_empty() && m.license_url.is_none());
    }
}
