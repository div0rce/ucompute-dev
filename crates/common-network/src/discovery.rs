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
//! The browser's table is bounded: entries older than [`MAX_RECORD_AGE_SECS`] are evicted, at most
//! [`MAX_SEEN`] are kept (oldest dropped), and a record for an id already seen at another address
//! replaces it only with a strictly newer `ts`, so a replayed record cannot redirect a live entry.
//!
//! Disabled (both sides inert, nothing bound) when `NetworkConfig.enabled` or `.mdns` is false
//! or the environment has `COMMON_MDNS=0`.
use crate::tls::{store_verdict, Verdict};
use crate::{NetworkConfig, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use common_core::{CommonError, ErrorCode, Result};
use common_identity::{utc_now, DeviceIdentity, PeerId, PeerStore};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_common._udp.local.";
/// A signed record older (or newer) than this is ignored (ADR-0003 D4).
pub const MAX_RECORD_AGE_SECS: u64 = 300;
/// The advertiser re-signs and re-announces at this interval so its `ts` stays fresh.
const REFRESH: Duration = Duration::from_secs(120);
/// Most records the browser keeps (anyone on the LAN can mint ids).
pub const MAX_SEEN: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredPeer {
    pub id: PeerId,
    pub addr: SocketAddr,
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
            ts: now_unix(),
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
    // Dropping the sender ends the refresh thread.
    _stop: Option<mpsc::Sender<()>>,
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
            return Ok(Advertiser {
                state: None,
                _stop: None,
            });
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
        state.register(true)?;
        tracing::info!(instance = %short, port, pairing = pairing_port.is_some(), "mdns advertising");
        let (tx, rx) = mpsc::channel::<()>();
        let refresher = state.clone();
        std::thread::spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(REFRESH) {
                if let Err(e) = refresher.register(false) {
                    tracing::warn!(error = %e, "mdns refresh failed");
                }
            }
        });
        Ok(Advertiser {
            state: Some(state),
            _stop: Some(tx),
        })
    }

    pub fn active(&self) -> bool {
        self.state.is_some()
    }

    /// Re-sign with a fresh `ts` and re-announce now (the refresh thread does this every 120 s).
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
}

impl Seen {
    fn fresh(&self, now_unix: u64) -> bool {
        self.at.elapsed().as_secs() <= MAX_RECORD_AGE_SECS && now_unix.abs_diff(self.ts) <= MAX_RECORD_AGE_SECS
    }
}

/// Insert a verified record: stale entries are evicted first; an entry for the same id at another
/// address is replaced only by a strictly newer `ts`; past [`MAX_SEEN`] the oldest entry goes.
fn remember(map: &mut HashMap<PeerId, Seen>, new: Seen, now_unix: u64) {
    map.retain(|_, s| s.fresh(now_unix));
    if let Some(old) = map.get(&new.peer.id) {
        if old.peer.addr != new.peer.addr && new.ts <= old.ts {
            tracing::debug!(id = %new.peer.id.short_label(), "discovery record for a known id from another address ignored (not newer)");
            return;
        }
    }
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

/// Loopback and link-local addresses only when nothing else is advertised (a LAN peer must never
/// be told to dial 127.0.0.1, and a link-local IPv6 address needs a scope id we do not carry);
/// then IPv4 (what the workload endpoint binds by default) before IPv6; lowest address wins.
fn pick_addr(ips: impl Iterator<Item = IpAddr>, port: u16) -> Option<SocketAddr> {
    let local_only = |ip: &IpAddr| match ip {
        IpAddr::V4(v) => v.is_loopback() || v.is_link_local(),
        IpAddr::V6(v) => v.is_loopback() || v.is_unicast_link_local(),
    };
    ips.min_by_key(|ip| (local_only(ip), ip.is_ipv6(), *ip))
        .map(|ip| SocketAddr::new(ip, port))
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
                        let addr = pick_addr(info.addresses.iter().map(|a| a.to_ip_addr()), info.port);
                        match (Advert::verify(&txt, info.port, now_unix()), addr) {
                            (Ok(advert), _) if Some(advert.id) == own => {}
                            (Ok(advert), Some(addr)) => {
                                let peer = DiscoveredPeer {
                                    id: advert.id,
                                    addr,
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
                                };
                                remember(&mut lock(&map), seen, now_unix());
                            }
                            (Err(e), _) => {
                                tracing::debug!(name = %info.fullname, error = %e, "discovery record dropped")
                            }
                            (_, None) => tracing::debug!(name = %info.fullname, "discovery record without address"),
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        lock(&map).retain(|_, s| s.fullname != fullname);
                    }
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
            .filter(|s| s.at.elapsed().as_secs() <= max_age_secs)
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
        Seen {
            peer: DiscoveredPeer {
                id,
                addr: addr.parse().unwrap(),
                pairing: false,
                pair_port: None,
                proto: "1.0".into(),
                trusted: false,
                seen_utc: String::new(),
            },
            ts,
            at: Instant::now(),
            fullname: String::new(),
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
        // A re-announce from the same address, or a strictly newer record from a new one, wins.
        remember(&mut map, seen(id, "10.0.0.2:8193", now - 5), now);
        assert_eq!(map[&id].ts, now - 5);
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

    #[test]
    fn pick_addr_prefers_ipv4() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
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
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
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
}
