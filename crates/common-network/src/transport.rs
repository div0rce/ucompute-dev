//! The node: one quinn Endpoint for workloads (+ a temporary one for pairing, see `pairing`).
//!
//! Wire sequence after the TLS handshake (ADR-0003 D5): the connector opens the first
//! bidirectional stream (the control stream); both sides exchange `Hello` (major must match,
//! minor = min), then signed `Capabilities` (≤ 64 KiB, signature and `peer_id` checked against
//! the TLS identity). Anything else closes the connection with a typed application code.
use crate::proto::{control_message::Body, read_frame, write_frame, Capabilities, ControlMessage, Hello};
use crate::tls::{self, Check, PeerPolicy, Verdict};
use crate::{NetworkConfig, ALPN_WORKLOAD, MAX_CAPABILITIES_BYTES, MAX_CONTROL_FRAME, PROTOCOL_MAJOR, PROTOCOL_MINOR};
use common_core::{CommonError, ErrorCode, Result};
use common_identity::{DeviceIdentity, PeerId, PeerStore};
use common_observability::Audit;
use common_policy::PolicyEngine;
use prost::Message;
use quinn::{ConnectionError, IdleTimeout, TransportConfig, TransportErrorCode, VarInt};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::mpsc;

/// Application close codes (QUIC CONNECTION_CLOSE, application layer).
pub const CLOSE_OK: u32 = 0;
/// `'RE'`: the peer was revoked (F-06-05).
pub const CLOSE_REVOKED: u32 = 0x5245;
/// `'UA'`: Hello/Capabilities did not match the TLS identity.
pub const CLOSE_UNAUTHORIZED: u32 = 0x5541;
/// `'PV'`: protocol major mismatch or a malformed control exchange.
pub const CLOSE_PROTOCOL: u32 = 0x5056;
/// `'FT'`: a control frame exceeded its bound.
pub const CLOSE_FRAME_TOO_LARGE: u32 = 0x4654;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// The first bidirectional stream (opened by the connector): Hello/Capabilities, then Status and
/// Revocation messages from later modules. Other work opens its own streams on `conn`.
pub struct ControlStream {
    pub send: quinn::SendStream,
    pub recv: quinn::RecvStream,
}

/// An authenticated connection to a pinned peer.
pub struct PeerConnection {
    pub peer_id: PeerId,
    pub remote_addr: SocketAddr,
    pub conn: quinn::Connection,
    /// Received right after Hello; refreshed by Status messages.
    pub capabilities: std::sync::Mutex<Option<Capabilities>>,
    pub negotiated_minor: u32,
    pub control: tokio::sync::Mutex<ControlStream>,
}

impl std::fmt::Debug for PeerConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerConnection")
            .field("peer_id", &self.peer_id)
            .field("remote_addr", &self.remote_addr)
            .field("negotiated_minor", &self.negotiated_minor)
            .finish_non_exhaustive()
    }
}

pub struct Node {
    pub config: NetworkConfig,
    pub identity: Arc<DeviceIdentity>,
    pub peers: Arc<PeerStore>,
    pub policy: Arc<PolicyEngine>,
    pub audit: Arc<Audit>,
    pub endpoint: quinn::Endpoint,
    /// `common hardware --json` taken once at start; sent in every Capabilities record.
    pub hardware_json: String,
    pub free_memory_bytes: u64,
    /// SHA-256 of locally cached models, advertised in Capabilities. Filled by the daemon.
    // ponytail: the node has no model-store handle; the daemon sets this. A Status refresh later.
    pub cached_models: Mutex<Vec<String>>,
    transport: Arc<TransportConfig>,
    conns: Mutex<Vec<Arc<PeerConnection>>>,
    inbound: tokio::sync::Mutex<mpsc::Receiver<Arc<PeerConnection>>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// ADR-0003 D3 transport parameters: keep-alive / idle from config, windows fixed, no datagrams.
pub fn transport_config(cfg: &NetworkConfig) -> Arc<TransportConfig> {
    let mut t = TransportConfig::default();
    t.keep_alive_interval(Some(Duration::from_millis(cfg.keep_alive_ms)));
    let idle = IdleTimeout::try_from(Duration::from_millis(cfg.idle_timeout_ms))
        .unwrap_or_else(|_| IdleTimeout::from(VarInt::from_u32(6000)));
    t.max_idle_timeout(Some(idle));
    t.stream_receive_window(VarInt::from_u32(8 << 20));
    t.receive_window(VarInt::from_u32(64 << 20));
    t.max_concurrent_bidi_streams(VarInt::from_u32(64));
    t.datagram_receive_buffer_size(None);
    Arc::new(t)
}

fn short(s: &str) -> String {
    s.chars().take(32).collect()
}

/// A quinn connection error as a typed `CommonError`. TLS alerts carry the verifier's verdict
/// (`certificate_revoked`, `unknown_ca`, `no_application_protocol`), application close codes carry
/// ours; everything else is `PeerLost`.
pub fn map_conn_error(e: &ConnectionError) -> CommonError {
    match e {
        ConnectionError::ConnectionClosed(c) => tls_error(c.error_code, "closed by peer"),
        ConnectionError::TransportError(t) => tls_error(t.code, "closed locally"),
        ConnectionError::ApplicationClosed(a) => app_error(u64::from(a.error_code), &a.reason),
        ConnectionError::TimedOut => CommonError::new(
            ErrorCode::PeerLost,
            "idle timeout: the peer stopped answering keep-alives",
        ),
        other => CommonError::new(ErrorCode::PeerLost, other.to_string()),
    }
}

fn tls_error(code: TransportErrorCode, side: &str) -> CommonError {
    use rustls::AlertDescription as A;
    let is = |alerts: &[A]| alerts.iter().any(|a| code == TransportErrorCode::crypto((*a).into()));
    let (ec, why) = if is(&[A::CertificateRevoked]) {
        (ErrorCode::PeerRevoked, "the peer's key is revoked")
    } else if is(&[
        A::UnknownCA,
        A::BadCertificate,
        A::CertificateRequired,
        A::CertificateUnknown,
        A::UnsupportedCertificate,
        A::AccessDenied,
    ]) {
        (ErrorCode::PeerUnauthorized, "the peer's key is not pinned")
    } else if is(&[
        A::NoApplicationProtocol,
        A::ProtocolVersion,
        A::HandshakeFailure,
        A::InsufficientSecurity,
    ]) {
        (
            ErrorCode::ProtocolIncompatible,
            "no protocol in common (ALPN or TLS version); update Common on both devices",
        )
    } else {
        (ErrorCode::PeerLost, "transport error")
    };
    CommonError::new(ec, format!("{why} ({side}: {code})"))
}

fn app_error(code: u64, reason: &[u8]) -> CommonError {
    let reason = short(&String::from_utf8_lossy(reason));
    let (ec, why) = match code {
        c if c == u64::from(CLOSE_REVOKED) => (ErrorCode::PeerRevoked, "the peer revoked this device"),
        c if c == u64::from(CLOSE_UNAUTHORIZED) => (ErrorCode::PeerUnauthorized, "the peer refused this device"),
        c if c == u64::from(CLOSE_PROTOCOL) => (ErrorCode::ProtocolIncompatible, "the peer rejected the protocol"),
        c if c == u64::from(CLOSE_FRAME_TOO_LARGE) => {
            (ErrorCode::FrameTooLarge, "the peer rejected a frame as too large")
        }
        _ => (ErrorCode::PeerLost, "closed by peer"),
    };
    CommonError::new(ec, format!("{why} (code {code:#x} {reason})"))
}

fn close_code(e: &CommonError) -> u32 {
    match e.code {
        ErrorCode::PeerRevoked => CLOSE_REVOKED,
        ErrorCode::PeerUnauthorized => CLOSE_UNAUTHORIZED,
        ErrorCode::FrameTooLarge => CLOSE_FRAME_TOO_LARGE,
        _ => CLOSE_PROTOCOL,
    }
}

/// Protocol negotiation (F-08-10): same major or `ProtocolIncompatible` naming both versions;
/// minor = min.
pub fn negotiate(ours: &Hello, theirs: &Hello) -> Result<u32> {
    if theirs.major != ours.major {
        return Err(CommonError::new(
            ErrorCode::ProtocolIncompatible,
            format!(
                "peer speaks protocol {}.{} (Common {}), this device speaks {}.{} (Common {}); update Common on the older device",
                theirs.major,
                theirs.minor,
                short(&theirs.common_version),
                ours.major,
                ours.minor,
                ours.common_version
            ),
        ));
    }
    Ok(ours.minor.min(theirs.minor))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `"<name> <release> <platform>"` of the pinned engine for this platform, e.g.
/// `llama.cpp b11609 macos-arm64` (catalog/backends.json). Empty when none is pinned here.
pub fn engines() -> Vec<String> {
    common_artifacts::backend::BackendPins::builtin()
        .ok()
        .and_then(|p| {
            p.for_current_platform("llama.cpp")
                .map(|b| format!("{} {} {}", b.name, b.release, b.platform))
        })
        .into_iter()
        .collect()
}

/// Build this device's signed Capabilities record from `common hardware --json` and the store.
pub fn local_capabilities(
    identity: &DeviceIdentity,
    groups: &[String],
    cached_models: &[String],
) -> Result<Capabilities> {
    let hw = common_hardware::probe()?;
    sign_capabilities(
        identity,
        serde_json::to_string(&hw)?,
        hw.memory.available_bytes,
        groups,
        cached_models,
    )
}

/// Sign a Capabilities record: Ed25519 over the protobuf encoding with `signature` empty.
pub fn sign_capabilities(
    identity: &DeviceIdentity,
    hardware_json: String,
    free_memory_bytes: u64,
    groups: &[String],
    cached_models: &[String],
) -> Result<Capabilities> {
    let mut c = Capabilities {
        schema_version: 1,
        peer_id: identity.id.0.to_vec(),
        common_version: common_core::VERSION.into(),
        hardware_json,
        free_memory_bytes,
        engines: engines(),
        cached_model_sha256: cached_models.to_vec(),
        groups: groups.to_vec(),
        issued_at: now_unix(),
        signature: vec![],
    };
    c.signature = identity.sign(&c.encode_to_vec()).to_vec();
    let len = c.encoded_len();
    if len > MAX_CAPABILITIES_BYTES {
        return Err(CommonError::new(
            ErrorCode::FrameTooLarge,
            format!("own capabilities record is {len} bytes, bound is {MAX_CAPABILITIES_BYTES}"),
        ));
    }
    Ok(c)
}

/// `peer_id` must be the TLS identity and the signature must verify under it.
pub fn verify_capabilities(c: &Capabilities, peer: &PeerId) -> Result<()> {
    if c.peer_id != peer.0 {
        return Err(CommonError::new(
            ErrorCode::PeerUnauthorized,
            "capabilities peer_id does not match the TLS identity",
        ));
    }
    let mut unsigned = c.clone();
    unsigned.signature.clear();
    if !common_identity::verify(peer, &unsigned.encode_to_vec(), &c.signature) {
        return Err(CommonError::new(
            ErrorCode::PeerUnauthorized,
            "capabilities signature does not verify",
        ));
    }
    Ok(())
}

impl Node {
    /// Bind the workload endpoint (refuses when `config.enabled == false`), start accepting.
    pub async fn start(
        config: NetworkConfig,
        identity: Arc<DeviceIdentity>,
        peers: Arc<PeerStore>,
        policy: Arc<PolicyEngine>,
        audit: Arc<Audit>,
    ) -> Result<Arc<Node>> {
        if !config.enabled {
            return Err(CommonError::new(ErrorCode::PolicyDenied, "network disabled")
                .with_hint("set [network] enabled = true to listen for paired devices"));
        }
        let addr: SocketAddr = format!("{}:{}", config.bind_addr, config.listen_port)
            .parse()
            .map_err(|e| CommonError::invalid(format!("network bind address: {e}")))?;
        let transport = transport_config(&config);
        let mut sc = tls::server_config(&identity, PeerPolicy::Pinned(peers.clone()), ALPN_WORKLOAD)?;
        sc.transport_config(transport.clone());
        let endpoint = quinn::Endpoint::server(sc, addr)?;
        let hw = common_hardware::probe()?;
        let (tx, rx) = mpsc::channel(16);
        let node = Arc::new(Node {
            config,
            identity,
            peers,
            policy,
            audit,
            endpoint: endpoint.clone(),
            hardware_json: serde_json::to_string(&hw)?,
            free_memory_bytes: hw.memory.available_bytes,
            cached_models: Mutex::new(vec![]),
            transport,
            conns: Mutex::new(vec![]),
            inbound: tokio::sync::Mutex::new(rx),
        });
        tracing::info!(addr = %endpoint.local_addr()?, "peer endpoint listening");
        // The loop holds only a Weak: dropping the last Node closes the endpoint (Drop) and ends it.
        let weak = Arc::downgrade(&node);
        tokio::spawn(async move {
            while let Some(incoming) = endpoint.accept().await {
                let Some(node) = weak.upgrade() else { break };
                let tx = tx.clone();
                tokio::spawn(async move { node.serve_incoming(incoming, tx).await });
            }
        });
        Ok(node)
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.endpoint.local_addr()?)
    }

    fn audit(&self, kind: &str, peer: Option<&PeerId>, fields: serde_json::Value) {
        let hex = peer.map(|p| p.to_hex());
        if let Err(e) = self.audit.record(kind, hex.as_deref(), fields) {
            tracing::debug!(kind, error = %e, "audit record not written");
        }
    }

    fn hello(&self) -> Hello {
        let mut nonce = [0u8; 16];
        let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce);
        Hello {
            major: PROTOCOL_MAJOR,
            minor: PROTOCOL_MINOR,
            common_version: common_core::VERSION.into(),
            peer_id: self.identity.id.0.to_vec(),
            nonce: nonce.to_vec(),
        }
    }

    fn capabilities(&self) -> Result<Capabilities> {
        // ponytail: every local group counts as "mine"; membership semantics belong to F-02-09.
        let groups: Vec<String> = self
            .peers
            .groups()
            .unwrap_or_default()
            .into_iter()
            .map(|g| g.name)
            .collect();
        let cached = lock(&self.cached_models).clone();
        sign_capabilities(
            &self.identity,
            self.hardware_json.clone(),
            self.free_memory_bytes,
            &groups,
            &cached,
        )
    }

    async fn control_exchange(
        &self,
        conn: &quinn::Connection,
        peer: PeerId,
        connector: bool,
    ) -> Result<(ControlStream, Capabilities, u32)> {
        let lost = |m: &str| CommonError::new(ErrorCode::PeerLost, m);
        let (mut send, mut recv) = if connector {
            conn.open_bi().await
        } else {
            conn.accept_bi().await
        }
        .map_err(|e| map_conn_error(&e))?;
        let ours = self.hello();
        if connector {
            write_frame(&mut send, &Body::Hello(ours.clone()).into()).await?;
        }
        let theirs = match read_frame(&mut recv, MAX_CONTROL_FRAME).await? {
            Some(ControlMessage {
                body: Some(Body::Hello(h)),
            }) => h,
            Some(_) => {
                return Err(CommonError::new(
                    ErrorCode::ProtocolIncompatible,
                    "expected Hello first",
                ))
            }
            None => return Err(lost("peer closed the control stream before Hello")),
        };
        if theirs.peer_id != peer.0 {
            return Err(CommonError::new(
                ErrorCode::PeerUnauthorized,
                "Hello peer_id does not match the TLS identity",
            ));
        }
        let minor = negotiate(&ours, &theirs)?;
        if !connector {
            write_frame(&mut send, &Body::Hello(ours).into()).await?;
        }
        let mine = self.capabilities()?;
        if connector {
            write_frame(&mut send, &Body::Capabilities(mine.clone()).into()).await?;
        }
        let caps = match read_frame(&mut recv, MAX_CAPABILITIES_BYTES).await? {
            Some(ControlMessage {
                body: Some(Body::Capabilities(c)),
            }) => c,
            Some(_) => {
                return Err(CommonError::new(
                    ErrorCode::ProtocolIncompatible,
                    "expected Capabilities after Hello",
                ))
            }
            None => return Err(lost("peer closed the control stream before Capabilities")),
        };
        verify_capabilities(&caps, &peer)?;
        if !connector {
            write_frame(&mut send, &Body::Capabilities(mine).into()).await?;
        }
        Ok((ControlStream { send, recv }, caps, minor))
    }

    /// The control exchange with a deadline; on failure the connection is closed with the typed
    /// code, and a close received from the peer explains the failure instead of our read error.
    async fn finish_handshake(
        &self,
        conn: &quinn::Connection,
        peer: PeerId,
        connector: bool,
    ) -> Result<(ControlStream, Capabilities, u32)> {
        let r = tokio::time::timeout(HANDSHAKE_TIMEOUT, self.control_exchange(conn, peer, connector))
            .await
            .unwrap_or_else(|_| Err(CommonError::new(ErrorCode::Timeout, "control handshake timed out")));
        r.map_err(|e| {
            let e = match conn.close_reason() {
                Some(reason) => map_conn_error(&reason),
                None => e,
            };
            conn.close(VarInt::from_u32(close_code(&e)), e.code.as_str().as_bytes());
            e
        })
    }

    fn register(
        &self,
        conn: quinn::Connection,
        peer: PeerId,
        control: ControlStream,
        caps: Capabilities,
        minor: u32,
    ) -> Arc<PeerConnection> {
        let pc = Arc::new(PeerConnection {
            peer_id: peer,
            remote_addr: conn.remote_address(),
            conn,
            capabilities: Mutex::new(Some(caps)),
            negotiated_minor: minor,
            control: tokio::sync::Mutex::new(control),
        });
        lock(&self.conns).push(pc.clone());
        pc
    }

    async fn serve_incoming(self: Arc<Self>, incoming: quinn::Incoming, tx: mpsc::Sender<Arc<PeerConnection>>) {
        let addr = incoming.remote_address().to_string();
        let rejected = |peer: Option<&PeerId>, e: CommonError| {
            tracing::info!(addr, code = %e.code, "peer rejected");
            self.audit(
                "peer_rejected",
                peer,
                serde_json::json!({ "addr": addr, "reason": e.code.as_str(), "detail": e.message }),
            );
        };
        let connecting = match incoming.accept() {
            Ok(c) => c,
            Err(e) => return rejected(None, map_conn_error(&e)),
        };
        let conn = match connecting.await {
            Ok(c) => c,
            Err(e) => return rejected(None, map_conn_error(&e)),
        };
        let peer = match tls::peer_id_of(&conn) {
            Ok(p) => p,
            Err(e) => {
                conn.close(VarInt::from_u32(CLOSE_UNAUTHORIZED), b"no raw public key");
                return rejected(None, e);
            }
        };
        match self.finish_handshake(&conn, peer, false).await {
            Ok((control, caps, minor)) => {
                let pc = self.register(conn, peer, control, caps, minor);
                tracing::info!(peer = %peer.short_label(), addr, minor, "peer connected (inbound)");
                self.audit(
                    "peer_connected",
                    Some(&peer),
                    serde_json::json!({ "addr": addr, "direction": "inbound", "minor": minor }),
                );
                let _ = tx.send(pc).await;
            }
            Err(e) => rejected(Some(&peer), e),
        }
    }

    /// Connect to a pinned peer; Hello/Capabilities exchanged; typed errors: PeerUnauthorized,
    /// PeerRevoked, ProtocolIncompatible, PeerLost.
    pub async fn connect(&self, addr: SocketAddr, expected: PeerId) -> Result<Arc<PeerConnection>> {
        match tls::store_verdict(&self.peers, &expected) {
            Verdict::Revoked => {
                return Err(CommonError::new(
                    ErrorCode::PeerRevoked,
                    format!("peer {} is revoked", expected.short_label()),
                ))
            }
            Verdict::Unpinned => {
                return Err(CommonError::new(
                    ErrorCode::PeerUnauthorized,
                    format!("peer {} is not paired", expected.short_label()),
                ))
            }
            Verdict::Allowed => {}
        }
        // Exactly the expected key, still pinned and not revoked, checked inside the handshake.
        let store = self.peers.clone();
        let check: Check = Arc::new(move |id| {
            if *id == expected {
                tls::store_verdict(&store, id)
            } else {
                Verdict::Unpinned
            }
        });
        let mut cc = tls::client_config_with(self.identity.pkcs8_der(), check, ALPN_WORKLOAD)?;
        cc.transport_config(self.transport.clone());
        let fields = |reason: &CommonError| serde_json::json!({ "addr": addr.to_string(), "direction": "outbound", "reason": reason.code.as_str() });
        let connecting = self
            .endpoint
            .connect_with(cc, addr, "common")
            .map_err(|e| CommonError::new(ErrorCode::PeerLost, format!("connect: {e}")))?;
        let conn = match connecting.await {
            Ok(c) => c,
            Err(e) => {
                let e = map_conn_error(&e);
                self.audit("peer_rejected", Some(&expected), fields(&e));
                return Err(e);
            }
        };
        let peer = tls::peer_id_of(&conn)?;
        let (control, caps, minor) = match self.finish_handshake(&conn, peer, true).await {
            Ok(v) => v,
            Err(e) => {
                self.audit("peer_rejected", Some(&peer), fields(&e));
                return Err(e);
            }
        };
        let pc = self.register(conn, peer, control, caps, minor);
        tracing::info!(peer = %peer.short_label(), %addr, minor, "peer connected (outbound)");
        self.audit(
            "peer_connected",
            Some(&peer),
            serde_json::json!({ "addr": addr.to_string(), "direction": "outbound", "minor": minor }),
        );
        Ok(pc)
    }

    /// Next inbound authenticated connection (after Hello/Capabilities). None when the endpoint closed.
    pub async fn accept(&self) -> Option<Arc<PeerConnection>> {
        self.inbound.lock().await.recv().await
    }

    /// Currently connected peers (closed connections are dropped from the registry here).
    pub fn connections(&self) -> Vec<Arc<PeerConnection>> {
        let mut g = lock(&self.conns);
        g.retain(|c| c.conn.close_reason().is_none());
        g.clone()
    }

    /// Close every connection to `id` with the REVOKED close code (F-06-05, ≤ 5 s).
    pub async fn disconnect(&self, id: &PeerId, reason: &str) {
        let victims: Vec<Arc<PeerConnection>> = {
            let mut g = lock(&self.conns);
            let (gone, keep): (Vec<_>, Vec<_>) = g.drain(..).partition(|c| c.peer_id == *id);
            *g = keep;
            gone
        };
        for c in &victims {
            c.conn.close(VarInt::from_u32(CLOSE_REVOKED), reason.as_bytes());
        }
        if !victims.is_empty() {
            self.audit(
                "peer_disconnected",
                Some(id),
                serde_json::json!({ "reason": reason, "connections": victims.len() }),
            );
        }
    }

    pub async fn shutdown(&self) {
        let conns: Vec<Arc<PeerConnection>> = lock(&self.conns).drain(..).collect();
        for c in conns {
            c.conn.close(VarInt::from_u32(CLOSE_OK), b"shutdown");
        }
        self.endpoint.close(VarInt::from_u32(CLOSE_OK), b"shutdown");
        self.endpoint.wait_idle().await;
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.endpoint.close(VarInt::from_u32(CLOSE_OK), b"node dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::AlertDescription as A;

    fn hello(major: u32, minor: u32) -> Hello {
        Hello {
            major,
            minor,
            common_version: "0.1.0".into(),
            ..Default::default()
        }
    }

    #[test]
    fn negotiate_major_mismatch_is_typed_and_minor_is_min() {
        let e = negotiate(&hello(1, 0), &hello(2, 0)).unwrap_err();
        assert_eq!(e.code, ErrorCode::ProtocolIncompatible);
        assert!(
            e.message.contains("2.0") && e.message.contains("1.0") && e.message.contains("update"),
            "{e}"
        );
        assert_eq!(negotiate(&hello(1, 3), &hello(1, 7)).unwrap(), 3);
        assert_eq!(negotiate(&hello(1, 7), &hello(1, 3)).unwrap(), 3);
        let mut long = hello(3, 0);
        long.common_version = "v".repeat(10_000);
        assert!(negotiate(&hello(1, 0), &long).unwrap_err().message.len() < 300);
    }

    #[test]
    fn connection_errors_map_to_stable_codes() {
        let closed = |a: A| {
            ConnectionError::ConnectionClosed(quinn::ConnectionClose {
                error_code: TransportErrorCode::crypto(a.into()),
                frame_type: None,
                reason: Default::default(),
            })
        };
        assert_eq!(
            map_conn_error(&closed(A::CertificateRevoked)).code,
            ErrorCode::PeerRevoked
        );
        assert_eq!(map_conn_error(&closed(A::UnknownCA)).code, ErrorCode::PeerUnauthorized);
        assert_eq!(
            map_conn_error(&closed(A::NoApplicationProtocol)).code,
            ErrorCode::ProtocolIncompatible
        );
        assert_eq!(map_conn_error(&closed(A::InternalError)).code, ErrorCode::PeerLost);
        assert_eq!(map_conn_error(&ConnectionError::TimedOut).code, ErrorCode::PeerLost);
        let app = |code: u32| {
            ConnectionError::ApplicationClosed(quinn::ApplicationClose {
                error_code: VarInt::from_u32(code),
                reason: b"x".as_slice().into(),
            })
        };
        assert_eq!(map_conn_error(&app(CLOSE_REVOKED)).code, ErrorCode::PeerRevoked);
        assert_eq!(
            map_conn_error(&app(CLOSE_UNAUTHORIZED)).code,
            ErrorCode::PeerUnauthorized
        );
        assert_eq!(
            map_conn_error(&app(CLOSE_PROTOCOL)).code,
            ErrorCode::ProtocolIncompatible
        );
        assert_eq!(
            map_conn_error(&app(CLOSE_FRAME_TOO_LARGE)).code,
            ErrorCode::FrameTooLarge
        );
        assert_eq!(map_conn_error(&app(0)).code, ErrorCode::PeerLost);
        // Codes round-trip through close_code.
        for (ec, code) in [
            (ErrorCode::PeerRevoked, CLOSE_REVOKED),
            (ErrorCode::PeerUnauthorized, CLOSE_UNAUTHORIZED),
            (ErrorCode::FrameTooLarge, CLOSE_FRAME_TOO_LARGE),
            (ErrorCode::ProtocolIncompatible, CLOSE_PROTOCOL),
        ] {
            assert_eq!(close_code(&CommonError::new(ec, "")), code);
            assert_eq!(map_conn_error(&app(code)).code, ec);
        }
    }

    #[test]
    fn engines_names_the_pinned_build() {
        let e = engines();
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            assert_eq!(e, vec!["llama.cpp b11609 macos-arm64".to_string()]);
        } else {
            assert!(e.len() <= 1);
        }
    }

    #[test]
    fn transport_config_builds_from_config() {
        let cfg = NetworkConfig {
            keep_alive_ms: 2000,
            idle_timeout_ms: 6000,
            ..NetworkConfig::default()
        };
        let _ = transport_config(&cfg);
        let _ = transport_config(&NetworkConfig {
            idle_timeout_ms: u64::MAX,
            ..cfg
        });
    }
}
