//! Policy plane (ADR-0003, spec section 9).
//!
//! Every routing decision passes through `PolicyEngine::evaluate` (I-05). Defaults: privacy
//! `LocalOnly`, network disabled, contribution disabled: with default config no request leaves
//! the device and no external workload is accepted (F-06-02, F-07-01).
//!
//! Truth table (`evaluate`):
//! - target `Local` → Allow, whatever the privacy mode.
//! - target `Peer` + `LocalOnly` → Deny `PolicyDenied`.
//! - target `Peer` + `SwarmAllowed` → Deny `PolicyDenied` (swarm stays disabled until Gate F,
//!   `swarm_enabled` is ignored in M3).
//! - target `Peer` + `TrustedGroup` → needs `network_enabled`, then the peer must be not revoked
//!   (`PeerRevoked`), pinned (`PeerUnauthorized`) and in a group whose policy allows the workload
//!   kind (`PeerUnauthorized`).
//!
//! `may_serve` is the mirror image for inbound work: `network_enabled` and `serve_peers`, then the
//! same peer checks.

use common_core::{CommonError, ErrorCode, Result};
use common_identity::{PeerId, PeerStore};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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

impl WorkloadKind {
    fn label(self) -> &'static str {
        match self {
            WorkloadKind::RemoteInference => "remote inference",
            WorkloadKind::PooledInference => "pooled inference",
        }
    }
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
            Decision::Deny { code, reason } => Err(CommonError::new(code, reason)),
        }
    }
    fn deny(code: ErrorCode, reason: impl Into<String>) -> Decision {
        Decision::Deny {
            code,
            reason: reason.into(),
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
    /// `privacy == TrustedGroup`, networking enabled, a pinned, non-revoked peer sharing a group whose
    /// policy allows `kind`.
    pub fn evaluate(&self, privacy: PrivacyMode, target: ExecutionTarget, kind: WorkloadKind) -> Decision {
        let peer = match target {
            ExecutionTarget::Local => return Decision::Allow,
            ExecutionTarget::Peer(p) => p,
        };
        match privacy {
            PrivacyMode::LocalOnly => {
                Decision::deny(ErrorCode::PolicyDenied, "your privacy settings require local execution")
            }
            PrivacyMode::SwarmAllowed => Decision::deny(
                ErrorCode::PolicyDenied,
                "public swarm execution is disabled until it passes its security gate",
            ),
            PrivacyMode::TrustedGroup => {
                if !self.config.network_enabled {
                    return Decision::deny(ErrorCode::PolicyDenied, "peer networking is disabled in settings");
                }
                self.check_peer(&peer, kind)
            }
        }
    }

    /// May this device serve `kind` for `peer`? Requires `network_enabled`, `serve_peers`, a pinned
    /// non-revoked peer and a shared group allowing `kind`. Default deny.
    pub fn may_serve(&self, peer: &PeerId, kind: WorkloadKind) -> Decision {
        if !self.config.network_enabled {
            return Decision::deny(ErrorCode::PolicyDenied, "peer networking is disabled in settings");
        }
        if !self.config.serve_peers {
            return Decision::deny(
                ErrorCode::PolicyDenied,
                "this device does not accept workloads from other devices",
            );
        }
        self.check_peer(peer, kind)
    }

    /// Shared peer checks: revoked → pinned → group policy for `kind`.
    fn check_peer(&self, peer: &PeerId, kind: WorkloadKind) -> Decision {
        let label = peer.short_label();
        if self.peers.is_revoked(peer) {
            return Decision::deny(ErrorCode::PeerRevoked, format!("device {label} was revoked"));
        }
        if !self.peers.is_pinned(peer) {
            return Decision::deny(ErrorCode::PeerUnauthorized, format!("device {label} is not paired"));
        }
        let groups = match self.peers.groups_of(peer) {
            Ok(g) => g,
            Err(e) => return Decision::deny(e.code, e.message),
        };
        let allowed = groups.iter().any(|g| match kind {
            WorkloadKind::RemoteInference => g.policy.allow_remote_inference,
            WorkloadKind::PooledInference => g.policy.allow_pooled_inference,
        });
        if !allowed {
            return Decision::deny(
                ErrorCode::PeerUnauthorized,
                format!("no trusted group allows {} with device {label}", kind.label()),
            );
        }
        Decision::Allow
    }
}

/// Token-bucket quotas per peer (F-06-04). A value of 0 means unlimited.
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

/// One token bucket: `level` refills at `per_sec` up to `cap`.
#[derive(Debug, Clone)]
struct Bucket {
    level: f64,
    last: Instant,
}

impl Bucket {
    fn new(cap: f64, now: Instant) -> Self {
        Self { level: cap, last: now }
    }
    /// Take `n` after refilling; false (and nothing taken) when the bucket is short. `cap == 0` = unlimited.
    fn take(&mut self, n: f64, cap: f64, per_sec: f64, now: Instant) -> bool {
        if cap <= 0.0 {
            return true;
        }
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.level = (self.level + elapsed * per_sec).min(cap);
        self.last = now;
        if self.level >= n {
            self.level -= n;
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct PeerQuota {
    requests: Bucket,
    tokens: Bucket,
    bytes: Bucket,
    concurrent: u32,
}

/// Per-peer buckets. Peers are the pinned set, so the map is bounded by `peers.json`.
pub struct Quotas {
    cfg: QuotaConfig,
    peers: Mutex<HashMap<PeerId, PeerQuota>>,
}

fn overloaded(peer: &PeerId, what: &str) -> CommonError {
    CommonError::new(
        ErrorCode::Overloaded,
        format!("device {} exceeded its {what}; retry later", peer.short_label()),
    )
}

impl Quotas {
    pub fn new(cfg: QuotaConfig) -> Self {
        Quotas {
            cfg,
            peers: Mutex::new(HashMap::new()),
        }
    }
    /// Returns Err(Overloaded) with a typed message when the peer exceeds its request rate or its
    /// concurrency limit. On Ok the request counts as in flight until `release`.
    pub fn admit_request(&self, peer: &PeerId) -> Result<()> {
        self.admit_at(peer, Instant::now())
    }
    /// A request admitted earlier finished (success or failure).
    pub fn release(&self, peer: &PeerId) {
        let mut map = self.lock();
        if let Some(q) = map.get_mut(peer) {
            q.concurrent = q.concurrent.saturating_sub(1);
        }
    }
    /// Charge `n` generated tokens; Err(Overloaded) when the per-minute budget is exhausted.
    pub fn charge_tokens(&self, peer: &PeerId, n: u64) -> Result<()> {
        self.charge_tokens_at(peer, n, Instant::now())
    }
    /// Charge `n` transferred bytes (tunnels, F-06-04); unlimited by default.
    pub fn charge_bytes(&self, peer: &PeerId, n: u64) -> Result<()> {
        self.charge_bytes_at(peer, n, Instant::now())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<PeerId, PeerQuota>> {
        self.peers.lock().unwrap_or_else(|e| e.into_inner())
    }
    fn entry<'a>(&self, map: &'a mut HashMap<PeerId, PeerQuota>, peer: &PeerId, now: Instant) -> &'a mut PeerQuota {
        map.entry(*peer).or_insert_with(|| PeerQuota {
            requests: Bucket::new(self.cfg.requests_per_minute as f64, now),
            tokens: Bucket::new(self.cfg.tokens_per_minute as f64, now),
            bytes: Bucket::new(self.cfg.bytes_per_second as f64, now),
            concurrent: 0,
        })
    }
    fn admit_at(&self, peer: &PeerId, now: Instant) -> Result<()> {
        let mut map = self.lock();
        let max_concurrent = self.cfg.max_concurrent;
        let rpm = self.cfg.requests_per_minute as f64;
        let q = self.entry(&mut map, peer, now);
        if max_concurrent > 0 && q.concurrent >= max_concurrent {
            return Err(overloaded(
                peer,
                &format!("limit of {max_concurrent} concurrent requests"),
            ));
        }
        if !q.requests.take(1.0, rpm, rpm / 60.0, now) {
            return Err(overloaded(peer, &format!("limit of {rpm} requests per minute")));
        }
        q.concurrent += 1;
        Ok(())
    }
    fn charge_tokens_at(&self, peer: &PeerId, n: u64, now: Instant) -> Result<()> {
        let mut map = self.lock();
        let tpm = self.cfg.tokens_per_minute as f64;
        let q = self.entry(&mut map, peer, now);
        if q.tokens.take(n as f64, tpm, tpm / 60.0, now) {
            Ok(())
        } else {
            Err(overloaded(peer, &format!("limit of {tpm} tokens per minute")))
        }
    }
    fn charge_bytes_at(&self, peer: &PeerId, n: u64, now: Instant) -> Result<()> {
        let mut map = self.lock();
        let bps = self.cfg.bytes_per_second as f64;
        let q = self.entry(&mut map, peer, now);
        if q.bytes.take(n as f64, bps, bps, now) {
            Ok(())
        } else {
            Err(overloaded(peer, &format!("limit of {bps} bytes per second")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_identity::{Group, GroupPolicy, PeerRecord};
    use std::time::Duration;

    fn pid(b: u8) -> PeerId {
        PeerId([b; 32])
    }
    fn rec(id: PeerId) -> PeerRecord {
        PeerRecord {
            id,
            name: id.short_label(),
            paired_at_utc: "2026-10-14T00:00:00Z".into(),
            last_seen_utc: None,
            last_addr: None,
        }
    }
    fn store() -> (tempfile::TempDir, Arc<PeerStore>) {
        let t = tempfile::tempdir().unwrap();
        let s = PeerStore::open(&common_core::Paths::at(t.path())).unwrap();
        (t, Arc::new(s))
    }
    fn group(name: &str, members: Vec<PeerId>, remote: bool, pooled: bool) -> Group {
        Group {
            name: name.into(),
            members,
            policy: GroupPolicy {
                allow_remote_inference: remote,
                allow_pooled_inference: pooled,
            },
        }
    }
    fn open_config() -> PolicyConfig {
        PolicyConfig {
            default_privacy: PrivacyMode::TrustedGroup,
            network_enabled: true,
            serve_peers: true,
            swarm_enabled: false,
        }
    }
    fn code(d: Decision) -> ErrorCode {
        match d {
            Decision::Allow => panic!("expected deny"),
            Decision::Deny { code, .. } => code,
        }
    }
    use PrivacyMode::*;
    use WorkloadKind::*;

    #[test]
    fn policy_evaluation() {
        let (_t, s) = store();
        let a = pid(1);
        s.pin(rec(a)).unwrap();
        s.upsert_group(group("home", vec![a], true, false)).unwrap();
        let e = PolicyEngine::new(open_config(), s.clone());
        // Local is always allowed.
        for p in [LocalOnly, TrustedGroup, SwarmAllowed] {
            assert!(e.evaluate(p, ExecutionTarget::Local, RemoteInference).is_allow());
        }
        let peer = ExecutionTarget::Peer(a);
        let d = e.evaluate(LocalOnly, peer, RemoteInference);
        assert_eq!(code(d.clone()), ErrorCode::PolicyDenied);
        assert!(matches!(d, Decision::Deny { reason, .. } if reason.contains("privacy settings")));
        assert_eq!(
            code(e.evaluate(SwarmAllowed, peer, RemoteInference)),
            ErrorCode::PolicyDenied
        );
        assert!(e.evaluate(TrustedGroup, peer, RemoteInference).is_allow());
        assert_eq!(
            code(e.evaluate(TrustedGroup, peer, PooledInference)),
            ErrorCode::PeerUnauthorized
        );
        // Networking off → PolicyDenied before any peer lookup.
        let off = PolicyEngine::new(
            PolicyConfig {
                network_enabled: false,
                ..open_config()
            },
            s.clone(),
        );
        assert_eq!(
            code(off.evaluate(TrustedGroup, peer, RemoteInference)),
            ErrorCode::PolicyDenied
        );
        // swarm_enabled in config changes nothing in M3.
        let swarm = PolicyEngine::new(
            PolicyConfig {
                swarm_enabled: true,
                ..open_config()
            },
            s,
        );
        assert_eq!(
            code(swarm.evaluate(SwarmAllowed, peer, RemoteInference)),
            ErrorCode::PolicyDenied
        );
        assert!(e.evaluate(TrustedGroup, peer, RemoteInference).into_result().is_ok());
        assert_eq!(
            e.evaluate(LocalOnly, peer, RemoteInference)
                .into_result()
                .unwrap_err()
                .code,
            ErrorCode::PolicyDenied
        );
    }

    #[test]
    fn unauthorized_peer_request_refused() {
        let (_t, s) = store();
        let e = PolicyEngine::new(open_config(), s.clone());
        let unknown = ExecutionTarget::Peer(pid(9));
        let d = e.evaluate(TrustedGroup, unknown, RemoteInference);
        assert_eq!(code(d.clone()), ErrorCode::PeerUnauthorized);
        assert!(matches!(d, Decision::Deny { reason, .. } if reason.contains("not paired")));
        // Revoked beats unpinned in the message: the user learns *why* it is gone.
        let me = common_identity::DeviceIdentity::generate().unwrap();
        s.pin(rec(pid(9))).unwrap();
        s.revoke(&pid(9), &me).unwrap();
        assert_eq!(
            code(e.evaluate(TrustedGroup, unknown, RemoteInference)),
            ErrorCode::PeerRevoked
        );
        assert_eq!(code(e.may_serve(&pid(9), RemoteInference)), ErrorCode::PeerRevoked);
    }

    #[test]
    fn workload_only_to_group_members() {
        let (_t, s) = store();
        let member = pid(1);
        let pooled_only = pid(2);
        s.pin(rec(member)).unwrap();
        s.pin(rec(pooled_only)).unwrap();
        s.upsert_group(group("inference", vec![member], true, false)).unwrap();
        s.upsert_group(group("pool", vec![pooled_only], false, true)).unwrap();
        let e = PolicyEngine::new(open_config(), s);
        assert!(e
            .evaluate(TrustedGroup, ExecutionTarget::Peer(member), RemoteInference)
            .is_allow());
        assert_eq!(
            code(e.evaluate(TrustedGroup, ExecutionTarget::Peer(member), PooledInference)),
            ErrorCode::PeerUnauthorized
        );
        assert!(e
            .evaluate(TrustedGroup, ExecutionTarget::Peer(pooled_only), PooledInference)
            .is_allow());
        assert_eq!(
            code(e.evaluate(TrustedGroup, ExecutionTarget::Peer(pooled_only), RemoteInference)),
            ErrorCode::PeerUnauthorized
        );
        assert!(e.may_serve(&member, RemoteInference).is_allow());
        assert_eq!(code(e.may_serve(&member, PooledInference)), ErrorCode::PeerUnauthorized);
    }

    #[test]
    fn non_member_rejected() {
        let (_t, s) = store();
        let pinned_no_group = pid(3);
        s.pin(rec(pinned_no_group)).unwrap();
        s.upsert_group(group("home", vec![pid(4)], true, true)).unwrap();
        let e = PolicyEngine::new(open_config(), s);
        let d = e.evaluate(TrustedGroup, ExecutionTarget::Peer(pinned_no_group), RemoteInference);
        assert_eq!(code(d.clone()), ErrorCode::PeerUnauthorized);
        assert!(matches!(d, Decision::Deny { reason, .. } if reason.contains("no trusted group")));
        assert_eq!(
            code(e.may_serve(&pinned_no_group, RemoteInference)),
            ErrorCode::PeerUnauthorized
        );
        // In the group but never pinned (stale membership) → still refused.
        assert_eq!(
            code(e.evaluate(TrustedGroup, ExecutionTarget::Peer(pid(4)), RemoteInference)),
            ErrorCode::PeerUnauthorized
        );
    }

    #[test]
    fn may_serve_requires_serve_peers_and_network() {
        let (_t, s) = store();
        let a = pid(1);
        s.pin(rec(a)).unwrap();
        s.upsert_group(group("home", vec![a], true, true)).unwrap();
        let cases = [
            (false, false, ErrorCode::PolicyDenied),
            (true, false, ErrorCode::PolicyDenied),
            (false, true, ErrorCode::PolicyDenied),
        ];
        for (network_enabled, serve_peers, expect) in cases {
            let e = PolicyEngine::new(
                PolicyConfig {
                    network_enabled,
                    serve_peers,
                    ..open_config()
                },
                s.clone(),
            );
            assert_eq!(
                code(e.may_serve(&a, RemoteInference)),
                expect,
                "{network_enabled} {serve_peers}"
            );
        }
        assert!(PolicyEngine::new(open_config(), s)
            .may_serve(&a, PooledInference)
            .is_allow());
    }

    // ---- property-style fuzz over random (config, store, request) combinations ----

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            // xorshift64*
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn bool(&mut self) -> bool {
            self.next() & 1 == 1
        }
    }

    /// Random store: 8 candidate ids; each pinned/revoked/grouped at random. Returns (store, ids).
    fn random_store(rng: &mut Rng) -> (tempfile::TempDir, Arc<PeerStore>, Vec<PeerId>) {
        let (t, s) = store();
        let me = common_identity::DeviceIdentity::generate().unwrap();
        let ids: Vec<PeerId> = (1..=8).map(pid).collect();
        for id in &ids {
            if rng.bool() {
                s.pin(rec(*id)).unwrap();
            }
        }
        for g in 0..rng.below(4) {
            let members = ids.iter().copied().filter(|_| rng.bool()).collect();
            s.upsert_group(group(&format!("g{g}"), members, rng.bool(), rng.bool()))
                .unwrap();
        }
        for id in &ids {
            if rng.below(5) == 0 {
                s.revoke(id, &me).unwrap();
            }
        }
        (t, s, ids)
    }
    fn random_config(rng: &mut Rng) -> PolicyConfig {
        PolicyConfig {
            default_privacy: random_privacy(rng),
            network_enabled: rng.bool(),
            serve_peers: rng.bool(),
            swarm_enabled: rng.bool(),
        }
    }
    fn random_privacy(rng: &mut Rng) -> PrivacyMode {
        [LocalOnly, TrustedGroup, SwarmAllowed][rng.below(3) as usize]
    }
    fn random_target(rng: &mut Rng, ids: &[PeerId]) -> ExecutionTarget {
        match rng.below(10) {
            0 => ExecutionTarget::Local,
            1 => ExecutionTarget::Peer(pid(200)), // never pinned
            _ => ExecutionTarget::Peer(ids[rng.below(ids.len() as u64) as usize]),
        }
    }
    fn random_kind(rng: &mut Rng) -> WorkloadKind {
        if rng.bool() {
            RemoteInference
        } else {
            PooledInference
        }
    }
    const ITERATIONS: usize = 2_000;
    const STORES: usize = 40;

    /// F-06-02: with `PolicyConfig::default()` no peer target is ever allowed and nothing is ever
    /// served, whatever the store (pins, groups, revocations) and the request say.
    #[test]
    fn default_deny() {
        let mut rng = Rng(0x6f06_0200_dead_beef);
        let stores: Vec<_> = (0..STORES).map(|_| random_store(&mut rng)).collect();
        let mut local_allowed = 0;
        for i in 0..ITERATIONS {
            let (_t, s, ids) = &stores[i % STORES];
            let e = PolicyEngine::new(PolicyConfig::default(), s.clone());
            let (privacy, target, kind) = (
                random_privacy(&mut rng),
                random_target(&mut rng, ids),
                random_kind(&mut rng),
            );
            let d = e.evaluate(privacy, target, kind);
            match target {
                ExecutionTarget::Local => {
                    assert!(d.is_allow());
                    local_allowed += 1;
                }
                ExecutionTarget::Peer(p) => {
                    assert!(!d.is_allow(), "default config allowed {privacy:?} {p:?} {kind:?}");
                    assert!(
                        !e.may_serve(&p, kind).is_allow(),
                        "default config served {p:?} {kind:?}"
                    );
                }
            }
        }
        assert!(local_allowed > 0, "fuzz exercised Local targets");
    }

    /// F-06-07: no (config, store, request) combination with privacy `LocalOnly` allows a peer
    /// target, and `SwarmAllowed` is never allowed in M3. The complement (TrustedGroup with an
    /// authorized peer) must be allowed at least once, so the test is not vacuous.
    #[test]
    fn local_only_never_leaves_device_fuzz() {
        let mut rng = Rng(0x6f06_0700_c0ff_ee00);
        let stores: Vec<_> = (0..STORES).map(|_| random_store(&mut rng)).collect();
        let mut allowed = 0;
        for i in 0..ITERATIONS {
            let (_t, s, ids) = &stores[i % STORES];
            let config = random_config(&mut rng);
            let e = PolicyEngine::new(config.clone(), s.clone());
            let (privacy, target, kind) = (
                random_privacy(&mut rng),
                random_target(&mut rng, ids),
                random_kind(&mut rng),
            );
            let d = e.evaluate(privacy, target, kind);
            let peer = match target {
                ExecutionTarget::Local => {
                    assert!(d.is_allow());
                    continue;
                }
                ExecutionTarget::Peer(p) => p,
            };
            if privacy == LocalOnly {
                assert_eq!(
                    code(d.clone()),
                    ErrorCode::PolicyDenied,
                    "LocalOnly leaked to {peer:?} with {config:?}"
                );
            }
            if privacy == SwarmAllowed {
                assert_eq!(
                    code(d.clone()),
                    ErrorCode::PolicyDenied,
                    "swarm allowed with {config:?}"
                );
            }
            if d.is_allow() {
                allowed += 1;
                // Every Allow is explained by the model: TrustedGroup, network on, pinned, not revoked, group allows kind.
                assert_eq!(privacy, TrustedGroup);
                assert!(config.network_enabled);
                assert!(s.is_pinned(&peer) && !s.is_revoked(&peer));
                assert!(s.groups_of(&peer).unwrap().iter().any(|g| match kind {
                    RemoteInference => g.policy.allow_remote_inference,
                    PooledInference => g.policy.allow_pooled_inference,
                }));
            }
            // Serving never happens without serve_peers.
            if !config.serve_peers {
                assert!(!e.may_serve(&peer, kind).is_allow());
            }
        }
        assert!(
            allowed > 0,
            "fuzz never produced an authorized TrustedGroup request; generator is broken"
        );
    }

    // ---- quotas ----

    #[test]
    fn token_bucket() {
        let q = Quotas::new(QuotaConfig {
            requests_per_minute: 3,
            tokens_per_minute: 100,
            bytes_per_second: 10,
            max_concurrent: 2,
        });
        let a = pid(1);
        let t0 = Instant::now();
        // Requests: 3 per minute, concurrency 2.
        q.admit_at(&a, t0).unwrap();
        q.admit_at(&a, t0).unwrap();
        let e = q.admit_at(&a, t0).unwrap_err();
        assert_eq!(e.code, ErrorCode::Overloaded);
        assert!(e.message.contains("concurrent"));
        q.release(&a);
        q.admit_at(&a, t0).unwrap(); // third request of the minute
        q.release(&a);
        let e = q.admit_at(&a, t0).unwrap_err();
        assert!(e.message.contains("requests per minute"), "{}", e.message);
        // One request refills every 20 s at 3/min (18 s and 21 s keep clear of float rounding).
        assert!(q.admit_at(&a, t0 + Duration::from_secs(18)).is_err());
        q.admit_at(&a, t0 + Duration::from_secs(21)).unwrap();
        q.release(&a);
        q.release(&a); // extra release never underflows
                       // Tokens: 100 per minute.
        q.charge_tokens_at(&a, 60, t0).unwrap();
        assert_eq!(q.charge_tokens_at(&a, 50, t0).unwrap_err().code, ErrorCode::Overloaded);
        q.charge_tokens_at(&a, 40, t0).unwrap(); // a refused charge takes nothing
        q.charge_tokens_at(&a, 50, t0 + Duration::from_secs(31)).unwrap();
        // Bytes: 10 per second.
        q.charge_bytes_at(&a, 10, t0).unwrap();
        assert!(q.charge_bytes_at(&a, 1, t0).is_err());
        q.charge_bytes_at(&a, 5, t0 + Duration::from_millis(600)).unwrap();
        // Bucket never exceeds its capacity after a long idle period.
        assert!(q.charge_tokens_at(&a, 101, t0 + Duration::from_secs(3600)).is_err());
        q.charge_tokens_at(&a, 100, t0 + Duration::from_secs(3600)).unwrap();
        // 0 = unlimited.
        let unlimited = Quotas::new(QuotaConfig {
            requests_per_minute: 0,
            tokens_per_minute: 0,
            bytes_per_second: 0,
            max_concurrent: 0,
        });
        for _ in 0..1000 {
            unlimited.admit_request(&a).unwrap();
            unlimited.charge_tokens(&a, 1 << 40).unwrap();
            unlimited.charge_bytes(&a, 1 << 40).unwrap();
        }
    }

    #[test]
    fn peer_flood_throttled() {
        let q = Quotas::new(QuotaConfig::default());
        let (flooder, other) = (pid(1), pid(2));
        let t0 = Instant::now();
        let admitted = (0..1000)
            .filter(|_| {
                let ok = q.admit_at(&flooder, t0).is_ok();
                q.release(&flooder);
                ok
            })
            .count();
        assert_eq!(admitted, 120, "exactly requests_per_minute admitted at t0");
        // Another peer is unaffected by the flood.
        q.admit_at(&other, t0).unwrap();
        q.release(&other);
        // The flooder gets a typed error, not a panic or a silent drop.
        assert_eq!(q.admit_at(&flooder, t0).unwrap_err().code, ErrorCode::Overloaded);
    }

    #[test]
    fn types_are_send_sync() {
        fn check<T: Send + Sync>() {}
        check::<PolicyEngine>();
        check::<Quotas>();
    }
}
