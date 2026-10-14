//! Device identity and the trust store (ADR-0003 D1, D7). STUB — implementation pending.
//!
//! - One Ed25519 keypair per device, generated and used through `ring` (same provider as TLS).
//! - `PeerId` = the raw 32-byte public key; shown to users as 64 lowercase hex (8 groups of 8),
//!   short label = first 12 hex as `xxxx-xxxx-xxxx` (labels only, never a trust decision).
//! - Storage: `COMMON_HOME/identity/` (dir 0700): `device.key` (PKCS#8 v2 DER, 0600, tmp+rename),
//!   `device.pub` (64 hex), `peers.json` (0600; pinned peers, groups, revocations).
//!   // ponytail: JSON trust store; move to SQLite when groups/audit need queries.
//! - The secret never implements Serialize; Debug prints `Ed25519Secret(<redacted>)`.

use common_core::{CommonError, ErrorCode, Paths, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Raw Ed25519 public key. Serialized as 64 lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
    /// `xxxx-xxxx-xxxx` from the first 12 hex chars. Display only.
    pub fn short_label(&self) -> String {
        let h = self.to_hex();
        format!("{}-{}-{}", &h[0..4], &h[4..8], &h[8..12])
    }
    /// 8 groups of 8 hex, for the pairing confirmation screen.
    pub fn grouped(&self) -> String {
        let h = self.to_hex();
        (0..8).map(|i| &h[i * 8..i * 8 + 8]).collect::<Vec<_>>().join(" ")
    }
    pub fn parse_hex(s: &str) -> Result<PeerId> {
        let bytes = hex::decode(s.trim()).map_err(|_| CommonError::invalid("peer id must be 64 hex characters"))?;
        let arr: [u8; 32] = bytes
            .try_into()
            .map_err(|_| CommonError::invalid("peer id must be 32 bytes"))?;
        Ok(PeerId(arr))
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}
impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", self.short_label())
    }
}
impl Serialize for PeerId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}
impl<'de> Deserialize<'de> for PeerId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        PeerId::parse_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// The device's own keypair. The secret is never serialized or logged.
pub struct DeviceIdentity {
    pkcs8: Vec<u8>,
    pub id: PeerId,
}

impl fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DeviceIdentity({}, Ed25519Secret(<redacted>))",
            self.id.short_label()
        )
    }
}

impl DeviceIdentity {
    /// Load `identity/device.key` or generate + persist a new keypair (dir 0700, file 0600).
    pub fn load_or_create(paths: &Paths) -> Result<DeviceIdentity> {
        let _ = paths;
        Err(CommonError::new(ErrorCode::NotImplemented, "identity"))
    }
    /// Generate an in-memory identity (tests, ephemeral pairing endpoints).
    pub fn generate() -> Result<DeviceIdentity> {
        Err(CommonError::new(ErrorCode::NotImplemented, "identity"))
    }
    /// PKCS#8 v2 DER for rustls/quinn.
    pub fn pkcs8_der(&self) -> &[u8] {
        &self.pkcs8
    }
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let _ = msg;
        [0u8; 64]
    }
}

/// Verify an Ed25519 signature made by `peer`.
pub fn verify(peer: &PeerId, msg: &[u8], sig: &[u8]) -> bool {
    let _ = (peer, msg, sig);
    false
}

/// Per-group policy (F-02-09). Default deny everything (F-06-02).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GroupPolicy {
    /// Members may run whole inference requests on this device (Mode B).
    #[serde(default)]
    pub allow_remote_inference: bool,
    /// Members may host layer ranges for this device / use this device's layers (Mode C, M4).
    #[serde(default)]
    pub allow_pooled_inference: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Group {
    pub name: String,
    pub members: Vec<PeerId>,
    #[serde(default)]
    pub policy: GroupPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PeerRecord {
    pub id: PeerId,
    pub name: String,
    pub paired_at_utc: String,
    #[serde(default)]
    pub last_seen_utc: Option<String>,
    /// Last known address (ip:port) for reconnects; advisory.
    #[serde(default)]
    pub last_addr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Revocation {
    pub seq: u64,
    pub peer_id: PeerId,
    pub revoked_by: PeerId,
    pub at_utc: String,
    /// Ed25519 signature by `revoked_by` over `seq || peer_id || at_utc` (hex).
    pub signature: String,
}

/// Pinned peers, groups and revocations. Persisted to `identity/peers.json` (0600) atomically.
#[derive(Debug)]
pub struct PeerStore {
    pub path: std::path::PathBuf,
}

impl PeerStore {
    pub fn open(paths: &Paths) -> Result<PeerStore> {
        Ok(PeerStore {
            path: paths.identity().join("peers.json"),
        })
    }
    pub fn list(&self) -> Result<Vec<PeerRecord>> {
        Err(todo())
    }
    pub fn get(&self, id: &PeerId) -> Result<Option<PeerRecord>> {
        let _ = id;
        Err(todo())
    }
    pub fn is_pinned(&self, id: &PeerId) -> bool {
        let _ = id;
        false
    }
    pub fn pin(&self, record: PeerRecord) -> Result<()> {
        let _ = record;
        Err(todo())
    }
    pub fn remove(&self, id: &PeerId) -> Result<()> {
        let _ = id;
        Err(todo())
    }
    pub fn groups(&self) -> Result<Vec<Group>> {
        Err(todo())
    }
    pub fn upsert_group(&self, group: Group) -> Result<()> {
        let _ = group;
        Err(todo())
    }
    pub fn groups_of(&self, id: &PeerId) -> Result<Vec<Group>> {
        let _ = id;
        Err(todo())
    }
    /// Remove the pin, record a signed revocation, return it for propagation (F-06-05).
    pub fn revoke(&self, id: &PeerId, by: &DeviceIdentity) -> Result<Revocation> {
        let _ = (id, by);
        Err(todo())
    }
    /// Apply a revocation received from a group member (signature verified against a pinned key).
    pub fn apply_revocation(&self, r: &Revocation) -> Result<()> {
        let _ = r;
        Err(todo())
    }
    pub fn is_revoked(&self, id: &PeerId) -> bool {
        let _ = id;
        false
    }
    pub fn revocations_since(&self, seq: u64) -> Result<Vec<Revocation>> {
        let _ = seq;
        Err(todo())
    }
}

fn todo() -> CommonError {
    CommonError::new(ErrorCode::NotImplemented, "peer store")
}
