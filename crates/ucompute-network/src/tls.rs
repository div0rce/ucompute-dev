//! rustls configs with RFC 7250 raw public keys (ADR-0003 D3).
//!
//! Server: TLS 1.3 only, `AlwaysResolvesServerRawPublicKeys`, client auth mandatory via
//! `PinnedPeerVerifier`. Client: `AlwaysResolvesClientRawPublicKeys`, server verified by the same
//! verifier. The verifier parses the peer's SubjectPublicKeyInfo into a `PeerId` and decides
//! **inside the handshake**: an unpinned key fails with `unknown_ca`, a revoked key with
//! `certificate_revoked`, so the rejected side never opens a stream. The pairing endpoint uses
//! `PeerPolicy::AcceptAny` (records the peer id; no trust). No X.509, no CA, no TLS 1.2 (every
//! config pins TLS 1.3, see `tls12_downgrade_attempt_fails`), no session resumption (a resumed
//! handshake would skip the verifier).
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls13_signature_with_raw_key, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{CertificateError, DigitallySignedStruct, DistinguishedName, SignatureScheme};
use std::fmt;
use std::sync::Arc;
use ucompute_core::{ErrorCode, Result, UComputeError};
use ucompute_identity::{DeviceIdentity, PeerId, PeerStore};

/// Which keys a handshake may complete with.
pub enum PeerPolicy {
    /// Workload endpoint: pinned and not revoked, checked inside the handshake.
    Pinned(Arc<PeerStore>),
    /// Pairing endpoint: any Ed25519 key (the pairing protocol then authenticates with the code).
    AcceptAny,
}

/// What the verifier decided about a key. Only `Allowed` completes the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allowed,
    Unpinned,
    Revoked,
}

/// The trust decision, as a closure so `connect` can pin to one expected key and tests can use
/// in-memory sets while `ucompute-identity` lands.
pub(crate) type Check = Arc<dyn Fn(&PeerId) -> Verdict + Send + Sync>;

impl PeerPolicy {
    pub(crate) fn into_check(self) -> Check {
        match self {
            PeerPolicy::AcceptAny => Arc::new(|_| Verdict::Allowed),
            PeerPolicy::Pinned(store) => Arc::new(move |id| store_verdict(&store, id)),
        }
    }
}

pub(crate) fn store_verdict(store: &PeerStore, id: &PeerId) -> Verdict {
    if store.is_revoked(id) {
        Verdict::Revoked
    } else if store.is_pinned(id) {
        Verdict::Allowed
    } else {
        Verdict::Unpinned
    }
}

/// DER prefix of an Ed25519 SubjectPublicKeyInfo (RFC 8410 §4): `SEQUENCE { SEQUENCE { OID
/// 1.3.101.112 }, BIT STRING (0 unused bits) }` followed by the 32 key bytes. Ed25519 SPKI has
/// no parameters, so the encoding is fixed and an exact prefix match is a complete parser.
const ED25519_SPKI_PREFIX: [u8; 12] = [0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00];

/// The 32-byte key inside an Ed25519 SPKI; `None` for any other algorithm or encoding.
pub fn peer_id_from_spki(spki: &[u8]) -> Option<PeerId> {
    let (prefix, key) = spki.split_at_checked(ED25519_SPKI_PREFIX.len())?;
    if prefix != ED25519_SPKI_PREFIX {
        return None;
    }
    Some(PeerId(key.try_into().ok()?))
}

/// Inverse of [`peer_id_from_spki`]: the SPKI DER a peer with this id presents.
pub fn spki_from_peer_id(id: &PeerId) -> Vec<u8> {
    let mut v = ED25519_SPKI_PREFIX.to_vec();
    v.extend_from_slice(&id.0);
    v
}

struct PinnedPeerVerifier {
    check: Check,
    provider: Arc<CryptoProvider>,
}

impl fmt::Debug for PinnedPeerVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PinnedPeerVerifier")
    }
}

impl PinnedPeerVerifier {
    fn accept(&self, end_entity: &CertificateDer<'_>) -> std::result::Result<(), rustls::Error> {
        let id = peer_id_from_spki(end_entity.as_ref()).ok_or(CertificateError::BadEncoding)?;
        match (self.check)(&id) {
            Verdict::Allowed => Ok(()),
            Verdict::Unpinned => Err(CertificateError::UnknownIssuer.into()),
            Verdict::Revoked => Err(CertificateError::Revoked.into()),
        }
    }
    fn verify13(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(cert.as_ref()),
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
}

fn no_tls12() -> rustls::Error {
    rustls::Error::PeerIncompatible(rustls::PeerIncompatible::Tls12NotOffered)
}

impl ClientCertVerifier for PinnedPeerVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        self.accept(end_entity).map(|_| ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(no_tls12())
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.verify13(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

impl ServerCertVerifier for PinnedPeerVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        self.accept(end_entity).map(|_| ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Err(no_tls12())
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        self.verify13(message, cert, dss)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }
    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

fn tls_err(e: impl fmt::Display) -> UComputeError {
    UComputeError::internal(format!("tls config: {e}"))
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// The device key as rustls sees it: the Ed25519 signing key plus its SPKI in the certificate slot.
fn certified_key(pkcs8: &[u8]) -> Result<Arc<rustls::sign::CertifiedKey>> {
    let key = rustls::crypto::ring::sign::any_eddsa_type(&PrivatePkcs8KeyDer::from(pkcs8)).map_err(tls_err)?;
    let spki = key
        .public_key()
        .ok_or_else(|| UComputeError::internal("tls config: signing key has no public key"))?;
    let spki = CertificateDer::from(spki.as_ref().to_vec());
    Ok(Arc::new(rustls::sign::CertifiedKey::new(vec![spki], key)))
}

pub(crate) fn server_config_with(pkcs8: &[u8], check: Check, alpn: &[u8]) -> Result<quinn::ServerConfig> {
    let provider = provider();
    let verifier = Arc::new(PinnedPeerVerifier {
        check,
        provider: provider.clone(),
    });
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_err)?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(Arc::new(rustls::server::AlwaysResolvesServerRawPublicKeys::new(
            certified_key(pkcs8)?,
        )));
    cfg.alpn_protocols = vec![alpn.to_vec()];
    // No 0-RTT: QUIC allows only 0 or u32::MAX here and 0 is the no-early-data value.
    cfg.max_early_data_size = 0;
    // No resumption: a resumed TLS 1.3 handshake restores the client key from the session and
    // never calls `verify_client_cert`, so a revoked or unpinned device could reconnect with a
    // cached ticket. No tickets, no session cache (`revoked_client_cannot_resume_with_cached_session`).
    cfg.session_storage = Arc::new(rustls::server::NoServerSessionStorage {});
    cfg.send_tls13_tickets = 0;
    cfg.max_tls13_tickets = 0;
    let quic = QuicServerConfig::try_from(cfg).map_err(tls_err)?;
    let mut sc = quinn::ServerConfig::with_crypto(Arc::new(quic));
    sc.transport_config(crate::transport::transport_config(&crate::NetworkConfig::default()));
    Ok(sc)
}

pub(crate) fn client_config_with(pkcs8: &[u8], check: Check, alpn: &[u8]) -> Result<quinn::ClientConfig> {
    let provider = provider();
    let verifier = Arc::new(PinnedPeerVerifier {
        check,
        provider: provider.clone(),
    });
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(tls_err)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_cert_resolver(Arc::new(rustls::client::AlwaysResolvesClientRawPublicKeys::new(
            certified_key(pkcs8)?,
        )));
    cfg.alpn_protocols = vec![alpn.to_vec()];
    cfg.enable_early_data = false;
    // Every handshake runs the verifier on the server key too (see `server_config_with`).
    cfg.resumption = rustls::client::Resumption::disabled();
    let quic = QuicClientConfig::try_from(cfg).map_err(tls_err)?;
    let mut cc = quinn::ClientConfig::new(Arc::new(quic));
    cc.transport_config(crate::transport::transport_config(&crate::NetworkConfig::default()));
    Ok(cc)
}

pub fn server_config(identity: &DeviceIdentity, policy: PeerPolicy, alpn: &[u8]) -> Result<quinn::ServerConfig> {
    server_config_with(identity.pkcs8_der(), policy.into_check(), alpn)
}

pub fn client_config(identity: &DeviceIdentity, policy: PeerPolicy, alpn: &[u8]) -> Result<quinn::ClientConfig> {
    client_config_with(identity.pkcs8_der(), policy.into_check(), alpn)
}

/// The authenticated peer id of a connection (from its raw public key).
pub fn peer_id_of(conn: &quinn::Connection) -> Result<PeerId> {
    let unauthorized = |m: &str| UComputeError::new(ErrorCode::PeerUnauthorized, m);
    let certs = conn
        .peer_identity()
        .and_then(|id| id.downcast::<Vec<CertificateDer<'static>>>().ok())
        .ok_or_else(|| unauthorized("connection has no peer identity"))?;
    certs
        .first()
        .and_then(|c| peer_id_from_spki(c.as_ref()))
        .ok_or_else(|| unauthorized("peer presented no Ed25519 raw public key"))
}

#[cfg(test)]
mod tests {
    //! The spike (ADR-0003 D3): quinn 0.11 + rustls 0.23 raw-public-key mutual TLS 1.3 between two
    //! endpoints on 127.0.0.1. Keys come from `ring` directly so this evidence does not depend on
    //! `ucompute-identity`. SINGLE-HOST by construction.
    use super::*;
    use crate::transport::map_conn_error;
    use crate::{ALPN_PAIRING, ALPN_WORKLOAD};
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::net::SocketAddr;
    use std::time::Duration;

    struct Key {
        pkcs8: Vec<u8>,
        id: PeerId,
    }
    fn key() -> Key {
        let doc = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        let pair = Ed25519KeyPair::from_pkcs8(doc.as_ref()).unwrap();
        Key {
            pkcs8: doc.as_ref().to_vec(),
            id: PeerId(pair.public_key().as_ref().try_into().unwrap()),
        }
    }
    fn only(allowed: PeerId, verdict_otherwise: Verdict) -> Check {
        Arc::new(move |id| {
            if *id == allowed {
                Verdict::Allowed
            } else {
                verdict_otherwise
            }
        })
    }
    fn loopback() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }
    fn server(k: &Key, check: Check) -> quinn::Endpoint {
        quinn::Endpoint::server(server_config_with(&k.pkcs8, check, ALPN_WORKLOAD).unwrap(), loopback()).unwrap()
    }
    fn client(k: &Key, check: Check, alpn: &[u8]) -> quinn::Endpoint {
        let mut ep = quinn::Endpoint::client(loopback()).unwrap();
        ep.set_default_client_config(client_config_with(&k.pkcs8, check, alpn).unwrap());
        ep
    }

    #[test]
    fn spki_round_trip_and_rejects_other_algorithms() {
        let k = key();
        let spki = spki_from_peer_id(&k.id);
        assert_eq!(spki.len(), 44);
        assert_eq!(peer_id_from_spki(&spki), Some(k.id));
        assert_eq!(peer_id_from_spki(&spki[..43]), None);
        let mut p256 = spki.clone();
        p256[6] = 0x2a; // a different OID
        assert_eq!(peer_id_from_spki(&p256), None);
        // rustls derives the same SPKI from the private key.
        let ck = certified_key(&k.pkcs8).unwrap();
        assert_eq!(ck.cert[0].as_ref(), spki.as_slice());
    }

    #[tokio::test]
    async fn handshake_pinned_keys_succeeds_single_host() {
        let (s, c) = (key(), key());
        let server = server(&s, only(c.id, Verdict::Unpinned));
        let client = client(&c, only(s.id, Verdict::Unpinned), ALPN_WORKLOAD);
        let addr = server.local_addr().unwrap();
        let accept = tokio::spawn(async move {
            let conn = server.accept().await.unwrap().accept().unwrap().await.unwrap();
            let id = peer_id_of(&conn).unwrap();
            let (mut send, mut recv) = conn.accept_bi().await.unwrap();
            let data = recv.read_to_end(64).await.unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut send, &data).await.unwrap();
            send.finish().unwrap();
            let _ = send.stopped().await;
            id
        });
        let conn = client.connect(addr, "ucompute").unwrap().await.unwrap();
        assert_eq!(peer_id_of(&conn).unwrap(), s.id);
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut send, b"ping").await.unwrap();
        send.finish().unwrap();
        assert_eq!(recv.read_to_end(64).await.unwrap(), b"ping");
        assert_eq!(accept.await.unwrap(), c.id);
    }

    /// Runs one handshake that must fail; returns the client's raw error. The server side must
    /// never get a connection (no stream ever opens).
    async fn rejected(
        s: &Key,
        c: &Key,
        server_check: Check,
        client_check: Check,
        alpn: &[u8],
    ) -> quinn::ConnectionError {
        rejected_at(ALPN_WORKLOAD, s, c, server_check, client_check, alpn).await
    }

    /// [`rejected`] against a server that accepts only `server_alpn`.
    async fn rejected_at(
        server_alpn: &[u8],
        s: &Key,
        c: &Key,
        server_check: Check,
        client_check: Check,
        alpn: &[u8],
    ) -> quinn::ConnectionError {
        let server = quinn::Endpoint::server(
            server_config_with(&s.pkcs8, server_check, server_alpn).unwrap(),
            loopback(),
        )
        .unwrap();
        let client = client(c, client_check, alpn);
        let addr = server.local_addr().unwrap();
        let server_side = tokio::spawn(async move {
            match tokio::time::timeout(Duration::from_secs(5), server.accept()).await {
                Ok(Some(i)) => match i.accept() {
                    Ok(connecting) => connecting.await.map(|_| ()),
                    Err(e) => Err(e),
                },
                _ => Ok(()),
            }
        });
        // TLS 1.3: the client finishes first, so a server-side verdict arrives as an immediate
        // close right after `Connecting` resolves; a client-side verdict fails `Connecting` itself.
        let err = match client.connect(addr, "ucompute").unwrap().await {
            Ok(conn) => tokio::time::timeout(Duration::from_secs(5), conn.closed())
                .await
                .expect("server must close the connection"),
            Err(e) => e,
        };
        assert!(
            server_side.await.unwrap().is_err(),
            "server must not complete the handshake"
        );
        err
    }

    /// Closed by the peer (its verifier decided), as opposed to our own verifier.
    fn by_peer(e: &quinn::ConnectionError) -> bool {
        matches!(e, quinn::ConnectionError::ConnectionClosed(_))
    }

    #[tokio::test]
    async fn unpinned_key_rejected() {
        let (s, c, stranger) = (key(), key(), key());
        let e = rejected(
            &s,
            &c,
            only(stranger.id, Verdict::Unpinned),
            only(s.id, Verdict::Unpinned),
            ALPN_WORKLOAD,
        )
        .await;
        assert!(by_peer(&e), "{e:?}");
        assert_eq!(map_conn_error(&e).code, ErrorCode::PeerUnauthorized, "{e:?}");
    }

    #[tokio::test]
    async fn revoked_key_rejected() {
        let (s, c) = (key(), key());
        let revoked = c.id;
        let server_check: Check = Arc::new(move |id| {
            if *id == revoked {
                Verdict::Revoked
            } else {
                Verdict::Unpinned
            }
        });
        let e = rejected(&s, &c, server_check, only(s.id, Verdict::Unpinned), ALPN_WORKLOAD).await;
        assert!(by_peer(&e), "{e:?}");
        assert_eq!(map_conn_error(&e).code, ErrorCode::PeerRevoked, "{e:?}");
    }

    /// A client that keeps its rustls session cache (one `ClientConfig` reused) must not resume
    /// its way past a revocation: TLS 1.3 resumption restores the client key from the ticket and
    /// skips `verify_client_cert`, so resumption is off on both sides.
    #[tokio::test]
    async fn revoked_client_cannot_resume_with_cached_session() {
        let (s, c) = (key(), key());
        let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (flag, cid) = (revoked.clone(), c.id);
        let server_check: Check =
            Arc::new(
                move |id| match (*id == cid, flag.load(std::sync::atomic::Ordering::SeqCst)) {
                    (true, false) => Verdict::Allowed,
                    (true, true) => Verdict::Revoked,
                    _ => Verdict::Unpinned,
                },
            );
        let server = server(&s, server_check);
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            while let Some(i) = server.accept().await {
                tokio::spawn(async move {
                    let conn = match i.accept() {
                        Ok(connecting) => connecting.await,
                        Err(e) => Err(e),
                    };
                    if let Ok(conn) = conn {
                        let _ = conn.closed().await;
                    }
                });
            }
        });
        let client = client(&c, only(s.id, Verdict::Unpinned), ALPN_WORKLOAD);
        let first = client.connect(addr, "ucompute").unwrap().await.unwrap();
        // Give the server time to deliver any post-handshake session tickets.
        tokio::time::sleep(Duration::from_millis(300)).await;
        first.close(0u32.into(), b"");
        revoked.store(true, std::sync::atomic::Ordering::SeqCst);
        let second = match client.connect(addr, "ucompute").unwrap().await {
            Ok(conn) => tokio::time::timeout(Duration::from_secs(5), conn.closed())
                .await
                .expect("a revoked client must be closed by the server"),
            Err(e) => e,
        };
        assert_eq!(map_conn_error(&second).code, ErrorCode::PeerRevoked, "{second:?}");
    }

    #[tokio::test]
    async fn server_key_not_pinned_by_client_rejected() {
        // The client's verifier rejects the server: fails locally, still inside the handshake.
        let (s, c) = (key(), key());
        let any: Check = Arc::new(|_| Verdict::Allowed);
        let none: Check = Arc::new(|_| Verdict::Unpinned);
        let e = rejected(&s, &c, any, none, ALPN_WORKLOAD).await;
        assert!(!by_peer(&e), "{e:?}");
        assert_eq!(map_conn_error(&e).code, ErrorCode::PeerUnauthorized, "{e:?}");
    }

    /// Nothing but the current ids is accepted. A peer offering only another id (an older version, a
    /// foreign `other/1` or `other-pair/1`) or another endpoint's id is refused inside the handshake.
    #[tokio::test]
    async fn wrong_alpn_fails() {
        for (server_alpn, offered) in [
            (ALPN_WORKLOAD, &b"ucompute/0"[..]),
            (ALPN_WORKLOAD, b"other/1"),
            (ALPN_WORKLOAD, ALPN_PAIRING),
            (ALPN_PAIRING, b"other-pair/1"),
            (ALPN_PAIRING, b"ucompute-pair/0"),
            (ALPN_PAIRING, ALPN_WORKLOAD),
        ] {
            let (s, c) = (key(), key());
            let check = (only(c.id, Verdict::Unpinned), only(s.id, Verdict::Unpinned));
            let e = rejected_at(server_alpn, &s, &c, check.0, check.1, offered).await;
            assert_eq!(
                map_conn_error(&e).code,
                ErrorCode::ProtocolIncompatible,
                "{offered:?}: {e:?}"
            );
        }
    }

    #[test]
    fn tls12_downgrade_attempt_fails() {
        // rustls's `tls12` feature is unified into this build by reqwest, so TLS 1.2 suites exist in
        // the provider. Every config this module builds pins TLS 1.3; a TLS 1.2-only config is
        // refused by rustls's QUIC entry point (the call quinn makes before any packet is sent).
        let (s, c) = (key(), key());
        let provider = provider();
        let verifier = Arc::new(PinnedPeerVerifier {
            check: only(s.id, Verdict::Unpinned),
            provider: provider.clone(),
        });
        let Ok(builder) =
            rustls::ClientConfig::builder_with_provider(provider).with_protocol_versions(&[&rustls::version::TLS12])
        else {
            return; // no TLS 1.2 in this build at all
        };
        let mut cfg = builder
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_client_cert_resolver(Arc::new(rustls::client::AlwaysResolvesClientRawPublicKeys::new(
                certified_key(&c.pkcs8).unwrap(),
            )));
        cfg.alpn_protocols = vec![ALPN_WORKLOAD.to_vec()];
        let name = ServerName::try_from("ucompute").unwrap();
        let r = rustls::quic::ClientConnection::new(Arc::new(cfg), rustls::quic::Version::V1, name, vec![]);
        assert!(r.is_err(), "a TLS 1.2-only config must not start a QUIC session");
    }
}
