//! SPAKE2 pairing over a temporary accept-any QUIC endpoint (ADR-0003 D2).
//!
//! Protocol `common-pair/1`, one bidirectional stream opened by the joiner B on the initiator A's
//! pairing endpoint (TLS 1.3, raw public keys both ways, verifier accepts any key: trust comes
//! from the code, not the handshake). Frames are `u8 length ‖ bytes` (≤ 255 bytes):
//!
//! 1. B → A: `selector ‖ SPAKE2 msg_B` (selector 0 = 6-digit code, 1 = QR secret).
//! 2. A → B: `SPAKE2 msg_A`. Both derive `K` (`spake2` hashes pw, idA, idB, X, Y into it).
//! 3. Both: `ekm = export_keying_material(32, "common-pair/1 confirm", "")`, then send
//!    `HMAC-SHA256(K, side ‖ ekm ‖ idA ‖ idB)` (side `A`/`B`) `‖ u16 workload port ‖ device name`
//!    and verify the other side's MAC in constant time.
//! 4. Both pin the other id (`pin_after_pairing`) and audit `pairing_succeeded`.
//!
//! A relay that terminates TLS on both legs presents its own key on each, so `idA`/`idB` and `ekm`
//! differ per leg and step 3 fails on both ends (`mitm_relay_detected`). More than 5 failed
//! confirmations in 60 s close the endpoint with `PairingLocked`; the window expiry is
//! `PairingFailed`. The joiner connects from its workload endpoint with the pairing client
//! config, so this module binds exactly one extra socket, the initiator's pairing endpoint.
use crate::tls::{self, PeerPolicy};
use crate::transport::{map_conn_error, Node, CLOSE_OK};
use crate::ALPN_PAIRING;
use common_core::{CommonError, ErrorCode, Result};
use common_identity::{utc_now, DeviceIdentity, PeerId, PeerRecord};
use hmac::{Hmac, Mac};
use quinn::VarInt;
use ring::rand::SecureRandom;
use sha2::Sha256;
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

/// `'PF'`: this pairing attempt failed (wrong code, relay, malformed exchange, timeout).
pub const CLOSE_PAIRING_FAILED: u32 = 0x5046;
/// `'PL'`: the initiator locked pairing after too many failed confirmations.
pub const CLOSE_PAIRING_LOCKED: u32 = 0x504C;
const QR_PREFIX: &str = "common-pair:v1:";
const EKM_LABEL: &[u8] = b"common-pair/1 confirm";
const MAX_NAME_BYTES: usize = 64;
const MAC_LEN: usize = 32;
const PW_CODE: u8 = 0;
const PW_QR: u8 = 1;
/// One connect + exchange must finish within this; a stalled joiner cannot hold the window.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_WINDOW: Duration = Duration::from_secs(60);
const MAX_FAILURES: usize = 5;
// The confirm frame (MAC + port + name) always fits one u8-length frame.
const _: () = assert!(MAC_LEN + 2 + MAX_NAME_BYTES <= 255);

pub struct PairingSession {
    /// 6 decimal digits, single use, valid for the window.
    pub code: String,
    pub port: u16,
    pub expires_at_unix: u64,
    /// QR payload: `common-pair:v1:<peer hex>:<ip:port>:<128-bit secret hex>`.
    pub qr_payload: String,
}

fn failed(m: impl Into<String>) -> CommonError {
    CommonError::new(ErrorCode::PairingFailed, m)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    ring::rand::SystemRandom::new()
        .fill(&mut b)
        .map_err(|_| CommonError::internal("system randomness unavailable"))?;
    Ok(b)
}

/// 6 decimal digits, uniform (rejection sampling, no modulo bias).
fn random_code() -> Result<String> {
    loop {
        let n = u32::from_be_bytes(random_bytes::<4>()?);
        if n < 4_294_000_000 {
            return Ok(format!("{:06}", n % 1_000_000));
        }
    }
}

/// The 128-bit QR secret as 32 lowercase hex.
fn random_secret() -> Result<String> {
    Ok(hex::encode(random_bytes::<16>()?))
}

/// Which PAKE password the joiner's input is: the 6-digit code or the QR secret.
fn selector_for(pw: &str) -> Result<u8> {
    if pw.len() == 6 && pw.bytes().all(|b| b.is_ascii_digit()) {
        Ok(PW_CODE)
    } else if pw.len() == 32 && pw.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(PW_QR)
    } else {
        Err(CommonError::invalid(
            "pairing code must be 6 digits or the 32-hex QR secret",
        ))
    }
}

/// Printable, at most 64 bytes (cut on a char boundary); what the peer shows in its device list.
fn clean_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > MAX_NAME_BYTES {
            break;
        }
        out.push(c);
    }
    out.trim().to_string()
}

fn lost(e: impl std::fmt::Display) -> CommonError {
    failed(format!("pairing stream: {e}"))
}

async fn write_blob(s: &mut quinn::SendStream, b: &[u8]) -> Result<()> {
    let len = u8::try_from(b.len()).map_err(|_| failed("pairing frame too large"))?;
    s.write_all(&[&[len], b].concat()).await.map_err(lost)
}

async fn read_blob(r: &mut quinn::RecvStream) -> Result<Vec<u8>> {
    let len = r.read_u8().await.map_err(lost)?;
    let mut b = vec![0u8; usize::from(len)];
    r.read_exact(&mut b).await.map_err(lost)?;
    Ok(b)
}

fn exporter(conn: &quinn::Connection) -> Result<[u8; 32]> {
    let mut out = [0u8; 32];
    conn.export_keying_material(&mut out, EKM_LABEL, b"")
        .map_err(|_| failed("TLS exporter unavailable"))?;
    Ok(out)
}

fn confirm_mac(k: &[u8], side: &[u8], ekm: &[u8; 32], id_a: &PeerId, id_b: &PeerId) -> Hmac<Sha256> {
    let mut m = Hmac::<Sha256>::new_from_slice(k).expect("HMAC accepts any key length");
    m.update(side);
    m.update(ekm);
    m.update(&id_a.0);
    m.update(&id_b.0);
    m
}

enum Role<'a> {
    /// Picks the password the joiner selected.
    Initiator {
        code: &'a str,
        secret: &'a str,
    },
    Joiner {
        pw: &'a str,
    },
}

/// Runs the pairing exchange on an established pairing connection and returns the peer's id,
/// device name and workload port. Any error means nothing was pinned.
async fn exchange(
    conn: &quinn::Connection,
    me: &DeviceIdentity,
    role: Role<'_>,
    name: &str,
    port: u16,
) -> Result<(PeerId, String, u16)> {
    let peer = tls::peer_id_of(conn)?;
    let initiator = matches!(role, Role::Initiator { .. });
    let (id_a, id_b) = if initiator { (me.id, peer) } else { (peer, me.id) };
    let (ia, ib) = (Identity::new(&id_a.0), Identity::new(&id_b.0));
    let (mut send, mut recv) = if initiator {
        conn.accept_bi().await
    } else {
        conn.open_bi().await
    }
    .map_err(|e| failed(map_conn_error(&e).message))?;
    let (state, theirs) = match role {
        Role::Joiner { pw } => {
            let (state, msg) = Spake2::<Ed25519Group>::start_b(&Password::new(pw), &ia, &ib);
            write_blob(&mut send, &[&[selector_for(pw)?], msg.as_slice()].concat()).await?;
            (state, read_blob(&mut recv).await?)
        }
        Role::Initiator { code, secret } => {
            let frame = read_blob(&mut recv).await?;
            let (selector, msg_b) = frame.split_first().ok_or_else(|| failed("empty pairing frame"))?;
            let pw = match *selector {
                PW_CODE => code,
                PW_QR => secret,
                _ => return Err(failed("unknown password selector")),
            };
            let (state, msg) = Spake2::<Ed25519Group>::start_a(&Password::new(pw), &ia, &ib);
            write_blob(&mut send, &msg).await?;
            (state, msg_b.to_vec())
        }
    };
    let k = state.finish(&theirs).map_err(|_| failed("malformed SPAKE2 message"))?;
    let ekm = exporter(conn)?;
    let (mine, theirs) = if initiator { (b"A", b"B") } else { (b"B", b"A") };
    let mut frame = confirm_mac(&k, mine, &ekm, &id_a, &id_b)
        .finalize()
        .into_bytes()
        .to_vec();
    frame.extend_from_slice(&port.to_be_bytes());
    frame.extend_from_slice(name.as_bytes());
    write_blob(&mut send, &frame).await?;
    let got = read_blob(&mut recv).await?;
    if got.len() < MAC_LEN + 2
        || confirm_mac(&k, theirs, &ekm, &id_a, &id_b)
            .verify_slice(&got[..MAC_LEN])
            .is_err()
    {
        return Err(failed(
            "confirmation MAC mismatch: wrong code or the connection is relayed",
        ));
    }
    let their_port = u16::from_be_bytes([got[MAC_LEN], got[MAC_LEN + 1]]);
    let their_name = clean_name(&String::from_utf8_lossy(&got[MAC_LEN + 2..]));
    // `Connection::close` discards unsent stream data: wait until the peer acknowledged our
    // confirm frame (or 2 s) so the caller's close cannot eat it.
    let _ = send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(2), send.stopped()).await;
    Ok((peer, their_name, their_port))
}

fn record(peer: PeerId, name: String, ip: IpAddr, port: u16) -> PeerRecord {
    let now = utc_now();
    PeerRecord {
        id: peer,
        name,
        paired_at_utc: now.clone(),
        last_seen_utc: Some(now),
        last_addr: Some(SocketAddr::new(ip, port).to_string()),
    }
}

fn audit(node: &Node, kind: &str, peer: Option<&PeerId>, fields: serde_json::Value) {
    let hex = peer.map(|p| p.to_hex());
    if let Err(e) = node.audit.record(kind, hex.as_deref(), fields) {
        tracing::debug!(kind, error = %e, "audit record not written");
    }
}

async fn exchange_with_deadline(
    conn: &quinn::Connection,
    me: &DeviceIdentity,
    role: Role<'_>,
    name: &str,
    port: u16,
) -> Result<(PeerId, String, u16)> {
    tokio::time::timeout(EXCHANGE_TIMEOUT, exchange(conn, me, role, name, port))
        .await
        .unwrap_or_else(|_| Err(failed("pairing exchange timed out")))
}

/// A close received from the peer explains the failure instead of our own read error.
fn verdict_from_close(conn: &quinn::Connection, e: CommonError) -> CommonError {
    match conn.close_reason() {
        Some(quinn::ConnectionError::ApplicationClosed(a))
            if a.error_code == VarInt::from_u32(CLOSE_PAIRING_LOCKED) =>
        {
            CommonError::new(
                ErrorCode::PairingLocked,
                "the other device locked pairing after too many failed attempts",
            )
        }
        Some(reason) => failed(format!("pairing failed: {}", map_conn_error(&reason).message)),
        None => e,
    }
}

async fn accept_loop(
    node: &Node,
    endpoint: &quinn::Endpoint,
    code: &str,
    secret: &str,
    name: &str,
    port: u16,
) -> Result<PeerRecord> {
    // ponytail: one attempt at a time; a stalled attempt holds the loop for EXCHANGE_TIMEOUT.
    let mut failures: Vec<Instant> = Vec::new();
    while let Some(incoming) = endpoint.accept().await {
        let conn = match incoming.accept() {
            Ok(connecting) => connecting.await,
            Err(e) => Err(e),
        };
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %map_conn_error(&e), "pairing connection not established");
                continue;
            }
        };
        let role = Role::Initiator { code, secret };
        match exchange_with_deadline(&conn, &node.identity, role, name, port).await {
            Ok((peer, their_name, their_port)) => {
                let rec = record(peer, their_name, conn.remote_address().ip(), their_port);
                node.peers.pin_after_pairing(rec.clone())?;
                conn.close(VarInt::from_u32(CLOSE_OK), b"paired");
                tracing::info!(peer = %peer.short_label(), "pairing succeeded (initiator)");
                audit(
                    node,
                    "pairing_succeeded",
                    Some(&peer),
                    serde_json::json!({ "role": "initiator", "addr": conn.remote_address().to_string() }),
                );
                return Ok(rec);
            }
            Err(e) => {
                // The initiator decides and closes at once; the joiner waits for this verdict.
                let e = verdict_from_close(&conn, e);
                failures.retain(|t| t.elapsed() < LOCK_WINDOW);
                failures.push(Instant::now());
                let locked = failures.len() > MAX_FAILURES;
                let close = if locked {
                    CLOSE_PAIRING_LOCKED
                } else {
                    CLOSE_PAIRING_FAILED
                };
                conn.close(VarInt::from_u32(close), e.code.as_str().as_bytes());
                let peer = tls::peer_id_of(&conn).ok();
                tracing::info!(code = %e.code, failures = failures.len(), "pairing attempt failed");
                audit(
                    node,
                    "pairing_failed",
                    peer.as_ref(),
                    serde_json::json!({ "role": "initiator", "addr": conn.remote_address().to_string(), "reason": e.code.as_str(), "detail": e.message, "failures": failures.len() }),
                );
                if locked {
                    audit(
                        node,
                        "pairing_locked",
                        None,
                        serde_json::json!({ "failures": failures.len() }),
                    );
                    return Err(CommonError::new(
                        ErrorCode::PairingLocked,
                        format!("{} failed pairing attempts within 60 s; pairing closed", failures.len()),
                    )
                    .with_hint("start pairing again to get a new code"));
                }
            }
        }
    }
    Err(failed("pairing endpoint closed"))
}

/// Initiator: open the pairing endpoint, return the code. Completes at most one pairing; >5 failed
/// confirmations in 60 s locks pairing (PairingLocked) and closes the endpoint.
pub async fn start(
    node: Arc<Node>,
    device_name: &str,
) -> Result<(PairingSession, tokio::task::JoinHandle<Result<PeerRecord>>)> {
    let code = random_code()?;
    let secret = random_secret()?;
    let ip: IpAddr = node
        .config
        .bind_addr
        .parse()
        .map_err(|e| CommonError::invalid(format!("network bind address: {e}")))?;
    let sc = tls::server_config(&node.identity, PeerPolicy::AcceptAny, ALPN_PAIRING)?;
    let endpoint = quinn::Endpoint::server(sc, SocketAddr::new(ip, 0))?;
    let port = endpoint.local_addr()?.port();
    let window = Duration::from_secs(node.config.pairing_window_secs);
    // ponytail: the QR carries the bind address; for 0.0.0.0 the CLI substitutes the LAN IP
    // (mDNS already advertises it with `pairport`).
    let qr_payload = format!(
        "{QR_PREFIX}{}:{}:{}",
        node.identity.id.to_hex(),
        SocketAddr::new(ip, port),
        secret
    );
    let session = PairingSession {
        code: code.clone(),
        port,
        expires_at_unix: now_unix() + window.as_secs(),
        qr_payload,
    };
    let name = clean_name(device_name);
    let workload_port = node.local_addr()?.port();
    tracing::info!(port, window_secs = window.as_secs(), "pairing window open");
    audit(
        &node,
        "pairing_started",
        None,
        serde_json::json!({ "port": port, "window_secs": window.as_secs() }),
    );
    let handle = tokio::spawn(async move {
        let r = tokio::time::timeout(
            window,
            accept_loop(&node, &endpoint, &code, &secret, &name, workload_port),
        )
        .await
        .unwrap_or_else(|_| Err(failed("pairing window expired without a successful pairing")));
        let (close, why) = match &r {
            Ok(_) => (CLOSE_OK, "paired"),
            Err(e) if e.code == ErrorCode::PairingLocked => (CLOSE_PAIRING_LOCKED, "pairing locked"),
            Err(_) => (CLOSE_PAIRING_FAILED, "pairing window closed"),
        };
        endpoint.close(VarInt::from_u32(close), why.as_bytes());
        if r.is_err() {
            tracing::info!(why, "pairing window closed");
        }
        r
    });
    Ok((session, handle))
}

/// Joiner: connect to `addr` (pairing endpoint), run SPAKE2 with `code`, verify HMAC confirmation
/// bound to the TLS exporter and both PeerIds, pin the initiator. Wrong code / MITM → PairingFailed.
pub async fn join(node: Arc<Node>, addr: SocketAddr, code: &str, device_name: &str) -> Result<PeerRecord> {
    selector_for(code)?;
    let name = clean_name(device_name);
    let cc = tls::client_config(&node.identity, PeerPolicy::AcceptAny, ALPN_PAIRING)?;
    let connecting = node
        .endpoint
        .connect_with(cc, addr, "common")
        .map_err(|e| failed(format!("connect: {e}")))?;
    let conn = tokio::time::timeout(EXCHANGE_TIMEOUT, connecting)
        .await
        .map_err(|_| failed("pairing endpoint did not answer"))?
        .map_err(|e| failed(format!("pairing connect failed: {}", map_conn_error(&e).message)))?;
    let port = node.local_addr()?.port();
    let role = Role::Joiner { pw: code };
    match exchange_with_deadline(&conn, &node.identity, role, &name, port).await {
        Ok((peer, their_name, their_port)) => {
            let rec = record(peer, their_name, addr.ip(), their_port);
            node.peers.pin_after_pairing(rec.clone())?;
            conn.close(VarInt::from_u32(CLOSE_OK), b"paired");
            tracing::info!(peer = %peer.short_label(), "pairing succeeded (joiner)");
            audit(
                &node,
                "pairing_succeeded",
                Some(&peer),
                serde_json::json!({ "role": "joiner", "addr": addr.to_string() }),
            );
            Ok(rec)
        }
        Err(e) => {
            // Give the initiator a moment to deliver its verdict (failed vs locked) before closing.
            let _ = tokio::time::timeout(Duration::from_secs(1), conn.closed()).await;
            let e = verdict_from_close(&conn, e);
            conn.close(VarInt::from_u32(CLOSE_PAIRING_FAILED), e.code.as_str().as_bytes());
            let peer = tls::peer_id_of(&conn).ok();
            tracing::info!(code = %e.code, "pairing attempt failed (joiner)");
            audit(
                &node,
                "pairing_failed",
                peer.as_ref(),
                serde_json::json!({ "role": "joiner", "addr": addr.to_string(), "reason": e.code.as_str(), "detail": e.message }),
            );
            Err(e)
        }
    }
}

/// Parse `common-pair:v1:<peer hex>:<ip:port>:<secret hex>`; the secret is passed to [`join`] as
/// the code. The caller compares the returned id with the pinned record's id after joining.
pub fn expected_peer_from_qr(payload: &str) -> Result<(PeerId, SocketAddr, String)> {
    let bad = |m: &str| CommonError::invalid(format!("QR payload: {m}"));
    let rest = payload
        .strip_prefix(QR_PREFIX)
        .ok_or_else(|| bad("not a common-pair:v1 payload"))?;
    let (id, rest) = rest.split_once(':').ok_or_else(|| bad("missing address"))?;
    let (addr, secret) = rest.rsplit_once(':').ok_or_else(|| bad("missing secret"))?;
    let id = PeerId::parse_hex(id).map_err(|_| bad("bad peer id"))?;
    let addr: SocketAddr = addr.parse().map_err(|_| bad("bad address"))?;
    if selector_for(secret)? != PW_QR {
        return Err(bad("secret must be 32 hex characters"));
    }
    Ok((id, addr, secret.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_and_secret_have_the_documented_shape() {
        for _ in 0..50 {
            let c = random_code().unwrap();
            assert_eq!(selector_for(&c).unwrap(), PW_CODE, "{c}");
            let s = random_secret().unwrap();
            assert_eq!(selector_for(&s).unwrap(), PW_QR, "{s}");
        }
        assert!(selector_for("12345").is_err());
        assert!(selector_for("1234567").is_err());
        assert!(selector_for("12345a").is_err());
        assert!(selector_for(&"g".repeat(32)).is_err());
        assert!(selector_for("").is_err());
    }

    #[test]
    fn qr_payload_parses_and_rejects_garbage() {
        let id = DeviceIdentity::generate().unwrap().id;
        let secret = random_secret().unwrap();
        for addr in ["192.168.1.20:41234", "[fe80::1]:41234", "127.0.0.1:1"] {
            let payload = format!("common-pair:v1:{}:{addr}:{secret}", id.to_hex());
            let (pid, a, s) = expected_peer_from_qr(&payload).unwrap();
            assert_eq!(pid, id);
            assert_eq!(a, addr.parse::<SocketAddr>().unwrap());
            assert_eq!(s, secret);
        }
        for bad in [
            "",
            "common-pair:v2:x",
            &format!("common-pair:v1:{}:127.0.0.1:1", id.to_hex()),
            &format!("common-pair:v1:{}:127.0.0.1:1:123456", id.to_hex()),
            &format!("common-pair:v1:{}:nope:{secret}", id.to_hex()),
            &format!("common-pair:v1:abcd:127.0.0.1:1:{secret}"),
        ] {
            let e = expected_peer_from_qr(bad).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest, "{bad}");
        }
    }

    #[test]
    fn confirmation_mac_binds_side_exporter_and_ids() {
        let (a, b) = (
            DeviceIdentity::generate().unwrap().id,
            DeviceIdentity::generate().unwrap().id,
        );
        let k = [7u8; 32];
        let ekm = [9u8; 32];
        let tag = |k: &[u8], side: &[u8], ekm: &[u8; 32], x: &PeerId, y: &PeerId| {
            confirm_mac(k, side, ekm, x, y).finalize().into_bytes().to_vec()
        };
        let base = tag(&k, b"A", &ekm, &a, &b);
        assert_eq!(base, tag(&k, b"A", &ekm, &a, &b));
        assert_ne!(base, tag(&k, b"B", &ekm, &a, &b));
        assert_ne!(base, tag(&k, b"A", &[1u8; 32], &a, &b));
        assert_ne!(base, tag(&k, b"A", &ekm, &b, &a));
        assert_ne!(base, tag(&[8u8; 32], b"A", &ekm, &a, &b));
        assert!(confirm_mac(&k, b"A", &ekm, &a, &b).verify_slice(&base).is_ok());
        assert!(confirm_mac(&k, b"A", &ekm, &a, &b).verify_slice(&base[..31]).is_err());
    }

    #[test]
    fn names_are_printable_and_bounded() {
        assert_eq!(clean_name("  Nasr's Mac\n"), "Nasr's Mac");
        assert_eq!(clean_name(&"x".repeat(200)).len(), MAX_NAME_BYTES);
        let wide = clean_name(&"\u{1F600}".repeat(40));
        assert!(wide.len() <= MAX_NAME_BYTES && wide.chars().count() == 16, "{wide}");
        assert_eq!(clean_name("\u{7}\u{1b}[31m"), "[31m");
    }
}
