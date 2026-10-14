//! mDNS discovery between two advertisers and one browser inside one process on this one Mac.
//! SINGLE-HOST evidence only (F-02-01): it shows the signed-record path end to end and the
//! latency on a loopback-multicast host, never real L2/Wi-Fi discovery time.
mod common;

use common::Peer;
use common_network::discovery::{Advertiser, Browser};
use common_network::NetworkConfig;
use std::time::{Duration, Instant};

fn mdns_config() -> NetworkConfig {
    NetworkConfig {
        enabled: true,
        mdns: true,
        ..Peer::config()
    }
}

#[test]
fn two_instances_discover_within_5s_single_host() {
    if std::env::var("COMMON_MDNS").as_deref() == Ok("0") {
        eprintln!("SKIP two_instances_discover_within_5s_single_host: COMMON_MDNS=0");
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
    assert_eq!(da.proto, "1.0");
    assert!(!da.addr.ip().is_unspecified());
    // Records too old for the caller's window drop out without touching the signature logic.
    assert!(browser.peers(0).len() <= 2);
    // A refresh re-announces with a fresh ts: the browser re-resolves a's record (seen_utc moves
    // to a later second) and keeps exactly two entries (no duplicates).
    std::thread::sleep(Duration::from_millis(1100));
    _adv_a.refresh().unwrap();
    let refreshed = loop {
        let peers = browser.peers(10);
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
        "SINGLE-HOST refresh: a re-resolved at {} (first seen {})",
        refreshed.seen_utc, da.seen_utc
    );
}
