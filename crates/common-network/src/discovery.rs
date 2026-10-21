//! mDNS-SD discovery (ADR-0003 D4): `_common._udp.local.`, signed TXT, no secrets.
//!
//! TXT = `v=1`, `id=<64 hex PeerId>`, `pair=0|1`, `pairport=<n>` (only while pairing),
//! `proto=<major.minor>`, `ts=<unix s>`, `sig=<hex Ed25519 over the canonical string, which also
//! covers the SRV port>`. Nothing else:
//! no hostname beyond SRV, no model names, no hardware, no user names (those go only over the
//! authenticated channel). A browser drops unsigned, badly signed and stale records (`ts` more
//! than [`MAX_RECORD_AGE_SECS`] from its own clock, either direction), and marks `trusted` only
//! for ids pinned in the local store; the TLS verifier enforces trust independently, so a
//! spoofed record yields at most a failed handshake.
//!
//! The browser's table is bounded: an entry is kept while its record would still verify, listed
//! while heard within the caller's age bound, and at most [`MAX_SEEN`] are kept (oldest dropped).
//! Each entry is its id's replay guard: only a strictly newer `ts` replaces it, so a replayed
//! record cannot add, strip or redirect a dial candidate. A goodbye or expiry (`ServiceRemoved`,
//! unauthenticated) only hides the entry, so it cannot clear the guard either. A peer that
//! vanishes without a goodbye leaves the list when mdns-sd expires its SRV record (host TTL
//! 120 s); the age bound is only a backstop.
//!
//! The advertiser re-signs every [`REFRESH`], and [`IP_SETTLE`] after this host gains or loses an
//! address (mdns-sd re-announces the new address set within its 5 s interface poll, still with the
//! old `ts`, which browsers would not accept from a new address).
//!
//! Disabled (both sides inert, nothing bound) when `NetworkConfig.enabled` or `.mdns` is false
//! or the environment has `COMMON_MDNS=0`.
use crate::tls::{store_verdict, Verdict};
use crate::{NetworkConfig, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use common_core::{CommonError, ErrorCode, Result};
use common_identity::{utc_now, DeviceIdentity, PeerId, PeerStore};
use mdns_sd::{DaemonEvent, RecvTimeoutError, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_common._udp.local.";
/// A signed record older (or newer) than this is ignored (ADR-0003 D4).
pub const MAX_RECORD_AGE_SECS: u64 = 300;
/// The advertiser re-signs and re-announces at this interval so its `ts` stays fresh.
const REFRESH: Duration = Duration::from_secs(120);
/// Re-sign this long after the last interface change: more than mdns-sd's 1 s probe (a re-sign
/// during the probe renamed the instance "x (2)"), and a burst of changes gives one re-sign.
const IP_SETTLE: Duration = Duration::from_secs(2);
/// Most addresses kept per discovered peer, best first ([`DiscoveredPeer::addrs`]).
pub const MAX_DIAL_ADDRS: usize = 2;
/// Most records the browser keeps (anyone on the LAN can mint ids).
pub const MAX_SEEN: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredPeer {
    pub id: PeerId,
    /// The best address, `addrs[0]` (kept for API compatibility).
    pub addr: SocketAddr,
    /// Dial candidates, best first: 1 to [`MAX_DIAL_ADDRS`] entries. Hints only: a connection to
    /// any of them must still present the pinned key.
    #[serde(default)]
    pub addrs: Vec<SocketAddr>,
    pub pairing: bool,
    pub pair_port: Option<u16>,
    pub proto: String,
    /// True when pinned in the local store (never a trust decision by itself).
    pub trusted: bool,
    pub seen_utc: String,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The `ts` to sign: `now`, but past `last` (the last one this process signed) while that is less
/// than a minute ahead, so two records signed within one second (a pairing window opened and
/// closed, a re-sign racing `advertise`) still replace each other at browsers ([`remember`] takes
/// only a strictly newer ts). A clock set back further restarts from `now`: following `last` would
/// sign records every browser rejects as from the future ([`MAX_RECORD_AGE_SECS`]).
// ponytail: per process; a daemon restarted within the second it last signed in repeats that ts
// (browsers keep its old pairing fields until the next re-sign, at most REFRESH).
fn next_ts(now: u64, last: u64) -> u64 {
    if last >= now && last - now < 60 {
        last + 1
    } else {
        now
    }
}

fn signed_ts() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_unix();
    let last = LAST
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |last| Some(next_ts(now, last)))
        .unwrap_or_else(|l| l);
    next_ts(now, last)
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn mdns_err(e: mdns_sd::Error) -> CommonError {
    CommonError::io(format!("mdns: {e}"))
}

/// `enabled && mdns` from config, overridable by `COMMON_MDNS=0`.
pub fn mdns_enabled(config: &NetworkConfig) -> bool {
    config.enabled && config.mdns && std::env::var("COMMON_MDNS").map(|v| v != "0").unwrap_or(true)
}

/// The signed part of a TXT record.
#[derive(Debug, Clone, PartialEq)]
struct Advert {
    id: PeerId,
    /// The SRV port (workload endpoint); signed so a replay cannot point it elsewhere.
    port: u16,
    pairing: bool,
    pair_port: Option<u16>,
    proto: String,
    ts: u64,
}

impl Advert {
    /// The bytes the signature covers: every value in a fixed order, newline-separated.
    fn canonical(&self) -> String {
        format!(
            "v=1\nid={}\nport={}\npair={}\npairport={}\nproto={}\nts={}",
            self.id.to_hex(),
            self.port,
            u8::from(self.pairing),
            self.pair_port.map(|p| p.to_string()).unwrap_or_default(),
            self.proto,
            self.ts
        )
    }

    /// TXT properties, signed by `identity` (whose id must be `self.id`).
    fn sign(&self, identity: &DeviceIdentity) -> HashMap<String, String> {
        let sig = identity.sign(self.canonical().as_bytes());
        let mut txt = HashMap::from([
            ("v".to_string(), "1".to_string()),
            ("id".to_string(), self.id.to_hex()),
            ("pair".to_string(), u8::from(self.pairing).to_string()),
            ("proto".to_string(), self.proto.clone()),
            ("ts".to_string(), self.ts.to_string()),
            ("sig".to_string(), hex::encode(sig)),
        ]);
        if let Some(p) = self.pair_port {
            txt.insert("pairport".into(), p.to_string());
        }
        txt
    }

    /// Parse and verify a TXT record (with its SRV `port`) against `now`: version, signature by
    /// `id`, freshness.
    fn verify(txt: &HashMap<String, String>, port: u16, now: u64) -> Result<Advert> {
        let bad = |m: &str| CommonError::new(ErrorCode::PeerUnauthorized, format!("discovery record: {m}"));
        let get = |k: &str| txt.get(k).ok_or_else(|| bad(&format!("missing {k}")));
        if get("v")? != "1" {
            return Err(bad("unknown version"));
        }
        let id = PeerId::parse_hex(get("id")?).map_err(|_| bad("bad id"))?;
        let pairing = match get("pair")?.as_str() {
            "0" => false,
            "1" => true,
            _ => return Err(bad("bad pair flag")),
        };
        let pair_port = match txt.get("pairport") {
            None => None,
            Some(p) => Some(p.parse::<u16>().map_err(|_| bad("bad pairport"))?),
        };
        let ts: u64 = get("ts")?.parse().map_err(|_| bad("bad ts"))?;
        let advert = Advert {
            id,
            port,
            pairing,
            pair_port,
            proto: get("proto")?.clone(),
            ts,
        };
        let sig = hex::decode(get("sig")?).map_err(|_| bad("bad sig encoding"))?;
        if !common_identity::verify(&id, advert.canonical().as_bytes(), &sig) {
            return Err(bad("signature does not verify"));
        }
        if now.abs_diff(ts) > MAX_RECORD_AGE_SECS {
            return Err(bad("stale record"));
        }
        Ok(advert)
    }
}

struct AdvertState {
    daemon: ServiceDaemon,
    identity: Arc<DeviceIdentity>,
    port: u16,
    pairing_port: Option<u16>,
    /// The bind address when it is one specific IP: the only address advertised. `None`
    /// (0.0.0.0 / ::): every interface address, as mdns-sd finds them.
    bound: Option<IpAddr>,
    instance: String,
    host: String,
    fullname: Mutex<String>,
}

impl AdvertState {
    fn register(&self, probe: bool) -> Result<()> {
        let advert = Advert {
            id: self.identity.id,
            port: self.port,
            pairing: self.pairing_port.is_some(),
            pair_port: self.pairing_port,
            proto: format!("{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}"),
            ts: signed_ts(),
        };
        let ips: Vec<IpAddr> = self.bound.into_iter().collect();
        let mut info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.instance,
            &self.host,
            ips.as_slice(),
            self.port,
            advert.sign(&self.identity),
        )
        .map_err(mdns_err)?;
        if self.bound.is_none() {
            info = info.enable_addr_auto();
        }
        // A refresh re-announces the same name; probing it again would conflict with ourselves.
        info.set_requires_probe(probe);
        *lock(&self.fullname) = info.get_fullname().to_string();
        self.daemon.register(info).map_err(mdns_err)
    }
}

pub struct Advertiser {
    state: Option<Arc<AdvertState>>,
}

/// What woke the refresh thread.
#[derive(Debug, PartialEq)]
enum Wake {
    /// This host gained or lost an address.
    IpChange,
    /// Any other daemon event.
    Other,
    /// The refresh deadline passed.
    Deadline,
    /// The daemon shut down.
    Closed,
}

fn wake(r: std::result::Result<DaemonEvent, RecvTimeoutError>) -> Wake {
    match r {
        Ok(DaemonEvent::IpAdd(_) | DaemonEvent::IpDel(_)) => Wake::IpChange,
        Ok(_) => Wake::Other,
        Err(RecvTimeoutError::Timeout) => Wake::Deadline,
        Err(RecvTimeoutError::Disconnected) => Wake::Closed,
    }
}

/// Calls `refresh` every `every`, and `settle` after an interface change (each change within
/// `settle` pushes it out again, never past the periodic deadline). `wait(due)` blocks until `due`
/// or an event. Returns on [`Wake::Closed`].
fn refresh_loop(mut wait: impl FnMut(Instant) -> Wake, mut refresh: impl FnMut(), every: Duration, settle: Duration) {
    let mut periodic = Instant::now() + every;
    let mut due = periodic;
    loop {
        match wait(due) {
            Wake::Closed => return,
            Wake::IpChange => due = (Instant::now() + settle).min(periodic),
            Wake::Other | Wake::Deadline => {}
        }
        // Checked on every wake: a steady stream of other events cannot starve the refresh.
        if Instant::now() >= due {
            refresh();
            periodic = Instant::now() + every;
            due = periodic;
        }
    }
}

impl Advertiser {
    /// Advertise (signed TXT: v, id, pair, pairport, proto, ts, sig). Dropping stops the advert.
    /// Inert (`active() == false`) when mDNS is disabled. A specific `config.bind_addr` (e.g.
    /// 127.0.0.1 for a SINGLE-HOST run) is the only address advertised, so nobody is told to dial
    /// an interface nothing listens on; 0.0.0.0 / :: advertises every interface address.
    pub fn start(
        config: &NetworkConfig,
        identity: Arc<DeviceIdentity>,
        port: u16,
        pairing_port: Option<u16>,
    ) -> Result<Advertiser> {
        if !mdns_enabled(config) {
            tracing::info!("mdns disabled: not advertising");
            return Ok(Advertiser { state: None });
        }
        let short: String = identity.id.to_hex().chars().take(12).collect();
        let state = Arc::new(AdvertState {
            daemon: ServiceDaemon::new().map_err(mdns_err)?,
            identity,
            port,
            pairing_port,
            bound: config.bind_addr.parse().ok().filter(|ip: &IpAddr| !ip.is_unspecified()),
            instance: short.clone(),
            host: format!("common-{short}.local."),
            fullname: Mutex::new(String::new()),
        });
        let events = state.daemon.monitor().map_err(mdns_err)?;
        state.register(true)?;
        tracing::info!(instance = %short, port, pairing = pairing_port.is_some(), "mdns advertising");
        // Weak: a dropped Advertiser is never re-announced, even if the daemon's shutdown failed.
        // The thread ends when `drop` shuts the daemon down (the monitor channel disconnects).
        let refresher = Arc::downgrade(&state);
        std::thread::spawn(move || {
            refresh_loop(
                |due| wake(events.recv_deadline(due)),
                || {
                    if let Some(s) = refresher.upgrade() {
                        if let Err(e) = s.register(false) {
                            tracing::warn!(error = %e, "mdns refresh failed");
                        }
                    }
                },
                REFRESH,
                IP_SETTLE,
            )
        });
        Ok(Advertiser { state: Some(state) })
    }

    pub fn active(&self) -> bool {
        self.state.is_some()
    }

    /// Re-sign with a fresh `ts` and re-announce now (the refresh thread does this every 120 s and
    /// 2 s after an interface change).
    pub fn refresh(&self) -> Result<()> {
        match &self.state {
            Some(s) => s.register(false),
            None => Ok(()),
        }
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        if let Some(s) = &self.state {
            let fullname = lock(&s.fullname).clone();
            if let Ok(rx) = s.daemon.unregister(&fullname) {
                // Goodbye packets go out before the daemon thread exits.
                let _ = rx.recv_timeout(Duration::from_secs(1));
            }
            let _ = s.daemon.shutdown();
        }
    }
}

struct Seen {
    peer: DiscoveredPeer,
    ts: u64,
    at: Instant,
    fullname: String,
    /// Removed by an mDNS goodbye or expiry ([`forget`]): not listed, still the replay guard.
    gone: bool,
}

impl Seen {
    /// Kept exactly while a replay of its record would still verify (the replay guard); listing
    /// also needs a recent `at` ([`Browser::peers`]).
    fn fresh(&self, now_unix: u64) -> bool {
        now_unix.abs_diff(self.ts) <= MAX_RECORD_AGE_SECS
    }
}

/// Insert a verified record: stale entries are evicted first; past [`MAX_SEEN`] the oldest entry
/// goes. For a known id (listed or gone) only a strictly newer `ts` replaces the entry. A record
/// that is not newer is a replay or the owner re-announcing (mdns-sd re-resolves whenever a cached
/// record changes): with the entry's own addresses it only marks the device heard (and listed
/// again after a goodbye); with any other address it is ignored. So a replay cannot add, strip or
/// redirect a dial candidate, nor roll back the name, pairing window or `ts`.
// ponytail: an address the owner lost stays a dial candidate until its next re-sign (IP_SETTLE
// after the change, else REFRESH); the other candidates are still dialled.
fn remember(map: &mut HashMap<PeerId, Seen>, new: Seen, now_unix: u64) {
    map.retain(|_, s| s.fresh(now_unix));
    if let Some(old) = map.get_mut(&new.peer.id).filter(|old| new.ts <= old.ts) {
        if new.peer.addrs.iter().all(|a| old.peer.addrs.contains(a)) {
            old.at = new.at;
            old.peer.seen_utc = new.peer.seen_utc;
            old.gone = false;
        } else {
            tracing::debug!(id = %new.peer.id.short_label(), "discovery record for a known id with another address ignored (not newer)");
        }
        return;
    }
    // ponytail: a flood of MAX_SEEN freshly minted ids still evicts a guard (TLS key pinning is the
    // backstop); evict unpinned ids first if that matters.
    if !map.contains_key(&new.peer.id) && map.len() >= MAX_SEEN {
        if let Some(oldest) = map.iter().min_by_key(|(_, s)| s.at).map(|(id, _)| *id) {
            map.remove(&oldest);
        }
    }
    map.insert(new.peer.id, new);
}

pub struct Browser {
    daemon: Option<ServiceDaemon>,
    seen: Arc<Mutex<HashMap<PeerId, Seen>>>,
    store: Arc<PeerStore>,
}

/// Dial candidates, best first, at most [`MAX_DIAL_ADDRS`]. Loopback and link-local addresses only
/// when nothing else is advertised (a LAN peer must never be told to dial 127.0.0.1, and a
/// link-local IPv6 address needs a scope id we do not carry); then IPv4 (what the workload endpoint
/// binds by default) before IPv6; lowest address first.
// ponytail: sequential dials, cap 2; happy-eyeballs if dial latency matters
fn rank_addrs(ips: impl Iterator<Item = IpAddr>, port: u16) -> Vec<SocketAddr> {
    let local_only = |ip: &IpAddr| match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_link_local(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unicast_link_local(),
    };
    let mut ips: Vec<IpAddr> = ips.collect();
    if ips.iter().any(|ip| !local_only(ip)) {
        ips.retain(|ip| !local_only(ip));
    }
    ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    ips.dedup();
    ips.into_iter()
        .take(MAX_DIAL_ADDRS)
        .map(|ip| SocketAddr::new(ip, port))
        .collect()
}

/// A `ServiceRemoved` for `fullname` (goodbye or expiry): hides the entry last seen under that
/// instance name. An id re-announced under another name ("x (2)") stays listed. Goodbyes are
/// unauthenticated (anyone on the LAN can send one), so the entry stays as the replay guard.
fn forget(map: &mut HashMap<PeerId, Seen>, fullname: &str) {
    for s in map.values_mut().filter(|s| s.fullname == fullname) {
        s.gone = true;
    }
}

impl Browser {
    /// Browse `_common._udp.local.`; `store` decides `trusted`. Inert when mDNS is disabled.
    /// Lists every valid record, this device's own included; a daemon uses [`Browser::start_excluding`].
    pub fn start(config: &NetworkConfig, store: Arc<PeerStore>) -> Result<Browser> {
        Self::start_with(config, store, None)
    }

    /// [`Browser::start`] that never lists `own` (this device's id: its own advert comes back
    /// over multicast loopback, and dialling itself is never useful).
    pub fn start_excluding(config: &NetworkConfig, store: Arc<PeerStore>, own: PeerId) -> Result<Browser> {
        Self::start_with(config, store, Some(own))
    }

    fn start_with(config: &NetworkConfig, store: Arc<PeerStore>, own: Option<PeerId>) -> Result<Browser> {
        let seen = Arc::new(Mutex::new(HashMap::new()));
        if !mdns_enabled(config) {
            tracing::info!("mdns disabled: not browsing");
            return Ok(Browser {
                daemon: None,
                seen,
                store,
            });
        }
        let daemon = ServiceDaemon::new().map_err(mdns_err)?;
        let rx = daemon.browse(SERVICE_TYPE).map_err(mdns_err)?;
        let map = seen.clone();
        std::thread::spawn(move || {
            for ev in rx.iter() {
                match ev {
                    ServiceEvent::ServiceResolved(info) => {
                        let txt = info.txt_properties.clone().into_property_map_str();
                        let addrs = rank_addrs(info.addresses.iter().map(|a| a.to_ip_addr()), info.port);
                        match (Advert::verify(&txt, info.port, now_unix()), addrs.first().copied()) {
                            (Ok(advert), _) if Some(advert.id) == own => {}
                            (Ok(advert), Some(addr)) => {
                                let peer = DiscoveredPeer {
                                    id: advert.id,
                                    addr,
                                    addrs,
                                    pairing: advert.pairing,
                                    pair_port: advert.pair_port,
                                    proto: advert.proto,
                                    trusted: false,
                                    seen_utc: utc_now(),
                                };
                                let seen = Seen {
                                    peer,
                                    ts: advert.ts,
                                    at: Instant::now(),
                                    fullname: info.fullname.clone(),
                                    gone: false,
                                };
                                remember(&mut lock(&map), seen, now_unix());
                            }
                            (Err(e), _) => {
                                tracing::debug!(name = %info.fullname, error = %e, "discovery record dropped")
                            }
                            (_, None) => tracing::debug!(name = %info.fullname, "discovery record without address"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => forget(&mut lock(&map), &fullname),
                    _ => {}
                }
            }
        });
        Ok(Browser {
            daemon: Some(daemon),
            seen,
            store,
        })
    }

    pub fn active(&self) -> bool {
        self.daemon.is_some()
    }

    /// Peers seen in the last `max_age_secs` with valid signatures. Unsigned/expired records are dropped.
    pub fn peers(&self, max_age_secs: u64) -> Vec<DiscoveredPeer> {
        let now = now_unix();
        let mut seen = lock(&self.seen);
        seen.retain(|_, s| s.fresh(now));
        let mut out: Vec<DiscoveredPeer> = seen
            .values()
            .filter(|s| !s.gone && s.at.elapsed().as_secs() <= max_age_secs)
            .map(|s| DiscoveredPeer {
                trusted: store_verdict(&self.store, &s.peer.id) == Verdict::Allowed,
                ..s.peer.clone()
            })
            .collect();
        out.sort_by_key(|p| p.id);
        out
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if let Some(d) = &self.daemon {
            let _ = d.stop_browse(SERVICE_TYPE);
            let _ = d.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn advert(id: PeerId, ts: u64) -> Advert {
        Advert {
            id,
            port: 8193,
            pairing: true,
            pair_port: Some(40123),
            proto: "1.0".into(),
            ts,
        }
    }

    #[test]
    fn txt_sign_verify_round_trip() {
        let me = DeviceIdentity::generate().unwrap();
        let a = advert(me.id, 1_760_000_000);
        let txt = a.sign(&me);
        assert_eq!(txt.len(), 7);
        assert!(
            txt.values().all(|v| v.len() + 10 < 255),
            "TXT entries stay under 255 bytes"
        );
        assert_eq!(Advert::verify(&txt, 8193, 1_760_000_100).unwrap(), a);
        // The SRV port is signed: the same TXT behind another port does not verify.
        assert!(Advert::verify(&txt, 8194, 1_760_000_100).is_err());
        // Without a pairing port the key is absent and still verifies.
        let quiet = Advert {
            pairing: false,
            pair_port: None,
            ..a.clone()
        };
        let txt = quiet.sign(&me);
        assert!(!txt.contains_key("pairport"));
        assert_eq!(Advert::verify(&txt, 8193, 1_760_000_000).unwrap(), quiet);
    }

    #[test]
    fn tampered_unsigned_and_foreign_records_rejected() {
        let (me, other) = (DeviceIdentity::generate().unwrap(), DeviceIdentity::generate().unwrap());
        let now = 1_760_000_000;
        let good = advert(me.id, now).sign(&me);
        for (key, value) in [
            ("pair", "0"),
            ("pairport", "1"),
            ("proto", "2.0"),
            ("ts", "1760000001"),
            ("id", &other.id.to_hex()),
            ("sig", "00"),
            ("sig", ""),
            ("v", "2"),
        ] {
            let mut t = good.clone();
            t.insert(key.into(), value.into());
            let e = Advert::verify(&t, 8193, now).unwrap_err();
            assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{key}={value}: {e}");
        }
        for key in ["sig", "id", "ts", "v", "pair", "proto"] {
            let mut t = good.clone();
            t.remove(key);
            assert!(Advert::verify(&t, 8193, now).is_err(), "missing {key}");
        }
        // Signed by someone else over our id.
        let forged = advert(me.id, now).sign(&other);
        assert!(Advert::verify(&forged, 8193, now)
            .unwrap_err()
            .message
            .contains("signature"));
        assert!(Advert::verify(&good, 8193, now).is_ok());
    }

    #[test]
    fn stale_records_rejected_both_directions() {
        let me = DeviceIdentity::generate().unwrap();
        let ts = 1_760_000_000;
        let txt = advert(me.id, ts).sign(&me);
        assert!(Advert::verify(&txt, 8193, ts + MAX_RECORD_AGE_SECS).is_ok());
        assert!(Advert::verify(&txt, 8193, ts - MAX_RECORD_AGE_SECS).is_ok());
        let e = Advert::verify(&txt, 8193, ts + MAX_RECORD_AGE_SECS + 1).unwrap_err();
        assert!(e.message.contains("stale"), "{e}");
        assert!(Advert::verify(&txt, 8193, ts - MAX_RECORD_AGE_SECS - 1).is_err());
    }

    #[test]
    fn disabled_config_is_inert() {
        let off = NetworkConfig {
            enabled: true,
            mdns: false,
            ..NetworkConfig::default()
        };
        assert!(!mdns_enabled(&off));
        assert!(
            !mdns_enabled(&NetworkConfig::default()),
            "enabled=false implies no mDNS"
        );
        let me = Arc::new(DeviceIdentity::generate().unwrap());
        let a = Advertiser::start(&off, me.clone(), 8193, None).unwrap();
        assert!(!a.active());
        a.refresh().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let paths = common_core::Paths::at(dir.path());
        paths.ensure_dirs().unwrap();
        let store = Arc::new(PeerStore::open(&paths).unwrap());
        let b = Browser::start(&off, store).unwrap();
        assert!(!b.active());
        assert!(b.peers(10).is_empty());
    }

    fn seen(id: PeerId, addr: &str, ts: u64) -> Seen {
        seen_at(id, &[addr], ts)
    }

    fn seen_at(id: PeerId, addrs: &[&str], ts: u64) -> Seen {
        let addrs: Vec<SocketAddr> = addrs.iter().map(|a| a.parse().unwrap()).collect();
        Seen {
            peer: DiscoveredPeer {
                id,
                addr: addrs[0],
                addrs,
                pairing: false,
                pair_port: None,
                proto: "1.0".into(),
                trusted: false,
                seen_utc: String::new(),
            },
            ts,
            at: Instant::now(),
            fullname: String::new(),
            gone: false,
        }
    }

    #[test]
    fn table_is_bounded_and_replays_cannot_redirect() {
        let now = 1_760_000_000;
        let id = PeerId([1; 32]);
        let mut map = HashMap::new();
        remember(&mut map, seen(id, "10.0.0.2:8193", now), now);
        // Same ts (a replay) or older from another address: ignored.
        remember(&mut map, seen(id, "10.0.0.66:8193", now), now);
        remember(&mut map, seen(id, "10.0.0.66:8193", now - 10), now);
        assert_eq!(map[&id].peer.addr, "10.0.0.2:8193".parse().unwrap());
        // A re-announce from the same address is accepted but cannot roll the ts back; a strictly
        // newer record from a new address wins.
        remember(&mut map, seen(id, "10.0.0.2:8193", now - 5), now);
        assert_eq!(map[&id].ts, now);
        remember(&mut map, seen(id, "10.0.0.3:8193", now + 1), now);
        assert_eq!(map[&id].peer.addr, "10.0.0.3:8193".parse().unwrap());
        // A stale entry is evicted by the next insert.
        map.insert(
            PeerId([2; 32]),
            seen(PeerId([2; 32]), "10.0.0.4:1", now - MAX_RECORD_AGE_SECS - 1),
        );
        remember(&mut map, seen(PeerId([3; 32]), "10.0.0.5:1", now), now);
        assert!(!map.contains_key(&PeerId([2; 32])));
        // A flood of fresh ids keeps at most MAX_SEEN entries, dropping the oldest first.
        for i in 0..(MAX_SEEN as u32 + 50) {
            let mut b = [9u8; 32];
            b[..4].copy_from_slice(&i.to_be_bytes());
            remember(&mut map, seen(PeerId(b), "10.0.1.1:1", now), now);
        }
        assert_eq!(map.len(), MAX_SEEN);
        assert!(!map.contains_key(&id), "the oldest entries went first");
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn pick_addr(ips: impl Iterator<Item = IpAddr>, port: u16) -> Option<SocketAddr> {
        rank_addrs(ips, port).first().copied()
    }

    #[test]
    fn pick_addr_prefers_ipv4() {
        let ips = [ip("fe80::1"), ip("192.168.1.5"), ip("10.0.0.9")];
        assert_eq!(
            pick_addr(ips.iter().copied(), 7).unwrap(),
            "10.0.0.9:7".parse::<SocketAddr>().unwrap()
        );
        assert!(pick_addr(ips[..1].iter().copied(), 7).unwrap().is_ipv6());
        assert!(pick_addr(std::iter::empty(), 7).is_none());
    }

    #[test]
    fn pick_addr_never_offers_loopback_or_link_local_when_a_lan_address_exists() {
        let pick = |v: &[&str]| pick_addr(v.iter().map(|s| ip(s)), 7).unwrap().ip();
        // 127.0.0.1 sorts below every LAN IPv4 address; it must still lose.
        assert_eq!(pick(&["127.0.0.1", "192.168.1.5"]), ip("192.168.1.5"));
        assert_eq!(pick(&["169.254.3.4", "127.0.0.1", "10.0.0.9"]), ip("10.0.0.9"));
        // A routable IPv6 address beats IPv4 loopback and link-local.
        assert_eq!(pick(&["127.0.0.1", "169.254.3.4", "fe80::1", "fd00::5"]), ip("fd00::5"));
        assert_eq!(pick(&["::1", "fe80::1", "2001:db8::1"]), ip("2001:db8::1"));
        // Only local addresses: still an answer (SINGLE-HOST tests advertise on loopback), IPv4 first.
        assert_eq!(pick(&["fe80::1", "127.0.0.1"]), ip("127.0.0.1"));
        assert_eq!(pick(&["::1"]), ip("::1"));
    }

    /// D2: the Windows laptop advertised its hotspot address plus link-local addresses of
    /// disconnected adapters. Both routable addresses are kept (best first); local ones never
    /// while a routable one exists.
    #[test]
    fn rank_addrs_keeps_two_routable_addresses() {
        let rank = |v: &[&str]| rank_addrs(v.iter().map(|s| ip(s)), 8193);
        let sa = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert_eq!(
            rank(&["172.20.10.6", "10.0.0.9", "169.254.3.4", "127.0.0.1"]),
            vec![sa("10.0.0.9:8193"), sa("172.20.10.6:8193")]
        );
        assert_eq!(rank(&["172.20.10.6", "172.20.10.6"]), vec![sa("172.20.10.6:8193")]);
        assert_eq!(
            rank(&["fd00::5", "192.168.1.5", "10.0.0.9"]),
            vec![sa("10.0.0.9:8193"), sa("192.168.1.5:8193")]
        );
        assert_eq!(rank(&["169.254.3.4", "127.0.0.1"])[0], sa("127.0.0.1:8193"));
        assert!(rank(&[]).is_empty());
    }

    /// Duplicate adverts: a replayed record (same id, not newer) cannot add a dial candidate, not
    /// even after first rolling the entry's ts back with an older replay of the same address.
    #[test]
    fn replayed_record_cannot_add_a_dial_candidate() {
        let now = 1_760_000_000;
        let id = PeerId([7; 32]);
        let (a, b) = ("172.20.10.6:8193", "10.66.0.1:8193");
        let mut map = HashMap::new();
        remember(&mut map, seen_at(id, &[a], now), now);
        remember(&mut map, seen_at(id, &[a, b], now), now);
        assert_eq!(map[&id].peer.addrs.len(), 1, "same ts may not add {b}");
        remember(&mut map, seen_at(id, &[a], now - 10), now);
        remember(&mut map, seen_at(id, &[a, b], now - 5), now);
        assert_eq!(map[&id].peer.addrs.len(), 1, "a replay cannot roll the ts back");
        assert_eq!(map[&id].ts, now);
        // The owner's next signed record (newer ts) adds it. A not-newer record with fewer
        // addresses cannot strip one (an attacker could strip the one that works); the owner's
        // next re-sign narrows the entry.
        remember(&mut map, seen_at(id, &[b, a], now + 1), now);
        let both: Vec<SocketAddr> = vec![b.parse().unwrap(), a.parse().unwrap()];
        assert_eq!(map[&id].peer.addrs, both);
        remember(&mut map, seen_at(id, &[a], now + 1), now);
        assert_eq!(map[&id].peer.addrs, both, "a replay cannot strip {b}");
        remember(&mut map, seen_at(id, &[a], now + 2), now);
        assert_eq!(map[&id].peer.addrs, vec![a.parse().unwrap()]);
    }

    /// Duplicate adverts: one device under two instance names (mdns-sd renames on a probe
    /// conflict) is one entry; the goodbye of the old name does not remove it.
    #[test]
    fn duplicate_adverts_for_one_id_are_one_entry() {
        let now = 1_760_000_000;
        let id = PeerId([8; 32]);
        let mut map = HashMap::new();
        let named = |name: &str, ts| Seen {
            fullname: name.into(),
            ..seen(id, "172.20.10.6:8193", ts)
        };
        let mut pairing_x = named("x", now);
        pairing_x.peer.pairing = true;
        pairing_x.peer.pair_port = Some(40123);
        remember(&mut map, pairing_x, now);
        remember(&mut map, named("x (2)", now + 1), now);
        assert_eq!(map.len(), 1);
        assert_eq!(map[&id].fullname, "x (2)");
        // A late copy of the older record (or a replay of it) changes nothing: not the name, not
        // the pairing window, not the ts.
        let mut late = named("x", now);
        late.peer.pairing = true;
        late.peer.pair_port = Some(40123);
        remember(&mut map, late, now);
        let e = &map[&id];
        assert_eq!(
            (e.fullname.as_str(), e.peer.pairing, e.peer.pair_port, e.ts),
            ("x (2)", false, None, now + 1)
        );
        forget(&mut map, "x");
        assert!(!map[&id].gone, "the old name's goodbye does not hide the device");
        forget(&mut map, "x (2)");
        assert!(map[&id].gone);
    }

    /// Two records signed in one second (pairing opened, then closed) get increasing ts, so
    /// browsers take the second one: no stale "Pair" for a closed window.
    #[test]
    fn records_signed_in_one_second_replace_each_other() {
        let now = 1_760_000_000;
        assert_eq!(next_ts(now, 0), now);
        assert_eq!(next_ts(now, now), now + 1);
        assert_eq!(next_ts(now, now + 3), now + 4);
        assert_eq!(next_ts(now, now + 3600), now, "a clock set back restarts from now");
        let (t1, t2) = (signed_ts(), signed_ts());
        assert!(t2 > t1, "{t1} {t2}");
        let id = PeerId([9; 32]);
        let mut open = seen(id, "172.20.10.6:8193", t1);
        open.peer.pairing = true;
        open.peer.pair_port = Some(40123);
        let mut map = HashMap::new();
        remember(&mut map, open, t1);
        remember(&mut map, seen(id, "172.20.10.6:8193", t2), t1);
        assert_eq!(
            (map[&id].peer.pairing, map[&id].peer.pair_port, map[&id].ts),
            (false, None, t2)
        );
    }

    fn inert_browser() -> (tempfile::TempDir, Browser) {
        let dir = tempfile::tempdir().unwrap();
        let paths = common_core::Paths::at(dir.path());
        paths.ensure_dirs().unwrap();
        let store = Arc::new(PeerStore::open(&paths).unwrap());
        let b = Browser::start(&NetworkConfig::default(), store).unwrap();
        assert!(!b.active());
        (dir, b)
    }

    /// Goodbyes are unauthenticated (mdns-sd 0.21.5 turns any TTL-0 answer into TTL 1 and emits
    /// `ServiceRemoved`), and a record from a peer whose clock runs ahead verifies for longer than
    /// it is listed. Neither may drop the replay guard: the last signed record replayed under
    /// another name with other addresses must not become a dial candidate.
    #[test]
    fn goodbye_and_age_never_drop_the_replay_guard() {
        let (_dir, b) = inert_browser();
        let now = now_unix();
        let id = PeerId([5; 32]);
        let (a, x) = ("172.20.10.6:8193", "10.66.0.1:8193");
        let named = |name: &str, addr: &str, ts| Seen {
            fullname: name.into(),
            ..seen(id, addr, ts)
        };
        let listed = || -> Vec<Vec<SocketAddr>> { b.peers(MAX_RECORD_AGE_SECS).into_iter().map(|p| p.addrs).collect() };
        remember(&mut lock(&b.seen), named("x", a, now), now);
        forget(&mut lock(&b.seen), "x"); // forged goodbye
        assert!(listed().is_empty());
        remember(&mut lock(&b.seen), named("evil", x, now), now);
        assert!(listed().is_empty(), "a replay after a goodbye cannot bring {x}");
        // The owner's own re-announce (same ts, same address) brings it back unchanged.
        remember(&mut lock(&b.seen), named("x", a, now), now);
        assert_eq!(listed(), vec![vec![a.parse::<SocketAddr>().unwrap()]]);
        // Heard 400 s ago with a ts 200 s ahead of this clock: no longer listed, but a replay of
        // that record would still verify for 100 s, so the guard stays.
        let at = Instant::now()
            .checked_sub(Duration::from_secs(400))
            .expect("uptime above 400 s");
        lock(&b.seen).insert(
            id,
            Seen {
                at,
                ..named("x", a, now + 200)
            },
        );
        remember(&mut lock(&b.seen), named("evil", x, now + 200), now);
        assert!(listed().is_empty());
        assert_eq!(lock(&b.seen)[&id].peer.addrs, vec![a.parse::<SocketAddr>().unwrap()]);
    }

    /// Discovery timeouts: an entry not re-announced for longer than the age bound is neither
    /// listed nor kept (the backstop behind mdns-sd's own SRV expiry, 120 s).
    #[test]
    fn vanished_peer_leaves_the_table() {
        let (_dir, b) = inert_browser();
        let now = now_unix();
        for (i, age) in [10u64, 301, 400].into_iter().enumerate() {
            let id = PeerId([i as u8 + 1; 32]);
            let at = Instant::now()
                .checked_sub(Duration::from_secs(age))
                .expect("uptime above 400 s");
            lock(&b.seen).insert(
                id,
                Seen {
                    at,
                    ..seen(id, "10.0.0.2:8193", now - age)
                },
            );
        }
        let listed: Vec<PeerId> = b.peers(MAX_RECORD_AGE_SECS).into_iter().map(|p| p.id).collect();
        assert_eq!(listed, vec![PeerId([1; 32])]);
        assert!(b.peers(5).is_empty());
        assert_eq!(lock(&b.seen).len(), 1, "stale entries are evicted");
    }

    #[test]
    fn interface_events_wake_the_refresh() {
        let ip_add = DaemonEvent::IpAdd(ip("172.20.10.2"));
        assert_eq!(wake(Ok(ip_add)), Wake::IpChange);
        assert_eq!(wake(Ok(DaemonEvent::IpDel(ip("10.0.0.9")))), Wake::IpChange);
        assert_eq!(wake(Ok(DaemonEvent::Respond("en0".into()))), Wake::Other);
        assert_eq!(wake(Err(RecvTimeoutError::Timeout)), Wake::Deadline);
        assert_eq!(wake(Err(RecvTimeoutError::Disconnected)), Wake::Closed);
    }

    /// D1 interface change: the Mac moved between the hotspot and eduroam on its own. A re-sign
    /// follows an address change after `settle` (one for a burst), not the next periodic refresh.
    #[test]
    fn refresh_follows_ip_changes_after_settling() {
        use std::sync::mpsc;
        let settle = Duration::from_millis(50);
        let run = |every: Duration| {
            let (tx, rx) = mpsc::channel::<Wake>();
            let (done_tx, done) = mpsc::channel::<Instant>();
            let t = std::thread::spawn(move || {
                refresh_loop(
                    |due| match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                        Ok(w) => w,
                        Err(mpsc::RecvTimeoutError::Timeout) => Wake::Deadline,
                        Err(mpsc::RecvTimeoutError::Disconnected) => Wake::Closed,
                    },
                    || done_tx.send(Instant::now()).unwrap(),
                    every,
                    settle,
                )
            });
            (tx, done, t)
        };
        let quiet = |done: &mpsc::Receiver<Instant>, ms| done.recv_timeout(Duration::from_millis(ms)).is_err();

        let (tx, done, t) = run(Duration::from_secs(10));
        let t0 = Instant::now();
        tx.send(Wake::IpChange).unwrap();
        let took = done
            .recv_timeout(Duration::from_secs(2))
            .expect("refresh after an IP change")
            - t0;
        assert!(
            took >= Duration::from_millis(40) && took <= Duration::from_millis(200),
            "{took:?}"
        );
        assert!(quiet(&done, 300), "exactly one refresh");
        // A burst (IpDel then IpAdd within settle) gives one refresh, settle after the last event.
        tx.send(Wake::IpChange).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let last = Instant::now();
        tx.send(Wake::IpChange).unwrap();
        tx.send(Wake::Other).unwrap();
        let took = done
            .recv_timeout(Duration::from_secs(2))
            .expect("one refresh for the burst")
            - last;
        assert!(took >= Duration::from_millis(40), "{took:?}");
        assert!(quiet(&done, 300), "one refresh for the burst");
        drop(tx);
        t.join().expect("Closed ends the loop");

        // No events: the periodic refresh still runs.
        let (tx, done, t) = run(Duration::from_millis(100));
        assert!(done.recv_timeout(Duration::from_secs(2)).is_ok());
        assert!(done.recv_timeout(Duration::from_secs(2)).is_ok());
        drop(tx);
        t.join().unwrap();
    }
}
