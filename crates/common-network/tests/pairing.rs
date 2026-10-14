//! Pairing between two nodes on this one Mac (F-02-02, ADR-0003 D2). SINGLE-HOST evidence only:
//! loopback QUIC, two in-process identities and stores. The relay test adds a third in-process
//! party that terminates TLS on both legs.
mod common;

use common::{audit_kinds, Peer};
use common_core::ErrorCode;
use common_network::pairing::{self, expected_peer_from_qr};
use common_network::tls::{self, PeerPolicy};
use common_network::ALPN_PAIRING;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// The code with its first digit changed: always wrong, always well-formed.
fn wrong(code: &str) -> String {
    let d = (code.as_bytes()[0] - b'0' + 1) % 10;
    format!("{d}{}", &code[1..])
}

#[tokio::test]
async fn correct_code_pairs_and_pins_both_sides_single_host() {
    let (a, b) = (Peer::new(), Peer::new());
    let (na, nb) = (a.start().await, b.start().await);
    let t0 = Instant::now();
    let (session, handle) = pairing::start(na.clone(), "initiator-mac").await.unwrap();
    assert_eq!(session.code.len(), 6);
    assert!(session.expires_at_unix > 0);
    let (qr_id, qr_addr, qr_secret) = expected_peer_from_qr(&session.qr_payload).unwrap();
    assert_eq!(qr_id, a.id());
    assert_eq!(qr_addr.port(), session.port);
    let addr: SocketAddr = format!("127.0.0.1:{}", session.port).parse().unwrap();
    let rec_b_of_a = pairing::join(nb.clone(), addr, &session.code, "joiner-mac")
        .await
        .unwrap();
    let joined = t0.elapsed();
    let rec_a_of_b = timeout(Duration::from_secs(5), handle).await.unwrap().unwrap().unwrap();
    let total = t0.elapsed();
    eprintln!(
        "SINGLE-HOST bench:handshake-latency pairing: join returned after {:.1} ms, initiator pinned after {:.1} ms (start + QUIC/TLS handshake + SPAKE2 + confirmation)",
        joined.as_secs_f64() * 1000.0,
        total.as_secs_f64() * 1000.0
    );
    assert_eq!(rec_b_of_a.id, a.id());
    assert_eq!(rec_b_of_a.name, "initiator-mac");
    assert_eq!(rec_a_of_b.id, b.id());
    assert_eq!(rec_a_of_b.name, "joiner-mac");
    // Both stores pinned the other side, with the other side's workload port for reconnects.
    assert!(a.peers.is_pinned(&b.id()));
    assert!(b.peers.is_pinned(&a.id()));
    assert_eq!(
        rec_b_of_a.last_addr.as_deref(),
        Some(na.local_addr().unwrap().to_string().as_str())
    );
    assert_eq!(
        rec_a_of_b.last_addr.as_deref(),
        Some(nb.local_addr().unwrap().to_string().as_str())
    );
    // The code is single use: the pairing endpoint is gone.
    let again = timeout(
        Duration::from_secs(12),
        pairing::join(nb.clone(), addr, &session.code, "x"),
    )
    .await;
    assert!(
        matches!(again, Ok(Err(_))),
        "pairing endpoint must be closed after one pairing"
    );
    // The QR secret has the documented shape and the code was never it.
    assert_ne!(qr_secret, session.code);
    // Trust took effect inside TLS: the workload endpoints now accept each other.
    let (out, inn) = tokio::join!(na.connect(nb.local_addr().unwrap(), b.id()), async {
        nb.accept().await.unwrap()
    });
    assert_eq!(out.unwrap().peer_id, b.id());
    assert_eq!(inn.peer_id, a.id());
    for p in [&a, &b] {
        let kinds = audit_kinds(p);
        assert!(kinds.contains(&"pairing_succeeded".to_string()), "{kinds:?}");
    }
    assert!(audit_kinds(&a).contains(&"pairing_started".to_string()));
}

#[tokio::test]
async fn wrong_code_fails_and_pins_nothing() {
    let (a, b) = (Peer::new(), Peer::new());
    let (na, nb) = (a.start().await, b.start().await);
    let (session, handle) = pairing::start(na.clone(), "a").await.unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", session.port).parse().unwrap();
    let e = pairing::join(nb.clone(), addr, &wrong(&session.code), "b")
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::PairingFailed, "{e}");
    assert!(!a.peers.is_pinned(&b.id()));
    assert!(!b.peers.is_pinned(&a.id()));
    assert!(a.peers.list().unwrap().is_empty() && b.peers.list().unwrap().is_empty());
    // The initiator is still waiting for a correct attempt within the window.
    assert!(!handle.is_finished());
    // Both sides audited the failure, with codes only.
    tokio::time::sleep(Duration::from_millis(200)).await;
    for p in [&a, &b] {
        let kinds = audit_kinds(p);
        assert!(kinds.contains(&"pairing_failed".to_string()), "{kinds:?}");
        assert!(!kinds.contains(&"pairing_succeeded".to_string()));
    }
    // A malformed code is refused before any packet is sent.
    let e = pairing::join(nb.clone(), addr, "12", "b").await.unwrap_err();
    assert_eq!(e.code, ErrorCode::InvalidRequest);
    // The right code still works afterwards: one failure does not poison the window.
    let rec = pairing::join(nb.clone(), addr, &session.code, "b").await.unwrap();
    assert_eq!(rec.id, a.id());
    assert_eq!(
        timeout(Duration::from_secs(5), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .id,
        b.id()
    );
}

#[tokio::test]
async fn brute_force_rate_limited() {
    let (a, b) = (Peer::new(), Peer::new());
    let (na, nb) = (a.start().await, b.start().await);
    let (session, handle) = pairing::start(na.clone(), "a").await.unwrap();
    let addr: SocketAddr = format!("127.0.0.1:{}", session.port).parse().unwrap();
    let bad = wrong(&session.code);
    let mut codes = Vec::new();
    for _ in 0..6 {
        let e = pairing::join(nb.clone(), addr, &bad, "b").await.unwrap_err();
        codes.push(e.code);
    }
    assert_eq!(&codes[..5], &[ErrorCode::PairingFailed; 5]);
    assert_eq!(
        codes[5],
        ErrorCode::PairingLocked,
        "the 6th failure trips the lock on the wire"
    );
    let e = timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::PairingLocked, "{e}");
    assert!(audit_kinds(&a).contains(&"pairing_locked".to_string()));
    // The 7th connection, even with the correct code, is refused: the endpoint is closed.
    let r = timeout(
        Duration::from_secs(12),
        pairing::join(nb.clone(), addr, &session.code, "b"),
    )
    .await;
    match r {
        Ok(Err(e)) => assert_ne!(e.code, ErrorCode::InvalidRequest, "{e}"),
        Ok(Ok(_)) => panic!("pairing must be locked"),
        Err(_) => panic!("join must fail within the handshake timeout"),
    }
    assert!(!a.peers.is_pinned(&b.id()) && !b.peers.is_pinned(&a.id()));
}

/// A third party M terminates TLS on both legs and forwards every pairing frame byte for byte.
#[tokio::test]
async fn mitm_relay_detected() {
    let (a, b, m) = (Peer::new(), Peer::new(), Peer::new());
    let (na, nb) = (a.start().await, b.start().await);
    let (session, handle) = pairing::start(na.clone(), "a").await.unwrap();
    let a_addr: SocketAddr = format!("127.0.0.1:{}", session.port).parse().unwrap();
    // M's own accept-any pairing endpoint (its key, not A's) and its own client config towards A.
    let relay = quinn::Endpoint::server(
        tls::server_config(&m.identity, PeerPolicy::AcceptAny, ALPN_PAIRING).unwrap(),
        "127.0.0.1:0".parse().unwrap(),
    )
    .unwrap();
    let m_addr = relay.local_addr().unwrap();
    let onward = tls::client_config(&m.identity, PeerPolicy::AcceptAny, ALPN_PAIRING).unwrap();
    let relayed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = relayed.clone();
    let relay_task = tokio::spawn(async move {
        let incoming = relay.accept().await.unwrap();
        let from_b = incoming.await.unwrap();
        let to_a = relay.connect_with(onward, a_addr, "common").unwrap().await.unwrap();
        // Forward each stream B opens as a fresh stream to A, copying bytes both ways.
        while let Ok((mut b_send, mut b_recv)) = from_b.accept_bi().await {
            let (mut a_send, mut a_recv) = to_a.open_bi().await.unwrap();
            // Byte-for-byte in both directions, counted as it flows (either end may reset later).
            let (c_up, c_down) = (counter.clone(), counter.clone());
            tokio::spawn(async move {
                while let Ok(Some(chunk)) = b_recv.read_chunk(256, true).await {
                    if a_send.write_all(&chunk.bytes).await.is_err() {
                        break;
                    }
                    c_up.fetch_add(chunk.bytes.len(), std::sync::atomic::Ordering::SeqCst);
                }
            });
            tokio::spawn(async move {
                while let Ok(Some(chunk)) = a_recv.read_chunk(256, true).await {
                    if b_send.write_all(&chunk.bytes).await.is_err() {
                        break;
                    }
                    c_down.fetch_add(chunk.bytes.len(), std::sync::atomic::Ordering::SeqCst);
                }
            });
        }
    });
    let e = pairing::join(nb.clone(), m_addr, &session.code, "b").await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PairingFailed, "{e}");
    assert!(
        e.message.contains("MAC") || e.message.contains("relayed") || e.message.contains("pairing"),
        "{e}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        relayed.load(std::sync::atomic::Ordering::SeqCst) >= 34,
        "the relay really carried the SPAKE2 frames ({} bytes)",
        relayed.load(std::sync::atomic::Ordering::SeqCst)
    );
    // Nobody pinned anybody: not A↔B, and not M on either side.
    for (p, others) in [(&a, [&b, &m]), (&b, [&a, &m]), (&m, [&a, &b])] {
        for o in others {
            assert!(
                !p.peers.is_pinned(&o.id()),
                "{} must not pin {}",
                p.id().short_label(),
                o.id().short_label()
            );
        }
    }
    assert!(
        !handle.is_finished(),
        "one relayed failure counts as one failed attempt"
    );
    let kinds = audit_kinds(&a);
    assert!(kinds.contains(&"pairing_failed".to_string()), "{kinds:?}");
    handle.abort();
    relay_task.abort();
}
