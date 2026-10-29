//! Two in-process nodes with distinct identities and stores. SINGLE-HOST by construction: this is
//! one machine, so nothing here satisfies a two-machine criterion.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::UdpSocket;
use ucompute_core::Paths;
use ucompute_identity::{DeviceIdentity, PeerId, PeerRecord, PeerStore};
use ucompute_network::tls::{self, PeerPolicy};
use ucompute_network::transport::Node;
use ucompute_network::{NetworkConfig, ALPN_WORKLOAD};
use ucompute_observability::Audit;
use ucompute_policy::{PolicyConfig, PolicyEngine};

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
        let identity = Arc::new(DeviceIdentity::load_or_create(&paths).unwrap());
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
    /// A peer over an existing UCOMPUTE_HOME (child processes reopen the parent's temp dir).
    pub fn open(home: &std::path::Path) -> (Arc<DeviceIdentity>, Arc<PeerStore>, Arc<Audit>, Arc<PolicyEngine>) {
        let paths = Paths::at(home);
        paths.ensure_dirs().unwrap();
        let identity = Arc::new(DeviceIdentity::load_or_create(&paths).unwrap());
        let peers = Arc::new(PeerStore::open(&paths).unwrap());
        let audit = Arc::new(Audit::open(&paths).unwrap());
        let policy = Arc::new(PolicyEngine::new(PolicyConfig::default(), peers.clone()));
        (identity, peers, audit, policy)
    }
    pub fn home(&self) -> &std::path::Path {
        self._dir.path()
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
        let conn = ep.connect(addr, "ucompute").unwrap().await.unwrap();
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

/// Relayed datagrams: (client→server?, payload).
pub type Capture = Arc<Mutex<Vec<(bool, Vec<u8>)>>>;

/// TEST-ONLY UDP relay between one client endpoint and `server`: forwards every datagram, records
/// each payload in `log`, and drops everything in both directions while `blocked` is set (a
/// network partition). No such socket exists in `src/` (tests/invariants.rs).
pub async fn relay(r_in: UdpSocket, r_out: UdpSocket, server: SocketAddr, log: Capture, blocked: Arc<AtomicBool>) {
    let (mut bi, mut bo) = (vec![0u8; 65535], vec![0u8; 65535]);
    let mut client: Option<SocketAddr> = None;
    loop {
        tokio::select! {
            r = r_in.recv_from(&mut bi) => {
                let (n, from) = r.unwrap();
                client = Some(from);
                if blocked.load(Ordering::SeqCst) {
                    continue;
                }
                log.lock().unwrap().push((true, bi[..n].to_vec()));
                let _ = r_out.send_to(&bi[..n], server).await;
            }
            r = r_out.recv_from(&mut bo) => {
                let Ok((n, _)) = r else { continue };
                if blocked.load(Ordering::SeqCst) {
                    continue;
                }
                log.lock().unwrap().push((false, bo[..n].to_vec()));
                if let Some(c) = client {
                    let _ = r_in.send_to(&bo[..n], c).await;
                }
            }
        }
    }
}

/// Starts a relay in front of `server`; returns (address clients dial, the relay's address as the
/// server sees it, the capture, the partition switch, the task).
pub async fn start_relay(
    server: SocketAddr,
) -> (
    SocketAddr,
    SocketAddr,
    Capture,
    Arc<AtomicBool>,
    tokio::task::JoinHandle<()>,
) {
    let r_in = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let r_out = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (front, back) = (r_in.local_addr().unwrap(), r_out.local_addr().unwrap());
    let log: Capture = Arc::default();
    let blocked = Arc::new(AtomicBool::new(false));
    let task = tokio::spawn(relay(r_in, r_out, server, log.clone(), blocked.clone()));
    (front, back, log, blocked, task)
}

/// Polls until `f` holds or `bound` passes; the time it took, or None.
pub async fn poll_until(bound: std::time::Duration, mut f: impl FnMut() -> bool) -> Option<std::time::Duration> {
    let t = std::time::Instant::now();
    while t.elapsed() < bound {
        if f() {
            return Some(t.elapsed());
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    None
}

/// Runs the `#[ignore]`d helper test `name` of the current test binary as a separate process with
/// `UCOMPUTE_NET_TEST_CHILD_HOME=home`; stdout is piped back.
pub fn spawn_child(name: &str, home: &std::path::Path) -> std::process::Child {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("UCOMPUTE_NET_TEST_CHILD_HOME", home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// The child's UCOMPUTE_HOME when this process is a helper child, else None (helpers return at once).
pub fn child_home() -> Option<std::path::PathBuf> {
    std::env::var_os("UCOMPUTE_NET_TEST_CHILD_HOME").map(Into::into)
}

/// The first `<tag> <value>` a child prints (libtest may put `test name ... ` in front of it on the
/// same line), within 20 s; kills the child and panics otherwise.
pub async fn child_value(child: &mut std::process::Child, tag: &'static str) -> String {
    use std::io::BufRead;
    let stdout = child.stdout.take().unwrap();
    let read = tokio::task::spawn_blocking(move || {
        std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
            .find_map(|l| l.split_once(tag).map(|(_, v)| v.trim().to_string()))
    });
    match tokio::time::timeout(std::time::Duration::from_secs(20), read).await {
        Ok(Ok(Some(v))) => v,
        other => {
            let _ = child.kill();
            panic!("child never printed {tag}: {other:?}");
        }
    }
}
