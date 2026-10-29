//! WP-04 C3, REAL and SINGLE-HOST: the pinned `ggml-rpc-server` started by `rpc_server::start`,
//! checked through the OS's own socket tables (`sockets::live`). One host, loopback only: this is
//! not a stage host on another machine and exercises no transport. The socket owner checks exist on
//! Linux and macOS only; the last three tests are Linux only (`pre_exec`).
#![cfg(any(target_os = "linux", target_os = "macos"))]

mod support;

use std::io::Read;
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::time::{Duration, Instant};
use support::*;
use ucompute_core::ErrorCode;
use ucompute_runtime::llamacpp::{process, rpc_server, sockets};

fn engine_dir() -> Option<std::path::PathBuf> {
    setup().map(|_| {
        ucompute_core::Paths::at(ucompute_home())
            .backends()
            .join("llama.cpp")
            .join("b11609")
    })
}

/// Poll `f` every 50 ms for up to `within`.
async fn eventually(within: Duration, mut f: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + within;
    while Instant::now() < end {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    f()
}

/// This host's outbound IPv4 address, if it has one (a UDP connect sends nothing).
fn lan_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    s.connect(("192.0.2.1", 9)).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

#[tokio::test]
async fn rpc_server_binds_loopback_only() {
    let Some(engine) = engine_dir() else { return };
    let logs = tempfile::tempdir().unwrap();
    let s = rpc_server::start(&engine, logs.path(), 2, None).await.unwrap();
    let pid = s.child.id().unwrap();
    assert!(sockets::live::listener_owned_by(pid, s.port).unwrap());
    assert!(!s.device.is_empty(), "the banner names the device");
    eprintln!("rpc-server pid {pid} on 127.0.0.1:{}, device {}", s.port, s.device);
    if let Some(ip) = lan_ip() {
        assert!(
            TcpStream::connect_timeout(&(ip, s.port).into(), Duration::from_secs(2)).is_err(),
            "{ip}:{} must refuse",
            s.port
        );
    }
    process::stop(s.child).await;
    assert!(!process::alive(pid));
}

#[tokio::test]
async fn rpc_server_stops_on_drop() {
    let Some(engine) = engine_dir() else { return };
    let logs = tempfile::tempdir().unwrap();
    let s = rpc_server::start(&engine, logs.path(), 2, None).await.unwrap();
    let (pid, port) = (s.child.id().unwrap(), s.port);
    drop(s);
    assert!(
        eventually(Duration::from_secs(5), || TcpStream::connect(("127.0.0.1", port))
            .is_err())
        .await,
        "listener gone after drop"
    );
    assert!(
        eventually(Duration::from_secs(5), || !process::alive(pid)).await,
        "process {pid} gone after drop"
    );
}

/// A squatter that records whatever reaches it.
fn squatter(listener: TcpListener) -> std::sync::mpsc::Receiver<usize> {
    listener.set_nonblocking(true).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(3);
        let mut got = 0;
        while Instant::now() < end {
            if let Ok((mut c, _)) = listener.accept() {
                let _ = c.set_read_timeout(Some(Duration::from_millis(200)));
                let mut buf = [0u8; 4096];
                got += 1 + c.read(&mut buf).unwrap_or(0);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = tx.send(got);
    });
    rx
}

#[tokio::test]
async fn squatted_rpc_port_is_detected_and_sent_nothing() {
    let Some(engine) = engine_dir() else { return };
    let logs = tempfile::tempdir().unwrap();
    // the same address the server wants, and the same port on IPv6 loopback
    for addr in ["127.0.0.1:0", "[::1]:0"] {
        let l = TcpListener::bind(addr).unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = squatter(l);
        let e = rpc_server::start_at(&engine, logs.path(), 2, None, port)
            .await
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::BackendFailed, "{addr}: {}", e.message);
        assert!(e.message.contains("another listener holds"), "{addr}: {}", e.message);
        assert_eq!(
            seen.recv().unwrap(),
            0,
            "{addr}: the squatter got a connection or bytes"
        );
    }
}

#[tokio::test]
async fn foreign_client_on_rpc_port_is_detected() {
    let Some(engine) = engine_dir() else { return };
    let nc = ["/usr/bin/nc", "/bin/nc"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let Some(nc) = nc else {
        eprintln!("SKIP: no nc for a foreign client process");
        return;
    };
    let logs = tempfile::tempdir().unwrap();
    let s = rpc_server::start(&engine, logs.path(), 2, None).await.unwrap();
    let me = std::process::id();
    // our own client (this process stands in for the daemon) is not foreign
    let own = TcpStream::connect(("127.0.0.1", s.port)).unwrap();
    let own_port = own.local_addr().unwrap().port();
    assert!(
        eventually(Duration::from_secs(3), || {
            sockets::live::established_clients(s.port)
                .unwrap()
                .iter()
                .any(|&(p, _)| p == own_port)
        })
        .await
    );
    assert_eq!(sockets::live::foreign_clients_of(me, s.port).unwrap(), vec![]);
    // another process connecting to the port is
    let mut other = std::process::Command::new(nc)
        .args(["127.0.0.1", &s.port.to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let found = eventually(Duration::from_secs(5), || {
        sockets::live::foreign_clients_of(me, s.port).unwrap().len() == 1
    })
    .await;
    let _ = other.kill();
    let _ = other.wait();
    assert!(found, "nc's connection is reported as foreign");
    drop(own);
    process::stop(s.child).await;
}

/// A SYNTHETIC head: HELLO with no transport caps, then one `ALLOC_BUFFER` of `size` on device 0
/// (b11609 wire format, `| cmd u8 | size u64 LE | payload |`). The granted `remote_ptr`, 0 when the
/// server refused, or None when the server closed the connection.
#[cfg(target_os = "linux")]
fn alloc_buffer(port: u16, size: u64) -> Option<u64> {
    use std::io::Write;
    let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let mut hello = vec![14u8];
    hello.extend(24u64.to_le_bytes());
    hello.extend([0u8; 24]);
    c.write_all(&hello).unwrap();
    let mut rsp = [0u8; 8 + 28];
    c.read_exact(&mut rsp).unwrap();
    assert_eq!((rsp[0], rsp[8]), (28, 8), "HELLO reply of protocol v8");
    let mut req = vec![0u8];
    req.extend(12u64.to_le_bytes());
    req.extend(0u32.to_le_bytes());
    req.extend(size.to_le_bytes());
    c.write_all(&req).ok()?;
    let mut rsp = [0u8; 8 + 16];
    c.read_exact(&mut rsp).ok()?;
    Some(u64::from_le_bytes(rsp[8..16].try_into().unwrap()))
}

#[cfg(target_os = "linux")]
fn rlimit_as(pid: &str) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/limits"))
        .unwrap()
        .lines()
        .find(|l| l.starts_with("Max address space"))
        .unwrap()
        .to_string()
}

/// A hostile head asks for more memory than the cap allows: the server refuses (or dies) and the
/// daemon's own limit is untouched. The uncapped control on the same host grants the same request,
/// so the refusal is the cap's, not the kernel's overcommit rule.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn rpc_server_memory_cap_kills_server_not_daemon() {
    let Some(engine) = engine_dir() else { return };
    const CAP: u64 = 1 << 30;
    const ASK: u64 = 4 << 30;
    let ours = rlimit_as("self");
    let logs = tempfile::tempdir().unwrap();
    let control = rpc_server::start(&engine, logs.path(), 2, None).await.unwrap();
    let granted = alloc_buffer(control.port, ASK);
    process::stop(control.child).await;
    assert!(
        matches!(granted, Some(p) if p != 0),
        "control: uncapped server refused 4 GiB: {granted:?}"
    );
    let capped = rpc_server::start(&engine, logs.path(), 2, Some(CAP)).await.unwrap();
    let pid = capped.child.id().unwrap();
    let limit = rlimit_as(&pid.to_string());
    assert!(limit.contains(&CAP.to_string()), "child limit: {limit}");
    let reply = alloc_buffer(capped.port, ASK);
    eprintln!("capped server ({limit}) answered a 4 GiB ALLOC_BUFFER with {reply:?}");
    assert!(
        matches!(reply, None | Some(0)),
        "4 GiB granted under a 1 GiB cap: {reply:?}"
    );
    assert_eq!(rlimit_as("self"), ours, "the daemon's own limit changed");
    process::stop(capped.child).await;
}

/// The server yields the CPU to the daemon: nice 10 (`setpriority` in `pre_exec`).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn rpc_server_runs_at_nice_10() {
    let Some(engine) = engine_dir() else { return };
    let logs = tempfile::tempdir().unwrap();
    let s = rpc_server::start(&engine, logs.path(), 2, None).await.unwrap();
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", s.child.id().unwrap())).unwrap();
    // proc(5): field 19 is nice; fields after the `(comm)` start at field 3
    let nice = stat
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .nth(16)
        .unwrap()
        .to_string();
    process::stop(s.child).await;
    assert_eq!(nice, "10");
}

/// PDEATHSIG follows the thread that forked, and tokio retires an idle blocking-pool thread after
/// 10 s. Started from inside `spawn_blocking`, whose thread then idles out, the server lives on.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_server_survives_blocking_pool_idle() {
    let Some(engine) = engine_dir() else { return };
    let logs = tempfile::tempdir().unwrap();
    let dir = logs.path().to_path_buf();
    let rt = tokio::runtime::Handle::current();
    let mut s = tokio::task::spawn_blocking(move || rt.block_on(rpc_server::start(&engine, &dir, 2, None)))
        .await
        .unwrap()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(15)).await;
    let status = s.child.try_wait().unwrap();
    assert!(status.is_none(), "rpc-server died while idle: {status:?}");
    assert!(TcpStream::connect(("127.0.0.1", s.port)).is_ok());
    process::stop(s.child).await;
}

/// Child-process helper for [`rpc_server_dies_with_daemon`]: a stand-in daemon that starts the
/// server from a worker task, prints its pid and waits to be killed.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "child-process helper"]
fn daemon_helper() {
    let logs = std::path::PathBuf::from(std::env::var_os("RPC_TEST_LOGS").unwrap());
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async move {
        let engine = engine_dir().unwrap();
        let s = tokio::spawn(async move { rpc_server::start(&engine, &logs, 2, None).await })
            .await
            .unwrap()
            .unwrap();
        println!("rpc-pid {}", s.child.id().unwrap());
        std::future::pending::<()>().await;
    });
}

/// The daemon is SIGKILLed (no unwinding, no `kill_on_drop`): PDEATHSIG takes the server with it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn rpc_server_dies_with_daemon() {
    use std::io::BufRead;
    if engine_dir().is_none() {
        return;
    }
    let logs = tempfile::tempdir().unwrap();
    let mut daemon = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "daemon_helper", "--nocapture"])
        .env("RPC_TEST_LOGS", logs.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = std::io::BufReader::new(daemon.stdout.take().unwrap());
    let rpc: u32 = out
        .lines()
        .map_while(|l| l.ok())
        .find_map(|l| l.strip_prefix("rpc-pid ").map(|p| p.parse().unwrap()))
        .expect("helper printed the rpc-server pid");
    assert!(process::alive(rpc));
    daemon.kill().unwrap();
    daemon.wait().unwrap();
    let gone = eventually(Duration::from_secs(5), || !process::alive(rpc)).await;
    if !gone {
        // SAFETY: plain syscall on the pid the helper reported; worst case ESRCH.
        unsafe { libc::kill(rpc as libc::pid_t, libc::SIGKILL) };
    }
    assert!(gone, "rpc-server {rpc} outlived its daemon");
}
