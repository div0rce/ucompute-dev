//! WP-04 C5: the pooled-inference carriage between two in-process nodes on loopback QUIC.
//! **SINGLE-HOST**: both nodes and the stage's rpc-server run on this one machine; nothing here
//! satisfies a two-machine criterion (F-03-01, F-03-02, Gate D). The stage is the REAL pinned
//! `ggml-rpc-server` (b11609) from COMMON_HOME (skips loudly when absent; `COMMON_REQUIRE_FIXTURES=1`
//! fails instead). The head is SYNTHETIC: the test process speaks b11609's RPC framing to the
//! bridge itself (`| cmd u8 | size u64 LE | payload |`), so no model is loaded and no inference is
//! claimed. The R role runs off Linux only under `COMMON_TEST_SINGLE_HOST_POOL=1`, which these tests
//! set, and only because the peer is on loopback.
#![cfg(unix)]
mod common;

use common::{pair, poll_until, start_relay, Peer};
use common_artifacts::Store;
use common_core::{ErrorCode, Paths};
use common_identity::{Group, GroupPolicy};
use common_network::proto::{control_message::Body, encode_frame, RpcTunnelOpen};
use common_network::remote::{serve_peer_with, ServeOptions};
use common_network::rpc::{Limits, SplitSession, SplitSpec, Stage, MAX_TUNNELS, RPC_RESET, TEST_POOL_ENV};
use common_network::transport::{Node, PeerConnection};
use common_policy::{PolicyConfig, PolicyEngine, PrivacyMode};
use common_runtime::manager::ModelManager;
use common_runtime::EnginePid;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

fn common_home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

/// The pinned engine directory, or None after printing why (`COMMON_REQUIRE_FIXTURES=1` fails).
fn engine_dir() -> Option<PathBuf> {
    let dir = Paths::at(common_home()).backends().join("llama.cpp").join("b11609");
    if common_artifacts::backend::binary(&dir, "ggml-rpc-server").is_file() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "COMMON_REQUIRE_FIXTURES=1: pinned ggml-rpc-server missing in {}",
        dir.display()
    );
    eprintln!("SKIP: pinned ggml-rpc-server missing in {}", dir.display());
    None
}

fn open_policy(p: &Peer, cfg: PolicyConfig) -> Arc<PolicyEngine> {
    Arc::new(PolicyEngine::new(cfg, p.peers.clone()))
}

fn serving() -> PolicyConfig {
    PolicyConfig {
        network_enabled: true,
        serve_peers: true,
        ..PolicyConfig::default()
    }
}

/// `who` shares one group with `other`, allowing pooling iff `pooled` (remote inference always).
fn group(who: &Peer, other: &Peer, pooled: bool) {
    who.peers
        .upsert_group(Group {
            name: "pool".into(),
            members: vec![other.id()],
            policy: GroupPolicy {
                allow_remote_inference: true,
                allow_pooled_inference: pooled,
            },
        })
        .unwrap();
}

/// How a pool test is set up: each side's policy config and group opt-in.
struct Setup {
    h_cfg: PolicyConfig,
    r_cfg: PolicyConfig,
    h_pooled: bool,
    r_pooled: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            h_cfg: serving(),
            r_cfg: serving(),
            h_pooled: true,
            r_pooled: true,
        }
    }
}

fn fast() -> Limits {
    Limits {
        ttl: Duration::from_secs(120),
        idle: Duration::from_secs(30),
        first_idle: Duration::from_secs(30),
        t_rpc: Duration::from_secs(10),
        tick: Duration::from_millis(200),
    }
}

/// H, R, their nodes, H's connection to R, and R's stage serving it.
struct Pool {
    h: Peer,
    r: Peer,
    nh: Arc<Node>,
    nr: Arc<Node>,
    conn: Arc<PeerConnection>,
    stage: Arc<Stage>,
    _logs: tempfile::TempDir,
}

fn manager(paths: &Paths, engine: PathBuf) -> Arc<ModelManager> {
    let backend = common_runtime::backend_by_name("llama.cpp", engine, paths.clone()).unwrap();
    Arc::new(ModelManager::new(
        paths.clone(),
        Store::new(paths.clone()),
        backend,
        common_hardware::probe().unwrap(),
    ))
}

async fn pool_with(setup: Setup, tweak: impl FnOnce(&mut Stage)) -> Option<Pool> {
    let engine = engine_dir()?;
    std::env::set_var(TEST_POOL_ENV, "1");
    let (h, r) = pair();
    group(&h, &r, setup.h_pooled);
    group(&r, &h, setup.r_pooled);
    let start = |p: &Peer, cfg: PolicyConfig| {
        Node::start(
            Peer::config(),
            p.identity.clone(),
            p.peers.clone(),
            open_policy(p, cfg),
            p.audit.clone(),
        )
    };
    let nh = start(&h, setup.h_cfg).await.unwrap();
    let r_policy = open_policy(&r, setup.r_cfg.clone());
    let nr = start(&r, setup.r_cfg).await.unwrap();
    let logs = tempfile::tempdir().unwrap();
    let mut stage = Stage::new(engine.clone(), logs.path().to_path_buf(), "b11609".into(), 2);
    stage.limits = fast();
    // the stage's memory check against a fixed figure, not this Mac's free memory (8 GB, shared with
    // other work); `rpc_session_refused_when_memory_short_single_host` tests the check itself
    stage.available_memory = || Some(8 << 30);
    tweak(&mut stage);
    let stage = Arc::new(stage);
    let conn = nh.connect(nr.local_addr().unwrap(), r.id()).await.unwrap();
    let inn = timeout(Duration::from_secs(5), nr.accept()).await.unwrap().unwrap();
    let opts = ServeOptions {
        rpc: Some(stage.clone()),
        ..ServeOptions::standalone()
    };
    tokio::spawn(serve_peer_with(
        inn,
        manager(&r.paths, engine),
        r_policy,
        r.audit.clone(),
        opts,
    ));
    Some(Pool {
        h,
        r,
        nh,
        nr,
        conn,
        stage,
        _logs: logs,
    })
}

async fn pool() -> Option<Pool> {
    pool_with(Setup::default(), |_| {}).await
}

/// The synthetic head's own engine pid: this test process.
fn this_process() -> EnginePid {
    let e = EnginePid::default();
    e.set(std::process::id());
    e
}

fn spec() -> SplitSpec {
    SplitSpec {
        model_sha256: "ab".repeat(32),
        engine_release: "b11609".into(),
        layer_start: 0,
        layer_end: 3,
        n_layer: 6,
        est_bytes: 64 << 20,
        privacy: PrivacyMode::TrustedGroup.into(),
        engine: this_process(),
        byte_cap: None,
        limits: fast(),
    }
}

impl Pool {
    async fn open(&self, spec: SplitSpec) -> common_core::Result<SplitSession> {
        SplitSession::open(&self.nh, self.conn.clone(), spec).await
    }

    fn rpc_server_logs(&self) -> usize {
        std::fs::read_dir(self._logs.path()).map_or(0, |d| d.count())
    }

    fn r_kinds(&self) -> Vec<String> {
        common::audit_kinds(&self.r)
    }

    /// R's audit lines of `kind`, oldest first.
    fn r_lines(&self, kind: &str) -> Vec<serde_json::Value> {
        lines(&self.r, kind)
    }
}

fn lines(p: &Peer, kind: &str) -> Vec<serde_json::Value> {
    p.audit
        .tail(200)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind)
        .map(|e| serde_json::to_value(e).unwrap())
        .collect()
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

async fn wait_dead(pid: u32, bound: Duration) -> Option<Duration> {
    poll_until(bound, || !alive(pid)).await
}

// ---- the SYNTHETIC head: b11609 framing (`| cmd u8 | size u64 LE | payload |`; replies `| size u64 | data |`)

const HELLO: u8 = 14;
const ALLOC_BUFFER: u8 = 0;
const SET_TENSOR: u8 = 6;
const GET_TENSOR: u8 = 8;

fn cmd(c: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![c];
    v.extend((payload.len() as u64).to_le_bytes());
    v.extend_from_slice(payload);
    v
}

/// HELLO of protocol v8: 33 bytes up, 36 down.
async fn hello(c: &mut TcpStream) {
    c.write_all(&cmd(HELLO, &[0; 24])).await.unwrap();
    let mut rsp = [0u8; 8 + 28];
    timeout(Duration::from_secs(10), c.read_exact(&mut rsp))
        .await
        .expect("HELLO answered")
        .unwrap();
    assert_eq!((rsp[0], rsp[8]), (28, 8), "HELLO reply of protocol v8");
}

/// A small ALLOC_BUFFER on device 0: 21 bytes up, 24 down; the granted remote pointer.
async fn alloc(c: &mut TcpStream, size: u64) -> u64 {
    let mut p = 0u32.to_le_bytes().to_vec();
    p.extend(size.to_le_bytes());
    c.write_all(&cmd(ALLOC_BUFFER, &p)).await.unwrap();
    let mut rsp = [0u8; 8 + 16];
    timeout(Duration::from_secs(10), c.read_exact(&mut rsp))
        .await
        .expect("ALLOC_BUFFER answered")
        .unwrap();
    u64::from_le_bytes(rsp[8..16].try_into().unwrap())
}

async fn head(s: &SplitSession) -> TcpStream {
    TcpStream::connect(("127.0.0.1", s.rpc_port())).await.unwrap()
}

/// The connection gives EOF or an error (reset) within `bound`.
async fn closed_within(c: &mut TcpStream, bound: Duration) -> bool {
    let mut b = [0u8; 64];
    matches!(timeout(bound, c.read(&mut b)).await, Ok(Ok(0)) | Ok(Err(_)))
}

async fn failure_within(s: &SplitSession, bound: Duration) -> common_core::CommonError {
    timeout(bound, s.ended()).await.expect("the session ended in time")
}

// ---------------------------------------------------------------- admission

/// A group that allows only remote inference does not allow pooling: refused on R, nothing spawned
/// (ADR-0005 D5, default deny). H refuses first when its own group lacks the opt-in.
#[tokio::test]
async fn rpc_session_refused_without_pooled_policy_single_host() {
    let Some(p) = pool_with(
        Setup {
            r_pooled: false,
            ..Setup::default()
        },
        |_| {},
    )
    .await
    else {
        return;
    };
    let e = p.open(spec()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "{e}");
    assert!(e.message.contains("no trusted group allows pooled inference"), "{e}");
    assert_eq!(p.rpc_server_logs(), 0, "no rpc-server was started");
    assert!(p.stage.session().is_none());
    let rejected = p.r_lines("peer_rejected");
    assert_eq!(rejected[0]["fields"]["reason"], "peer_unauthorized", "{rejected:?}");
    assert!(!p.r_kinds().contains(&"session_open".to_string()));
}

/// The matrix of ADR-0005 D5: pooling needs `allow_pooled_inference` on both ends, `serve_peers`
/// and networking on R, networking and no hold or revocation on H. Each refusal starts nothing.
#[tokio::test]
async fn pooled_needs_policy_on_both_ends_single_host() {
    let Some(_) = engine_dir() else { return };
    let off = PolicyConfig {
        network_enabled: false,
        ..serving()
    };
    let no_serve = PolicyConfig {
        serve_peers: false,
        ..serving()
    };
    let cases: [(&str, Setup, ErrorCode, bool); 5] = [
        (
            "H without the group opt-in",
            Setup {
                h_pooled: false,
                ..Setup::default()
            },
            ErrorCode::PeerUnauthorized,
            false,
        ),
        (
            "R without the group opt-in",
            Setup {
                r_pooled: false,
                ..Setup::default()
            },
            ErrorCode::PeerUnauthorized,
            true,
        ),
        (
            "R does not serve peers",
            Setup {
                r_cfg: no_serve,
                ..Setup::default()
            },
            ErrorCode::PolicyDenied,
            true,
        ),
        (
            "R networking off",
            Setup {
                r_cfg: off.clone(),
                ..Setup::default()
            },
            ErrorCode::PolicyDenied,
            true,
        ),
        (
            "H networking off",
            Setup {
                h_cfg: off,
                ..Setup::default()
            },
            ErrorCode::PolicyDenied,
            false,
        ),
    ];
    for (what, setup, code, reaches_r) in cases {
        let p = pool_with(setup, |_| {}).await.unwrap();
        let e = p.open(spec()).await.unwrap_err();
        assert_eq!(e.code, code, "{what}: {e}");
        assert_eq!(p.rpc_server_logs(), 0, "{what}: nothing spawned");
        let r_saw = p.r_kinds().contains(&"peer_rejected".to_string());
        assert_eq!(
            r_saw,
            reaches_r,
            "{what}: refused on {}",
            if reaches_r { "R" } else { "H" }
        );
    }
    // H holds R, or revoked it: refused on H before any stream
    let p = pool().await.unwrap();
    p.nh.hold(&p.r.id());
    let e = p.open(spec()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::PolicyDenied, "held: {e}");
    assert!(e.message.contains("disconnected by you"), "{e}");
    let p = pool().await.unwrap();
    p.h.peers.revoke(&p.r.id(), &p.h.identity).unwrap();
    assert_eq!(p.open(spec()).await.unwrap_err().code, ErrorCode::PeerRevoked);
    assert_eq!(p.rpc_server_logs(), 0);
    // control: both ends opted in
    let p = pool().await.unwrap();
    let s = p.open(spec()).await.unwrap();
    assert!(s.accepted().device.contains('('), "{:?}", s.accepted());
    assert_eq!(p.rpc_server_logs(), 1);
}

#[tokio::test]
async fn rpc_session_refused_for_engine_release_mismatch_single_host() {
    let Some(p) = pool().await else { return };
    let e = p
        .open(SplitSpec {
            engine_release: "b11610".into(),
            ..spec()
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::ProtocolIncompatible, "{e}");
    assert!(e.message.contains("b11609") && e.message.contains("b11610"), "{e}");
    assert_eq!(p.rpc_server_logs(), 0);
}

/// `est_bytes × 1.1 ≤ available − 2 GiB` at admission (D6).
#[tokio::test]
async fn rpc_session_refused_when_memory_short_single_host() {
    let Some(p) = pool_with(Setup::default(), |s| {
        s.available_memory = || Some((2 << 30) + (100 << 20));
    })
    .await
    else {
        return;
    };
    let e = p
        .open(SplitSpec {
            est_bytes: 100 << 20,
            ..spec()
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, ErrorCode::InsufficientMemory, "{e}");
    assert!(
        e.message.contains("the stage needs 110 MiB"),
        "R's own words reach H: {e}"
    );
    assert_eq!(p.rpc_server_logs(), 0);
    // control: 90 MiB × 1.1 fits in the 100 MiB left
    let s = p
        .open(SplitSpec {
            est_bytes: 90 << 20,
            ..spec()
        })
        .await
        .unwrap();
    assert_eq!(s.accepted().granted_bytes, 90 << 20);
}

/// One session per R; the slot frees when the session ends and R's rpc-server is gone.
#[tokio::test]
async fn second_session_refused_single_host() {
    let Some(p) = pool().await else { return };
    let first = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let e = p.open(spec()).await.unwrap_err();
    assert_eq!(e.code, ErrorCode::Overloaded, "{e}");
    assert_eq!(p.rpc_server_logs(), 1, "the refused session started nothing");
    drop(first);
    assert!(
        wait_dead(pid, Duration::from_secs(6)).await.is_some(),
        "rpc-server stopped"
    );
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();
    let again = p.open(spec()).await.unwrap();
    assert_ne!(p.stage.session().unwrap().0, pid);
    drop(again);
}

// ---------------------------------------------------------------- tunnels

/// A tunnel stream naming a session R does not have on this connection is reset without any
/// connection to rpc-server: a random id from H, and H's real id from a third device.
#[tokio::test]
async fn tunnel_for_unknown_or_foreign_session_refused_single_host() {
    let Some(p) = pool().await else { return };
    let s = p.open(spec()).await.unwrap();
    let raw = |conn: quinn::Connection, id: Vec<u8>| async move {
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let frame = encode_frame(&Body::RpcTunnelOpen(RpcTunnelOpen { session_id: id }).into());
        send.write_all(&frame).await.unwrap();
        let _ = send.write_all(&cmd(HELLO, &[0; 24])).await;
        let mut b = [0u8; 8];
        timeout(Duration::from_secs(5), recv.read(&mut b)).await.unwrap()
    };
    let r = raw(p.conn.conn.clone(), vec![7; 16]).await;
    assert!(
        matches!(r, Err(quinn::ReadError::Reset(c)) if c.into_inner() == u64::from(RPC_RESET)),
        "{r:?}"
    );
    // a third device, allowed to pool with R, presents H's session id
    let c = Peer::new();
    c.pin(&p.r);
    p.r.pin(&c);
    group(&c, &p.r, true);
    p.r.peers
        .upsert_group(Group {
            name: "pool".into(),
            members: vec![p.h.id(), c.id()],
            policy: GroupPolicy {
                allow_remote_inference: true,
                allow_pooled_inference: true,
            },
        })
        .unwrap();
    let nc = c.start().await;
    let cc = nc.connect(p.nr.local_addr().unwrap(), p.r.id()).await.unwrap();
    let inn = timeout(Duration::from_secs(5), p.nr.accept()).await.unwrap().unwrap();
    let opts = ServeOptions {
        rpc: Some(p.stage.clone()),
        ..ServeOptions::standalone()
    };
    let policy = open_policy(&p.r, serving());
    tokio::spawn(serve_peer_with(
        inn,
        manager(&p.r.paths, engine_dir().unwrap()),
        policy,
        p.r.audit.clone(),
        opts,
    ));
    let id = hex::decode(s.session_id()).unwrap();
    assert!(matches!(
        raw(cc.conn.clone(), id).await,
        Err(quinn::ReadError::Reset(_))
    ));
    let closes = p.r_lines("rpc_tunnel_close");
    assert_eq!(closes.len(), 2, "{closes:?}");
    assert!(closes.iter().all(|l| l["fields"]["outcome"] == "unknown_session"));
    assert!(p.r_lines("rpc_tunnel_open").is_empty(), "no connection to rpc-server");
    assert!(s.failure().is_none(), "H's session is untouched");
    // a tunnel frame over MAX_RPC_FRAME is refused unread as a frame, not looked up
    let (mut send, mut recv) = p.conn.conn.open_bi().await.unwrap();
    let big = encode_frame(
        &Body::RpcTunnelOpen(RpcTunnelOpen {
            session_id: vec![7; 2048],
        })
        .into(),
    );
    send.write_all(&big).await.unwrap();
    let answer = timeout(
        Duration::from_secs(5),
        common_network::proto::read_frame(&mut recv, 1 << 20),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert!(
        matches!(&answer.body, Some(Body::Error(e)) if e.code == "frame_too_large"),
        "{answer:?}"
    );
    assert_eq!(p.r_lines("rpc_tunnel_close").len(), 2, "never looked up");
}

/// At most 8 tunnels at once (D6): H's bridge resets a ninth engine connection, and R resets a
/// ninth tunnel stream from a head that bypasses its bridge.
#[tokio::test]
async fn tunnel_cap_is_8_single_host() {
    let Some(p) = pool().await else { return };
    let s = p.open(spec()).await.unwrap();
    let mut held = Vec::new();
    for _ in 0..MAX_TUNNELS {
        held.push(head(&s).await);
    }
    poll_until(Duration::from_secs(10), || {
        p.r_lines("rpc_tunnel_open").len() == MAX_TUNNELS
    })
    .await
    .unwrap_or_else(|| {
        panic!(
            "8 tunnels carried: {:?} {:?}",
            p.r_lines("rpc_tunnel_close"),
            p.r_lines("session_close")
        )
    });
    let mut ninth = head(&s).await;
    assert!(closed_within(&mut ninth, Duration::from_secs(5)).await);
    let refused = lines(&p.h, "rpc_bridge_refused");
    assert_eq!(refused[0]["fields"]["reason"], "tunnel_cap", "{refused:?}");
    // rpc-server serves one of them, the rest wait in its backlog; all close in turn
    drop(held);
    poll_until(Duration::from_secs(10), || {
        p.r_lines("rpc_tunnel_close").len() == MAX_TUNNELS
    })
    .await
    .expect("the 8 tunnels closed");

    // R's own cap, against raw tunnel streams on the session's connection
    let id = hex::decode(s.session_id()).unwrap();
    let mut raw = Vec::new();
    for _ in 0..=MAX_TUNNELS {
        let (mut send, recv) = p.conn.conn.open_bi().await.unwrap();
        let frame = encode_frame(&Body::RpcTunnelOpen(RpcTunnelOpen { session_id: id.clone() }).into());
        send.write_all(&frame).await.unwrap();
        raw.push((send, recv));
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    poll_until(Duration::from_secs(10), || {
        p.r_lines("rpc_tunnel_close")
            .iter()
            .any(|l| l["fields"]["outcome"] == "tunnel_cap")
    })
    .await
    .expect("R refused the ninth stream");
    assert_eq!(
        p.r_lines("rpc_tunnel_open").len(),
        2 * MAX_TUNNELS,
        "8 more carried, not 9"
    );
}

/// The bridge carries only connections of its engine's pid: a local process that is not the engine
/// is reset and audited, and R never sees a tunnel for it (D4).
#[tokio::test]
async fn bridge_refuses_a_local_process_that_is_not_our_engine_single_host() {
    let Some(p) = pool().await else { return };
    let Some(nc) = ["/usr/bin/nc", "/bin/nc"]
        .into_iter()
        .find(|n| std::path::Path::new(n).exists())
    else {
        panic!("no nc for a foreign client process");
    };
    let s = p.open(spec()).await.unwrap();
    let mut other = std::process::Command::new(nc)
        .args(["127.0.0.1", &s.rpc_port().to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let refused = poll_until(Duration::from_secs(10), || {
        !lines(&p.h, "rpc_bridge_refused").is_empty()
    })
    .await;
    let _ = other.kill();
    let _ = other.wait();
    assert!(refused.is_some(), "nc's connection was refused");
    assert_eq!(
        lines(&p.h, "rpc_bridge_refused")[0]["fields"]["reason"],
        "not_the_engine"
    );
    assert!(p.r_lines("rpc_tunnel_open").is_empty(), "R saw no tunnel for it");
    // control: this process is the engine, and its connection is carried
    let mut c = head(&s).await;
    hello(&mut c).await;
    // an engine pid that was never set carries nothing
    let unset = p.open(spec()).await;
    assert!(unset.is_err(), "one session per R");
    drop(c);
    drop(s);
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();
    let s = p
        .open(SplitSpec {
            engine: EnginePid::default(),
            ..spec()
        })
        .await
        .unwrap();
    let mut c = head(&s).await;
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
}

/// SYNTHETIC head on the REAL rpc-server: the counters are the plaintext bytes at the pump, both
/// ways, and switches per tunnel (lab SCORING-INPUT.md): HELLO and ALLOC_BUFFER each go up and come
/// back, so 3 switches, and R's audit line carries the same bytes.
#[tokio::test]
async fn tunnel_bytes_and_switches_counted_both_directions_synthetic_head_single_host() {
    let Some(p) = pool().await else { return };
    let before = common_network::rpc::tunnel_bytes_total();
    let s = p.open(spec()).await.unwrap();
    assert_eq!(s.counters().to_r, 0);
    let mut c = head(&s).await;
    hello(&mut c).await;
    assert_ne!(alloc(&mut c, 4096).await, 0, "rpc-server granted the buffer");
    let c2 = s.counters();
    assert_eq!((c2.to_r, c2.to_h, c2.switches), (33 + 21, 36 + 24, 3), "{c2:?}");
    drop(c);
    poll_until(Duration::from_secs(10), || p.r_lines("rpc_tunnel_close").len() == 1)
        .await
        .expect("R closed its tunnel");
    let close = &p.r_lines("rpc_tunnel_close")[0]["fields"];
    assert_eq!(
        (close["to_r"].as_u64(), close["to_h"].as_u64()),
        (Some(54), Some(60)),
        "{close}"
    );
    assert_eq!(close["outcome"], "closed");
    let after = common_network::rpc::tunnel_bytes_total();
    // both roles pump in this one process: H's 54 and R's 54 head→stage bytes
    assert!(after.0 - before.0 >= 2 * 54 && after.1 - before.1 >= 2 * 60);
    assert!(s.failure().is_none());
}

/// The byte cap counts per load (ADR-0005 D6 and the lab's C2 review): a new engine start gets the
/// whole cap again, and bytes over it in one load reset the session.
#[tokio::test]
async fn session_byte_cap_resets_tunnels_per_load_single_host() {
    let Some(p) = pool().await else { return };
    let engine = this_process();
    let s = p
        .open(SplitSpec {
            byte_cap: Some(40),
            engine: engine.clone(),
            ..spec()
        })
        .await
        .unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await; // 33 of 40
    drop(c);
    engine.set(std::process::id()); // the engine started again: a second load
    let mut c = head(&s).await;
    hello(&mut c).await; // 33 of 40 in this load, 66 in the session
    assert_eq!(s.counters().to_r, 66);
    assert!(s.failure().is_none(), "a retry's upload does not trip the cap");
    c.write_all(&cmd(ALLOC_BUFFER, &[0; 12])).await.unwrap(); // 54 of 40
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::Overloaded, "{e}");
    assert!(e.message.contains("over the 40-byte cap"), "{e}");
    assert!(
        closed_within(&mut c, Duration::from_secs(5)).await,
        "the engine's connection is reset"
    );
    assert!(
        wait_dead(pid, Duration::from_secs(6)).await.is_some(),
        "R stopped rpc-server"
    );
    let close = &lines(&p.h, "session_close")[0]["fields"];
    assert_eq!(close["reason"], "overloaded", "{close}");
}

/// TTL and idle close (D6), on both sides: an idle session closes, and one at its TTL closes with
/// its tunnel open.
#[tokio::test]
async fn session_ttl_and_idle_close_single_host() {
    let Some(p) = pool().await else { return };
    let idle = Limits {
        idle: Duration::from_secs(1),
        first_idle: Duration::from_secs(1),
        ..fast()
    };
    let t = Instant::now();
    let s = p.open(SplitSpec { limits: idle, ..spec() }).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::Timeout);
    assert!(e.message.contains("no connection for 1 s"), "{e}");
    assert!(t.elapsed() >= Duration::from_secs(1));
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
    drop(s);
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();

    let ttl = Limits {
        ttl: Duration::from_secs(2),
        ..fast()
    };
    let t = Instant::now();
    let s = p.open(SplitSpec { limits: ttl, ..spec() }).await.unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    let e = failure_within(&s, Duration::from_secs(6)).await;
    assert_eq!(e.code, ErrorCode::Timeout, "{e}");
    assert!(e.message.contains("reached its limit of 2 s"), "{e}");
    assert!(t.elapsed() >= Duration::from_secs(2));
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
}

/// `T_rpc`: a stage that stops answering while a readback waits is reset; an idle tunnel (nothing
/// waiting) is not. The hang is real: rpc-server is SIGSTOPped.
#[tokio::test]
async fn silence_watchdog_resets_hung_stage_single_host() {
    let Some(p) = pool().await else { return };
    let limits = Limits {
        t_rpc: Duration::from_secs(1),
        ..fast()
    };
    let s = p.open(SplitSpec { limits, ..spec() }).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(s.failure().is_none(), "nothing waits on R: the watchdog is not armed");
    // SAFETY: plain signal to our own child.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) };
    let t = Instant::now();
    c.write_all(&cmd(GET_TENSOR, &[0; 40])).await.unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    // SAFETY: as above.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGCONT) };
    assert_eq!(e.code, ErrorCode::Timeout, "{e}");
    assert!(e.message.contains("while a pooled step waited"), "{e}");
    assert!(t.elapsed() >= Duration::from_secs(1) && t.elapsed() < Duration::from_secs(3));
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
}

// ---------------------------------------------------------------- mid-session policy

/// Turning the group's pooling off on either device ends the session within 5 s (the 1 s tick; D5),
/// with that side's reason, and R's rpc-server stops.
#[tokio::test]
async fn group_policy_off_mid_session_tears_down_within_5s_single_host() {
    let Some(p) = pool().await else { return };
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    group(&p.r, &p.h, false);
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "R's reason reaches H: {e}");
    assert!(e.message.contains("pooled inference"), "{e}");
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
    assert!(wait_dead(pid, Duration::from_secs(5)).await.is_some());

    group(&p.r, &p.h, true);
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    group(&p.h, &p.r, false);
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PeerUnauthorized, "H's own reason: {e}");
    assert!(wait_dead(pid, Duration::from_secs(5)).await.is_some());
}

/// A hold or revocation mid-session on either side ends it with the reason this side knows.
#[tokio::test]
async fn revoke_and_hold_mid_session_tear_down_single_host() {
    let Some(_) = engine_dir() else { return };
    // H holds R
    let p = pool().await.unwrap();
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    p.nh.hold(&p.r.id());
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("disconnected by you"), "{e}");
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
    // R revokes H
    let p = pool().await.unwrap();
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    p.r.peers.revoke(&p.h.id(), &p.r.identity).unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PeerRevoked, "{e}");
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
    // R holds H: R closes the connection, H reads the hold
    let p = pool().await.unwrap();
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    p.nr.hold(&p.h.id());
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PeerHeld, "{e}");
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
}

/// A foreign client on the stage's rpc-server port ends the session (D3), seen on R's tick.
#[tokio::test]
async fn foreign_client_on_the_stage_port_tears_down_single_host() {
    let Some(p) = pool().await else { return };
    let s = p.open(spec()).await.unwrap();
    let (pid, port) = p.stage.session().unwrap();
    let nc = ["/usr/bin/nc", "/bin/nc"]
        .into_iter()
        .find(|n| std::path::Path::new(n).exists())
        .expect("nc");
    let mut other = std::process::Command::new(nc)
        .args(["127.0.0.1", &port.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("not this device's bridge"), "{e}");
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
}

/// R checks the stage port's clients at each tunnel open too, not only on its tick: with the tick
/// out of the way (60 s), a tunnel opened while a foreign client holds rpc-server is refused before
/// R dials it, and the session ends.
#[tokio::test]
async fn foreign_client_refuses_the_next_tunnel_before_the_tick_single_host() {
    let Some(p) = pool_with(Setup::default(), |s| s.limits.tick = Duration::from_secs(60)).await else {
        return;
    };
    let s = p.open(spec()).await.unwrap();
    let (_, port) = p.stage.session().unwrap();
    // past R's first tick, which an interval runs at once
    tokio::time::sleep(Duration::from_millis(500)).await;
    let nc = ["/usr/bin/nc", "/bin/nc"]
        .into_iter()
        .find(|n| std::path::Path::new(n).exists())
        .expect("nc");
    let mut other = std::process::Command::new(nc)
        .args(["127.0.0.1", &port.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(s.failure().is_none(), "{:?}", s.failure());
    let mut c = head(&s).await;
    c.write_all(&cmd(HELLO, &[0; 24])).await.unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("not this device's bridge"), "{e}");
    let close = stage_close(&p, "foreign_client").await;
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(close["fields"]["to_r"], 0, "{close}");
}

/// A local IPv4 address other than 127.0.0.1 that can dial 127.0.0.1: 127.0.0.2 on Linux (all of
/// 127/8 is local), else this host's LAN address (a UDP `connect` sends nothing). None offline.
fn other_local_addr() -> Option<String> {
    if cfg!(target_os = "linux") {
        return Some("127.0.0.2".into());
    }
    let u = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    u.connect("192.0.2.1:9").ok()?;
    let ip = u.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

/// `nc` from `source` (address, optional port) to 127.0.0.1:`port`, another process.
fn nc_from(source: &str, source_port: Option<u16>, port: u16) -> std::process::Child {
    let nc = ["/usr/bin/nc", "/bin/nc"]
        .into_iter()
        .find(|n| std::path::Path::new(n).exists())
        .expect("nc");
    let mut c = std::process::Command::new(nc);
    c.args(["-s", source]);
    if let Some(p) = source_port {
        c.args(["-p", &p.to_string()]);
    }
    c.args(["127.0.0.1", &port.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// A client of the stage port bound to another local address (127.0.0.2, or the LAN address) is
/// foreign too: R must not tell clients apart by their source address (review NX-1).
#[tokio::test]
async fn foreign_client_from_another_local_address_tears_down_single_host() {
    let Some(p) = pool().await else { return };
    let Some(src) = other_local_addr() else {
        return eprintln!("SKIP: no local address other than 127.0.0.1");
    };
    let s = p.open(spec()).await.unwrap();
    let (pid, port) = p.stage.session().unwrap();
    let mut other = nc_from(&src, None, port);
    let e = failure_within(&s, Duration::from_secs(5)).await;
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("not this device's bridge"), "{e}");
    assert!(wait_dead(pid, Duration::from_secs(6)).await.is_some());
}

/// Another process connecting to the bridge from another local address with the engine
/// connection's own port number is not the engine: refused, and R sees no tunnel for it (NX-2).
#[tokio::test]
async fn bridge_refuses_the_engines_client_port_from_another_address_single_host() {
    let Some(p) = pool().await else { return };
    let Some(src) = other_local_addr() else {
        return eprintln!("SKIP: no local address other than 127.0.0.1");
    };
    let s = p.open(spec()).await.unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    assert_eq!(p.r_lines("rpc_tunnel_open").len(), 1);
    let engine_port = c.local_addr().unwrap().port();
    let mut other = nc_from(&src, Some(engine_port), s.rpc_port());
    let refused = poll_until(Duration::from_secs(10), || {
        !lines(&p.h, "rpc_bridge_refused").is_empty()
    })
    .await;
    let _ = other.kill();
    let _ = other.wait();
    assert!(refused.is_some(), "the connection from {src}:{engine_port} was refused");
    let line = &lines(&p.h, "rpc_bridge_refused")[0]["fields"];
    assert_eq!(
        (&line["reason"], &line["client_port"]),
        (&"not_the_engine".into(), &engine_port.into())
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(p.r_lines("rpc_tunnel_open").len(), 1, "R saw no tunnel for it");
    assert!(s.failure().is_none(), "{:?}", s.failure());
}

/// COMM_INIT (20) makes b11609's rpc-server listen on 0.0.0.0 or dial any host: H's meter refuses
/// it from its engine, and R refuses it from a head that bypasses H's bridge, at its first byte.
/// Only the command byte is ever sent, so even a broken filter could not open a listener.
#[tokio::test]
async fn comm_init_never_reaches_the_stage_single_host() {
    let Some(p) = pool().await else { return };
    let s = p.open(spec()).await.unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    c.write_all(&[20]).await.unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PolicyDenied, "{e}");
    assert!(e.message.contains("RPC command 20"), "{e}");
    assert_eq!(s.counters().to_r, 33, "the refused byte was not forwarded");
    drop(s);
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();

    // a hostile head: a raw tunnel on the session's connection, no bridge in the way. HELLO goes
    // through and is answered; COMM_INIT's first byte does not reach rpc-server.
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let (mut send, mut recv) = raw_tunnel(&p, &s).await;
    raw_hello(&mut send, &mut recv).await;
    send.write_all(&[20]).await.unwrap();
    let mut b = [0u8; 8];
    let r = timeout(Duration::from_secs(5), recv.read(&mut b)).await.unwrap();
    assert!(matches!(r, Err(quinn::ReadError::Reset(_))), "{r:?}");
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PolicyDenied, "R's reason: {e}");
    assert!(e.message.contains("RPC command 20"), "{e}");
    let close = stage_close(&p, "refused_command").await;
    assert_eq!(
        close["fields"]["to_r"], 33,
        "rpc-server got HELLO and nothing of COMM_INIT: {close}"
    );
    assert!(
        wait_dead(pid, Duration::from_secs(6)).await.is_some(),
        "R stopped rpc-server"
    );
    drop(s);
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();

    // COMM_INIT as the tunnel's very first byte: R never even dials rpc-server
    let s = p.open(spec()).await.unwrap();
    let (mut send, mut recv) = raw_tunnel(&p, &s).await;
    send.write_all(&[20]).await.unwrap();
    let r = timeout(Duration::from_secs(5), recv.read(&mut b)).await.unwrap();
    assert!(matches!(r, Err(quinn::ReadError::Reset(_))), "{r:?}");
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert!(e.message.contains("RPC command 20"), "{e}");
    let refused: Vec<_> = p
        .r_lines("rpc_tunnel_close")
        .into_iter()
        .filter(|l| l["fields"]["outcome"] == "refused_command")
        .collect();
    assert_eq!(refused.len(), 2, "{refused:?}");
    assert_eq!(refused[1]["fields"]["to_r"], 0, "{}", refused[1]);
}

/// A tunnel stream straight to R on the session's connection: a head that bypasses H's bridge.
async fn raw_tunnel(p: &Pool, s: &SplitSession) -> (quinn::SendStream, quinn::RecvStream) {
    let (mut send, recv) = p.conn.conn.open_bi().await.unwrap();
    let id = hex::decode(s.session_id()).unwrap();
    send.write_all(&encode_frame(
        &Body::RpcTunnelOpen(RpcTunnelOpen { session_id: id }).into(),
    ))
    .await
    .unwrap();
    (send, recv)
}

async fn raw_hello(send: &mut quinn::SendStream, recv: &mut quinn::RecvStream) {
    send.write_all(&cmd(HELLO, &[0; 24])).await.unwrap();
    let mut rsp = [0u8; 8 + 28];
    timeout(Duration::from_secs(10), recv.read_exact(&mut rsp))
        .await
        .expect("HELLO answered")
        .unwrap();
}

/// R's `rpc_tunnel_close` line with this outcome, once written.
async fn stage_close(p: &Pool, outcome: &str) -> serde_json::Value {
    poll_until(Duration::from_secs(5), || {
        p.r_lines("rpc_tunnel_close")
            .iter()
            .any(|l| l["fields"]["outcome"] == outcome)
    })
    .await
    .unwrap_or_else(|| panic!("no {outcome} line: {:?}", p.r_lines("rpc_tunnel_close")));
    p.r_lines("rpc_tunnel_close")
        .into_iter()
        .find(|l| l["fields"]["outcome"] == outcome)
        .unwrap()
}

/// A GRAPH_COMPUTE payload of b11609 (`| device u32 | uid u64 | n_nodes u32 | nodes u64 * n |
/// n_tensors u32 | rpc_tensor (296 B) * n |`) with one tensor per op; `op` sits at byte 52.
fn graph(ops: &[u32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend(0u32.to_le_bytes());
    p.extend(7u64.to_le_bytes());
    p.extend(1u32.to_le_bytes());
    p.extend(1u64.to_le_bytes());
    p.extend((ops.len() as u32).to_le_bytes());
    for (i, op) in ops.iter().enumerate() {
        let mut t = vec![0u8; 296];
        t[..8].copy_from_slice(&(i as u64 + 1).to_le_bytes());
        t[52..56].copy_from_slice(&op.to_le_bytes());
        p.extend(t);
    }
    cmd(10, &p)
}

/// R refuses a graph whose node carries a function pointer (op 95, `GGML_OP_CUSTOM`) from a head
/// that bypasses H's bridge: rpc-server never gets the graph's last byte, so never runs it; the
/// session ends with R's reason, rpc-server stops, and R's one tunnel line names the refusal and
/// carries no content. H's bridge refuses the same graph from its own engine first.
#[tokio::test]
async fn stage_refuses_a_function_pointer_graph_from_a_hostile_head_single_host() {
    let Some(p) = pool().await else { return };
    let g = graph(&[1, 95]);
    let s = p.open(spec()).await.unwrap();
    let (pid, _) = p.stage.session().unwrap();
    let (mut send, mut recv) = raw_tunnel(&p, &s).await;
    raw_hello(&mut send, &mut recv).await;
    send.write_all(&g).await.unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert_eq!(e.code, ErrorCode::PolicyDenied, "R's reason: {e}");
    assert!(e.message.contains("graph op 95"), "{e}");
    let close = stage_close(&p, "refused_graph_op").await;
    let to_r = close["fields"]["to_r"].as_u64().unwrap();
    assert!(
        (33..33 + g.len() as u64).contains(&to_r),
        "rpc-server got HELLO and not the whole graph: {close}"
    );
    let text = std::fs::read_to_string(&p.r.audit.path).unwrap();
    assert!(!text.contains(&hex::encode(&g[9..])), "no graph bytes in R's audit");
    assert!(
        wait_dead(pid, Duration::from_secs(6)).await.is_some(),
        "R stopped rpc-server"
    );
    drop(s);
    poll_until(Duration::from_secs(5), || p.stage.session().is_none())
        .await
        .unwrap();

    // through the bridge: H's own meter refuses it, and R sees only HELLO
    let s = p.open(spec()).await.unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    c.write_all(&g).await.unwrap();
    let e = failure_within(&s, Duration::from_secs(5)).await;
    assert!(e.message.contains("graph op 95"), "{e}");
    assert!(s.counters().to_r < 33 + g.len() as u64, "{:?}", s.counters());
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
}

/// Every command a REAL b11609 session sent (lab R0 configuration A, the meter's fixture: 14
/// commands, each at the smallest size the session sent it) passes both filters and reaches the
/// REAL rpc-server whole, one command per engine connection after HELLO. The head is SYNTHETIC
/// (zero payloads), so rpc-server may refuse their contents; the session stays up.
#[tokio::test]
async fn a_real_sessions_commands_all_reach_the_stage_synthetic_head_single_host() {
    let Some(p) = pool().await else { return };
    let mut cmds: std::collections::BTreeMap<u8, u64> = Default::default();
    for line in include_str!("../src/fixtures/rpc-session-b11609-r0a.txt").lines() {
        let f: Vec<&str> = line.split(' ').collect();
        if f[0] != "C" || f[2] != "0" {
            continue;
        }
        for h in &f[5..] {
            let hex = h.split_once(':').unwrap().1;
            if hex.len() == 18 {
                let b = hex::decode(hex).unwrap();
                let size = u64::from_le_bytes(b[1..].try_into().unwrap());
                let e = cmds.entry(b[0]).or_insert(size);
                *e = (*e).min(size);
            }
        }
    }
    assert_eq!(cmds.len(), 14, "{cmds:?}");
    let s = p.open(spec()).await.unwrap();
    let mut sent = Vec::new();
    for (&c, &size) in &cmds {
        let mut t = head(&s).await;
        hello(&mut t).await;
        let msg = cmd(c, &vec![0; size as usize]);
        t.write_all(&msg).await.unwrap();
        t.shutdown().await.unwrap();
        let mut rest = Vec::new();
        let _ = timeout(Duration::from_secs(10), t.read_to_end(&mut rest))
            .await
            .expect("closed");
        sent.push((c, 33 + msg.len() as u64));
        poll_until(Duration::from_secs(10), || {
            p.r_lines("rpc_tunnel_close").len() == sent.len()
        })
        .await
        .unwrap_or_else(|| panic!("command {c}: R's tunnel did not close"));
    }
    assert!(s.failure().is_none(), "{:?}", s.failure());
    assert!(lines(&p.h, "rpc_bridge_refused").is_empty());
    let mut closes = p.r_lines("rpc_tunnel_close");
    closes.sort_by_key(|l| l["fields"]["tunnel"].as_u64());
    for ((c, bytes), l) in sent.iter().zip(&closes) {
        // rpc-server may drop a connection over a zero payload (or a second HELLO): never a refusal
        let outcome = l["fields"]["outcome"].as_str().unwrap();
        assert!(["closed", "reset"].contains(&outcome), "command {c}: {l}");
        assert_eq!(
            l["fields"]["to_r"].as_u64(),
            Some(*bytes),
            "command {c} reached rpc-server whole: {l}"
        );
    }
    assert_eq!(s.counters().to_r, sent.iter().map(|x| x.1).sum::<u64>());
}

// ---------------------------------------------------------------- loss, audit, wire

/// A partition: R's connection idles out (4 s) and R stops rpc-server within 6 s; H reads the loss
/// within 10 s (D7).
#[tokio::test]
async fn connection_loss_stops_remote_rpc_server_within_6s_single_host() {
    let Some(engine) = engine_dir() else { return };
    std::env::set_var(TEST_POOL_ENV, "1");
    let (h, r) = pair();
    group(&h, &r, true);
    group(&r, &h, true);
    let nh = Node::start(
        Peer::config(),
        h.identity.clone(),
        h.peers.clone(),
        open_policy(&h, serving()),
        h.audit.clone(),
    )
    .await
    .unwrap();
    let r_policy = open_policy(&r, serving());
    let nr = Node::start(
        Peer::config(),
        r.identity.clone(),
        r.peers.clone(),
        r_policy.clone(),
        r.audit.clone(),
    )
    .await
    .unwrap();
    let (front, _back, _cap, blocked, _relay) = start_relay(nr.local_addr().unwrap()).await;
    let logs = tempfile::tempdir().unwrap();
    let mut stage = Stage::new(engine.clone(), logs.path().to_path_buf(), "b11609".into(), 2);
    stage.limits = fast();
    stage.available_memory = || Some(8 << 30);
    let stage = Arc::new(stage);
    let conn = nh.connect(front, r.id()).await.unwrap();
    let inn = timeout(Duration::from_secs(5), nr.accept()).await.unwrap().unwrap();
    let opts = ServeOptions {
        rpc: Some(stage.clone()),
        ..ServeOptions::standalone()
    };
    tokio::spawn(serve_peer_with(
        inn,
        manager(&r.paths, engine),
        r_policy,
        r.audit.clone(),
        opts,
    ));
    let s = SplitSession::open(&nh, conn, spec()).await.unwrap();
    let (pid, _) = stage.session().unwrap();
    let t = Instant::now();
    blocked.store(true, std::sync::atomic::Ordering::SeqCst);
    let dead = wait_dead(pid, Duration::from_secs(8))
        .await
        .expect("rpc-server stopped");
    assert!(t.elapsed() <= Duration::from_secs(6), "stopped after {dead:?}");
    let e = timeout(Duration::from_secs(10), s.ended()).await.unwrap();
    assert_eq!(e.code, ErrorCode::PeerLost, "{e}");
    assert!(t.elapsed() <= Duration::from_secs(10));
    // written after rpc-server stopped and its port was checked
    poll_until(Duration::from_secs(5), || !lines(&r, "session_close").is_empty())
        .await
        .expect("R's session_close");
    let close = &lines(&r, "session_close")[0]["fields"];
    assert_eq!(close["listener_left"], false, "{close}");
}

/// Payload bytes sent through a session carrying a canary.
const CANARY: &str = "CANARY-c5-7d1e9b";

async fn canary_session(p: &Pool) -> SplitSession {
    let s = p.open(spec()).await.unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    // a SET_TENSOR whose data is the canary; rpc-server reads it whole, then drops the connection
    // over the unknown buffer
    let mut payload = vec![0u8; 296 + 8];
    payload.extend(CANARY.repeat(8).as_bytes());
    c.write_all(&cmd(SET_TENSOR, &payload)).await.unwrap();
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
    drop(c); // the head's FIN: both directions are closed now
    poll_until(Duration::from_secs(10), || !p.r_lines("rpc_tunnel_close").is_empty())
        .await
        .unwrap();
    s
}

/// Audit lines on both sides: session and tunnel lines with ids, counts, bytes and durations, and
/// no payload byte (ADR-0005; the network crate's audit rule).
#[tokio::test]
async fn audit_has_session_and_tunnel_lines_without_content_single_host() {
    let Some(p) = pool().await else { return };
    let s = canary_session(&p).await;
    let id = s.session_id();
    drop(s);
    poll_until(Duration::from_secs(10), || !p.r_lines("session_close").is_empty())
        .await
        .unwrap();
    for (who, peer) in [("head", &p.h), ("stage", &p.r)] {
        for kind in ["session_open", "rpc_tunnel_open", "rpc_tunnel_close", "session_close"] {
            let l = lines(peer, kind);
            assert!(!l.is_empty(), "{who}: no {kind}");
            assert_eq!(l[0]["fields"]["session"], id.as_str(), "{who} {kind}: {}", l[0]);
            assert_eq!(l[0]["fields"]["role"], who);
        }
        let close = &lines(peer, "session_close")[0]["fields"];
        assert!(
            close["duration_ms"].is_u64() && close["to_r"].as_u64().unwrap() > 300,
            "{close}"
        );
        let text = std::fs::read_to_string(&peer.audit.path).unwrap();
        assert!(!text.contains(CANARY), "{who}'s audit has payload");
        assert!(!text.contains(&hex::encode(CANARY)), "{who}'s audit has payload as hex");
    }
}

/// Every datagram between the two nodes, captured by a relay: the canary the head sent through the
/// tunnel is never in plaintext on the wire. Positive control: the same scan finds it in the bytes
/// the head wrote. The weight-pattern control needs a real split load (C6, P1).
#[tokio::test]
async fn packet_capture_contains_no_plaintext_canary_single_host() {
    let Some(engine) = engine_dir() else { return };
    std::env::set_var(TEST_POOL_ENV, "1");
    let (h, r) = pair();
    group(&h, &r, true);
    group(&r, &h, true);
    let nh = Node::start(
        Peer::config(),
        h.identity.clone(),
        h.peers.clone(),
        open_policy(&h, serving()),
        h.audit.clone(),
    )
    .await
    .unwrap();
    let r_policy = open_policy(&r, serving());
    let nr = Node::start(
        Peer::config(),
        r.identity.clone(),
        r.peers.clone(),
        r_policy.clone(),
        r.audit.clone(),
    )
    .await
    .unwrap();
    let (front, _back, capture, _blocked, _relay) = start_relay(nr.local_addr().unwrap()).await;
    let logs = tempfile::tempdir().unwrap();
    let mut stage = Stage::new(engine.clone(), logs.path().to_path_buf(), "b11609".into(), 2);
    stage.limits = fast();
    stage.available_memory = || Some(8 << 30);
    let stage = Arc::new(stage);
    let conn = nh.connect(front, r.id()).await.unwrap();
    let inn = timeout(Duration::from_secs(5), nr.accept()).await.unwrap().unwrap();
    let opts = ServeOptions {
        rpc: Some(stage.clone()),
        ..ServeOptions::standalone()
    };
    tokio::spawn(serve_peer_with(
        inn,
        manager(&r.paths, engine),
        r_policy,
        r.audit.clone(),
        opts,
    ));
    let s = SplitSession::open(&nh, conn, spec()).await.unwrap();
    let mut c = head(&s).await;
    hello(&mut c).await;
    let mut payload = vec![0u8; 296 + 8];
    payload.extend(CANARY.repeat(8).as_bytes());
    let sent = cmd(SET_TENSOR, &payload);
    c.write_all(&sent).await.unwrap();
    assert!(closed_within(&mut c, Duration::from_secs(5)).await);
    assert!(s.counters().to_r >= sent.len() as u64, "the canary crossed the tunnel");
    let datagrams: Vec<Vec<u8>> = capture.lock().unwrap().iter().map(|(_, d)| d.clone()).collect();
    assert!(datagrams.len() > 10, "{} datagrams", datagrams.len());
    assert!(common_network::scan_for_plaintext(&datagrams, &[CANARY]).is_empty());
    assert_eq!(common_network::scan_for_plaintext(&[sent], &[CANARY]), vec![0]);
}
