//! Two in-process nodes with distinct identities and stores. SINGLE-HOST by construction: this is
//! one machine, so nothing here satisfies a two-machine criterion.
#![allow(dead_code)]

use common_core::Paths;
use common_identity::{DeviceIdentity, PeerId, PeerRecord, PeerStore};
use common_network::tls::{self, PeerPolicy};
use common_network::transport::Node;
use common_network::{NetworkConfig, ALPN_WORKLOAD};
use common_observability::Audit;
use common_policy::{PolicyConfig, PolicyEngine};
use std::net::SocketAddr;
use std::sync::Arc;

pub struct Peer {
    pub identity: Arc<DeviceIdentity>,
    pub peers: Arc<PeerStore>,
    pub audit: Arc<Audit>,
    pub policy: Arc<PolicyEngine>,
    pub paths: Paths,
    _dir: tempfile::TempDir,
}

impl Peer {
    pub fn new() -> Peer {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::at(dir.path());
        paths.ensure_dirs().unwrap();
        let identity = Arc::new(DeviceIdentity::generate().unwrap());
        let peers = Arc::new(PeerStore::open(&paths).unwrap());
        let audit = Arc::new(Audit::open(&paths).unwrap());
        let policy = Arc::new(PolicyEngine::new(PolicyConfig::default(), peers.clone()));
        Peer {
            identity,
            peers,
            audit,
            policy,
            paths,
            _dir: dir,
        }
    }
    pub fn id(&self) -> PeerId {
        self.identity.id
    }
    pub fn pin(&self, other: &Peer) {
        self.peers
            .pin(PeerRecord {
                id: other.id(),
                name: "test peer".into(),
                paired_at_utc: "2026-10-14T00:00:00Z".into(),
                last_seen_utc: None,
                last_addr: None,
            })
            .unwrap();
    }
    pub fn config() -> NetworkConfig {
        NetworkConfig {
            enabled: true,
            listen_port: 0,
            bind_addr: "127.0.0.1".into(),
            mdns: false,
            ..NetworkConfig::default()
        }
    }
    pub async fn start(&self) -> Arc<Node> {
        Node::start(
            Self::config(),
            self.identity.clone(),
            self.peers.clone(),
            self.policy.clone(),
            self.audit.clone(),
        )
        .await
        .unwrap()
    }
    /// A raw quinn client speaking as this peer (for hand-written control exchanges).
    pub async fn raw_connect(&self, addr: SocketAddr) -> (quinn::Endpoint, quinn::Connection) {
        let mut ep = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        ep.set_default_client_config(
            tls::client_config(&self.identity, PeerPolicy::Pinned(self.peers.clone()), ALPN_WORKLOAD).unwrap(),
        );
        let conn = ep.connect(addr, "common").unwrap().await.unwrap();
        (ep, conn)
    }
    /// A raw quinn server endpoint speaking as this peer.
    pub fn raw_server(&self) -> quinn::Endpoint {
        let sc = tls::server_config(&self.identity, PeerPolicy::Pinned(self.peers.clone()), ALPN_WORKLOAD).unwrap();
        quinn::Endpoint::server(sc, "127.0.0.1:0".parse().unwrap()).unwrap()
    }
}

/// Two peers pinned to each other.
pub fn pair() -> (Peer, Peer) {
    let (a, b) = (Peer::new(), Peer::new());
    a.pin(&b);
    b.pin(&a);
    (a, b)
}

pub fn audit_kinds(p: &Peer) -> Vec<String> {
    p.audit.tail(50).unwrap().into_iter().map(|e| e.kind).collect()
}
