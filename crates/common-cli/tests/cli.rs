//! Runs the built `common` binary against the real assets in COMMON_HOME (default: the repo's
//! `.common`). Tests that need the pinned engine or the fixture model skip loudly when absent.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MODEL: &str = "stories15M-q4_0";
const BLOB: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";

fn home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

fn assets_present() -> bool {
    let engine = home().join("backends/llama.cpp/b11609/llama-server");
    let blob = home().join(format!("models/blobs/sha256-{BLOB}"));
    for p in [&engine, &blob] {
        if !p.is_file() {
            eprintln!("SKIP: missing asset {}", p.display());
            return false;
        }
    }
    true
}

fn common(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_common"))
        .args(args)
        .env("COMMON_HOME", home())
        .env_remove("RUST_LOG")
        .output()
        .expect("spawn common")
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
    let o = common(&["hardware", "--json"]);
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

#[test]
fn models_list_json_has_fixture_with_unknown_license() {
    if !assets_present() {
        return;
    }
    let o = common(&["models", "list", "--json"]);
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
    let o = common(&[
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
    let hw = json(&common(&["hardware", "--json"]));
    if !hw["backends"]
        .as_array()
        .unwrap()
        .iter()
        .any(|b| b["available"] == true && b["name"] != "cpu")
    {
        eprintln!("SKIP: no GPU backend on this host");
        return;
    }
    let o = common(&[
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
    let o = common(&["run", "nosuchmodel", "-p", "x"]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(err.contains("error[model_not_found]") && err.contains("hint:"), "{err}");
    assert!(o.stdout.is_empty());

    let o = common(&["run", "nosuchmodel", "-p", "x", "--json"]);
    assert_eq!(o.status.code(), Some(1));
    let e: Value = serde_json::from_str(text(&o.stderr).trim()).expect("json error on stderr");
    assert_eq!(e["code"], "model_not_found");
}

#[test]
fn inspect_json_reports_block_count() {
    if !assets_present() {
        return;
    }
    let o = common(&["models", "inspect", MODEL, "--json"]);
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
    let o = common(&[
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

    let o = common(&["replay", path.to_str().unwrap(), "--json"]);
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
    let o = common(&["replay", path.to_str().unwrap(), "--json"]);
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
    let o = common(&[
        "run", MODEL, "-p", PROMPT, "-n", "4", "--temp", "0", "--seed", "42", "--device", "cpu",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let path = manifest_path(&text(&o.stderr));
    let o = common(&["replay", path.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let err = text(&o.stderr);
    assert!(
        err.contains("error[invalid_request]") && err.contains("--save-prompt"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// Detached engines (ppid 1) started from our pinned binary; `ps` zombie rows show `(llama-server)` and never match.
#[cfg(unix)]
fn orphaned_engines() -> Vec<String> {
    let bin = home().join("backends/llama.cpp/b11609/llama-server");
    let out = Command::new("ps").args(["-axo", "ppid=,args="]).output().unwrap();
    text(&out.stdout)
        .lines()
        .filter(|l| {
            let mut f = l.split_whitespace();
            f.next() == Some("1") && f.next().is_some_and(|a| Path::new(a) == bin)
        })
        .map(str::to_string)
        .collect()
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

/// SIGTERM to `common serve` with a model loaded stops that engine process before exiting.
#[cfg(unix)]
#[test]
fn sigterm_to_serve_stops_the_loaded_engine() {
    use std::io::Write;
    use std::net::TcpStream;
    if !assets_present() {
        return;
    }
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_common"))
        .args(["serve", "--port", &port.to_string()])
        .env("COMMON_HOME", home())
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn common serve");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut conn = loop {
        if let Ok(c) = TcpStream::connect(("127.0.0.1", port)) {
            break c;
        }
        assert!(Instant::now() < deadline, "serve did not listen on {port} within 30 s");
        assert!(child.try_wait().unwrap().is_none(), "serve exited early");
        std::thread::sleep(Duration::from_millis(100));
    };
    conn.set_read_timeout(Some(Duration::from_secs(120))).unwrap();
    write!(conn, "POST /api/models/{MODEL}/load HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
    let mut resp = String::new();
    conn.read_to_string(&mut resp).unwrap();
    if resp.contains("insufficient_memory") {
        eprintln!("SKIP: not enough free memory to load {MODEL} right now");
        let _ = child.kill();
        return;
    }
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
    let body: Value = serde_json::from_str(resp.split("\r\n\r\n").nth(1).unwrap_or(""))
        .unwrap_or_else(|e| panic!("load body is not JSON ({e}): {resp}"));
    let engine_pid = body["pid"].as_u64().expect("engine pid in LoadedModelInfo") as u32;
    assert!(pid_alive(engine_pid), "engine {engine_pid} running after load");

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
        eprintln!("SKIP: {SLOW} is not downloaded");
        return;
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_common"))
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
        .env("COMMON_HOME", home())
        .env_remove("RUST_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn common");
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
                eprintln!("SKIP: not enough free memory to load {SLOW} right now: {}", err.trim());
                return;
            }
            panic!("common exited with {status} before streaming any token: {err}");
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
    let mut child = Command::new(env!("CARGO_BIN_EXE_common"))
        .args(["hardware", "--json"])
        .env("COMMON_HOME", home())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn common");
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
