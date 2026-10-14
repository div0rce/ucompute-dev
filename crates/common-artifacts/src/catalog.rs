//! Built-in model catalog, embedded from `catalog/models.json` at the repo root.

use common_core::Result;
use serde::{Deserialize, Serialize};

pub const CATALOG_JSON: &str = include_str!("../../../catalog/models.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelEntry {
    /// Stable id, safe as a file name (see `common_core::paths::safe_name`).
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

fn unknown() -> String { "unknown".into() }

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
    pub fn search(&self, query: &str) -> Vec<&ModelEntry> {
        let q = query.to_lowercase();
        self.models.iter().filter(|m| {
            m.id.to_lowercase().contains(&q) || m.name.to_lowercase().contains(&q)
                || m.family.as_deref().map(|f| f.to_lowercase().contains(&q)).unwrap_or(false)
                || m.tags.iter().any(|t| t.to_lowercase().contains(&q))
        }).collect()
    }
}
