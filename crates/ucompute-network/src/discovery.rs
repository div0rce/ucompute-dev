//! mDNS-SD discovery (ADR-0003 D4): `_ucompute._udp.local.`, signed TXT, no secrets.
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
//! vanishes without a goodbye leaves the list once it has not been heard for [`LISTED_SECS`]. A
//! live peer is heard at every re-sign announcement ([`REFRESH`]); when a listed peer has missed
//! one ([`OVERDUE`]), the browser re-queries (a fresh browse, so mdns-sd's cache and its
//! known-answer suppression start empty and every live peer answers in full), at most once per
//! [`REQUERY_GAP`] (2026-10-23: two missed re-sign announcements aged a live peer out). With every
//! announcement heard it never re-queries, so mdns-sd's own query backoff (doubling, RFC 6762
//! section 5.2) is all that is sent; a peer that vanished costs at most two re-queries.
//!
//! Only IPv4 addresses are dial candidates (the workload endpoint binds IPv4 only,
//! [`NetworkConfig::bind_ip`], and IPv6 is neither advertised nor browsed). A peer heard only at
//! IPv6 addresses is listed apart, with the reason ([`Browser::unreachable`]), and never dialled.
//!
//! The advertiser re-signs every [`REFRESH`], and [`IP_SETTLE`] after this host gains or loses an
//! address (mdns-sd re-announces the new address set within its 5 s interface poll, still with the
//! old `ts`, which browsers would not accept from a new address).
//!
//! Disabled (both sides inert, nothing bound) when `NetworkConfig.enabled` or `.mdns` is false
//! or the environment has `UCOMPUTE_MDNS=0`.
use crate::tls::{store_verdict, Verdict};
use crate::{NetworkConfig, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use mdns_sd::{DaemonEvent, IfKind, RecvTimeoutError, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use ucompute_core::{ErrorCode, Result, UComputeError};
use ucompute_identity::{utc_now, DeviceIdentity, PeerId, PeerStore};

pub const SERVICE_TYPE: &str = "_ucompute._udp.local.";
/// A signed record older (or newer) than this is ignored (ADR-0003 D4).
pub const MAX_RECORD_AGE_SECS: u64 = 300;
/// The advertiser re-signs and re-announces at this interval so its `ts` stays fresh.
const REFRESH: Duration = Duration::from_secs(120);
/// Re-sign this long after the last interface change: more than mdns-sd's 1 s probe (a re-sign
/// during the probe renamed the instance "x (2)"), and a burst of changes gives one re-sign.
const IP_SETTLE: Duration = Duration::from_secs(2);
/// The browser checks this often whether to re-query ([`requery_due`]).
const CHECK: Duration = Duration::from_secs(10);
/// A listed peer not heard for this long has missed its last re-sign announcement (one every
/// [`REFRESH`]).
const OVERDUE: Duration = Duration::from_secs(REFRESH.as_secs() + 5);
/// At least this between two re-queries: one restarts mdns-sd's query backoff, about 5 queries
/// over the next 31 s (1 s, doubling).
const REQUERY_GAP: Duration = Duration::from_secs(30);
/// A peer not heard for this long is no longer listed: one missed announcement leaves room for two
/// re-queries ([`requery_due`]) before the peer drops out.
pub const LISTED_SECS: u64 = 175;
const _: () = assert!(OVERDUE.as_secs() + CHECK.as_secs() + REQUERY_GAP.as_secs() < LISTED_SECS);
// Heard at least every LISTED_SECS, a live peer's record is never older than REFRESH + LISTED_SECS.
const _: () = assert!(REFRESH.as_secs() + LISTED_SECS < MAX_RECORD_AGE_SECS);
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

/// Why a peer with a valid signed record has no dial candidate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NoAddress {
    /// Heard only at IPv6 link-local addresses (`fe80::/10`): they need a scope id we do not carry.
    LinkLocalOnly,
    /// Heard only at IPv6 addresses, some routable: the workload endpoint is IPv4 only.
    OtherFamily,
}

/// A peer seen with a valid signed record but at no address this device can dial: "discovered, no
/// reachable address". Nothing is dialled from it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnreachablePeer {
    pub id: PeerId,
    pub reason: NoAddress,
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

fn mdns_err(e: mdns_sd::Error) -> UComputeError {
    UComputeError::io(format!("mdns: {e}"))
}

/// `enabled && mdns` from config, overridable by `UCOMPUTE_MDNS=0`.
pub fn mdns_enabled(config: &NetworkConfig) -> bool {
    config.enabled && config.mdns && std::env::var("UCOMPUTE_MDNS").map(|v| v != "0").unwrap_or(true)
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
        let bad = |m: &str| UComputeError::new(ErrorCode::PeerUnauthorized, format!("discovery record: {m}"));
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
        if !ucompute_identity::verify(&id, advert.canonical().as_bytes(), &sig) {
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
    /// (0.0.0.0): every IPv4 interface address, as mdns-sd finds them.
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

/// The browse thread's receiver: listening, browsing again at the next check (a browse that
/// failed while the daemon still runs, e.g. `Error::Again` with its command queue full), or ended
/// (the daemon shut down).
enum Listen<R> {
    On(R),
    Retry,
    Gone,
}

fn listen<R>(r: mdns_sd::Result<R>) -> Listen<R> {
    match r {
        Ok(rx) => Listen::On(rx),
        Err(mdns_sd::Error::DaemonShutdown) => Listen::Gone,
        Err(e) => {
            tracing::warn!(error = %e, "mdns browse failed; retrying at the next check");
            Listen::Retry
        }
    }
}

/// The browse thread: `recv(rx, due)` waits on the current browse's receiver until `due`; every
/// `every`, `requery(rx)` keeps the receiver (`Some`) or has stopped that browse (`None`), and then
/// `browse()` starts a new one. A failed browse is retried at the next check rather than ending
/// discovery; the loop returns once the daemon is gone.
fn browse_loop<R>(
    first: R,
    mut browse: impl FnMut() -> mdns_sd::Result<R>,
    mut recv: impl FnMut(&R, Instant) -> Wake,
    mut requery: impl FnMut(R) -> Option<R>,
    every: Duration,
) {
    let state = RefCell::new(Listen::On(first));
    refresh_loop(
        |due| match &*state.borrow() {
            Listen::On(rx) => recv(rx, due),
            Listen::Retry => {
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                Wake::Deadline
            }
            Listen::Gone => Wake::Closed,
        },
        || {
            let next = match state.replace(Listen::Retry) {
                Listen::On(rx) => match requery(rx) {
                    Some(rx) => Listen::On(rx),
                    None => listen(browse()),
                },
                Listen::Retry => listen(browse()),
                Listen::Gone => Listen::Gone,
            };
            *state.borrow_mut() = next;
        },
        every,
        every,
    )
}

impl Advertiser {
    /// Advertise (signed TXT: v, id, pair, pairport, proto, ts, sig). Dropping stops the advert.
    /// Inert (`active() == false`) when mDNS is disabled. A specific `config.bind_addr` (e.g.
    /// 127.0.0.1 for a SINGLE-HOST run) is the only address advertised, so nobody is told to dial
    /// an interface nothing listens on; 0.0.0.0 advertises every IPv4 interface address (IPv6 is
    /// not advertised: nothing listens on it).
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
            bound: config.bind_ip().ok().filter(|ip| !ip.is_unspecified()).map(IpAddr::V4),
            instance: short.clone(),
            host: format!("ucompute-{short}.local."),
            fullname: Mutex::new(String::new()),
        });
        // Before `monitor`: dropping the IPv6 interfaces is no address change to re-sign for.
        state.daemon.disable_interface(IfKind::IPv6).map_err(mdns_err)?;
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

#[derive(Clone)]
struct Seen {
    peer: DiscoveredPeer,
    ts: u64,
    at: Instant,
    fullname: String,
    /// Removed by an mDNS goodbye or expiry ([`forget`]): not listed, still the replay guard.
    gone: bool,
    /// Heard at no address this device can dial: `peer.addrs` is empty (and `peer.addr` a
    /// placeholder); listed only by [`Browser::unreachable`].
    unreachable: Option<NoAddress>,
}

impl Seen {
    /// Kept exactly while a replay of its record would still verify (the replay guard); listing
    /// also needs a recent `at` ([`Browser::peers`]).
    fn fresh(&self, now_unix: u64) -> bool {
        now_unix.abs_diff(self.ts) <= MAX_RECORD_AGE_SECS
    }

    /// Listed at `now` (`now_unix` on the wall clock): not said goodbye, record still verifiable,
    /// heard within `max_age`.
    fn listed(&self, now_unix: u64, now: Instant, max_age: Duration) -> bool {
        !self.gone && self.fresh(now_unix) && now.saturating_duration_since(self.at) <= max_age
    }
}

/// Insert a verified record: stale entries are evicted first; past [`MAX_SEEN`] the oldest entry
/// goes. For a known id (listed or gone) only a strictly newer `ts` replaces the entry. A record
/// that is not newer is a replay or the owner re-announcing (mdns-sd re-resolves whenever a cached
/// record changes): with the entry's own addresses it only marks the device heard (and listed
/// again after a goodbye); with any other address it is ignored. So a replay cannot add, strip or
/// redirect a dial candidate, nor roll back the name, pairing window or `ts`. The one exception: a
/// record with the same `ts` fills an entry that has no dial candidate (mdns-sd resolves a record
/// once per address family it hears it on, so its first copy may carry only link-local
/// addresses); with nothing to redirect, that is the same as hearing the record first.
// ponytail: an address the owner lost stays a dial candidate until its next re-sign (IP_SETTLE
// after the change, else REFRESH); the other candidates are still dialled.
fn remember(map: &mut HashMap<PeerId, Seen>, new: Seen, now_unix: u64) {
    map.retain(|_, s| s.fresh(now_unix));
    let fills = |old: &Seen| new.ts == old.ts && old.peer.addrs.is_empty() && !new.peer.addrs.is_empty();
    if let Some(old) = map.get_mut(&new.peer.id).filter(|old| new.ts <= old.ts && !fills(old)) {
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

/// Dial candidates, best first, at most [`MAX_DIAL_ADDRS`]: IPv4 addresses only (the workload
/// endpoint binds IPv4); `Err` says why when that leaves none. Loopback and link-local addresses
/// only when nothing else is left (a LAN peer must never be told to dial 127.0.0.1); lowest
/// address first. No addresses at all: `Ok` and empty.
// ponytail: sequential dials, cap 2; happy-eyeballs if dial latency matters
fn rank_addrs(ips: impl Iterator<Item = IpAddr>, port: u16) -> std::result::Result<Vec<SocketAddr>, NoAddress> {
    let heard: Vec<IpAddr> = ips.collect();
    let mut v4: Vec<Ipv4Addr> = heard
        .iter()
        .filter_map(|ip| match ip {
            IpAddr::V4(v) => Some(*v),
            IpAddr::V6(_) => None,
        })
        .collect();
    if v4.is_empty() && !heard.is_empty() {
        let link_local = |ip: &IpAddr| matches!(ip, IpAddr::V6(v) if v.is_unicast_link_local());
        return Err(if heard.iter().all(link_local) {
            NoAddress::LinkLocalOnly
        } else {
            NoAddress::OtherFamily
        });
    }
    let local_only = |v: &Ipv4Addr| v.is_loopback() || v.is_link_local();
    if v4.iter().any(|v| !local_only(v)) {
        v4.retain(|v| !local_only(v));
    }
    v4.sort();
    v4.dedup();
    Ok(v4
        .into_iter()
        .take(MAX_DIAL_ADDRS)
        .map(|v| SocketAddr::new(v.into(), port))
        .collect())
}

/// Whether the browser should re-query now: some listed entry has missed a re-sign announcement
/// ([`OVERDUE`]) and the last re-query was at least [`REQUERY_GAP`] ago. A peer that vanished is
/// re-queried for only while it is still listed.
fn requery_due(map: &HashMap<PeerId, Seen>, now_unix: u64, now: Instant, last: Option<Instant>) -> bool {
    let listed = Duration::from_secs(LISTED_SECS);
    last.is_none_or(|l| now.saturating_duration_since(l) >= REQUERY_GAP)
        && map
            .values()
            .any(|s| s.listed(now_unix, now, listed) && now.saturating_duration_since(s.at) > OVERDUE)
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
    /// Browse `_ucompute._udp.local.`; `store` decides `trusted`. Inert when mDNS is disabled.
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
        // Answers sent over IPv6 carry only IPv6 addresses, which this endpoint cannot dial.
        daemon.disable_interface(IfKind::IPv6).map_err(mdns_err)?;
        let rx = daemon.browse(SERVICE_TYPE).map_err(mdns_err)?;
        let (map, d) = (seen.clone(), daemon.clone());
        std::thread::spawn(move || {
            let handle = |ev: ServiceEvent| match ev {
                ServiceEvent::ServiceResolved(info) => {
                    let txt = info.txt_properties.clone().into_property_map_str();
                    let ips = info.addresses.iter().map(|a| a.to_ip_addr());
                    match (Advert::verify(&txt, info.port, now_unix()), rank_addrs(ips, info.port)) {
                        (Ok(advert), _) if Some(advert.id) == own => {}
                        (Ok(_), Ok(addrs)) if addrs.is_empty() => {
                            tracing::debug!(name = %info.fullname, "discovery record without address")
                        }
                        (Ok(advert), ranked) => {
                            let unreachable = ranked.as_ref().err().copied();
                            let addrs = ranked.unwrap_or_default();
                            let peer = DiscoveredPeer {
                                id: advert.id,
                                // Never listed without addresses (`unreachable`).
                                addr: addrs
                                    .first()
                                    .copied()
                                    .unwrap_or(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), info.port)),
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
                                unreachable,
                            };
                            remember(&mut lock(&map), seen, now_unix());
                        }
                        (Err(e), _) => {
                            tracing::debug!(name = %info.fullname, error = %e, "discovery record dropped")
                        }
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => forget(&mut lock(&map), &fullname),
                _ => {}
            };
            let mut last_requery = None;
            browse_loop(
                rx,
                || d.browse(SERVICE_TYPE),
                |rx, due| match rx.recv_deadline(due) {
                    Ok(ev) => {
                        handle(ev);
                        Wake::Other
                    }
                    Err(e) => wake(Err(e)),
                },
                // Re-query: a new browse after `stop_browse` (which empties mdns-sd's cache for the
                // type) sends a query without known answers, so every live peer answers in full.
                |old| {
                    let now = Instant::now();
                    if !requery_due(&lock(&map), now_unix(), now, last_requery) {
                        return Some(old);
                    }
                    last_requery = Some(now);
                    if let Err(e) = d.stop_browse(SERVICE_TYPE) {
                        tracing::warn!(error = %e, "mdns re-query skipped");
                        return Some(old);
                    }
                    old.try_iter().for_each(&handle);
                    None
                },
                CHECK,
            )
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

    /// The entries listed now (heard in the last `max_age_secs`, valid signatures), by id; stale
    /// ones are evicted first.
    fn listed(&self, max_age_secs: u64) -> Vec<(bool, Seen)> {
        let now = now_unix();
        let mut seen = lock(&self.seen);
        seen.retain(|_, s| s.fresh(now));
        let mut out: Vec<(bool, Seen)> = seen
            .values()
            .filter(|s| s.listed(now, Instant::now(), Duration::from_secs(max_age_secs)))
            .map(|s| {
                let trusted = store_verdict(&self.store, &s.peer.id) == Verdict::Allowed;
                (trusted, s.clone())
            })
            .collect();
        out.sort_by_key(|(_, s)| s.peer.id);
        out
    }

    /// Peers seen in the last `max_age_secs` with valid signatures and an address this device can
    /// dial. Unsigned/expired records are dropped.
    pub fn peers(&self, max_age_secs: u64) -> Vec<DiscoveredPeer> {
        self.listed(max_age_secs)
            .into_iter()
            .filter(|(_, s)| s.unreachable.is_none())
            .map(|(trusted, s)| DiscoveredPeer { trusted, ..s.peer })
            .collect()
    }

    /// Peers seen in the last `max_age_secs` with valid signatures but no address this device can
    /// dial, with the reason. Never in [`Browser::peers`], so never dialled from discovery.
    pub fn unreachable(&self, max_age_secs: u64) -> Vec<UnreachablePeer> {
        self.listed(max_age_secs)
            .into_iter()
            .filter_map(|(trusted, s)| {
                Some(UnreachablePeer {
                    id: s.peer.id,
                    reason: s.unreachable?,
                    trusted,
                    seen_utc: s.peer.seen_utc,
                })
            })
            .collect()
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

    /// D-028: one service type, the renamed one; the former `_common._udp` is neither advertised nor browsed.
    #[test]
    fn only_the_ucompute_service_type() {
        assert_eq!(SERVICE_TYPE, "_ucompute._udp.local.");
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
        let paths = ucompute_core::Paths::at(dir.path());
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
            unreachable: None,
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

    /// Best candidate.
    fn pick_addr(ips: impl Iterator<Item = IpAddr>, port: u16) -> Option<SocketAddr> {
        rank_addrs(ips, port).ok()?.first().copied()
    }

    #[test]
    fn pick_addr_takes_the_lowest_ipv4_address() {
        let ips = [ip("fe80::1"), ip("192.168.1.5"), ip("10.0.0.9"), ip("fd00::5")];
        assert_eq!(
            pick_addr(ips.iter().copied(), 7).unwrap(),
            "10.0.0.9:7".parse::<SocketAddr>().unwrap()
        );
        assert!(pick_addr(std::iter::empty(), 7).is_none());
    }

    #[test]
    fn pick_addr_never_offers_loopback_or_link_local_when_a_lan_address_exists() {
        let pick = |v: &[&str]| pick_addr(v.iter().map(|s| ip(s)), 7).unwrap().ip();
        // 127.0.0.1 sorts below every LAN IPv4 address; it must still lose.
        assert_eq!(pick(&["127.0.0.1", "192.168.1.5"]), ip("192.168.1.5"));
        assert_eq!(pick(&["169.254.3.4", "127.0.0.1", "10.0.0.9"]), ip("10.0.0.9"));
        // Only local addresses: still an answer (SINGLE-HOST tests advertise on loopback).
        assert_eq!(pick(&["fe80::1", "169.254.3.4", "127.0.0.1"]), ip("127.0.0.1"));
    }

    /// D2: the Windows laptop advertised its hotspot address plus link-local addresses of
    /// disconnected adapters. Both routable addresses are kept (best first); local ones never
    /// while a routable one exists.
    #[test]
    fn rank_addrs_keeps_two_routable_addresses() {
        let rank = |v: &[&str]| rank_addrs(v.iter().map(|s| ip(s)), 8193).unwrap();
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

    /// 2026-10-23: a freshly started Linux daemon (0.0.0.0, IPv4 endpoint) first heard the Mac only
    /// at IPv6 link-local addresses. Only IPv4 addresses are candidates; with none left the reason
    /// is typed.
    #[test]
    fn rank_addrs_offers_only_ipv4_and_types_the_reason_when_none_is_left() {
        let rank = |v: &[&str]| rank_addrs(v.iter().map(|s| ip(s)), 8193);
        let sa = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert_eq!(
            rank(&["fe80::1", "fe80::18a3:7cff:fe21:9b0e"]),
            Err(NoAddress::LinkLocalOnly)
        );
        assert_eq!(
            rank(&["fd00::5", "2001:db8::1", "fe80::1", "192.168.1.5"]),
            Ok(vec![sa("192.168.1.5:8193")])
        );
        assert_eq!(rank(&["2001:db8::1", "fe80::1"]), Err(NoAddress::OtherFamily));
        assert_eq!(rank(&["::1"]), Err(NoAddress::OtherFamily));
        // IPv4 link-local needs no scope id: still a last resort.
        assert_eq!(rank(&["169.254.3.4", "fe80::1"]), Ok(vec![sa("169.254.3.4:8193")]));
        assert_eq!(rank(&[]), Ok(vec![]));
    }

    fn unreachable_seen(id: PeerId, ts: u64, reason: NoAddress) -> Seen {
        let mut s = seen(id, "0.0.0.0:8193", ts);
        s.peer.addrs.clear();
        s.unreachable = Some(reason);
        s
    }

    /// A peer heard only at link-local addresses is "discovered, no reachable address": listed with
    /// its reason, never as a dial candidate. The same record heard again with a routable address
    /// (mdns-sd resolves it per address family) makes it dialable at once, not at the next re-sign;
    /// after that the replay guard holds as before.
    #[test]
    fn a_link_local_only_peer_is_unreachable_until_its_record_brings_an_address() {
        let (_dir, b) = inert_browser();
        let now = now_unix();
        let id = PeerId([6; 32]);
        remember(
            &mut lock(&b.seen),
            unreachable_seen(id, now, NoAddress::LinkLocalOnly),
            now,
        );
        assert!(b.peers(LISTED_SECS).is_empty(), "nothing to dial");
        let u = b.unreachable(LISTED_SECS);
        assert_eq!((u.len(), u[0].id, u[0].reason), (1, id, NoAddress::LinkLocalOnly));
        assert_eq!(serde_json::to_value(&u[0]).unwrap()["reason"], "link_local_only");
        let (lan, evil) = ("192.168.1.20:8193", "10.66.0.1:8193");
        remember(&mut lock(&b.seen), seen(id, lan, now), now);
        let listed: Vec<Vec<SocketAddr>> = b.peers(LISTED_SECS).into_iter().map(|p| p.addrs).collect();
        assert_eq!(listed, vec![vec![lan.parse::<SocketAddr>().unwrap()]]);
        assert!(b.unreachable(LISTED_SECS).is_empty());
        remember(&mut lock(&b.seen), seen(id, evil, now), now);
        remember(
            &mut lock(&b.seen),
            unreachable_seen(id, now, NoAddress::LinkLocalOnly),
            now,
        );
        assert_eq!(b.peers(LISTED_SECS)[0].addrs, vec![lan.parse::<SocketAddr>().unwrap()]);
        assert!(
            b.unreachable(LISTED_SECS).is_empty(),
            "an incomplete copy does not hide it"
        );
    }

    /// Synthetic clock (seconds from `t0`/`base`) driving `remember` and `requery_due` as the
    /// browser thread does: peers re-sign every REFRESH (with `lost` announcements dropped), the
    /// browser checks every CHECK and, when due, re-queries; every peer still alive answers a
    /// re-query unless its time is in `lost`. Returns the re-query times and, per second, whether
    /// peer 0 is listed.
    fn simulate(peers: u8, until: u64, lost: &[u64], silent_from: u64) -> (Vec<u64>, Vec<bool>) {
        let (t0, base) = (1_760_000_000u64, Instant::now());
        let refresh = REFRESH.as_secs();
        let mut map = HashMap::new();
        let hear = |map: &mut HashMap<PeerId, Seen>, i: u8, t: u64| {
            // Peer i re-signs at i, i + REFRESH, ...: its current record.
            let signed = t0 + i as u64 + (t - i as u64) / refresh * refresh;
            let s = Seen {
                at: base + Duration::from_secs(t),
                ..seen(PeerId([i + 1; 32]), &format!("192.168.1.{}:8193", i + 10), signed)
            };
            remember(map, s, t0 + t);
        };
        let (mut requeries, mut listed, mut last) = (Vec::new(), Vec::new(), None);
        for t in 0..=until {
            let now = base + Duration::from_secs(t);
            for i in 0..peers {
                let announces = t >= i as u64 && (t - i as u64).is_multiple_of(refresh);
                if announces && t < silent_from && !(i == 0 && lost.contains(&t)) {
                    hear(&mut map, i, t);
                }
            }
            if t.is_multiple_of(CHECK.as_secs()) && requery_due(&map, t0 + t, now, last) {
                last = Some(now);
                requeries.push(t);
                // The fresh browse's queries go out over the next few seconds; answered at once here.
                for i in (0..peers).filter(|_| t < silent_from) {
                    if !(i == 0 && lost.contains(&t)) {
                        hear(&mut map, i, t);
                    }
                }
            }
            let s = map.get(&PeerId([1; 32]));
            listed.push(s.is_some_and(|s| s.listed(t0 + t, now, Duration::from_secs(LISTED_SECS))));
        }
        (requeries, listed)
    }

    /// RFC 6762 section 5.2: with every announcement heard, the browser never re-queries (only
    /// mdns-sd's doubling backoff is sent), however many peers. Before, it restarted its browse
    /// every 30 s (about 5 queries per 30 s per daemon, each answered in full by every peer).
    #[test]
    fn the_browser_does_not_re_query_while_every_peer_is_heard() {
        let (requeries, listed) = simulate(20, 3600, &[], u64::MAX);
        assert_eq!(requeries, Vec::<u64>::new());
        assert!(listed.iter().all(|l| *l));
    }

    /// 2026-10-23 (finding 2): two missed re-sign announcements aged a live peer out. Now a missed
    /// announcement makes the browser re-query, and the peer stays listed: two announcements lost in
    /// a row, and once also the first re-query's answers. Each lost announcement costs at most two
    /// re-queries.
    #[test]
    fn a_live_peer_whose_announcements_are_lost_stays_listed() {
        let refresh = REFRESH.as_secs();
        // Announcements at 240 and 360 lost; at 1200 lost and the answers to the re-query at 1210.
        let lost = [2 * refresh, 3 * refresh, 10 * refresh, 1210];
        let (requeries, listed) = simulate(3, 3600, &lost, u64::MAX);
        assert!(listed.iter().all(|l| *l), "listed throughout");
        // Heard at 120 (overdue after 245), by re-query at 250 (overdue after 375), at 1080
        // (overdue after 1205; the re-query at 1210 unanswered, the next REQUERY_GAP later).
        assert_eq!(requeries, vec![250, 380, 1210, 1240]);
    }

    /// A peer that vanished without a goodbye: at most two re-queries, then none; it leaves the
    /// list LISTED_SECS after it was last heard.
    #[test]
    fn a_vanished_peer_costs_two_re_queries_and_leaves_the_list() {
        let (requeries, listed) = simulate(3, 3600, &[], 1000);
        assert_eq!(requeries.len(), 2, "{requeries:?}");
        assert!(requeries.iter().all(|t| (1000..1000 + LISTED_SECS).contains(t)));
        let last_heard = 960; // peer 0 re-signs at 0, 120, ..., 960
        assert!(listed[..=(last_heard + LISTED_SECS) as usize].iter().all(|l| *l));
        assert!(!listed[(last_heard + LISTED_SECS + 1) as usize..].iter().any(|l| *l));
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
        let paths = ucompute_core::Paths::at(dir.path());
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

    /// Round 6b: a browse that fails after `stop_browse` (`Error::Again`, the daemon's command
    /// queue full) is retried at the next check instead of ending discovery until networking
    /// restarts; only a daemon that shut down ends the loop.
    #[test]
    fn a_failed_browse_is_retried_at_the_next_check() {
        use std::sync::mpsc;
        let recv = |rx: &mpsc::Receiver<u32>, got: &mut Vec<u32>, due: Instant| match rx
            .recv_timeout(due.saturating_duration_since(Instant::now()))
        {
            Ok(ev) => {
                got.push(ev);
                Wake::Other
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Wake::Deadline,
            Err(mpsc::RecvTimeoutError::Disconnected) => Wake::Closed,
        };
        let run = |results: Vec<mdns_sd::Result<u32>>| {
            let (_tx0, rx0) = mpsc::channel::<u32>();
            let mut results = results.into_iter();
            let (mut browses, mut got) = (0, vec![]);
            let got_ref = std::cell::RefCell::new(&mut got);
            browse_loop(
                rx0,
                || {
                    browses += 1;
                    // The next browse's receiver holds one event, then disconnects (ends the loop).
                    results
                        .next()
                        .expect("no browse after the loop should have ended")
                        .map(|ev| {
                            let (tx, rx) = mpsc::channel();
                            tx.send(ev).unwrap();
                            rx
                        })
                },
                |rx, due| recv(rx, &mut got_ref.borrow_mut(), due),
                |_old| None,
                Duration::from_millis(10),
            );
            (browses, got)
        };
        let t0 = Instant::now();
        assert_eq!(run(vec![Err(mdns_sd::Error::Again), Ok(7)]), (2, vec![7]));
        assert_eq!(
            run(vec![
                Err(mdns_sd::Error::Msg("signal".into())),
                Err(mdns_sd::Error::Again),
                Ok(8)
            ]),
            (3, vec![8])
        );
        assert_eq!(run(vec![Err(mdns_sd::Error::DaemonShutdown)]), (1, vec![]));
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
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
