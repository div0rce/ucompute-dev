//! Audit trail and redacted diagnostics (F-06-10). STUB — implementation pending.
//!
//! `audit.jsonl` (0600) under `COMMON_HOME/logs/` records every off-device action: pairing,
//! peer connect/disconnect/reject, revocation, remote requests, sessions. Never prompt text,
//! token strings or key material. The diagnostics bundle redacts prompts, API keys and private
//! keys and is scanned by a test before it is considered shareable.

use common_core::{Paths, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditEvent {
    pub ts_utc: String,
    /// e.g. `pairing_succeeded`, `peer_rejected`, `remote_request`, `peer_revoked`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer_id: Option<String>,
    /// Structured, redaction-safe fields (sizes, ids, reasons, durations). Never prompt text.
    #[serde(default)]
    pub fields: serde_json::Value,
}

pub struct Audit {
    pub path: std::path::PathBuf,
}

impl Audit {
    pub fn open(paths: &Paths) -> Result<Audit> {
        Ok(Audit {
            path: paths.logs().join("audit.jsonl"),
        })
    }
    /// Append one JSON line (0600 file). Rejects events whose fields contain keys named like
    /// `prompt`, `content`, `messages`, `api_key`, `private_key`.
    pub fn record(&self, kind: &str, peer_id: Option<&str>, fields: serde_json::Value) -> Result<()> {
        let _ = (kind, peer_id, fields);
        Err(common_core::CommonError::new(
            common_core::ErrorCode::NotImplemented,
            "audit",
        ))
    }
    pub fn tail(&self, n: usize) -> Result<Vec<AuditEvent>> {
        let _ = n;
        Ok(vec![])
    }
}

/// Redacted diagnostics bundle (JSON): version, hardware, status, config (no keys), audit tail,
/// scrubbed engine log tails. `redact()` is the single place that knows the secret patterns.
pub fn redact(text: &str) -> String {
    text.to_string()
}
pub fn bundle(paths: &Paths, extra: serde_json::Value) -> Result<serde_json::Value> {
    let _ = (paths, extra);
    Ok(serde_json::json!({}))
}
