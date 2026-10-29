//! Built-in model catalog, embedded from `catalog/models.json` at the repo root.

use serde::{Deserialize, Serialize};
use ucompute_core::Result;

pub const CATALOG_JSON: &str = include_str!("../../../catalog/models.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    /// Stable id, safe as a file name (see `ucompute_core::paths::safe_name`).
    pub id: String,
    pub name: String,
    pub file_name: String,
    /// Immutable URL (pin to a revision, not `main`).
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    /// SPDX id or free text; `"unknown"` when not established (never omitted, F-05-04).
    pub license: String,
    #[serde(default)]
    pub license_url: Option<String>,
    pub source: String,
    #[serde(default)]
    pub family: Option<String>,
    #[serde(default)]
    pub parameters: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    /// True when the model has a chat template and is meant for conversation.
    #[serde(default)]
    pub chat: bool,
    /// `allowed` | `disallowed` | `unknown` — governs peer seeding later (X-08).
    #[serde(default = "unknown")]
    pub redistribution: String,
    #[serde(default)]
    pub notes: Option<String>,
    /// Tags such as `fixture`, `recommended`, `embedding`.
    #[serde(default)]
    pub tags: Vec<String>,
}

fn unknown() -> String {
    "unknown".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Catalog {
    pub schema_version: u32,
    pub models: Vec<ModelEntry>,
}

impl Catalog {
    pub fn builtin() -> Result<Catalog> {
        Ok(serde_json::from_str(CATALOG_JSON)?)
    }
    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }
    /// The models tests load: those tagged `fixture` (two tiny ones, always provisioned) and, with
    /// `suite`, those tagged `test-suite` (about 3 GB), which the suite also needs under
    /// `UCOMPUTE_REQUIRE_FIXTURES=1`. `scripts/fetch-fixtures.sh` selects the same tags.
    pub fn fixtures(&self, suite: bool) -> Vec<&ModelEntry> {
        self.models
            .iter()
            .filter(|m| m.tags.iter().any(|t| t == "fixture" || (suite && t == "test-suite")))
            .collect()
    }
    pub fn search(&self, query: &str) -> Vec<&ModelEntry> {
        let q = query.to_lowercase();
        self.models
            .iter()
            .filter(|m| {
                m.id.to_lowercase().contains(&q)
                    || m.name.to_lowercase().contains(&q)
                    || m.family
                        .as_deref()
                        .map(|f| f.to_lowercase().contains(&q))
                        .unwrap_or(false)
                    || m.tags.iter().any(|t| t.to_lowercase().contains(&q))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::is_sha256_hex;
    use crate::store::Store;
    use std::path::{Path, PathBuf};
    use ucompute_core::paths::{safe_name, Paths};

    #[test]
    fn builtin_catalog_is_well_formed() {
        let c = Catalog::builtin().unwrap();
        assert_eq!(c.schema_version, 1);
        assert!(c.models.len() >= 8, "{}", c.models.len());
        let mut ids = std::collections::HashSet::new();
        for m in &c.models {
            assert!(safe_name(&m.id), "id {:?}", m.id);
            assert!(ids.insert(&m.id), "duplicate id {}", m.id);
            assert!(safe_name(&m.file_name), "file_name {:?}", m.file_name);
            assert!(m.url.starts_with("https://"), "{}: url", m.id);
            assert!(!m.url.contains("/main/"), "{}: url must pin a revision", m.id);
            assert!(is_sha256_hex(&m.sha256), "{}: sha256", m.id);
            assert!(m.bytes > 0, "{}: bytes", m.id);
            assert!(
                !m.license.is_empty() && !m.name.is_empty() && !m.source.is_empty(),
                "{}",
                m.id
            );
            assert!(
                ["allowed", "disallowed", "unknown"].contains(&m.redistribution.as_str()),
                "{}: redistribution",
                m.id
            );
        }
        assert_eq!(c.get("stories15M-q4_0").unwrap().quantization.as_deref(), Some("Q4_0"));
        assert!(c.get("missing").is_none());
        assert!(c.search("qwen").len() >= 4);
        assert!(c
            .search("EMBEDDING")
            .iter()
            .any(|m| m.id == "nomic-embed-text-v1.5-q8_0"));
        assert!(c.search("fixture").iter().any(|m| m.id == "stories260K"));
    }

    /// Every catalog model a test file names is provisioned with the suite's fixtures, so
    /// `fetch-fixtures.sh` / `ucompute setup --fixtures` under `UCOMPUTE_REQUIRE_FIXTURES=1` give the
    /// strict suite all it loads.
    #[test]
    fn suite_fixtures_cover_every_model_the_tests_name() {
        let c = Catalog::builtin().unwrap();
        let ids = |suite| c.fixtures(suite).into_iter().map(|m| m.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids(false), ["stories15M-q4_0", "stories260K"]);
        let suite = ids(true);
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut named = std::collections::BTreeSet::new();
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&crates)
            .unwrap()
            .map(|e| e.unwrap().path().join("tests"))
            .filter(|d| d.is_dir())
            .collect();
        while let Some(dir) = dirs.pop() {
            for f in std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()) {
                if f.is_dir() {
                    dirs.push(f);
                } else if f.extension().is_some_and(|x| x == "rs") {
                    let src = std::fs::read_to_string(&f).unwrap();
                    named.extend(
                        c.models
                            .iter()
                            .filter(|m| src.contains(&format!("\"{}\"", m.id)))
                            .map(|m| m.id.as_str()),
                    );
                }
            }
        }
        assert!(
            named.contains("smollm2-360m-instruct-q8_0"),
            "the walk found the test files: {named:?}"
        );
        let missing: Vec<_> = named.iter().filter(|id| !suite.contains(id)).collect();
        assert!(
            missing.is_empty(),
            "tests load {missing:?}; tag them test-suite in catalog/models.json"
        );
    }

    #[test]
    fn entry_defaults() {
        let v = serde_json::json!({
            "id": "x", "name": "x", "file_name": "x.gguf", "url": "https://e/x", "sha256": "00", "bytes": 1,
            "license": "unknown", "source": "s"
        });
        let e: ModelEntry = serde_json::from_value(v).unwrap();
        assert_eq!(e.redistribution, "unknown");
        assert!(!e.chat && e.tags.is_empty() && e.family.is_none());
    }

    /// Cached manifests in UCOMPUTE_HOME (or the repo's `.ucompute`) must agree with the catalog.
    #[test]
    fn ucompute_home_manifests_match_catalog() {
        let root = std::env::var_os("UCOMPUTE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.ucompute"));
        let store = Store::new(Paths::at(root));
        if !store.paths.model_manifests().is_dir() {
            assert!(
                std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
                "UCOMPUTE_REQUIRE_FIXTURES=1: no manifests directory in {}",
                store.paths.root.display()
            );
            eprintln!("SKIP: no UCOMPUTE_HOME manifests directory");
            return;
        }
        let catalog = Catalog::builtin().unwrap();
        let manifests = store.list().unwrap();
        let mut checked = 0;
        for m in &manifests {
            let Some(e) = catalog.get(&m.id) else { continue };
            assert_eq!(m.sha256, e.sha256, "{}: sha256", m.id);
            assert_eq!(m.bytes, e.bytes, "{}: bytes", m.id);
            assert_eq!(m.file_name, e.file_name, "{}: file_name", m.id);
            assert_eq!(m.license, e.license, "{}: license", m.id);
            assert!(store.blob_path(&m.sha256).is_file(), "{}: blob", m.id);
            assert_eq!(
                std::fs::metadata(store.blob_path(&m.sha256)).unwrap().len(),
                e.bytes,
                "{}: blob size",
                m.id
            );
            checked += 1;
        }
        eprintln!(
            "cross-checked {checked} of {} manifests against the catalog",
            manifests.len()
        );
    }
}
