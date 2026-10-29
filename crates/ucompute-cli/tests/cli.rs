//! Runs the built `ucompute` binary against the real assets in UCOMPUTE_HOME (default: the repo's
//! `.ucompute`). Tests that need the pinned engine or the fixture model skip loudly when absent, and
//! fail instead under `UCOMPUTE_REQUIRE_FIXTURES=1` (the evidence job): a green run always ran.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
// The signal and reaper tests are Unix only.
#[cfg(unix)]
use std::{
    io::Read,
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering::SeqCst},
    sync::Arc,
    time::{Duration, Instant},
};

const MODEL: &str = "stories15M-q4_0";
const BLOB: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

fn home() -> PathBuf {
    std::env::var_os("UCOMPUTE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.ucompute"))
}

/// Print why a test did not run, or fail under `UCOMPUTE_REQUIRE_FIXTURES=1`.
fn skip(why: &str) {
    assert!(
        std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "UCOMPUTE_REQUIRE_FIXTURES=1: {why}"
    );
    eprintln!("SKIP: {why}");
}

fn assets_present() -> bool {
    let engine = home().join("backends/llama.cpp/b11609/llama-server");
    let blob = home().join(format!("models/blobs/sha256-{BLOB}"));
    for p in [&engine, &blob] {
        if !p.is_file() {
            skip(&format!("missing asset {}", p.display()));
            return false;
        }
    }
    true
}

fn ucompute(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ucompute"))
        .args(args)
        .env("UCOMPUTE_HOME", home())
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn ucompute")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_str(&text(&o.stdout)).unwrap_or_else(|e| {
        panic!(
            "stdout is not JSON ({e}): {}\nstderr: {}",
            text(&o.stdout),
            text(&o.stderr)
        )
    })
}

fn manifest_path(stderr: &str) -> PathBuf {
    let token = stderr
        .split_whitespace()
        .find_map(|t| t.strip_prefix("manifest="))
        .unwrap_or_else(|| panic!("no manifest= in stderr: {stderr}"));
    PathBuf::from(token)
}

fn read_manifest(path: &Path) -> Value {
    let v: Value = serde_json::from_slice(&std::fs::read(path).expect("manifest readable")).expect("manifest is JSON");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
    v
}

#[test]
fn hardware_json_matches_sysctl() {
    let o = ucompute(&["hardware", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v = json(&o);
    assert_eq!(v["backends"].as_array().map(Vec::len), Some(4));
    if cfg!(target_os = "macos") {
        let out = Command::new("/usr/sbin/sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .unwrap();
        let memsize: u64 = text(&out.stdout).trim().parse().unwrap();
        assert_eq!(v["memory"]["total_bytes"].as_u64(), Some(memsize));
    }
}

/// D-028: the former `COMMON_HOME` is not read: with only it set, the fixture store it names is not
/// the one listed (the default folder under an empty HOME is).
#[test]
fn former_common_home_is_not_read() {
    if !assets_present() {
        return;
    }
    let empty = std::env::temp_dir().join(format!("ucompute-cli-oldhome-{}", std::process::id()));
    std::fs::create_dir_all(&empty).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_ucompute"))
        .args(["models", "list", "--json"])
        .env_remove("UCOMPUTE_HOME")
        .env("COMMON_HOME", home())
        .env("HOME", &empty)
        .env("XDG_DATA_HOME", empty.join("xdg"))
        .env("LOCALAPPDATA", empty.join("local"))
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn ucompute");
    let err = text(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(!json(&o)
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == MODEL && r["downloaded"] == true));
    assert!(!err.contains("COMMON_HOME"), "{err}");
    let _ = std::fs::remove_dir_all(&empty);
}

#[test]
fn models_list_json_has_fixture_with_unknown_license() {
    if !assets_present() {
        return;
    }
    let o = ucompute(&["models", "list", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let rows = json(&o);
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == MODEL)
        .expect("fixture listed");
    assert_eq!(row["license"], "unknown");
    assert_eq!(row["downloaded"], true);
    assert_eq!(row["bytes"], 19_077_344);
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .all(|r| r["license"].as_str().is_some_and(|l| !l.is_empty())),
        "every row has a license"
    );
}

#[test]
fn run_cpu_prints_golden_text_and_writes_manifest() {
    if !assets_present() {
        return;
    }
    let o = ucompute(&[
        "run", MODEL, "-p", PROMPT, "-n", "32", "--temp", "0", "--seed", "42", "--device", "cpu",
    ]);
    let err = text(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert_eq!(text(&o.stdout), format!("{GOLDEN}\n"));
    assert!(
        err.contains("device=cpu offloaded=0/6") && err.contains("decode_tok_s="),
        "{err}"
    );
    let m = read_manifest(&manifest_path(&err));
    assert_eq!(m["schema_version"], 1);
    assert_eq!(m["model_sha256"], BLOB);
    assert_eq!(m["device_actual"], "cpu");
    assert_eq!(
        (m["prompt_tokens"].as_u64(), m["completion_tokens"].as_u64()),
        (Some(10), Some(32))
    );
    assert_eq!(m["seed"], 42);
    assert!(
        m.get("prompt").is_none(),
        "prompt text is only stored with --save-prompt"
    );
    assert_eq!(m["completion_token_ids"].as_array().map(Vec::len), Some(32));
    assert_eq!(m["scope"], "single_physical_host");
}

#[test]
fn run_gpu_json_shows_offloaded_layers() {
    if !assets_present() {
        return;
    }
    let hw = json(&ucompute(&["hardware", "--json"]));
    if !hw["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["available"] == true && b["name"] != "cpu")
    {
        // Only the macOS and Windows pins ship a GPU backend; the Linux pin is CPU only (D-024).
        if cfg!(any(target_os = "macos", windows)) {
            skip("no GPU backend on this host");
        } else {
            eprintln!("N/A: the pinned llama.cpp build for this OS is CPU only");
        }
        return;
    }
    let o = ucompute(&[
        "run", MODEL, "-p", PROMPT, "-n", "32", "--temp", "0", "--seed", "42", "--device", "gpu", "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v = json(&o);
    assert!(v["offloaded_layers"].as_u64().unwrap() > 0, "{v}");
    assert_ne!(v["device_actual"], "cpu");
    assert_eq!(v["device_requested"], "gpu");
    assert_eq!(v["completion"], GOLDEN);
    let _ = read_manifest(Path::new(v["manifest_path"].as_str().unwrap()));
}

#[test]
fn unknown_model_is_a_typed_error_on_stderr() {
    if !assets_present() {
        return;
    }
    let o = ucompute(&["run", "nosuchmodel", "-p", "x"]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(err.contains("error[model_not_found]") && err.contains("hint:"), "{err}");
    assert!(o.stdout.is_empty());

    let o = ucompute(&["run", "nosuchmodel", "-p", "x", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let e: Value = serde_json::from_str(text(&o.stderr).trim()).expect("json error on stderr");
    assert_eq!(e["code"], "model_not_found");
}

#[test]
fn inspect_json_reports_block_count() {
    if !assets_present() {
        return;
    }
    let o = ucompute(&["models", "inspect", MODEL, "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v = json(&o);
    assert_eq!(v["block_count"], 6);
    assert_eq!(v["architecture"], "llama");
    assert_eq!(v["license"], "unknown");
    assert_eq!(v["estimates"]["trained_ctx"]["estimate"]["n_ctx"], 128);
    assert_eq!(v["estimates"]["ctx_4096"]["estimate"]["n_ctx"], 4096);
}

/// X-15: a manifest saved with `--save-prompt` replays to the identical token sequence.
#[test]
fn replay_manifest_reproduces_tokens() {
    if !assets_present() {
        return;
    }
    let o = ucompute(&[
        "run",
        MODEL,
        "-p",
        PROMPT,
        "-n",
        "32",
        "--temp",
        "0",
        "--seed",
        "42",
        "--device",
        "cpu",
        "--save-prompt",
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let v = json(&o);
    let path = PathBuf::from(v["manifest_path"].as_str().unwrap());
    // top_p is an f32 in the manifest, so compare at f32 precision.
    assert_eq!(
        (
            v["prompt"].as_str(),
            v["input_mode"].as_str(),
            v["top_p"].as_f64().map(|x| x as f32),
            v["parallel_slots"].as_u64()
        ),
        (Some(PROMPT), Some("raw"), Some(0.95), Some(1))
    );

    let o = ucompute(&["replay", path.to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    assert_eq!(
        (r["match"].as_bool(), r["tokens"].as_u64(), r["device_actual"].as_str()),
        (Some(true), Some(32), Some("cpu")),
        "{r}"
    );
    let replayed = read_manifest(Path::new(r["manifest_path"].as_str().unwrap()));
    assert_eq!(replayed["completion_sha256"], v["completion_sha256"]);
    assert_eq!(replayed["completion_token_ids"], v["completion_token_ids"]);

    // Tampered ids are detected and reported with the first differing index.
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    m["completion_token_ids"][5] = Value::from(1);
    std::fs::write(&path, serde_json::to_vec(&m).unwrap()).unwrap();
    let o = ucompute(&["replay", path.to_str().unwrap(), "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let r = json(&o);
    assert_eq!(
        (r["match"].as_bool(), r["first_mismatch"].as_u64()),
        (Some(false), Some(5)),
        "{r}"
    );
    assert!(text(&o.stderr).contains("integrity_mismatch"), "{}", text(&o.stderr));
    let _ = read_manifest(Path::new(r["manifest_path"].as_str().unwrap()));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A manifest without the prompt text cannot be replayed; the error says how to make one that can.
#[test]
fn replay_without_saved_prompt_is_invalid_request() {
    if !assets_present() {
        return;
    }
    let o = ucompute(&[
        "run", MODEL, "-p", PROMPT, "-n", "4", "--temp", "0", "--seed", "42", "--device", "cpu",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let path = manifest_path(&text(&o.stderr));
    let o = ucompute(&["replay", path.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(
        err.contains("error[invalid_request]") && err.contains("--save-prompt"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Engines started from our pinned binary that nothing owns, by the reaper's rule: the parent is
/// not a `ucompute` process and not in the engine's process group (pid 1, or on Linux a subreaper
/// such as `systemd --user`, adopted it after its `ucompute` died). Another test binary's engines on
/// the same UCOMPUTE_HOME share their harness's group and do not count. `ps` zombie rows show
/// `(llama-server)` and never match.
#[cfg(unix)]
fn orphaned_engines() -> Vec<String> {
    let bin = home().join("backends/llama.cpp/b11609/llama-server");
    let out = Command::new("ps")
        .args(["-axo", "pid=,ppid=,pgid=,ucomm=,args="])
        .output()
        .unwrap();
    let out = text(&out.stdout);
    let rows: Vec<Vec<&str>> = out
        .lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>())
        .filter(|f| f.len() >= 5)
        .collect();
    let owned = |e: &[&str]| {
        rows.iter()
            .any(|p| p[0] == e[1] && (p[3] == "ucompute" || (p[0] != "1" && p[2] == e[2])))
    };
    rows.iter()
        .filter(|f| Path::new(f[4]) == bin && !owned(f))
        .map(|f| f.join(" "))
        .collect()
}

/// A spawned `ucompute` that is killed, with the engines it started, if it is still running when
/// this drops: a test that fails mid-run (a timeout's panic) leaves no engine behind. The CLI is
/// stopped first so it cannot start another engine in between. Derefs to the [`std::process::Child`].
#[cfg(unix)]
struct Reaped(std::process::Child);

#[cfg(unix)]
impl std::ops::Deref for Reaped {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(unix)]
impl std::ops::DerefMut for Reaped {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[cfg(unix)]
impl Drop for Reaped {
    fn drop(&mut self) {
        // Exited and reaped (try_wait keeps the status): its pid may belong to someone else now.
        if !matches!(self.0.try_wait(), Ok(None)) {
            return;
        }
        let pid = self.0.id() as libc::pid_t;
        // SAFETY: plain syscalls on the child this test spawned (not yet reaped) and its children.
        unsafe { libc::kill(pid, libc::SIGSTOP) };
        if let Ok(o) = Command::new("pgrep").args(["-P", &pid.to_string()]).output() {
            for engine in text(&o.stdout).split_whitespace().filter_map(|p| p.parse().ok()) {
                unsafe { libc::kill(engine, libc::SIGKILL) };
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks existence.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// F-08-02: SIGINT while tokens are streaming prints `stopped`, exits 130 and leaves no engine
/// behind. Needs a model slow enough to interrupt (the 15M fixture finishes in ~0.3 s).
#[cfg(unix)]
#[test]
fn sigint_mid_generation_stops_and_exits_130() {
    signal_mid_generation("-INT", false);
}

/// SIGTERM (supervisor, `kill`) takes the same path as Ctrl-C: the engine is stopped, not orphaned.
#[cfg(unix)]
#[test]
fn sigterm_mid_generation_stops_and_exits_130() {
    signal_mid_generation("-TERM", false);
}

/// An engine that cannot exit (stopped here with SIGSTOP, as a starved or wedged process would be)
/// still ends: after the 2 s grace in `process::stop` the CLI kills it, prints `stopped`, exits 130
/// and leaves nothing behind. Exit lands between the grace and the grace plus margin.
#[cfg(unix)]
#[test]
fn sigint_with_a_stuck_engine_kills_it_after_the_grace_and_exits_130() {
    signal_mid_generation("-INT", true);
}

/// A 200 from this port's `/health` (a raw HTTP/1.1 request; any other listener fails it).
#[cfg(unix)]
fn health_ok(port: u16) -> bool {
    use std::io::Write;
    let Ok(mut c) = std::net::TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
    if write!(
        c,
        "GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .is_err()
    {
        return false;
    }
    let mut resp = String::new();
    c.read_to_string(&mut resp).is_ok() && resp.starts_with("HTTP/1.1 200") && resp.contains("\"status\":\"ok\"")
}

/// `ucompute serve` on a free port with {MODEL} loaded, and its engine's pid; `None` after a skip.
#[cfg(unix)]
fn serve_with_engine() -> Option<(Reaped, u32)> {
    use std::io::Write;
    use std::net::TcpStream;
    if !assets_present() {
        return None;
    }
    // The port is picked by binding and dropping, so another test's socket can take it before serve
    // binds it (ECONNRESET on the first write, seen once in a full run): only a 200 from /health while
    // our child is still running counts as "serve is up", and a serve that exits early (port taken) is
    // retried on a new port.
    let (mut child, port) = 'start: {
        for _ in 0..3 {
            let port = std::net::TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let mut child = Command::new(env!("CARGO_BIN_EXE_ucompute"))
                .args(["serve", "--port", &port.to_string()])
                .env("UCOMPUTE_HOME", home())
                .env_remove("RUST_LOG")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map(Reaped)
                .expect("spawn ucompute serve");
            let deadline = Instant::now() + Duration::from_secs(30);
            while child.try_wait().unwrap().is_none() {
                if health_ok(port) {
                    break 'start (child, port);
                }
                assert!(
                    Instant::now() < deadline,
                    "serve did not answer /health on {port} within 30 s"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        panic!("serve exited early three times (ports taken?)");
    };
    let mut conn = TcpStream::connect(("127.0.0.1", port)).unwrap();
    conn.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    write!(conn, "POST /api/models/{MODEL}/load HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
    let mut resp = String::new();
    conn.read_to_string(&mut resp).unwrap();
    if resp.contains("insufficient_memory") {
        let _ = child.kill();
        skip(&format!("not enough free memory to load {MODEL} right now"));
        return None;
    }
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap_or(""))
        .unwrap_or_else(|e| panic!("load body is not JSON ({e}): {resp}"));
    let engine_pid = body["pid"].as_u64().expect("engine pid in LoadedModelInfo") as u32;
    assert!(pid_alive(engine_pid), "engine {engine_pid} running after load");
    Some((child, engine_pid))
}

/// The guard the engine-starting tests rely on: dropping a [`Reaped`] CLI that is still running
/// (as a failed assertion's unwinding does) kills it and the engine it started.
#[cfg(unix)]
#[test]
fn reaped_kills_a_running_cli_and_its_engine() {
    let Some((child, engine_pid)) = serve_with_engine() else {
        return;
    };
    let cli = child.id();
    drop(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(engine_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!pid_alive(cli), "the CLI {cli} is gone");
    assert!(!pid_alive(engine_pid), "engine {engine_pid} left running");
    assert!(orphaned_engines().is_empty(), "{:?}", orphaned_engines());
}

/// SIGTERM to `ucompute serve` with a model loaded stops that engine process before exiting.
#[cfg(unix)]
#[test]
fn sigterm_to_serve_stops_the_loaded_engine() {
    let Some((mut child, engine_pid)) = serve_with_engine() else {
        return;
    };
    assert!(Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let status = child.wait().unwrap();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(status.success(), "serve exits cleanly on SIGTERM: {status} {err}");
    let deadline = Instant::now() + Duration::from_secs(5);
    while pid_alive(engine_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !pid_alive(engine_pid),
        "engine {engine_pid} orphaned after SIGTERM to serve: {err}"
    );
    assert!(orphaned_engines().is_empty(), "{:?}", orphaned_engines());
}

/// How long the CLI waits for its engine after SIGTERM before killing it (`process::stop`), and
/// the margin allowed on top for the CLI's own exit on a loaded machine. The exit is bounded by
/// these, not by how fast the engine is: on an 8 GB Mac swapping during the full suite the engine
/// can take the whole grace (observed 2.40 s exits), while an idle run exits in about 60 ms.
#[cfg(unix)]
const ENGINE_STOP_GRACE: Duration = Duration::from_secs(2);
#[cfg(unix)]
const EXIT_MARGIN: Duration = Duration::from_secs(1);

#[cfg(unix)]
fn signal_mid_generation(signal: &str, stall_engine: bool) {
    const SLOW: &str = "qwen2.5-0.5b-instruct-q4_k_m";
    if !assets_present() {
        return;
    }
    if !home().join(format!("models/manifests/{SLOW}.json")).is_file() {
        return skip(&format!("{SLOW} is not downloaded"));
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_ucompute"))
        .args([
            "run",
            SLOW,
            "-p",
            "Write a very long story about a rabbit who travels the world.",
            "-n",
            "2000",
            "--temp",
            "0",
            "--device",
            "cpu",
        ])
        .env("UCOMPUTE_HOME", home())
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(Reaped)
        .expect("spawn ucompute");
    let mut stdout = child.stdout.take().unwrap();
    let streaming = Arc::new(AtomicBool::new(false));
    let flag = streaming.clone();
    let reader = std::thread::spawn(move || {
        let (mut buf, mut total) = ([0u8; 256], 0usize);
        loop {
            match stdout.read(&mut buf) {
                Ok(0) | Err(_) => return total,
                Ok(n) => {
                    total += n;
                    flag.store(true, SeqCst);
                }
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(90);
    while !streaming.load(SeqCst) {
        if let Some(status) = child.try_wait().unwrap() {
            let mut err = String::new();
            child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
            // Sibling tests load engines in parallel on an 8 GB box; a starved load is not a CLI bug.
            if err.contains("error[insufficient_memory]") {
                let _ = child.kill();
                return skip(&format!(
                    "not enough free memory to load {SLOW} right now: {}",
                    err.trim()
                ));
            }
            panic!("ucompute exited with {status} before streaming any token: {err}");
        }
        assert!(Instant::now() < deadline, "no tokens within 90 s");
        std::thread::sleep(Duration::from_millis(50));
    }
    if stall_engine {
        let kids = Command::new("pgrep")
            .args(["-P", &child.id().to_string()])
            .output()
            .unwrap();
        let engine = String::from_utf8_lossy(&kids.stdout)
            .split_whitespace()
            .filter_map(|p| p.parse::<i32>().ok())
            .next()
            .expect("the CLI's engine process");
        // SAFETY: plain syscall on the engine this run started.
        assert_eq!(unsafe { libc::kill(engine, libc::SIGSTOP) }, 0);
    }
    let sent = Instant::now();
    assert!(Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let status = child.wait().unwrap();
    let exit_latency = sent.elapsed();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert_eq!(status.code(), Some(130), "{err}");
    assert!(err.trim_end().ends_with("stopped"), "{err}");
    assert!(reader.join().unwrap() > 0, "tokens were streamed before the interrupt");
    assert!(
        exit_latency < ENGINE_STOP_GRACE + EXIT_MARGIN,
        "exit after {signal} took {exit_latency:?}: more than the engine grace plus margin"
    );
    if stall_engine {
        assert!(
            exit_latency >= ENGINE_STOP_GRACE,
            "a stuck engine must be waited for, then killed: exit after {exit_latency:?}"
        );
    }
    let m = read_manifest(&manifest_path(&err));
    assert!(
        m["completion_tokens"].as_u64().unwrap() > 0,
        "cancelled run still records the tokens that arrived: {m}"
    );
    assert!(
        orphaned_engines().is_empty(),
        "engine left behind after {signal}: {:?}",
        orphaned_engines()
    );
}

/// A reader that goes away (`| head`) must not turn into a "Broken pipe" panic.
#[cfg(unix)]
#[test]
fn closed_stdout_does_not_panic() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ucompute"))
        .args(["hardware", "--json"])
        .env("UCOMPUTE_HOME", home())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn ucompute");
    drop(child.stdout.take()); // close the read end before the probe finishes writing
    let status = child.wait().unwrap();
    let mut err = String::new();
    child.stderr.take().unwrap().read_to_string(&mut err).unwrap();
    assert!(!err.contains("panicked"), "{err}");
    assert!(
        status.success() || status.code().is_none(),
        "killed by SIGPIPE or finished first, never a panic exit: {status}"
    );
}

/// The guard itself: with `UCOMPUTE_REQUIRE_FIXTURES=1` and no assets, a fixture test FAILS rather
/// than printing SKIP and passing; without it, the same test skips loudly. Re-runs this test binary
/// on `models_list_json_has_fixture_with_unknown_license` with UCOMPUTE_HOME pointing at an empty location.
#[test]
fn require_fixtures_turns_a_missing_asset_into_a_failure() {
    let empty = std::env::temp_dir().join(format!("ucompute-no-fixtures-{}", std::process::id()));
    let run = |require: &str| {
        let o = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "models_list_json_has_fixture_with_unknown_license",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("UCOMPUTE_HOME", &empty)
            .env("UCOMPUTE_REQUIRE_FIXTURES", require)
            .output()
            .unwrap();
        let out = format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        (o.status.success(), out)
    };
    let (ok, out) = run("1");
    assert!(
        !ok && out.contains("UCOMPUTE_REQUIRE_FIXTURES=1: missing asset"),
        "{out}"
    );
    let (ok, out) = run("0");
    assert!(ok && out.contains("SKIP: missing asset"), "{out}");
}

/// `ucompute setup --fixtures` pulls the `fixture` models and, under `UCOMPUTE_REQUIRE_FIXTURES=1`, the
/// `test-suite` ones too. No network: a fresh home borrows the verified engine and the fixture
/// blobs from UCOMPUTE_HOME, and an unreachable HTTPS proxy fails any download at once, so only the
/// strict run (which needs a test-suite model this home lacks) fails.
#[cfg(unix)]
#[test]
fn setup_fixtures_honours_require_fixtures() {
    let catalog: Value = serde_json::from_str(include_str!("../../../catalog/models.json")).expect("catalog is JSON");
    let tagged = |tag: &str| -> Vec<(String, String)> {
        catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| m["tags"].as_array().unwrap().iter().any(|t| t == tag))
            .map(|m| (m["id"].as_str().unwrap().into(), m["sha256"].as_str().unwrap().into()))
            .collect()
    };
    let (fixtures, suite) = (tagged("fixture"), tagged("test-suite"));
    assert!(fixtures.len() >= 2 && !suite.is_empty());
    let src = home();
    for (_, sha) in &fixtures {
        let blob = src.join(format!("models/blobs/sha256-{sha}"));
        if !blob.is_file() {
            return skip(&format!("missing asset {}", blob.display()));
        }
    }
    let fresh = std::env::temp_dir().join(format!("ucompute-setup-fixtures-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fresh);
    std::fs::create_dir_all(fresh.join("models/blobs")).unwrap();
    std::os::unix::fs::symlink(src.join("backends"), fresh.join("backends")).unwrap();
    for (_, sha) in &fixtures {
        let name = format!("models/blobs/sha256-{sha}");
        std::os::unix::fs::symlink(src.join(&name), fresh.join(&name)).unwrap();
    }
    let setup = |require: Option<&str>| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_ucompute"));
        c.args(["setup", "--fixtures"])
            .env("UCOMPUTE_HOME", &fresh)
            .env_remove("RUST_LOG")
            .env_remove("UCOMPUTE_REQUIRE_FIXTURES");
        for v in ["NO_PROXY", "no_proxy"] {
            c.env_remove(v);
        }
        for v in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
            c.env(v, "http://127.0.0.1:9");
        }
        if let Some(r) = require {
            c.env("UCOMPUTE_REQUIRE_FIXTURES", r);
        }
        c.output().expect("spawn ucompute")
    };
    let o = setup(None);
    let out = text(&o.stdout);
    assert!(o.status.success(), "{out}{}", text(&o.stderr));
    assert_eq!(out.matches("model    ").count(), fixtures.len(), "{out}");
    for (id, _) in &fixtures {
        assert!(out.contains(&format!("model    {id} verified")), "{out}");
    }
    let o = setup(Some("1"));
    let err = text(&o.stderr);
    assert!(
        !o.status.success(),
        "the strict run must also pull the test-suite models: {}",
        text(&o.stdout)
    );
    assert!(err.contains("error[download_failed]"), "{err}");
    let _ = std::fs::remove_dir_all(&fresh);
}
