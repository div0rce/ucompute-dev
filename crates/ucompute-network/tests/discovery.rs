//! mDNS discovery between two advertisers and one browser inside one process on this one Mac.
//! SINGLE-HOST evidence only (F-02-01): it shows the signed-record path end to end and the
//! latency on a loopback-multicast host, never real L2/Wi-Fi discovery time.
mod support;

use std::collections::HashSet;
use std::time::{Duration, Instant};
use support::{child_home, spawn_child, Peer};
use ucompute_network::discovery::{Advertiser, Browser};
use ucompute_network::{NetworkConfig, PROTOCOL_MAJOR, PROTOCOL_MINOR};

/// How long a discovery child runs before it exits on its own.
const CHILD_RUN: Duration = Duration::from_secs(8);

fn mdns_config() -> NetworkConfig {
    NetworkConfig {
        enabled: true,
        mdns: true,
        ..Peer::config()
    }
}

#[test]
fn two_instances_discover_within_5s_single_host() {
    if std::env::var("UCOMPUTE_MDNS").ok().as_deref() == Some("0") {
        eprintln!("SKIP two_instances_discover_within_5s_single_host: UCOMPUTE_MDNS=0");
        return;
    }
    let cfg = mdns_config();
    let (a, b, watcher) = (Peer::new(), Peer::new(), Peer::new());
    watcher.pin(&a); // a is trusted by the watcher, b is not
    let started = Instant::now();
    let browser = Browser::start(&cfg, watcher.peers.clone()).unwrap();
    assert!(browser.active());
    let _adv_a = Advertiser::start(&cfg, a.identity.clone(), 40001, None).unwrap();
    let _adv_b = Advertiser::start(&cfg, b.identity.clone(), 40002, Some(40102)).unwrap();
    let deadline = started + Duration::from_secs(5);
    let both = loop {
        let peers = browser.peers(10);
        let has = |p: &Peer| peers.iter().any(|d| d.id == p.id());
        if has(&a) && has(&b) {
            break peers;
        }
        assert!(
            Instant::now() < deadline,
            "SINGLE-HOST: expected both ids within 5 s, saw {peers:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let latency = started.elapsed();
    eprintln!(
        "SINGLE-HOST discovery latency: both advertisers visible after {:.0} ms",
        latency.as_secs_f64() * 1000.0
    );
    let da = both.iter().find(|d| d.id == a.id()).unwrap();
    let db = both.iter().find(|d| d.id == b.id()).unwrap();
    assert_eq!(da.addr.port(), 40001);
    assert!(!da.pairing && da.pair_port.is_none());
    assert!(da.trusted, "pinned id is marked trusted");
    assert_eq!(db.addr.port(), 40002);
    assert!(db.pairing);
    assert_eq!(db.pair_port, Some(40102));
    assert!(!db.trusted, "unpinned id is never trusted");
    assert_eq!(da.proto, format!("{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}"));
    // Both advertisers bind 127.0.0.1 (`Peer::config`): that is the only address advertised, never
    // a LAN address nothing listens on (this Mac also has en0).
    for d in [da, db] {
        assert_eq!(d.addr.ip(), std::net::Ipv4Addr::LOCALHOST, "{d:?}");
    }
    // Records too old for the caller's window drop out without touching the signature logic.
    assert!(
        browser
            .peers(0)
            .iter()
            .filter(|d| d.id == a.id() || d.id == b.id())
            .count()
            <= 2
    );
    // A refresh re-announces with a fresh ts: the browser re-resolves a's record (seen_utc moves
    // to a later second) and keeps exactly two entries (no duplicates).
    std::thread::sleep(Duration::from_millis(1100));
    _adv_a.refresh().unwrap();
    let refreshed = loop {
        // Other UCompute instances on this host (parallel tests) may show up too; count ours.
        let peers: Vec<_> = browser
            .peers(10)
            .into_iter()
            .filter(|d| d.id == a.id() || d.id == b.id())
            .collect();
        assert_eq!(peers.len(), 2, "{peers:?}");
        let da_now = peers.iter().find(|d| d.id == a.id()).unwrap().clone();
        if da_now.seen_utc > da.seen_utc {
            break da_now;
        }
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "refresh never re-resolved: {da_now:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    eprintln!(
        "SINGLE-HOST refresh: a re-resolved at {} {} (first seen {} {})",
        refreshed.addr, refreshed.seen_utc, da.addr, da.seen_utc
    );
    // Re-resolved from every address record the browser holds by now: still only the bound one.
    assert_eq!(refreshed.addr.ip(), std::net::Ipv4Addr::LOCALHOST, "{refreshed:?}");
}

/// Helper, not a test of its own: one UCompute instance (its own process, identity and store) that
/// advertises and browses for `CHILD_RUN`, printing `SAW <id hex> <ms since start> <trusted>` once
/// per id it discovers.
#[test]
#[ignore = "child-process helper; run by two_processes_discover_within_5s_single_host"]
fn child_mdns_process() {
    let Some(home) = child_home() else { return };
    let (identity, peers, _, _) = Peer::open(&home);
    let cfg = mdns_config();
    let t0 = Instant::now();
    let browser = Browser::start(&cfg, peers).unwrap();
    let _adv = Advertiser::start(&cfg, identity.clone(), 40003, None).unwrap();
    let mut seen = HashSet::new();
    while t0.elapsed() < CHILD_RUN {
        for p in browser.peers(10) {
            if p.id != identity.id && seen.insert(p.id) {
                println!("SAW {} {} {}", p.id.to_hex(), t0.elapsed().as_millis(), p.trusted);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// F-02-01 `two_processes_discover_within_5s`: two separate OS processes (the test binary re-run
/// as `child_mdns_process`, each with its own UCOMPUTE_HOME and identity) find each other over mDNS
/// within 5 s and list each other as untrusted (not paired). SINGLE-HOST: two processes on this
/// one Mac sharing its interfaces, not two machines on an L2 segment.
#[test]
fn two_processes_discover_within_5s_single_host() {
    if std::env::var("UCOMPUTE_MDNS").ok().as_deref() == Some("0") {
        eprintln!("SKIP two_processes_discover_within_5s_single_host: UCOMPUTE_MDNS=0");
        return;
    }
    let (a, b) = (Peer::new(), Peer::new());
    let children = [
        spawn_child("child_mdns_process", a.home()),
        spawn_child("child_mdns_process", b.home()),
    ];
    let outputs: Vec<String> = children
        .into_iter()
        .map(|c| String::from_utf8(c.wait_with_output().unwrap().stdout).unwrap())
        .collect();
    let saw = |out: &str, who: &Peer| -> Option<(u64, bool)> {
        out.lines().find_map(|l| {
            let rest = l.split_once("SAW ")?.1;
            let mut f = rest.split_whitespace();
            (f.next()? == who.id().to_hex()).then_some(())?;
            Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?))
        })
    };
    let (a_saw_b, b_saw_a) = (saw(&outputs[0], &b), saw(&outputs[1], &a));
    eprintln!("SINGLE-HOST two-process discovery: a saw b {a_saw_b:?}, b saw a {b_saw_a:?} (ms since that process started, trusted)");
    for (who, hit) in [("a", a_saw_b), ("b", b_saw_a)] {
        let (ms, trusted) = hit.unwrap_or_else(|| panic!("{who} never saw the other process: {outputs:?}"));
        assert!(ms <= 5_000, "{who} took {ms} ms");
        assert!(!trusted, "an unpaired peer is listed untrusted");
    }
}

/// A daemon's browser never lists its own advert (it comes back over multicast loopback), while
/// another device advertised from the same process is listed. SINGLE-HOST: one Mac.
#[test]
fn browser_never_lists_own_id_single_host() {
    if std::env::var("UCOMPUTE_MDNS").ok().as_deref() == Some("0") {
        eprintln!("SKIP browser_never_lists_own_id_single_host: UCOMPUTE_MDNS=0");
        return;
    }
    let cfg = mdns_config();
    let (me, other) = (Peer::new(), Peer::new());
    let browser = Browser::start_excluding(&cfg, me.peers.clone(), me.id()).unwrap();
    let _mine = Advertiser::start(&cfg, me.identity.clone(), 40011, None).unwrap();
    let _theirs = Advertiser::start(&cfg, other.identity.clone(), 40012, None).unwrap();
    let started = Instant::now();
    let mut seen_other = None;
    // Watch until the other device appears, then one more second: our own id never shows up.
    while seen_other.is_none_or(|t: Instant| t.elapsed() < Duration::from_secs(1)) {
        let peers = browser.peers(10);
        assert!(!peers.iter().any(|d| d.id == me.id()), "own id listed: {peers:?}");
        if seen_other.is_none() {
            if let Some(d) = peers.iter().find(|d| d.id == other.id()) {
                eprintln!(
                    "SINGLE-HOST: other device listed at {} after {:?}; own advert excluded",
                    d.addr,
                    started.elapsed()
                );
                seen_other = Some(Instant::now());
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "SINGLE-HOST: the other device was never listed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
