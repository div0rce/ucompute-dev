//! Policy plane (ADR-0003, spec section 9). STUB — implementation pending.
//!
//! Every routing decision passes through `PolicyEngine::evaluate` (I-05). Defaults: privacy
//! `LocalOnly`, network disabled, contribution disabled: with default config no request leaves
//! the device and no external workload is accepted (F-06-02, F-07-01).

use common_core::{ErrorCode, Result};
use common_identity::{PeerId, PeerStore};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    /// Never leaves this device.
    #[default]
    LocalOnly,
    /// May run on paired devices in a trusted group.
    TrustedGroup,
    /// May run on opt-in public peers (gated; disabled until Gate F).
    SwarmAllowed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PolicyConfig {
    #[serde(default)]
    pub default_privacy: PrivacyMode,
    /// Peer networking listener enabled (default false).
    #[serde(default)]
    pub network_enabled: bool,
    /// Accept workloads from trusted peers (default false).
    #[serde(default)]
    pub serve_peers: bool,
    /// Public swarm participation (always false until Gate F; cannot be enabled by config in M3).
    #[serde(default)]
    pub swarm_enabled: bool,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            default_privacy: PrivacyMode::LocalOnly,
            network_enabled: false,
            serve_peers: false,
            swarm_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionTarget {
    Local,
    Peer(PeerId),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadKind {
    /// A whole inference request executed on a peer (Mode B).
    RemoteInference,
    /// A layer range hosted for / by a peer (Mode C, M4).
    PooledInference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny { code: ErrorCode, reason: String },
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }
    pub fn into_result(self) -> Result<()> {
        match self {
            Decision::Allow => Ok(()),
            Decision::Deny { code, reason } => Err(common_core::CommonError::new(code, reason)),
        }
    }
}

pub struct PolicyEngine {
    pub config: PolicyConfig,
    pub peers: Arc<PeerStore>,
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig, peers: Arc<PeerStore>) -> Self {
        Self { config, peers }
    }

    /// May a request with `privacy` be sent to `target`? Local is always allowed. Peer targets require
    /// `privacy != LocalOnly`, a pinned, non-revoked peer sharing a group whose policy allows `kind`.
    pub fn evaluate(&self, privacy: PrivacyMode, target: ExecutionTarget, kind: WorkloadKind) -> Decision {
        let _ = (privacy, target, kind);
        Decision::Deny {
            code: ErrorCode::NotImplemented,
            reason: "policy engine not implemented".into(),
        }
    }

    /// May this device serve `kind` for `peer`? Requires `serve_peers`, a pinned non-revoked peer and a
    /// shared group allowing `kind`. Default deny.
    pub fn may_serve(&self, peer: &PeerId, kind: WorkloadKind) -> Decision {
        let _ = (peer, kind);
        Decision::Deny {
            code: ErrorCode::NotImplemented,
            reason: "policy engine not implemented".into(),
        }
    }
}

/// Token-bucket quotas per peer (F-06-04). STUB.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaConfig {
    pub requests_per_minute: u32,
    pub tokens_per_minute: u64,
    pub bytes_per_second: u64,
    pub max_concurrent: u32,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 120,
            tokens_per_minute: 200_000,
            bytes_per_second: 0,
            max_concurrent: 4,
        }
    }
}

pub struct Quotas;
impl Quotas {
    pub fn new(cfg: QuotaConfig) -> Self {
        let _ = cfg;
        Quotas
    }
    /// Returns Err(Overloaded) with a typed message when the peer exceeds its bucket.
    pub fn admit_request(&self, peer: &PeerId) -> Result<()> {
        let _ = peer;
        Ok(())
    }
    pub fn charge_tokens(&self, peer: &PeerId, n: u64) -> Result<()> {
        let _ = (peer, n);
        Ok(())
    }
}
