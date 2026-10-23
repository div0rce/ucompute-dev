//! Node-level tests of the authenticated transport (F-02-03, F-02-04, F-02-05, F-06-03, F-08-10).
//! Every test runs two nodes on this one Mac: SINGLE-HOST evidence only.
mod common;

use common::{audit_kinds, child_home, child_value, pair, poll_until, spawn_child, start_relay, Peer};
use common_core::ErrorCode;
use common_identity::PeerId;
use common_network::health::{supervise, PeerHealth};
use common_network::proto::{
    control_message::Body, read_frame, write_frame, Capabilities, ControlMessage, Hello, RevocationMsg,
    SignedCapabilities,
};
use common_network::tls::{self, PeerPolicy};
use common_network::transport::{
    map_conn_error, sign_capabilities, sign_capabilities_record, verify_capabilities, ControlStream, HeldControl, Node,
    CLOSE_FRAME_TOO_LARGE, CLOSE_HELD, CLOSE_PROTOCOL, CLOSE_REVOKED, CLOSE_UNAUTHORIZED,
};
use common_network::{
    ALPN_WORKLOAD, HELD_EXCHANGE_MINOR, MAX_CAPABILITIES_BYTES, MAX_CONTROL_FRAME, PROTOCOL_MAJOR, PROTOCOL_MINOR,
};
use futures_util::future::BoxFuture;
use prost::Message;
use quinn::ConnectionError;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// F-02-05 bound: a dead peer is noticed within 3 heartbeat intervals, default <= 6 s.
const DETECTION_BOUND: Duration = Duration::from_secs(6);

fn app_close(e: &ConnectionError) -> Option<u64> {
    match e {
        ConnectionError::ApplicationClosed(a) => Some(u64::from(a.error_code)),
        _ => None,
    }
}

#[tokio::test]
async fn network_disabled_refuses_to_listen() {
    let p = Peer::new();
    let cfg = common_network::NetworkConfig {
        enabled: false,
        ..Peer::config()
    };
    let e = Node::start(
        cfg,
        p.identity.clone(),
        p.peers.clone(),
        p.policy.clone(),
        p.audit.clone(),
    )
    .await
    .err()
    .expect("must refuse");
    assert_eq!(e.code, ErrorCode::PolicyDenied);
    assert!(e.message.contains("disabled"));
}

#[tokio::test]
async fn handshake_pinned_keys_succeeds_single_host() {
    let (a, b) = pair();
    let (na, nb) = (a.start().await, b.start().await);
    let addr = nb.local_addr().unwrap();
    let (out, inn) = tokio::join!(na.connect(addr, b.id()), async { nb.accept().await.unwrap() });
    let out = out.unwrap();
    assert_eq!(out.peer_id, b.id());
    assert_eq!(inn.peer_id, a.id());
    assert_eq!(out.negotiated_minor, PROTOCOL_MINOR);
    assert_eq!(inn.negotiated_minor, PROTOCOL_MINOR);
    assert!(out.capabilities.lock().unwrap().is_some());
    assert_eq!(na.connections().len(), 1);
    assert_eq!(nb.connections().len(), 1);
    // The authenticated connection carries ordinary streams for other modules.
    let echo = tokio::spawn(async move {
        let (mut s, mut r) = inn.conn.accept_bi().await.unwrap();
        let d = r.read_to_end(64).await.unwrap();
        s.write_all(&d).await.unwrap();
        s.finish().unwrap();
        let _ = s.stopped().await;
    });
    let (mut s, mut r) = out.conn.open_bi().await.unwrap();
    s.write_all(b"ping").await.unwrap();
    s.finish().unwrap();
    assert_eq!(r.read_to_end(64).await.unwrap(), b"ping");
    echo.await.unwrap();
    assert!(audit_kinds(&a).contains(&"peer_connected".to_string()));
    assert!(audit_kinds(&b).contains(&"peer_connected".to_string()));
}

#[tokio::test]
async fn unpinned_key_rejected() {
    // b does not pin a: b's verifier fails the handshake; no control stream is ever accepted.
    let (a, b) = (Peer::new(), Peer::new());
    a.pin(&b);
    let (na, nb) = (a.start().await, b.start().await);
    let e = na.connect(nb.local_addr().unwrap(), b.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{e}");
    assert!(
        timeout(Duration::from_millis(500), nb.accept()).await.is_err(),
        "no connection may be accepted"
    );
    assert!(nb.connections().is_empty());
    assert!(audit_kinds(&b).contains(&"peer_rejected".to_string()));
    // And the other way round: a does not pin b → refused before any packet is sent.
    let c = Peer::new();
    let e = na.connect(nb.local_addr().unwrap(), c.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerUnauthorized);
}

/// B's verifier refuses A's revoked key inside the handshake. A cannot tell that alert from one
/// forged on the path (it arrives before B sent any authenticated control data), so A reads it as
/// a device that did not prove it is B, never as revoked; nothing is accepted on B.
#[tokio::test]
async fn revoked_key_rejected() {
    let (a, b) = pair();
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    let (na, nb) = (a.start().await, b.start().await);
    let e = na.connect(nb.local_addr().unwrap(), b.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{e}");
    assert!(e.message.contains("did not prove"), "{e}");
    assert!(timeout(Duration::from_millis(500), nb.accept()).await.is_err());
    // Local revocation refuses before connecting.
    a.peers.revoke(&b.id(), &a.identity).unwrap();
    let e = na.connect(nb.local_addr().unwrap(), b.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerRevoked);
}

/// A server Initial packet with source connection id `scid` and packet number `pn` carrying
/// `frame`, answering the client Initial `client`. Initial keys come from the client's public
/// destination connection id (RFC 9001 5.2), so anything that sees the dial can send this.
fn forged_initial(client: &[u8], scid: &[u8], pn: u8, frame: &[u8]) -> Vec<u8> {
    use rustls::quic::{Keys, Version};
    let dcid_len = usize::from(client[5]);
    let client_dcid = &client[6..6 + dcid_len];
    let cscid_len = usize::from(client[6 + dcid_len]);
    let client_scid = &client[7 + dcid_len..7 + dcid_len + cscid_len];
    let suite = rustls::crypto::ring::cipher_suite::TLS13_AES_128_GCM_SHA256
        .tls13()
        .unwrap();
    let keys = Keys::initial(
        Version::V1,
        suite,
        suite.quic.unwrap(),
        client_dcid,
        rustls::Side::Server,
    );
    let mut payload = frame.to_vec();
    payload.resize(payload.len().max(20), 0); // PADDING: room for the header protection sample
    let mut p = vec![0xc0, 0, 0, 0, 1, cscid_len as u8]; // Initial, 1-byte packet number, QUIC v1
    p.extend_from_slice(client_scid);
    p.push(scid.len() as u8);
    p.extend_from_slice(scid);
    p.push(0); // no token
    let len = 1 + payload.len() + 16;
    p.extend_from_slice(&[0x40 | (len >> 8) as u8, len as u8, pn]);
    let pn_at = p.len() - 1;
    let tag = keys
        .local
        .packet
        .encrypt_in_place(u64::from(pn), &p, &mut payload)
        .unwrap();
    p.extend_from_slice(&payload);
    p.extend_from_slice(tag.as_ref());
    let sample = p[pn_at + 4..pn_at + 20].to_vec();
    let (head, rest) = p.split_at_mut(1);
    keys.local
        .header
        .encrypt_in_place(&sample, &mut head[0], &mut rest[pn_at - 1..pn_at])
        .unwrap();
    p
}

/// A server Initial packet that closes the connection with TLS alert `alert`, answering the
/// client Initial `client`, before any key was shown.
fn forged_initial_close(client: &[u8], alert: u8) -> Vec<u8> {
    // CONNECTION_CLOSE (0x1c), error 0x100 + alert as a 2-byte varint, frame type 0, no reason.
    forged_initial(client, &[1, 2, 3, 4, 5, 6, 7, 8], 0, &[0x1c, 0x41, alert, 0, 0])
}

/// A `certificate_revoked` alert from an address where the dialled device never showed its key
/// is not that device's verdict: it reads as another device at that address, never as revoked
/// (a host on a stale address could otherwise get a healthy device revoked or re-paired).
#[tokio::test]
async fn forged_revoked_alert_before_the_key_is_not_a_revocation() {
    let (a, b) = pair();
    let na = a.start().await;
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let forger = tokio::spawn(async move {
        let mut buf = [0u8; 1500];
        let (n, from) = sock.recv_from(&mut buf).await.unwrap();
        sock.send_to(&forged_initial_close(&buf[..n], 44), from).await.unwrap();
    });
    let e = timeout(Duration::from_secs(5), na.connect(addr, b.id()))
        .await
        .expect("the forged close ends the dial")
        .unwrap_err();
    forger.await.unwrap();
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{e}");
    assert!(
        e.message
            .contains(&format!("{addr} did not prove it is {}", b.id().short_label())),
        "{e}"
    );
    assert!(
        e.hint.as_deref().unwrap_or("").contains("nothing was trusted or saved"),
        "{e}"
    );
}

/// A QUIC varint at the start of `b`: (value, its length).
fn varint(b: &[u8]) -> (usize, usize) {
    let k = 1usize << (b[0] >> 6);
    let v = b[1..k]
        .iter()
        .fold(usize::from(b[0] & 0x3f), |v, x| v << 8 | usize::from(*x));
    (v, k)
}

/// Where the long-header packets at the start of datagram `d` end, when one of them is a
/// Handshake packet (the server's flight: Certificate, CertificateVerify, Finished).
fn after_handshake_packets(d: &[u8]) -> Option<usize> {
    let (mut at, mut handshake) = (0, false);
    while at < d.len() && d[at] & 0x80 != 0 {
        let kind = (d[at] >> 4) & 3; // 0 Initial, 1 0-RTT, 2 Handshake, 3 Retry
        if kind == 3 {
            return None;
        }
        handshake |= kind == 2;
        let mut p = at + 6 + usize::from(d[at + 5]);
        p += 1 + usize::from(d[p]);
        if kind == 0 {
            let (token, k) = varint(&d[p..]);
            p += k + token;
        }
        let (len, k) = varint(&d[p..]);
        at = p + k + len;
    }
    handshake.then_some(at)
}

/// TEST-ONLY on-path attacker in front of `server`: relays a dial both ways and, into the
/// server's first datagram that carries its Handshake flight, right after that flight, inserts a
/// forged Initial packet carrying `frame` (a CONNECTION_CLOSE). The dialler checks the genuine
/// key, signature and Finished, then processes the forgery before it sends anything (its Initial
/// keys are still live). Returns the address to dial and whether the forgery went out.
async fn forging_relay(server: std::net::SocketAddr, frame: Vec<u8>) -> (std::net::SocketAddr, Arc<AtomicBool>) {
    let r_in = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let r_out = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let front = r_in.local_addr().unwrap();
    let forged = Arc::new(AtomicBool::new(false));
    let done = forged.clone();
    tokio::spawn(async move {
        let (mut bi, mut bo) = (vec![0u8; 65535], vec![0u8; 65535]);
        let (mut client, mut initial) = (None, None::<Vec<u8>>);
        loop {
            tokio::select! {
                r = r_in.recv_from(&mut bi) => {
                    let Ok((n, from)) = r else { return };
                    client = Some(from);
                    if bi[0] & 0xb0 == 0x80 {
                        initial = Some(bi[..n].to_vec()); // the client's latest Initial: its keys
                    }
                    let _ = r_out.send_to(&bi[..n], server).await;
                }
                r = r_out.recv_from(&mut bo) => {
                    let (Ok((n, _)), Some(to)) = (r, client) else { continue };
                    let mut d = bo[..n].to_vec();
                    if let (false, Some(at), Some(ci)) = (done.load(Ordering::SeqCst), after_handshake_packets(&d), &initial) {
                        let scid = &d[7 + usize::from(d[5])..][..usize::from(d[6 + usize::from(d[5])])];
                        let packet = forged_initial(ci, scid, 9, &frame);
                        d.splice(at..at, packet);
                        done.store(true, Ordering::SeqCst);
                    }
                    let _ = r_in.send_to(&d, to).await;
                }
            }
        }
    });
    (front, forged)
}

/// trust-3: a close appended on the path to the device's genuine flight (after its pinned key was
/// shown and its signature and Finished checked) is processed by the dialler in the Initial space,
/// whose keys anyone on the path has: it proves nothing. Neither a `certificate_revoked` alert nor
/// a `CLOSE_HELD` reads as `peer_revoked`/`peer_held`, so it can never end a dial walk; it reads
/// as another device at that address. SINGLE-HOST: an in-process relay on loopback forges it.
#[tokio::test]
async fn a_close_forged_after_the_genuine_flight_is_not_a_refusal_single_host() {
    let (a, b) = pair();
    let (na, nb) = (a.start().await, b.start().await);
    let held = CLOSE_HELD.to_be_bytes();
    for (what, frame) in [
        ("certificate_revoked", vec![0x1c, 0x41, 44, 0, 0]),
        ("CLOSE_HELD", vec![0x1d, 0x80, held[1], held[2], held[3], 0]),
    ] {
        let (front, forged) = forging_relay(nb.local_addr().unwrap(), frame).await;
        let e = timeout(Duration::from_secs(10), na.connect(front, b.id()))
            .await
            .expect("the forged close ends the dial")
            .unwrap_err();
        assert!(forged.load(Ordering::SeqCst), "{what}: the forgery was sent");
        assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{what}: {e}");
        assert!(
            e.message
                .contains(&format!("{front} did not prove it is {}", b.id().short_label())),
            "{what}: {e}"
        );
        eprintln!("SINGLE-HOST forged {what} after the genuine flight: {e}");
    }
    assert!(nb.connections().is_empty());
}

/// Counts what a held exchange hands the daemon; writes nothing.
#[derive(Default)]
struct Counts {
    offers: AtomicUsize,
    received: AtomicUsize,
}

struct CountingHook(Arc<Counts>);

impl HeldControl for CountingHook {
    fn offer<'a>(&'a self, _: PeerId, _: &'a mut ControlStream) -> BoxFuture<'a, bool> {
        self.0.offers.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { true })
    }
    fn received<'a>(&'a self, _: &'a Node, _: PeerId, _: ControlMessage) -> BoxFuture<'a, ()> {
        self.0.received.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {})
    }
}

/// `b`'s Node holding `a` (E5), with the daemon's held exchange stood in for by counters.
async fn holder(a: &common::Peer, b: &common::Peer) -> (Arc<Node>, Arc<Counts>) {
    let nb = b.start().await;
    let counts = Arc::new(Counts::default());
    nb.set_held_control(Box::new(CountingHook(counts.clone())));
    nb.hold(&a.id());
    (nb, counts)
}

fn caps_frame(me: &common::Peer) -> ControlMessage {
    Body::Capabilities(sign_capabilities(&me.identity, "{}".into(), 1, &[], &[]).unwrap()).into()
}

/// E5 across protocol minors, SINGLE-HOST. A held peer below [`HELD_EXCHANGE_MINOR`] (the
/// deployed minor-0 builds) gets `CLOSE_HELD` right after its Capabilities, with no frame first
/// (an `ErrorMsg` where it expects Capabilities would read as a protocol error there); a minor-1
/// peer gets `ErrorMsg{peer_held}` first, then the revocation exchange, then `CLOSE_HELD`. One
/// `peer_rejected` line (reason `held`) per hold. Unheld, a minor-0 peer still connects at minor 0.
/// (Pairing carries no protocol minor.)
#[tokio::test]
async fn a_held_peer_of_each_minor_reads_the_hold_single_host() {
    let (a, b) = pair();
    let (nb, counts) = holder(&a, &b).await;
    let addr = nb.local_addr().unwrap();

    let old = HELD_EXCHANGE_MINOR - 1;
    let (_ep, conn, mut send, mut recv, theirs) = raw_after_hello(&a, addr, hello(&a, PROTOCOL_MAJOR, old)).await;
    assert_eq!(theirs.minor, PROTOCOL_MINOR);
    write_frame(&mut send, &caps_frame(&a)).await.unwrap();
    let frame = read_frame(&mut recv, MAX_CONTROL_FRAME).await;
    assert!(!matches!(frame, Ok(Some(_))), "no frame before the close: {frame:?}");
    let reason = timeout(Duration::from_secs(5), conn.closed()).await.expect("closed");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_HELD)), "{reason:?}");
    assert_eq!(map_conn_error(&reason).code, ErrorCode::PeerHeld);
    assert_eq!(
        counts.offers.load(Ordering::SeqCst),
        0,
        "nothing exchanged at minor {old}"
    );

    let (_ep, conn, mut send, mut recv, _) =
        raw_after_hello(&a, addr, hello(&a, PROTOCOL_MAJOR, HELD_EXCHANGE_MINOR)).await;
    write_frame(&mut send, &caps_frame(&a)).await.unwrap();
    match read_frame(&mut recv, MAX_CONTROL_FRAME).await.unwrap() {
        Some(ControlMessage {
            body: Some(Body::Error(e)),
        }) => assert_eq!(e.code, "peer_held"),
        other => panic!("expected ErrorMsg peer_held, got {:?}", other.map(|m| m.body)),
    }
    assert!(
        read_frame(&mut recv, MAX_CONTROL_FRAME).await.unwrap().is_none(),
        "the holder's records end"
    );
    send.finish().unwrap();
    let reason = timeout(Duration::from_secs(5), conn.closed()).await.expect("closed");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_HELD)), "{reason:?}");
    assert_eq!(counts.offers.load(Ordering::SeqCst), 1);
    let held: Vec<_> = b
        .audit
        .tail(50)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "peer_rejected" && e.fields["reason"] == "held")
        .collect();
    assert_eq!(held.len(), 1, "{held:?}");

    nb.release(&a.id());
    let (_ep, conn, mut send, mut recv, _) = raw_after_hello(&a, addr, hello(&a, PROTOCOL_MAJOR, old)).await;
    write_frame(&mut send, &caps_frame(&a)).await.unwrap();
    let accepted = timeout(Duration::from_secs(5), nb.accept()).await.unwrap().unwrap();
    assert_eq!(accepted.negotiated_minor, old);
    assert!(matches!(
        read_frame(&mut recv, MAX_CAPABILITIES_BYTES).await.unwrap(),
        Some(ControlMessage {
            body: Some(Body::Capabilities(_))
        })
    ));
    assert!(conn.close_reason().is_none());
}

/// F-06-05 on a held exchange (E5), SINGLE-HOST: like a registered connection, it exchanges
/// nothing with a key the store stopped allowing meanwhile, and a revoke or unpair closes it at
/// once. (1) Revoked after Hello: the holder offers nothing and closes `CLOSE_REVOKED`. (2) Unpaired
/// after the holder's records: the peer's frame is not applied, closed `CLOSE_UNAUTHORIZED`.
/// (3) Revoked while the holder waits for the peer's records: closed `CLOSE_REVOKED` at once, not
/// at the exchange's 10 s bound.
#[tokio::test]
async fn a_held_exchange_follows_the_store_single_host() {
    let first_frame = |recv: quinn::RecvStream| async move {
        let mut recv = recv;
        let _ = read_frame(&mut recv, MAX_CONTROL_FRAME).await;
        recv
    };

    let (a, b) = pair();
    let (nb, counts) = holder(&a, &b).await;
    let (_ep, conn, mut send, _recv, _) =
        raw_after_hello(&a, nb.local_addr().unwrap(), hello(&a, PROTOCOL_MAJOR, PROTOCOL_MINOR)).await;
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    write_frame(&mut send, &caps_frame(&a)).await.unwrap();
    let reason = timeout(Duration::from_secs(5), conn.closed()).await.expect("closed");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_REVOKED)), "{reason:?}");
    assert_eq!(
        counts.offers.load(Ordering::SeqCst),
        0,
        "nothing offered to a revoked key"
    );

    let (a, b) = pair();
    let (nb, counts) = holder(&a, &b).await;
    let (_ep, conn, mut send, recv, _) =
        raw_after_hello(&a, nb.local_addr().unwrap(), hello(&a, PROTOCOL_MAJOR, PROTOCOL_MINOR)).await;
    write_frame(&mut send, &caps_frame(&a)).await.unwrap();
    let mut recv = first_frame(recv).await; // peer_held
    assert!(read_frame(&mut recv, MAX_CONTROL_FRAME).await.unwrap().is_none());
    b.peers.remove(&a.id()).unwrap();
    let rev = RevocationMsg {
        seq: 1,
        peer_id: vec![9; 32],
        revoked_by: a.id().0.to_vec(),
        at_utc: "2026-10-22T00:00:00Z".into(),
        signature: vec![0; 64],
    };
    write_frame(&mut send, &Body::Revocation(rev).into()).await.unwrap();
    send.finish().unwrap();
    let reason = timeout(Duration::from_secs(5), conn.closed()).await.expect("closed");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_UNAUTHORIZED)), "{reason:?}");
    assert_eq!(
        counts.received.load(Ordering::SeqCst),
        0,
        "nothing applied from an unpaired key"
    );

    let (a, b) = pair();
    let (nb, _counts) = holder(&a, &b).await;
    let (_ep, conn, mut send, recv, _) =
        raw_after_hello(&a, nb.local_addr().unwrap(), hello(&a, PROTOCOL_MAJOR, PROTOCOL_MINOR)).await;
    write_frame(&mut send, &caps_frame(&a)).await.unwrap();
    let mut recv = first_frame(recv).await;
    assert!(read_frame(&mut recv, MAX_CONTROL_FRAME).await.unwrap().is_none());
    // The holder now waits for a's records, which never come.
    let t0 = Instant::now();
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    nb.disconnect(&a.id(), "revoked").await;
    let reason = timeout(Duration::from_secs(3), conn.closed())
        .await
        .expect("the revoke closed the held exchange");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_REVOKED)), "{reason:?}");
    eprintln!("SINGLE-HOST held exchange closed {:?} after the revoke", t0.elapsed());
}

#[tokio::test]
async fn capabilities_exchanged_after_handshake() {
    let (a, b) = pair();
    let (na, nb) = (a.start().await, b.start().await);
    na.cached_models.lock().unwrap().push("66967fbece6dbe97".into());
    let (out, inn) = tokio::join!(na.connect(nb.local_addr().unwrap(), b.id()), async {
        nb.accept().await.unwrap()
    });
    let out = out.unwrap();
    let from_a = inn.capabilities.lock().unwrap().clone().unwrap();
    let from_b = out.capabilities.lock().unwrap().clone().unwrap();
    // The record b received is a's own `common hardware --json`, byte for byte.
    assert_eq!(from_a.hardware_json, na.hardware_json);
    assert_eq!(from_b.hardware_json, nb.hardware_json);
    let hw: common_hardware::HardwareReport = serde_json::from_str(&from_a.hardware_json).unwrap();
    assert_eq!(hw.host.os, std::env::consts::OS);
    assert_eq!(from_a.peer_id, a.id().0);
    assert_eq!(from_a.cached_model_sha256, vec!["66967fbece6dbe97".to_string()]);
    assert_eq!(from_a.common_version, common_core::VERSION);
    assert!(from_a.issued_at > 1_700_000_000);
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        assert_eq!(from_a.engines, vec!["llama.cpp b11609 macos-arm64".to_string()]);
    }
    // A record a signs itself verifies under a's key only and stays under the bound.
    let mine = common_network::transport::local_capabilities(&a.identity, &["home".into()], &[]).unwrap();
    assert_eq!(
        verify_capabilities(&mine, &a.id()).unwrap().groups,
        vec!["home".to_string()]
    );
    assert_eq!(
        verify_capabilities(&mine, &b.id()).unwrap_err().code,
        ErrorCode::PeerUnauthorized
    );
    assert!(ControlMessage::from(Body::Capabilities(mine)).encoded_len() <= MAX_CAPABILITIES_BYTES);
}

/// Hand-written client: TLS handshake as `me`, then the control stream up to and including Hello.
async fn raw_after_hello(
    me: &Peer,
    addr: std::net::SocketAddr,
    hello: Hello,
) -> (
    quinn::Endpoint,
    quinn::Connection,
    quinn::SendStream,
    quinn::RecvStream,
    Hello,
) {
    let (ep, conn) = me.raw_connect(addr).await;
    let (mut send, mut recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Body::Hello(hello).into()).await.unwrap();
    let theirs = match read_frame(&mut recv, MAX_CONTROL_FRAME).await.unwrap() {
        Some(ControlMessage {
            body: Some(Body::Hello(h)),
        }) => h,
        other => panic!("expected Hello, got {:?}", other.map(|m| m.body.is_some())),
    };
    (ep, conn, send, recv, theirs)
}

fn hello(me: &Peer, major: u32, minor: u32) -> Hello {
    Hello {
        major,
        minor,
        common_version: "9.9.9".into(),
        peer_id: me.id().0.to_vec(),
        nonce: vec![1; 16],
    }
}

#[tokio::test]
async fn oversized_capability_message_rejected() {
    let (a, b) = pair();
    let nb = b.start().await;
    let (_ep, conn, mut send, _recv, _) =
        raw_after_hello(&a, nb.local_addr().unwrap(), hello(&a, PROTOCOL_MAJOR, 0)).await;
    let caps = sign_capabilities(&a.identity, "x".repeat(MAX_CAPABILITIES_BYTES + 1), 0, &[], &[]);
    // Signing refuses an over-bound record locally; build the wire message by hand instead.
    assert_eq!(caps.unwrap_err().code, ErrorCode::FrameTooLarge);
    let big = Capabilities {
        peer_id: a.id().0.to_vec(),
        hardware_json: "x".repeat(MAX_CAPABILITIES_BYTES + 1),
        ..Default::default()
    };
    let big = sign_capabilities_record(&a.identity, big.encode_to_vec());
    write_frame(&mut send, &Body::Capabilities(big).into()).await.unwrap();
    let reason = timeout(Duration::from_secs(5), conn.closed())
        .await
        .expect("server must close");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_FRAME_TOO_LARGE)), "{reason:?}");
    assert_eq!(map_conn_error(&reason).code, ErrorCode::FrameTooLarge);
    assert!(timeout(Duration::from_millis(300), nb.accept()).await.is_err());
    let rejected = b
        .audit
        .tail(10)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "peer_rejected")
        .expect("audited");
    assert_eq!(rejected.fields["reason"], "frame_too_large");
}

#[tokio::test]
async fn protocol_major_mismatch_is_typed() {
    // Inbound: a foreign major is closed with the protocol code (typed on the sender's side).
    let (a, b) = pair();
    let nb = b.start().await;
    let (ep, conn) = a.raw_connect(nb.local_addr().unwrap()).await;
    let (mut send, _recv) = conn.open_bi().await.unwrap();
    write_frame(&mut send, &Body::Hello(hello(&a, PROTOCOL_MAJOR + 1, 0)).into())
        .await
        .unwrap();
    let reason = timeout(Duration::from_secs(5), conn.closed())
        .await
        .expect("server must close");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_PROTOCOL)), "{reason:?}");
    assert_eq!(map_conn_error(&reason).code, ErrorCode::ProtocolIncompatible);
    assert!(timeout(Duration::from_millis(300), nb.accept()).await.is_err());
    drop((ep, send));

    // Outbound: `Node::connect` against a server answering with major 2 → ProtocolIncompatible
    // naming both versions.
    let na = a.start().await;
    let server = b.raw_server();
    let addr = server.local_addr().unwrap();
    let b_id = b.id();
    let srv = tokio::spawn(async move {
        let conn = server.accept().await.unwrap().accept().unwrap().await.unwrap();
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
        let _ = read_frame(&mut recv, MAX_CONTROL_FRAME).await.unwrap();
        let h = Hello {
            major: 2,
            minor: 0,
            common_version: "9.9.9".into(),
            peer_id: b_id.0.to_vec(),
            nonce: vec![2; 16],
        };
        write_frame(&mut send, &Body::Hello(h).into()).await.unwrap();
        timeout(Duration::from_secs(5), conn.closed()).await.unwrap()
    });
    let e = na.connect(addr, b.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::ProtocolIncompatible, "{e}");
    assert!(
        e.message.contains("2.0") && e.message.contains(&format!("{PROTOCOL_MAJOR}.{PROTOCOL_MINOR}")),
        "{e}"
    );
    assert!(e.message.contains("update Common"), "{e}");
    assert_eq!(app_close(&srv.await.unwrap()), Some(u64::from(CLOSE_PROTOCOL)));
}

#[tokio::test]
async fn minor_negotiation_is_min() {
    let (a, b) = pair();
    let nb = b.start().await;
    let (_ep, conn, mut send, mut recv, theirs) = raw_after_hello(
        &a,
        nb.local_addr().unwrap(),
        hello(&a, PROTOCOL_MAJOR, PROTOCOL_MINOR + 7),
    )
    .await;
    assert_eq!((theirs.major, theirs.minor), (PROTOCOL_MAJOR, PROTOCOL_MINOR));
    assert_eq!(theirs.peer_id, b.id().0);
    let caps = sign_capabilities(&a.identity, "{}".into(), 1, &[], &[]).unwrap();
    write_frame(&mut send, &Body::Capabilities(caps).into()).await.unwrap();
    let accepted = timeout(Duration::from_secs(5), nb.accept()).await.unwrap().unwrap();
    assert_eq!(accepted.peer_id, a.id());
    // min(ours = PROTOCOL_MINOR, theirs = PROTOCOL_MINOR + 7); both orders are covered by the
    // unit test `negotiate_major_mismatch_is_typed_and_minor_is_min`.
    assert_eq!(accepted.negotiated_minor, PROTOCOL_MINOR);
    // The server answers with its own signed Capabilities.
    match read_frame(&mut recv, MAX_CAPABILITIES_BYTES).await.unwrap() {
        Some(ControlMessage {
            body: Some(Body::Capabilities(c)),
        }) => assert_eq!(verify_capabilities(&c, &b.id()).unwrap().peer_id, b.id().0),
        other => panic!("expected Capabilities, got {:?}", other.map(|m| m.body.is_some())),
    }
    assert!(conn.close_reason().is_none());
}

/// F-08-10 old/new interop: a newer peer (higher minor) sends a Capabilities record carrying a
/// field this build does not know, signed over the bytes it sends. The handshake completes, the
/// unknown field is skipped, the known ones arrive intact. SINGLE-HOST.
#[tokio::test]
async fn old_new_peer_interop_single_host() {
    let (a, b) = pair();
    let nb = b.start().await;
    let (_ep, conn, mut send, mut recv, theirs) = raw_after_hello(
        &a,
        nb.local_addr().unwrap(),
        hello(&a, PROTOCOL_MAJOR, PROTOCOL_MINOR + 1),
    )
    .await;
    assert_eq!(theirs.minor, PROTOCOL_MINOR);
    let known = Capabilities {
        schema_version: 2,
        peer_id: a.id().0.to_vec(),
        common_version: "9.9.9".into(),
        hardware_json: "{}".into(),
        free_memory_bytes: 42,
        engines: vec!["llama.cpp b99999 macos-arm64".into()],
        groups: vec!["home".into()],
        issued_at: 1_800_000_000,
        ..Default::default()
    };
    // Minor 1 adds `string accelerator_note = 11;` (length-delimited, key 0x5a) and a varint at tag 99.
    let mut record = known.encode_to_vec();
    record.extend_from_slice(&[0x5a, 0x03, b'n', b'p', b'u', 0x98, 0x06, 0x07]);
    write_frame(
        &mut send,
        &Body::Capabilities(sign_capabilities_record(&a.identity, record)).into(),
    )
    .await
    .unwrap();
    let accepted = timeout(Duration::from_secs(5), nb.accept())
        .await
        .expect("the newer peer is accepted")
        .unwrap();
    assert_eq!(accepted.negotiated_minor, PROTOCOL_MINOR);
    assert_eq!(accepted.capabilities.lock().unwrap().clone().unwrap(), known);
    assert!(matches!(
        read_frame(&mut recv, MAX_CAPABILITIES_BYTES).await.unwrap(),
        Some(ControlMessage {
            body: Some(Body::Capabilities(SignedCapabilities { .. }))
        })
    ));
    assert!(conn.close_reason().is_none());
}

/// F-06-05: a revocation that lands after the TLS handshake but before the control exchange
/// finishes still closes the connection (it is never registered as connected).
#[tokio::test]
async fn revoked_during_control_handshake_is_closed() {
    let (a, b) = pair();
    let nb = b.start().await;
    let (_ep, conn, mut send, _recv, _) =
        raw_after_hello(&a, nb.local_addr().unwrap(), hello(&a, PROTOCOL_MAJOR, PROTOCOL_MINOR)).await;
    // TLS and Hello are done; b revokes a now, then a sends its Capabilities.
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    nb.disconnect(&a.id(), "revoked").await; // finds nothing: not registered yet
    let caps = sign_capabilities(&a.identity, "{}".into(), 1, &[], &[]).unwrap();
    write_frame(&mut send, &Body::Capabilities(caps).into()).await.unwrap();
    let reason = timeout(Duration::from_secs(5), conn.closed())
        .await
        .expect("closed although the revoke raced the handshake");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_REVOKED)), "{reason:?}");
    assert!(timeout(Duration::from_millis(300), nb.accept()).await.is_err());
    assert!(nb.connections().is_empty());
}

/// The workload endpoint never resumes a TLS session: a revoked device that reuses its client
/// config (and so its rustls session cache) is still refused inside the handshake.
#[tokio::test]
async fn revoked_key_cannot_reconnect_with_cached_session() {
    let (a, b) = pair();
    let nb = b.start().await;
    let addr = nb.local_addr().unwrap();
    let mut ep = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    ep.set_default_client_config(
        tls::client_config(&a.identity, PeerPolicy::Pinned(a.peers.clone()), ALPN_WORKLOAD).unwrap(),
    );
    let first = ep.connect(addr, "common").unwrap().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await; // any session ticket would arrive now
    first.close(0u32.into(), b"");
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    let e = match ep.connect(addr, "common").unwrap().await {
        Ok(conn) => timeout(Duration::from_secs(5), conn.closed())
            .await
            .expect("the server must close a revoked client"),
        Err(e) => e,
    };
    assert_eq!(map_conn_error(&e).code, ErrorCode::PeerRevoked, "{e:?}");
    assert!(timeout(Duration::from_millis(300), nb.accept()).await.is_err());
}

/// Unauthenticated connection attempts write at most 20 `peer_rejected` lines per minute.
#[tokio::test]
async fn rejection_audit_is_rate_limited() {
    let (a, b) = (Peer::new(), Peer::new());
    a.pin(&b);
    let nb = b.start().await;
    let addr = nb.local_addr().unwrap();
    let na = a.start().await;
    for _ in 0..30 {
        assert_eq!(
            na.connect(addr, b.id()).await.unwrap_err().code,
            ErrorCode::PeerUnauthorized
        );
    }
    let rejected = || {
        b.audit
            .tail(100)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "peer_rejected")
            .count()
    };
    poll_until(Duration::from_secs(2), || rejected() >= 20).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rejected(),
        20,
        "audit lines beyond the per-minute cap are counted, not written"
    );
}

#[tokio::test]
async fn disconnect_within_5s() {
    let (a, b) = pair();
    let (na, nb) = (a.start().await, b.start().await);
    let (out, inn) = tokio::join!(na.connect(nb.local_addr().unwrap(), b.id()), async {
        nb.accept().await.unwrap()
    });
    let out = out.unwrap();
    let t0 = Instant::now();
    nb.disconnect(&a.id(), "revoked").await;
    let reason = timeout(Duration::from_secs(5), out.conn.closed())
        .await
        .expect("closed within 5 s");
    assert_eq!(app_close(&reason), Some(u64::from(CLOSE_REVOKED)), "{reason:?}");
    assert_eq!(u64::from(CLOSE_REVOKED), 0x5245);
    assert_eq!(map_conn_error(&reason).code, ErrorCode::PeerRevoked);
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert!(nb.connections().is_empty());
    assert!(na.connections().is_empty(), "closed connections are pruned");
    assert!(inn.conn.close_reason().is_some());
    assert!(audit_kinds(&b).contains(&"peer_disconnected".to_string()));
    // A plain shutdown closes with code 0.
    let (out2, _inn2) = tokio::join!(na.connect(nb.local_addr().unwrap(), b.id()), async {
        nb.accept().await.unwrap()
    });
    let out2 = out2.unwrap();
    nb.shutdown().await;
    let reason = timeout(Duration::from_secs(5), out2.conn.closed()).await.unwrap();
    assert_eq!(app_close(&reason), Some(0));
    assert!(nb.accept().await.is_none(), "accept ends after shutdown");
}

/// F-02-05: the server vanishes without a close (its runtime is torn down, so no CONNECTION_CLOSE
/// is ever sent); the client must observe the loss within 6 s (idle 4 s + one 1 s keep-alive
/// worst case). SINGLE-HOST: both nodes run in this process on this Mac; see
/// `sigkill_peer_process_detected_within_bound_single_host` for a separate, SIGKILLed process.
#[tokio::test]
async fn kill_peer_detected_within_bound_single_host() {
    let (a, b) = pair();
    let na = a.start().await;
    let server_rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let b_id = b.id();
    server_rt.spawn(async move {
        let nb = b.start().await;
        tx.send(nb.local_addr().unwrap()).unwrap();
        let _held = nb.accept().await;
        std::future::pending::<()>().await;
    });
    let addr = rx.await.unwrap();
    let out = na.connect(addr, b_id).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        out.conn.close_reason().is_none(),
        "keep-alives keep it up while the peer lives"
    );
    let t0 = Instant::now();
    server_rt.shutdown_background();
    let reason = timeout(DETECTION_BOUND, out.conn.closed())
        .await
        .expect("peer loss must be detected within 6 s");
    let took = t0.elapsed();
    assert!(matches!(reason, ConnectionError::TimedOut), "{reason:?}");
    assert_eq!(map_conn_error(&reason).code, ErrorCode::PeerLost);
    assert!(took <= DETECTION_BOUND, "{took:?}");
    eprintln!("SINGLE-HOST: in-process peer teardown detected in {took:?} (bound 6 s)");
    assert!(na.connections().is_empty());
    assert_eq!(na.health(&b_id), Some(PeerHealth::Reconnecting));
}

/// Helper, not a test of its own: the serving node of
/// `sigkill_peer_process_detected_within_bound_single_host`, run as a separate process.
#[test]
#[ignore = "child-process helper; run by sigkill_peer_process_detected_within_bound_single_host"]
fn child_node_process() {
    let Some(home) = child_home() else { return };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let (identity, peers, audit, policy) = Peer::open(&home);
        let node = Node::start(Peer::config(), identity, peers, policy, audit)
            .await
            .unwrap();
        println!("CHILD_ADDR {}", node.local_addr().unwrap());
        let _held = node.accept().await;
        std::future::pending::<()>().await;
    });
}

/// F-02-05 as written: a peer **process** is SIGKILLed (no close, no goodbye) and the loss is
/// detected within 3 heartbeat intervals (<= 6 s). SINGLE-HOST: the child is a second process
/// on this Mac (the test binary re-run as `child_node_process`), not a second machine.
#[tokio::test]
async fn sigkill_peer_process_detected_within_bound_single_host() {
    let (a, c) = pair();
    let mut child = spawn_child("child_node_process", c.home());
    let addr: std::net::SocketAddr = child_value(&mut child, "CHILD_ADDR ").await.parse().unwrap();
    let na = a.start().await;
    let out = na.connect(addr, c.id()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(out.conn.close_reason().is_none(), "alive while the child runs");
    assert_eq!(na.health(&c.id()), Some(PeerHealth::Connected));
    let t0 = Instant::now();
    child.kill().unwrap(); // SIGKILL
    child.wait().unwrap();
    let reason = timeout(DETECTION_BOUND, out.conn.closed())
        .await
        .expect("SIGKILLed peer must be detected within 6 s");
    let took = t0.elapsed();
    assert!(matches!(reason, ConnectionError::TimedOut), "{reason:?}");
    assert!(took <= DETECTION_BOUND, "{took:?}");
    assert_eq!(na.health(&c.id()), Some(PeerHealth::Reconnecting));
    eprintln!("SINGLE-HOST bench:disconnect-detection: SIGKILLed peer process detected in {took:?} (bound 6 s, keep-alive 1 s, idle 4 s)");
}

/// F-02-05 partition and recovery: a relay between a and b drops every datagram (partition);
/// a's view of b goes Unhealthy after 3 missed keep-alives and the connection is declared lost
/// within 6 s; when the relay passes traffic again, `supervise` reconnects and health returns to
/// Connected. SINGLE-HOST: an in-process UDP relay on loopback, not a real network.
#[tokio::test]
async fn partition_detected_and_recovered_single_host() {
    let (a, b) = pair();
    let nb = b.start().await;
    let accepter = nb.clone();
    tokio::spawn(async move { while accepter.accept().await.is_some() {} });
    let (front, _, _, blocked, _relay) = start_relay(nb.local_addr().unwrap()).await;
    let na = a.start().await;
    let connects = Arc::new(AtomicUsize::new(0));
    let counter = connects.clone();
    let _sup = supervise(&na, b.id(), front, move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    let health = || na.health(&b.id());
    poll_until(Duration::from_secs(5), || {
        connects.load(Ordering::SeqCst) == 1 && health() == Some(PeerHealth::Connected)
    })
    .await
    .expect("supervisor connects");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(health(), Some(PeerHealth::Connected), "keep-alives count as heartbeats");

    blocked.store(true, Ordering::SeqCst);
    let t0 = Instant::now();
    let unhealthy = poll_until(DETECTION_BOUND, || health() == Some(PeerHealth::Unhealthy))
        .await
        .expect("3 missed keep-alives mark the peer unhealthy");
    let lost = poll_until(DETECTION_BOUND, || na.connections().is_empty())
        .await
        .map(|_| t0.elapsed())
        .expect("partition detected within 6 s");
    assert!(lost <= DETECTION_BOUND, "{lost:?}");
    assert!(matches!(health(), Some(PeerHealth::Reconnecting)), "{:?}", health());

    tokio::time::sleep(Duration::from_secs(2)).await; // a reconnect attempt fails into the partition
    assert_eq!(connects.load(Ordering::SeqCst), 1);
    blocked.store(false, Ordering::SeqCst);
    let recovered = poll_until(Duration::from_secs(15), || {
        connects.load(Ordering::SeqCst) == 2 && health() == Some(PeerHealth::Connected)
    })
    .await
    .expect("recovery restores Connected");
    assert_eq!(na.connections().len(), 1);
    eprintln!(
        "SINGLE-HOST bench:disconnect-detection partition: unhealthy after {unhealthy:?}, lost after {lost:?} (bound 6 s); reconnected {recovered:?} after the partition healed"
    );
}

/// A supervised peer that revokes this device ends supervision: no reconnect storm.
#[tokio::test]
async fn supervise_stops_when_the_peer_refuses() {
    let (a, b) = pair();
    let nb = b.start().await;
    let accepter = nb.clone();
    tokio::spawn(async move { while accepter.accept().await.is_some() {} });
    let na = a.start().await;
    let connects = Arc::new(AtomicUsize::new(0));
    let counter = connects.clone();
    let sup = supervise(&na, b.id(), nb.local_addr().unwrap(), move |_| {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    poll_until(Duration::from_secs(5), || connects.load(Ordering::SeqCst) == 1)
        .await
        .unwrap();
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    nb.disconnect(&a.id(), "revoked").await;
    timeout(Duration::from_secs(5), sup)
        .await
        .expect("supervisor ends after the peer refuses")
        .unwrap();
    assert_eq!(na.health(&b.id()), Some(PeerHealth::Disconnected));
    assert_eq!(connects.load(Ordering::SeqCst), 1);
}

/// Two daemons' dial loops dial each other at the same moment: within 2 s each side holds exactly
/// one live connection to the other, the one initiated by the lower PeerId (outbound on that side,
/// inbound on the other), it stays the only one, and a later `connect` returns it without
/// dialling. Repeated with fresh identities so either id order and both race outcomes occur.
/// SINGLE-HOST: two in-process nodes on loopback.
#[tokio::test]
async fn crossed_dials_keep_one_connection_single_host() {
    for round in 0..6 {
        let (a, b) = pair();
        let (na, nb) = (a.start().await, b.start().await);
        for n in [&na, &nb] {
            let n = n.clone();
            tokio::spawn(async move { while n.accept().await.is_some() {} });
        }
        let t0 = Instant::now();
        let (ra, rb) = tokio::join!(
            na.connect(nb.local_addr().unwrap(), b.id()),
            nb.connect(na.local_addr().unwrap(), a.id())
        );
        let dialled = t0.elapsed();
        let (ra, rb) = (ra.unwrap(), rb.unwrap());
        assert_eq!((ra.peer_id, rb.peer_id), (b.id(), a.id()));
        // From the moment its own dial returned, the lower id hands out that connection first, even
        // while the higher id's crossed one is still open on its side (closed once idle).
        let a_lower = a.id() < b.id();
        let (lower, higher) = if a_lower { (&na, b.id()) } else { (&nb, a.id()) };
        let first = lower.connections().into_iter().find(|c| c.peer_id == higher).unwrap();
        assert!(
            first.outbound,
            "round {round}: the lower id routes onto its own connection"
        );
        assert!(Arc::ptr_eq(&first, &lower.live(&higher).unwrap()));
        let live = |n: &Node, other| {
            n.connections()
                .into_iter()
                .filter(|c| c.peer_id == other)
                .collect::<Vec<_>>()
        };
        let took = poll_until(Duration::from_secs(2), || {
            live(&na, b.id()).len() == 1 && live(&nb, a.id()).len() == 1
        })
        .await
        .unwrap_or_else(|| {
            panic!(
                "round {round}: a holds {:?}, b holds {:?}",
                live(&na, b.id()),
                live(&nb, a.id())
            )
        });
        let (ka, kb) = (live(&na, b.id()).remove(0), live(&nb, a.id()).remove(0));
        assert_eq!(ka.outbound, a_lower, "round {round}: a keeps the lower id's connection");
        assert_eq!(
            kb.outbound, !a_lower,
            "round {round}: b keeps the lower id's connection"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(live(&na, b.id()).len(), 1);
        assert_eq!(live(&nb, a.id()).len(), 1);
        assert!(ka.conn.close_reason().is_none() && kb.conn.close_reason().is_none());
        let again = na.connect(nb.local_addr().unwrap(), b.id()).await.unwrap();
        assert!(Arc::ptr_eq(&again, &ka), "connect reuses the live connection");
        assert_eq!(na.health(&b.id()), Some(PeerHealth::Connected));
        assert_eq!(nb.health(&a.id()), Some(PeerHealth::Connected));
        eprintln!(
            "SINGLE-HOST crossed dials round {round}: both connects returned after {dialled:?}, one connection per side {took:?} later (a<b: {a_lower})"
        );
    }
}

/// A duplicate already carrying a request is retired, not cut: the higher id stops handing it out
/// as soon as the lower id's connection registers, keeps it open while a request stream is in
/// flight on it, and closes it with CLOSE_OK once idle. A revoke still closes a retired connection
/// at once (F-06-05). SINGLE-HOST: two in-process nodes plus a raw client (the lower id's key) on
/// loopback.
#[tokio::test]
async fn busy_duplicate_is_retired_not_cut_single_host() {
    for revoke in [false, true] {
        let (x, y) = pair();
        let (hi, lo) = if x.id() > y.id() { (x, y) } else { (y, x) };
        let (nh, nl) = (hi.start().await, lo.start().await);
        let (out, at_lo) = tokio::join!(nh.connect(nl.local_addr().unwrap(), lo.id()), async {
            nl.accept().await.unwrap()
        });
        let out = out.unwrap();
        // A request starts on the higher id's connection, the only one so far.
        let busy = out.in_flight();
        let (mut send, mut recv) = out.conn.open_bi().await.unwrap();
        send.write_all(b"ping").await.unwrap();
        // The lower id's own connection arrives (a raw client with its key): it wins.
        let (_ep, _raw, mut raw_send, _raw_recv, _) = raw_after_hello(
            &lo,
            nh.local_addr().unwrap(),
            hello(&lo, PROTOCOL_MAJOR, PROTOCOL_MINOR),
        )
        .await;
        let caps = sign_capabilities(&lo.identity, "{}".into(), 1, &[], &[]).unwrap();
        write_frame(&mut raw_send, &Body::Capabilities(caps).into())
            .await
            .unwrap();
        let won = timeout(Duration::from_secs(5), nh.accept()).await.unwrap().unwrap();
        assert!(!won.outbound);
        let handed: Vec<_> = nh.connections().into_iter().filter(|c| c.peer_id == lo.id()).collect();
        assert_eq!(handed.len(), 1, "the retired connection is no longer handed out");
        assert!(Arc::ptr_eq(&handed[0], &won) && Arc::ptr_eq(&nh.live(&lo.id()).unwrap(), &won));
        // Well past the idle poll, the busy connection is still open and the request completes.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            out.conn.close_reason().is_none(),
            "revoke={revoke}: a busy duplicate was cut"
        );
        let lo_side = at_lo.clone();
        let echo = tokio::spawn(async move {
            let (mut s, mut r) = lo_side.conn.accept_bi().await.unwrap();
            let mut buf = [0u8; 4];
            r.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
            s.finish().unwrap();
            let _ = s.stopped().await;
        });
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(64).await.unwrap(), b"ping");
        echo.await.unwrap();
        let t0 = Instant::now();
        let expect = if revoke {
            // Still busy: a revoke does not wait for it.
            nh.disconnect(&lo.id(), "revoked").await;
            CLOSE_REVOKED
        } else {
            drop((busy, send, recv));
            0 // CLOSE_OK
        };
        let reason = timeout(Duration::from_secs(3), at_lo.conn.closed())
            .await
            .expect("the retired duplicate closes");
        assert_eq!(
            app_close(&reason),
            Some(u64::from(expect)),
            "revoke={revoke}: {reason:?}"
        );
        eprintln!(
            "SINGLE-HOST retired duplicate (revoke={revoke}): open while busy for 1.5 s, closed {:?} after {}",
            t0.elapsed(),
            if revoke { "the revoke" } else { "its last request ended" }
        );
    }
}

/// Unreachable peers (D5): a dial to an address where nothing answers (a bound UDP socket that
/// never reads, so no ICMP either) fails within the idle timeout with a message naming the address,
/// a hint, and a recorded failure. SINGLE-HOST: loopback.
#[tokio::test]
async fn dial_to_silent_address_fails_within_idle_timeout_single_host() {
    let (a, b) = pair();
    let na = a.start().await;
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = silent.local_addr().unwrap();
    let t0 = Instant::now();
    let e = na.connect(addr, b.id()).await.unwrap_err();
    let took = t0.elapsed();
    assert!(
        took >= Duration::from_millis(3500) && took <= Duration::from_secs(6),
        "{took:?}"
    );
    assert_eq!(e.code, ErrorCode::PeerLost, "{e}");
    assert!(
        e.message.contains("no answer") && e.message.contains(&addr.to_string()),
        "{e}"
    );
    assert!(e.hint.is_some(), "{e:?}");
    let f = na.dial_failure(&b.id()).expect("failure recorded");
    assert_eq!((f.addr, f.code), (addr, ErrorCode::PeerLost));
    assert_eq!(f.hint, e.hint);
    eprintln!("SINGLE-HOST dial to a silent address failed after {took:?}: {e}");
}

/// Unreachable peers and reconnection (E4): an offline paired device costs one `peer_rejected`
/// line per state change, not one per attempt; a connection in either direction clears the
/// recorded failure. SINGLE-HOST: a UDP relay on loopback stands in for the network.
#[tokio::test]
async fn dial_failures_are_audited_once_per_state_change_single_host() {
    let (a, b) = pair();
    let nb = b.start().await;
    let accepter = nb.clone();
    tokio::spawn(async move { while accepter.accept().await.is_some() {} });
    let (front, _, _, blocked, _relay) = start_relay(nb.local_addr().unwrap()).await;
    let cfg = common_network::NetworkConfig {
        idle_timeout_ms: 1000,
        keep_alive_ms: 300,
        ..Peer::config()
    };
    let na = Node::start(
        cfg,
        a.identity.clone(),
        a.peers.clone(),
        a.policy.clone(),
        a.audit.clone(),
    )
    .await
    .unwrap();
    let rejected = || {
        a.audit
            .tail(1000)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "peer_rejected")
            .collect::<Vec<_>>()
    };

    blocked.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..3 {
        assert_eq!(na.connect(front, b.id()).await.unwrap_err().code, ErrorCode::PeerLost);
    }
    let lines = rejected();
    assert_eq!(lines.len(), 1, "one line for three failed attempts");
    assert_eq!(lines[0].fields["reason"], "peer_lost");
    assert_eq!(lines[0].fields["direction"], "outbound");

    blocked.store(false, std::sync::atomic::Ordering::SeqCst);
    na.connect(front, b.id()).await.expect("reachable again");
    assert!(audit_kinds(&a).contains(&"peer_connected".to_string()));
    assert_eq!(na.dial_failure(&b.id()), None, "a connection clears the failure");

    na.disconnect(&b.id(), "test").await;
    blocked.store(true, std::sync::atomic::Ordering::SeqCst);
    for _ in 0..2 {
        assert!(na.connect(front, b.id()).await.is_err());
    }
    assert_eq!(rejected().len(), 2, "unreachable again: one more line");
    assert!(na.dial_failure(&b.id()).is_some());

    // b dials a directly: an inbound connection clears a's failure too.
    let accepter = na.clone();
    tokio::spawn(async move { while accepter.accept().await.is_some() {} });
    nb.connect(na.local_addr().unwrap(), a.id())
        .await
        .expect("inbound to a");
    poll_until(Duration::from_secs(2), || na.dial_failure(&b.id()).is_none())
        .await
        .expect("inbound connection clears the failure");
}
