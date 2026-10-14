//! Node-level tests of the authenticated transport (F-02-03, F-02-04, F-02-05, F-06-03, F-08-10).
//! Every test runs two nodes on this one Mac: SINGLE-HOST evidence only.
mod common;

use common::{audit_kinds, pair, Peer};
use common_core::ErrorCode;
use common_network::proto::{control_message::Body, read_frame, write_frame, Capabilities, ControlMessage, Hello};
use common_network::transport::{
    map_conn_error, sign_capabilities, verify_capabilities, Node, CLOSE_FRAME_TOO_LARGE, CLOSE_PROTOCOL, CLOSE_REVOKED,
};
use common_network::{MAX_CAPABILITIES_BYTES, MAX_CONTROL_FRAME, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use quinn::ConnectionError;
use std::time::{Duration, Instant};
use tokio::time::timeout;

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

#[tokio::test]
async fn revoked_key_rejected() {
    let (a, b) = pair();
    b.peers.revoke(&a.id(), &b.identity).unwrap();
    let (na, nb) = (a.start().await, b.start().await);
    let e = na.connect(nb.local_addr().unwrap(), b.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerRevoked, "{e}");
    assert!(timeout(Duration::from_millis(500), nb.accept()).await.is_err());
    // Local revocation refuses before connecting.
    a.peers.revoke(&b.id(), &a.identity).unwrap();
    let e = na.connect(nb.local_addr().unwrap(), b.id()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerRevoked);
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
    verify_capabilities(&from_a, &a.id()).unwrap();
    assert_eq!(
        verify_capabilities(&from_a, &b.id()).unwrap_err().code,
        ErrorCode::PeerUnauthorized
    );
    let mut forged = from_a.clone();
    forged.free_memory_bytes += 1;
    assert_eq!(
        verify_capabilities(&forged, &a.id()).unwrap_err().code,
        ErrorCode::PeerUnauthorized
    );
    // A record a signs itself verifies and stays under the bound.
    let mine = common_network::transport::local_capabilities(&a.identity, &["home".into()], &[]).unwrap();
    verify_capabilities(&mine, &a.id()).unwrap();
    assert!(prost::Message::encoded_len(&mine) <= MAX_CAPABILITIES_BYTES);
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
    let mut caps = sign_capabilities(&a.identity, "x".repeat(MAX_CAPABILITIES_BYTES + 1), 0, &[], &[]);
    // Signing refuses an over-bound record locally; build the wire message by hand instead.
    assert_eq!(caps.as_ref().unwrap_err().code, ErrorCode::FrameTooLarge);
    caps = Ok(Capabilities {
        peer_id: a.id().0.to_vec(),
        hardware_json: "x".repeat(MAX_CAPABILITIES_BYTES + 1),
        ..Default::default()
    });
    write_frame(&mut send, &Body::Capabilities(caps.unwrap()).into())
        .await
        .unwrap();
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
        }) => verify_capabilities(&c, &b.id()).unwrap(),
        other => panic!("expected Capabilities, got {:?}", other.map(|m| m.body.is_some())),
    }
    assert!(conn.close_reason().is_none());
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
/// is ever sent); the client must observe the loss within the 6 s idle timeout + 1 s.
/// SINGLE-HOST: both nodes run on this Mac.
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
    let reason = timeout(Duration::from_secs(7), out.conn.closed())
        .await
        .expect("peer loss must be detected within idle 6 s + 1 s");
    let took = t0.elapsed();
    assert!(matches!(reason, ConnectionError::TimedOut), "{reason:?}");
    assert_eq!(map_conn_error(&reason).code, ErrorCode::PeerLost);
    assert!(took <= Duration::from_secs(7), "{took:?}");
    eprintln!("SINGLE-HOST: peer kill detected in {took:?} (bound 6 s idle + 1 s)");
    assert!(na.connections().is_empty());
}
