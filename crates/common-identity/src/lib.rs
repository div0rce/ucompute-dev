//! Device identity and the trust store (ADR-0003 D1, D7).
//!
//! - One Ed25519 keypair per device, generated and used through `ring` (same provider as TLS).
//! - `PeerId` = the raw 32-byte public key; shown to users as 64 lowercase hex (8 groups of 8),
//!   short label = first 12 hex as `xxxx-xxxx-xxxx` (labels only, never a trust decision).
//! - Storage: `COMMON_HOME/identity/` (dir 0700): `device.key` (PKCS#8 v2 DER, 0600, tmp+rename),
//!   `device.pub` (64 hex), `peers.json` (0600; pinned peers, groups, revocations).
//!   // ponytail: JSON trust store; move to SQLite when groups/audit need queries.
//! - The secret never implements Serialize; Debug prints `Ed25519Secret(<redacted>)`.
//!
//! Revocation semantics (F-06-05): `revoke` removes the pin and group memberships, appends a
//! signed `Revocation` and returns it for propagation. A revoked id is refused by `pin` until a
//! fresh pairing calls `pin_after_pairing`, which clears the local revocation. Trust is per store:
//! other group members keep their copy of the revocation until they re-pair too (household model).

use common_core::{CommonError, ErrorCode, Paths, Result};
use ring::signature::{self, KeyPair as _};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Raw Ed25519 public key. Serialized as 64 lowercase hex.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PeerId(pub [u8; 32]);

/// RFC 8410 SubjectPublicKeyInfo prefix for Ed25519 (`id-Ed25519` OID 1.3.101.112).
const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

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
    /// DER SubjectPublicKeyInfo (44 bytes), the form TLS raw public keys (RFC 7250) carry.
    pub fn to_spki_der(&self) -> Vec<u8> {
        let mut v = ED25519_SPKI_PREFIX.to_vec();
        v.extend_from_slice(&self.0);
        v
    }
    /// Parse an Ed25519 SubjectPublicKeyInfo; anything else (other algorithms, X.509) is rejected.
    pub fn from_spki_der(der: &[u8]) -> Result<PeerId> {
        let key = der
            .strip_prefix(&ED25519_SPKI_PREFIX[..])
            .filter(|k| k.len() == 32)
            .ok_or_else(|| {
                CommonError::new(ErrorCode::PeerUnauthorized, "peer key is not an Ed25519 raw public key")
            })?;
        Ok(PeerId(key.try_into().expect("checked length")))
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
///
/// There is no `Serialize` impl, by construction:
/// ```compile_fail
/// let id = common_identity::DeviceIdentity::generate().unwrap();
/// let _ = serde_json::to_string(&id).unwrap();
/// ```
pub struct DeviceIdentity {
    // ponytail: pkcs8 bytes are not zeroized on drop; add `zeroize` when the dependency audit allows it.
    pkcs8: Vec<u8>,
    key: signature::Ed25519KeyPair,
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
    /// `device.pub` is rewritten whenever it does not match the key.
    pub fn load_or_create(paths: &Paths) -> Result<DeviceIdentity> {
        let dir = paths.identity();
        ensure_private_dir(&dir)?;
        let key_path = dir.join("device.key");
        let pub_path = dir.join("device.pub");
        let identity = if key_path.is_file() {
            tighten_mode(&key_path)?;
            Self::from_pkcs8(std::fs::read(&key_path)?).map_err(|e| {
                CommonError::io(format!(
                    "{} is not a valid Ed25519 PKCS#8 key: {}",
                    key_path.display(),
                    e.message
                ))
            })?
        } else {
            let id = Self::generate()?;
            write_private(&key_path, &id.pkcs8)?;
            tracing::info!(device = %id.id.short_label(), "generated new device identity");
            id
        };
        let hex = identity.id.to_hex();
        let current = std::fs::read_to_string(&pub_path).unwrap_or_default();
        if current.trim() != hex {
            write_private(&pub_path, format!("{hex}\n").as_bytes())?;
        }
        Ok(identity)
    }
    /// Generate an in-memory identity (tests, ephemeral pairing endpoints).
    pub fn generate() -> Result<DeviceIdentity> {
        let rng = ring::rand::SystemRandom::new();
        let doc = signature::Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| CommonError::internal("Ed25519 key generation failed"))?;
        Self::from_pkcs8(doc.as_ref().to_vec())
    }
    fn from_pkcs8(pkcs8: Vec<u8>) -> Result<DeviceIdentity> {
        let key = signature::Ed25519KeyPair::from_pkcs8(&pkcs8)
            .map_err(|_| CommonError::invalid("not a PKCS#8 v2 Ed25519 key"))?;
        let id = PeerId(
            key.public_key()
                .as_ref()
                .try_into()
                .expect("ed25519 public key is 32 bytes"),
        );
        Ok(DeviceIdentity { pkcs8, key, id })
    }
    /// PKCS#8 v2 DER for rustls/quinn.
    pub fn pkcs8_der(&self) -> &[u8] {
        &self.pkcs8
    }
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.key
            .sign(msg)
            .as_ref()
            .try_into()
            .expect("ed25519 signature is 64 bytes")
    }
}

/// Verify an Ed25519 signature made by `peer`.
pub fn verify(peer: &PeerId, msg: &[u8], sig: &[u8]) -> bool {
    signature::UnparsedPublicKey::new(&signature::ED25519, peer.0)
        .verify(msg, sig)
        .is_ok()
}

/// Current time as `YYYY-MM-DDTHH:MM:SSZ` from `std::time` (no chrono). Used for `paired_at_utc`
/// and revocation timestamps.
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

// ---- private file helpers (0700 dir, 0600 files, tmp+rename) ----

fn ensure_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write `bytes` to `path` with mode 0600 via `<path>.tmp` + rename.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let tmp = path.with_file_name(format!(
        "{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&tmp)?;
    tighten_mode(&tmp)?; // a leftover tmp keeps its old mode; `mode()` only applies on create
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Force 0600 on an existing file (loosened by a copy, umask or an older build).
fn tighten_mode(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)?.permissions().mode() & 0o777;
        if mode != 0o600 {
            tracing::warn!(path = %path.display(), mode = format!("{mode:o}"), "tightening permissions to 0600");
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let _ = path;
    Ok(())
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
    /// Ed25519 signature by `revoked_by` over [`revocation_message`] (hex).
    pub signature: String,
}

/// The bytes a revocation signs: `"common-revocation/1" || seq (u64 BE) || peer_id || at_utc`.
/// The prefix separates these signatures from every other use of the device key.
pub fn revocation_message(seq: u64, peer_id: &PeerId, at_utc: &str) -> Vec<u8> {
    let mut m = b"common-revocation/1".to_vec();
    m.extend_from_slice(&seq.to_be_bytes());
    m.extend_from_slice(&peer_id.0);
    m.extend_from_slice(at_utc.as_bytes());
    m
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct StoreData {
    #[serde(default)]
    peers: Vec<PeerRecord>,
    #[serde(default)]
    groups: Vec<Group>,
    #[serde(default)]
    revocations: Vec<Revocation>,
    #[serde(default = "first_seq")]
    next_seq: u64,
}
fn first_seq() -> u64 {
    1
}
impl Default for StoreData {
    fn default() -> Self {
        Self {
            peers: vec![],
            groups: vec![],
            revocations: vec![],
            next_seq: first_seq(),
        }
    }
}
impl StoreData {
    fn is_pinned(&self, id: &PeerId) -> bool {
        self.peers.iter().any(|p| &p.id == id)
    }
    fn is_revoked(&self, id: &PeerId) -> bool {
        self.revocations.iter().any(|r| &r.peer_id == id)
    }
    /// Drop the pin and every group membership.
    fn unpin(&mut self, id: &PeerId) {
        self.peers.retain(|p| &p.id != id);
        for g in &mut self.groups {
            g.members.retain(|m| m != id);
        }
    }
    fn upsert_peer(&mut self, record: PeerRecord) {
        match self.peers.iter_mut().find(|p| p.id == record.id) {
            Some(p) => *p = record,
            None => self.peers.push(record),
        }
    }
}

/// Pinned peers, groups and revocations. Persisted to `identity/peers.json` (0600) atomically.
/// All methods take `&self` (the network crate shares it behind `Arc`); `is_pinned`/`is_revoked`
/// read memory only, so the TLS verifier never touches the disk.
#[derive(Debug)]
pub struct PeerStore {
    pub path: PathBuf,
    data: Mutex<StoreData>,
}

impl PeerStore {
    pub fn open(paths: &Paths) -> Result<PeerStore> {
        let dir = paths.identity();
        ensure_private_dir(&dir)?;
        let path = dir.join("peers.json");
        let data = if path.is_file() {
            tighten_mode(&path)?;
            serde_json::from_slice(&std::fs::read(&path)?)
                .map_err(|e| CommonError::io(format!("{} is not valid: {e}", path.display())))?
        } else {
            StoreData::default()
        };
        Ok(PeerStore {
            path,
            data: Mutex::new(data),
        })
    }

    fn read<T>(&self, f: impl FnOnce(&StoreData) -> T) -> T {
        f(&self.data.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Mutate a copy, persist it, then commit: a failed write leaves memory and disk unchanged.
    fn mutate<T>(&self, f: impl FnOnce(&mut StoreData) -> Result<T>) -> Result<T> {
        let mut guard = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let mut next = guard.clone();
        let out = f(&mut next)?;
        write_private(&self.path, &serde_json::to_vec_pretty(&next)?)?;
        *guard = next;
        Ok(out)
    }

    pub fn list(&self) -> Result<Vec<PeerRecord>> {
        Ok(self.read(|d| d.peers.clone()))
    }
    pub fn get(&self, id: &PeerId) -> Result<Option<PeerRecord>> {
        Ok(self.read(|d| d.peers.iter().find(|p| &p.id == id).cloned()))
    }
    pub fn is_pinned(&self, id: &PeerId) -> bool {
        self.read(|d| d.is_pinned(id))
    }
    /// Pin (or update) a peer. A revoked id is refused with `PeerRevoked`; only a new pairing
    /// (`pin_after_pairing`) restores it.
    pub fn pin(&self, record: PeerRecord) -> Result<()> {
        self.mutate(|d| {
            if d.is_revoked(&record.id) {
                return Err(CommonError::new(
                    ErrorCode::PeerRevoked,
                    format!(
                        "device {} was revoked; pair it again to restore trust",
                        record.id.short_label()
                    ),
                ));
            }
            d.upsert_peer(record);
            Ok(())
        })
    }
    /// Pin after a successful pairing: clears any local revocation of this id first. Only the
    /// pairing flow (which proved possession of the key and the code) may call this.
    pub fn pin_after_pairing(&self, record: PeerRecord) -> Result<()> {
        self.mutate(|d| {
            d.revocations.retain(|r| r.peer_id != record.id);
            d.upsert_peer(record);
            Ok(())
        })
    }
    /// Remove the pin and group memberships without revoking (idempotent).
    pub fn remove(&self, id: &PeerId) -> Result<()> {
        self.mutate(|d| {
            d.unpin(id);
            Ok(())
        })
    }
    pub fn groups(&self) -> Result<Vec<Group>> {
        Ok(self.read(|d| d.groups.clone()))
    }
    /// Insert or replace the group with this name.
    pub fn upsert_group(&self, group: Group) -> Result<()> {
        if group.name.trim().is_empty() {
            return Err(CommonError::invalid("group name must not be empty"));
        }
        self.mutate(|d| {
            match d.groups.iter_mut().find(|g| g.name == group.name) {
                Some(g) => *g = group,
                None => d.groups.push(group),
            }
            Ok(())
        })
    }
    pub fn groups_of(&self, id: &PeerId) -> Result<Vec<Group>> {
        Ok(self.read(|d| d.groups.iter().filter(|g| g.members.contains(id)).cloned().collect()))
    }
    /// Remove the pin, record a signed revocation, return it for propagation (F-06-05).
    pub fn revoke(&self, id: &PeerId, by: &DeviceIdentity) -> Result<Revocation> {
        self.mutate(|d| {
            let seq = d.next_seq;
            d.next_seq += 1;
            let at_utc = utc_now();
            let signature = hex::encode(by.sign(&revocation_message(seq, id, &at_utc)));
            let r = Revocation {
                seq,
                peer_id: *id,
                revoked_by: by.id,
                at_utc,
                signature,
            };
            d.unpin(id);
            d.revocations.push(r.clone());
            tracing::info!(peer = %id.short_label(), seq, "peer revoked");
            Ok(r)
        })
    }
    /// Apply a revocation received from a group member: `revoked_by` must be pinned here and not
    /// itself revoked, and the signature must verify. Idempotent for an identical record.
    pub fn apply_revocation(&self, r: &Revocation) -> Result<()> {
        self.mutate(|d| {
            if d.revocations.contains(r) {
                return Ok(());
            }
            if d.is_revoked(&r.revoked_by) {
                return Err(CommonError::new(
                    ErrorCode::PeerRevoked,
                    "revocation was signed by a device that is itself revoked",
                ));
            }
            if !d.is_pinned(&r.revoked_by) {
                return Err(CommonError::new(
                    ErrorCode::PeerUnauthorized,
                    "revocation was signed by a device that is not paired here",
                ));
            }
            let sig = hex::decode(&r.signature).map_err(|_| CommonError::invalid("revocation signature is not hex"))?;
            if !verify(&r.revoked_by, &revocation_message(r.seq, &r.peer_id, &r.at_utc), &sig) {
                return Err(CommonError::new(
                    ErrorCode::PeerUnauthorized,
                    "revocation signature is invalid",
                ));
            }
            d.unpin(&r.peer_id);
            d.revocations.push(r.clone());
            tracing::info!(peer = %r.peer_id.short_label(), by = %r.revoked_by.short_label(), "applied revocation");
            Ok(())
        })
    }
    pub fn is_revoked(&self, id: &PeerId) -> bool {
        self.read(|d| d.is_revoked(id))
    }
    /// Revocations with `seq > since`, in insertion order.
    // ponytail: `seq` is per issuer, so peers converge by asking `since(0)` (lists are tiny);
    // upgrade path is an (issuer, seq) vector when groups grow past a household.
    pub fn revocations_since(&self, since: u64) -> Result<Vec<Revocation>> {
        Ok(self.read(|d| d.revocations.iter().filter(|r| r.seq > since).cloned().collect()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> (tempfile::TempDir, Paths) {
        let t = tempfile::tempdir().unwrap();
        let p = Paths::at(t.path());
        (t, p)
    }
    fn record(id: PeerId, name: &str) -> PeerRecord {
        PeerRecord {
            id,
            name: name.into(),
            paired_at_utc: utc_now(),
            last_seen_utc: None,
            last_addr: None,
        }
    }
    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn keygen_persist_load() {
        let (_t, p) = paths();
        let a = DeviceIdentity::load_or_create(&p).unwrap();
        let b = DeviceIdentity::load_or_create(&p).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.pkcs8_der(), b.pkcs8_der());
        let pub_hex = std::fs::read_to_string(p.identity().join("device.pub")).unwrap();
        assert_eq!(pub_hex.trim(), a.id.to_hex());
        assert_eq!(pub_hex.trim().len(), 64);
        assert_ne!(DeviceIdentity::generate().unwrap().id, a.id, "fresh keys differ");
        // A stale device.pub is repaired on the next load.
        std::fs::write(p.identity().join("device.pub"), "garbage").unwrap();
        DeviceIdentity::load_or_create(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(p.identity().join("device.pub")).unwrap().trim(),
            a.id.to_hex()
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_0600() {
        let (_t, p) = paths();
        let id = DeviceIdentity::load_or_create(&p).unwrap();
        let store = PeerStore::open(&p).unwrap();
        store.pin(record(DeviceIdentity::generate().unwrap().id, "x")).unwrap();
        assert_eq!(mode(&p.identity()), 0o700);
        assert_eq!(mode(&p.identity().join("device.key")), 0o600);
        assert_eq!(mode(&p.identity().join("device.pub")), 0o600);
        assert_eq!(mode(&p.identity().join("peers.json")), 0o600);
        assert!(!p.identity().join("device.key.tmp").exists(), "tmp renamed away");
        // Loosened on disk → tightened on load.
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(p.identity().join("device.key"), std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(DeviceIdentity::load_or_create(&p).unwrap().id, id.id);
        assert_eq!(mode(&p.identity().join("device.key")), 0o600);
    }

    #[test]
    fn corrupt_key_file_is_an_error_not_a_new_identity() {
        let (_t, p) = paths();
        std::fs::create_dir_all(p.identity()).unwrap();
        std::fs::write(p.identity().join("device.key"), b"not a key").unwrap();
        let e = DeviceIdentity::load_or_create(&p).unwrap_err();
        assert_eq!(e.code, ErrorCode::Io);
        assert!(!e.message.contains("not a key"), "file content never echoed");
    }

    #[test]
    fn sign_verify_roundtrip() {
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let sig = a.sign(b"hello");
        assert!(verify(&a.id, b"hello", &sig));
        assert!(!verify(&a.id, b"hellp", &sig));
        assert!(!verify(&b.id, b"hello", &sig));
        assert!(!verify(&a.id, b"hello", &sig[..63]));
        assert!(!verify(&a.id, b"hello", &[0u8; 64]));
    }

    #[test]
    fn spki_round_trip() {
        let id = DeviceIdentity::generate().unwrap().id;
        let der = id.to_spki_der();
        assert_eq!(der.len(), 44);
        assert_eq!(PeerId::from_spki_der(&der).unwrap(), id);
        assert_eq!(
            PeerId::from_spki_der(&der[..43]).unwrap_err().code,
            ErrorCode::PeerUnauthorized
        );
        let mut wrong_oid = der.clone();
        wrong_oid[8] = 0x71; // X448
        assert!(PeerId::from_spki_der(&wrong_oid).is_err());
    }

    #[test]
    fn peer_id_formats() {
        let id = PeerId([0xab; 32]);
        assert_eq!(id.to_hex().len(), 64);
        assert_eq!(id.short_label(), "abab-abab-abab");
        assert_eq!(id.grouped().split(' ').count(), 8);
        assert_eq!(PeerId::parse_hex(&id.to_hex()).unwrap(), id);
        assert!(PeerId::parse_hex("abcd").is_err());
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{}\"", id.to_hex()));
    }

    #[test]
    fn format_utc_known_values() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert!(utc_now().ends_with('Z'));
    }

    #[test]
    fn debug_redacts_secret() {
        let id = DeviceIdentity::generate().unwrap();
        let s = format!("{id:?}");
        assert!(s.contains("<redacted>"), "{s}");
        assert!(s.contains(&id.id.short_label()));
        assert!(!s.contains(&hex::encode(id.pkcs8_der())));
    }

    fn base64(bytes: &[u8]) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.iter().fold(0u32, |acc, b| (acc << 8) | *b as u32) << (8 * (3 - chunk.len()));
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn private_key_never_logged() {
        let (_t, p) = paths();
        let id = DeviceIdentity::load_or_create(&p).unwrap();
        let store = PeerStore::open(&p).unwrap();
        let other = DeviceIdentity::generate().unwrap();
        store.pin(record(other.id, "other")).unwrap();
        let rev = store.revoke(&other.id, &id).unwrap();
        // Everything a log line or diagnostics bundle could ever format:
        let mut buf = String::new();
        buf += &format!("{id:?} {} {:?} {}", id.id, id.id, id.id.grouped());
        buf += &format!("{store:?} {rev:?} {:?} {:?}", store.list(), store.groups());
        buf += &serde_json::to_string(&rev).unwrap();
        buf += &std::fs::read_to_string(&store.path).unwrap();
        buf += &std::fs::read_to_string(p.identity().join("device.pub")).unwrap();
        let pkcs8 = id.pkcs8_der();
        assert_eq!(
            pkcs8.len(),
            83,
            "ring PKCS#8 v2 Ed25519 layout changed; fix the seed offset below"
        );
        let seed = &pkcs8[16..48]; // ring's PKCS#8 v2 layout: the 32-byte seed sits after a 16-byte header
        for secret in [pkcs8, seed] {
            assert!(!buf.contains(&hex::encode(secret)), "hex of secret leaked");
            assert!(!buf.contains(&hex::encode_upper(secret)), "upper hex of secret leaked");
            assert!(!buf.contains(&base64(secret)), "base64 of secret leaked");
            assert!(!buf.contains(&format!("{secret:?}")), "byte list of secret leaked");
        }
    }

    #[test]
    fn peer_store_persists_across_reopen() {
        let (_t, p) = paths();
        let me = DeviceIdentity::generate().unwrap();
        let a = DeviceIdentity::generate().unwrap().id;
        let b = DeviceIdentity::generate().unwrap().id;
        {
            let s = PeerStore::open(&p).unwrap();
            s.pin(record(a, "a")).unwrap();
            s.pin(record(b, "b")).unwrap();
            s.upsert_group(Group {
                name: "home".into(),
                members: vec![a, b],
                policy: GroupPolicy {
                    allow_remote_inference: true,
                    allow_pooled_inference: false,
                },
            })
            .unwrap();
            s.revoke(&b, &me).unwrap();
        }
        let s = PeerStore::open(&p).unwrap();
        assert!(s.is_pinned(&a));
        assert!(!s.is_pinned(&b));
        assert!(s.is_revoked(&b));
        assert_eq!(s.get(&a).unwrap().unwrap().name, "a");
        assert_eq!(s.groups_of(&a).unwrap().len(), 1);
        assert!(s.groups_of(&b).unwrap().is_empty(), "revoke removed group membership");
        assert_eq!(s.revocations_since(0).unwrap().len(), 1);
        assert_eq!(s.revocations_since(1).unwrap().len(), 0);
        // The next revocation continues the sequence after a reopen.
        assert_eq!(s.revoke(&a, &me).unwrap().seq, 2);
    }

    #[test]
    fn group_membership_crud() {
        let (_t, p) = paths();
        let s = PeerStore::open(&p).unwrap();
        let a = PeerId([1; 32]);
        let b = PeerId([2; 32]);
        s.pin(record(a, "a")).unwrap();
        s.pin(record(a, "a-renamed")).unwrap();
        assert_eq!(s.list().unwrap().len(), 1, "pin is an upsert");
        assert_eq!(s.get(&a).unwrap().unwrap().name, "a-renamed");
        let g = |members: Vec<PeerId>| Group {
            name: "g".into(),
            members,
            policy: GroupPolicy::default(),
        };
        s.upsert_group(g(vec![a, b])).unwrap();
        s.upsert_group(g(vec![a])).unwrap();
        assert_eq!(s.groups().unwrap().len(), 1, "upsert by name");
        assert_eq!(s.groups_of(&a).unwrap().len(), 1);
        assert!(s.groups_of(&b).unwrap().is_empty());
        assert!(s
            .upsert_group(Group {
                name: " ".into(),
                members: vec![],
                policy: GroupPolicy::default()
            })
            .is_err());
        s.remove(&a).unwrap();
        s.remove(&a).unwrap();
        assert!(!s.is_pinned(&a));
        assert!(s.groups_of(&a).unwrap().is_empty());
        assert!(!s.is_revoked(&a), "remove is not a revocation");
        assert_eq!(s.get(&a).unwrap(), None);
    }

    #[test]
    fn revocation_round_trip() {
        // Device A and device B each have a store; both pinned C. A revokes C and B applies it.
        let (_ta, pa) = paths();
        let (_tb, pb) = paths();
        let a = DeviceIdentity::generate().unwrap();
        let b = DeviceIdentity::generate().unwrap();
        let c = DeviceIdentity::generate().unwrap();
        let sa = PeerStore::open(&pa).unwrap();
        let sb = PeerStore::open(&pb).unwrap();
        sa.pin(record(b.id, "b")).unwrap();
        sa.pin(record(c.id, "c")).unwrap();
        sb.pin(record(a.id, "a")).unwrap();
        sb.pin(record(c.id, "c")).unwrap();

        let r = sa.revoke(&c.id, &a).unwrap();
        assert_eq!(r.revoked_by, a.id);
        assert_eq!(r.seq, 1);
        assert!(!sa.is_pinned(&c.id) && sa.is_revoked(&c.id));

        // Propagate as the wire would: JSON round trip.
        let wire: Revocation = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
        sb.apply_revocation(&wire).unwrap();
        assert!(!sb.is_pinned(&c.id) && sb.is_revoked(&c.id));
        sb.apply_revocation(&wire).unwrap(); // idempotent
        assert_eq!(sb.revocations_since(0).unwrap().len(), 1);
        assert_eq!(sb.revocations_since(0).unwrap(), sa.revocations_since(0).unwrap());
    }

    #[test]
    fn bad_signature_rejected() {
        let (_ta, pa) = paths();
        let (_tb, pb) = paths();
        let a = DeviceIdentity::generate().unwrap();
        let c = DeviceIdentity::generate().unwrap();
        let stranger = DeviceIdentity::generate().unwrap();
        let sa = PeerStore::open(&pa).unwrap();
        let sb = PeerStore::open(&pb).unwrap();
        sa.pin(record(c.id, "c")).unwrap();
        sb.pin(record(a.id, "a")).unwrap();
        sb.pin(record(c.id, "c")).unwrap();
        let good = sa.revoke(&c.id, &a).unwrap();

        // Tampered signature.
        let mut bad = good.clone();
        bad.signature = hex::encode([0u8; 64]);
        assert_eq!(sb.apply_revocation(&bad).unwrap_err().code, ErrorCode::PeerUnauthorized);
        // Tampered target: signature no longer covers it.
        let mut bad = good.clone();
        bad.peer_id = a.id;
        assert_eq!(sb.apply_revocation(&bad).unwrap_err().code, ErrorCode::PeerUnauthorized);
        // Tampered timestamp / seq.
        let mut bad = good.clone();
        bad.at_utc.push('x');
        assert!(sb.apply_revocation(&bad).is_err());
        let mut bad = good.clone();
        bad.seq += 1;
        assert!(sb.apply_revocation(&bad).is_err());
        // Not hex.
        let mut bad = good.clone();
        bad.signature = "zz".into();
        assert_eq!(sb.apply_revocation(&bad).unwrap_err().code, ErrorCode::InvalidRequest);
        // Signed by a key that is not pinned here.
        let (_ts, ps) = paths();
        let ss = PeerStore::open(&ps).unwrap();
        ss.pin(record(c.id, "c")).unwrap();
        let foreign = ss.revoke(&c.id, &stranger).unwrap();
        assert_eq!(
            sb.apply_revocation(&foreign).unwrap_err().code,
            ErrorCode::PeerUnauthorized
        );
        // Signed by a pinned key that is itself revoked.
        let (_tx, px) = paths();
        let sx = PeerStore::open(&px).unwrap();
        sx.pin(record(c.id, "c")).unwrap();
        let by_c = sx.revoke(&a.id, &c).unwrap();
        sb.apply_revocation(&good).unwrap(); // c now revoked at b
        assert_eq!(sb.apply_revocation(&by_c).unwrap_err().code, ErrorCode::PeerRevoked);
        // Nothing above changed b's pins.
        assert!(sb.is_pinned(&a.id));
        assert_eq!(sb.revocations_since(0).unwrap().len(), 1);
    }

    #[test]
    fn revoked_key_cannot_be_repinned_without_new_pairing() {
        let (_t, p) = paths();
        let me = DeviceIdentity::generate().unwrap();
        let c = PeerId([7; 32]);
        let s = PeerStore::open(&p).unwrap();
        s.pin(record(c, "c")).unwrap();
        s.revoke(&c, &me).unwrap();
        assert_eq!(s.pin(record(c, "c again")).unwrap_err().code, ErrorCode::PeerRevoked);
        assert!(!s.is_pinned(&c));
        // Survives a reopen.
        let s = PeerStore::open(&p).unwrap();
        assert_eq!(s.pin(record(c, "c again")).unwrap_err().code, ErrorCode::PeerRevoked);
        // Only a fresh pairing clears it.
        s.pin_after_pairing(record(c, "c paired again")).unwrap();
        assert!(s.is_pinned(&c));
        assert!(!s.is_revoked(&c));
        assert!(s.revocations_since(0).unwrap().is_empty());
    }

    #[test]
    fn failed_write_leaves_store_unchanged() {
        let (_t, p) = paths();
        let s = PeerStore::open(&p).unwrap();
        let a = PeerId([1; 32]);
        s.pin(record(a, "a")).unwrap();
        // Make the directory unwritable so the tmp file cannot be created.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(p.identity(), std::fs::Permissions::from_mode(0o500)).unwrap();
            let err = s.pin(record(PeerId([2; 32]), "b")).is_err();
            std::fs::set_permissions(p.identity(), std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(err);
            assert_eq!(s.list().unwrap().len(), 1, "memory not mutated on failed write");
            assert_eq!(PeerStore::open(&p).unwrap().list().unwrap().len(), 1);
        }
    }

    #[test]
    fn types_are_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<DeviceIdentity>();
        check::<PeerStore>();
        check::<PeerId>();
    }
}
