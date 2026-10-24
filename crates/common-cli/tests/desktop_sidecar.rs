//! The desktop app starts its bundled daemon with the arguments in
//! `apps/desktop/src-tauri/daemon-args.txt`. These tests run the CLI built from this checkout with
//! exactly those arguments (only the port is swapped for a free one), so a CLI change that would make
//! the app's daemon exit with a usage error fails here, not on a user's Mac.
//!
//! Regression for: a packaged app passed `--exit-with-parent` to a bundled daemon built before that
//! flag existed ("unexpected argument '--exit-with-parent' found", exit 2, "daemon did not become
//! healthy within 20s").
//!
//! The lifecycle test uses the real pinned llama-server and the stories15M fixture (skips loudly
//! when absent; `COMMON_REQUIRE_FIXTURES=1` fails instead). Nothing is mocked.
// Unix only: symlinked homes and signals (libc::kill, waitpid).
#![cfg(unix)]
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const DESKTOP_ARGS: &str = include_str!("../../../apps/desktop/src-tauri/daemon-args.txt");
const MODEL: &str = "stories15M-q4_0";
const BLOB: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

fn desktop_args() -> Vec<String> {
    DESKTOP_ARGS.split_whitespace().map(String::from).collect()
}

/// The desktop's arguments with the value after `--port` replaced.
fn desktop_args_on(port: u16) -> Vec<String> {
    let mut a = desktop_args();
    let i = a.iter().position(|x| x == "--port").expect("desktop args name a port");
    a[i + 1] = port.to_string();
    a
}

#[test]
fn desktop_daemon_args_are_accepted_by_the_cli() {
    let args = desktop_args();
    assert_eq!(args.first().map(String::as_str), Some("serve"), "{args:?}");
    for flag in ["--host", "--port", "--exit-with-parent"] {
        assert!(args.iter().any(|a| a == flag), "the desktop must pass {flag}: {args:?}");
    }
    // clap rejects an unknown argument before it reaches --help: exit 0 here means the daemon the
    // app launches parses its command line.
    let out = Command::new(env!("CARGO_BIN_EXE_common"))
        .args(&args)
        .arg("--help")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "common rejects the desktop's daemon arguments {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn an_unknown_daemon_argument_is_a_usage_error() {
    // The failure the desktop now reports early: exit code 2 from clap.
    let out = Command::new(env!("CARGO_BIN_EXE_common"))
        .args(["serve", "--no-such-flag"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unexpected argument"));
}

fn real_home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

fn fixtures() -> Option<PathBuf> {
    let home = real_home();
    let engine = home.join("backends/llama.cpp/b11609/llama-server");
    let blob = home.join(format!("models/blobs/sha256-{BLOB}"));
    let manifest = home.join(format!("models/manifests/{MODEL}.json"));
    let missing = [&engine, &blob, &manifest].into_iter().find(|p| !p.is_file());
    let Some(missing) = missing else { return Some(home) };
    assert!(
        std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "COMMON_REQUIRE_FIXTURES=1: missing {}",
        missing.display()
    );
    eprintln!("SKIP: missing asset {}", missing.display());
    None
}

/// A fresh COMMON_HOME linking the real engine and fixture blob (manifest copied); removed on drop.
struct Home(PathBuf);

impl Home {
    fn new(real: &Path) -> Home {
        let root = std::env::temp_dir().join(format!("common-desktop-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let backends = root.join("backends/llama.cpp");
        std::fs::create_dir_all(&backends).unwrap();
        for e in std::fs::read_dir(real.join("backends/llama.cpp")).unwrap() {
            let e = e.unwrap();
            std::os::unix::fs::symlink(e.path(), backends.join(e.file_name())).unwrap();
        }
        std::fs::create_dir_all(root.join("models/blobs")).unwrap();
        std::fs::create_dir_all(root.join("models/manifests")).unwrap();
        let blob = format!("models/blobs/sha256-{BLOB}");
        std::os::unix::fs::symlink(real.join(&blob), root.join(&blob)).unwrap();
        let manifest = format!("models/manifests/{MODEL}.json");
        std::fs::copy(real.join(&manifest), root.join(&manifest)).unwrap();
        Home(root)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        // A failed assertion must not leave an engine from this home running.
        let _ = Command::new("pkill")
            .args(["-f", &format!("{}/backends/llama.cpp", self.0.display())])
            .status();
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn free_tcp_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the pid exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

fn wait_for(within: Duration, what: &str, cond: impl Fn() -> bool) -> Duration {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() < within, "{what} not within {within:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
    start.elapsed()
}

fn listening(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

/// One HTTP/1.1 request to the daemon's loopback API: (status, JSON body or Null).
fn http(port: u16, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (
        head.split(' ').nth(1).unwrap().parse().unwrap(),
        serde_json::from_str(body).unwrap_or(Value::Null),
    )
}

fn wait_healthy(port: u16, child: &mut Child) -> Duration {
    let start = Instant::now();
    loop {
        if listening(port) && http(port, "GET", "/health", None).0 == 200 {
            return start.elapsed();
        }
        if let Some(st) = child.try_wait().unwrap() {
            panic!("daemon exited before becoming healthy: {st}");
        }
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "daemon not healthy within 60 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Load the fixture and run the golden prompt; returns the engine (llama-server) pid.
fn infer(port: u16, home: &Path) -> i32 {
    let (st, v) = http(port, "POST", &format!("/api/models/{MODEL}/load"), Some(json!({})));
    assert_eq!(st, 200, "load: {v}");
    let (st, v) = http(
        port,
        "POST",
        "/v1/completions",
        Some(json!({ "model": MODEL, "prompt": PROMPT, "max_tokens": 32, "temperature": 0 })),
    );
    assert_eq!(st, 200, "completion: {v}");
    assert_eq!(v["choices"][0]["text"], GOLDEN, "{v}");
    let engines = engine_pids(home);
    assert_eq!(engines.len(), 1, "one engine for this home: {engines:?}");
    engines[0]
}

/// llama-server processes started from this home's engine directory.
fn engine_pids(home: &Path) -> Vec<i32> {
    let out = Command::new("pgrep")
        .args(["-f", &format!("{}/backends/llama.cpp", home.display())])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

/// SINGLE-HOST: the daemon started with the desktop's arguments becomes healthy, runs real inference,
/// stops itself and its engine when its parent process dies (what `--exit-with-parent` is for: the
/// app crashed or was force-quit), frees the port, and a second start on the same port works.
#[test]
fn desktop_daemon_starts_infers_exits_with_its_parent_and_restarts_single_host() {
    let Some(real) = fixtures() else { return };
    let home = Home::new(&real);
    let port = free_tcp_port();
    let bin = env!("CARGO_BIN_EXE_common");

    // A stand-in for the app: `sh` starts the daemon in the background, writes its pid to a file, then
    // becomes `sleep` (same pid), so the daemon's parent is a process this test can kill. Like the
    // app (tauri-plugin-shell), the daemon's stdout and stderr are a pipe whose read end goes away
    // when the "app" dies: here the test holds it and drops it when it kills the parent. A daemon that
    // writes to that broken pipe while shutting down must not die before it stops its engine.
    let (reader, writer) = std::io::pipe().unwrap();
    let pidfile = home.0.join("daemon.pid");
    let mut parent = Command::new("sh")
        .arg("-c")
        .arg(r#"pidfile="$1"; shift; "$0" "$@" & echo $! > "$pidfile"; exec sleep 600"#)
        .arg(bin)
        .arg(&pidfile)
        .args(desktop_args_on(port))
        .env("COMMON_HOME", &home.0)
        .env("COMMON_MDNS", "0")
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(writer.try_clone().unwrap())
        .stderr(writer)
        .spawn()
        .unwrap();
    wait_for(Duration::from_secs(10), "daemon pid file", || {
        std::fs::read_to_string(&pidfile).is_ok_and(|s| s.ends_with('\n'))
    });
    let daemon: i32 = std::fs::read_to_string(&pidfile).unwrap().trim().parse().unwrap();
    let healthy = wait_healthy(port, &mut parent);
    let engine = infer(port, &home.0);

    parent.kill().unwrap(); // SIGKILL: like a crashed app, no chance to stop its child
    parent.wait().unwrap();
    drop(reader); // the app's end of the daemon's stdout/stderr disappears with it
    let gone = wait_for(Duration::from_secs(10), "daemon exits after its parent died", || {
        !alive(daemon)
    });
    wait_for(Duration::from_secs(5), "engine stops with the daemon", || {
        !alive(engine)
    });
    wait_for(Duration::from_secs(5), "port is released", || !listening(port));
    assert!(engine_pids(&home.0).is_empty(), "no orphaned engine");

    // Restart on the same port: no conflict, one daemon, one engine, real inference again.
    let mut again = Command::new(bin)
        .args(desktop_args_on(port))
        .env("COMMON_HOME", &home.0)
        .env("COMMON_MDNS", "0")
        .env_remove("RUST_LOG")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let healthy2 = wait_healthy(port, &mut again);
    let engine2 = infer(port, &home.0);
    // SIGTERM, what the app sends on a normal quit: the daemon stops the engine and exits.
    // SAFETY: kill(2) on our own child.
    unsafe { libc::kill(again.id() as i32, libc::SIGTERM) };
    let again_pid = again.id() as i32;
    let stopped = wait_for(Duration::from_secs(10), "daemon exits on SIGTERM", || {
        // A zombie still answers kill(pid, 0); reap via waitpid without blocking.
        // SAFETY: waitpid on our own child with WNOHANG.
        unsafe { libc::waitpid(again_pid, std::ptr::null_mut(), libc::WNOHANG) == again_pid }
    });
    wait_for(Duration::from_secs(5), "engine stops on SIGTERM", || !alive(engine2));
    assert!(!listening(port), "port released after SIGTERM");
    eprintln!(
        "SINGLE-HOST desktop sidecar: healthy {healthy:?}; exited {gone:?} after its parent was killed; \
         restart healthy {healthy2:?}; stopped {stopped:?} after SIGTERM; no orphaned engine"
    );
}
