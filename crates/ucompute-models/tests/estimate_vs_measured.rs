//! F-01-03 / F-01-08 evidence: `estimate()` against the pinned llama-server b11609. REAL engine and
//! REAL fixtures from UCOMPUTE_HOME (default: the repo's `.ucompute`); skips loudly when they are missing,
//! and fails instead under `UCOMPUTE_REQUIRE_FIXTURES=1` (the evidence job).
//!
//! Each combination starts the engine with `--load-mode none` (weights are anonymous memory, so
//! the measurement cannot depend on page-cache state), on the device UCompute picks by default here
//! (Metal; the tiny fixtures also on CPU), fills the context with a token prompt so the KV and
//! compute buffers are actually touched, then reads the kernel's peak for the process: macOS
//! `phys_footprint_peak` with `/usr/bin/footprint`, Linux `VmHWM` (peak resident set; with
//! `--load-mode none` the weights are resident anonymous memory). Off macOS the pinned build is
//! CPU only, so the GPU rows run once, on the CPU. The engine's own `model buffer size` /
//! `llama_kv_cache: size` / `compute buffer size` lines are the per-component truth.
//!
//! Why the footprint and not `ps rss`: on this 8 GB machine `rss` for one identical Metal
//! configuration came out anywhere between 506 and 773 MiB across runs (the compressor takes
//! pages under load, and `rss` excludes them), while the footprint counts compressed pages and
//! the kernel tracks its peak itself (769 and 770 MiB on two runs of the same configuration).
//! Not in the set: nomic-embed (no KV cache) and qwen2.5-0.5b at its trained 32768 context,
//! which matched by hand but takes ~50 s of prompt processing.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use ucompute_models::{check_fit, estimate, plan_context, read_metadata, FitVerdict, KvType};

const S260: &str = "270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d";
const S15: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
const Q4: &str = "74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db";
const Q8: &str = "ca59ca7f13d0e15a8cfa77bd17e65d24f6844b554a7b6c12e07a5f89ff76844e";
const Q2: &str = "9ee36184e616dfc76df4f5dd66f908dbde6979524ae36e6cefb67f532f798cb8";
const SM: &str = "48ab3034d0dd401fbc721eb1df3217902fee7dab9078992d66431f09b7750201";
const Q15: &str = "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e";

/// (id, sha256, requested n_ctx, kv type, gpu): 6 models, 5 quantizations, 4 contexts, 2 KV types, both devices.
const COMBOS: &[(&str, &str, u32, KvType, bool)] = &[
    ("stories260K", S260, 128, KvType::F16, true),
    ("stories260K", S260, 128, KvType::F16, false),
    ("stories15M-q4_0", S15, 128, KvType::F16, true),
    ("stories15M-q4_0", S15, 128, KvType::F16, false),
    ("qwen2.5-0.5b-instruct-q4_k_m", Q4, 4096, KvType::F16, true),
    ("qwen2.5-0.5b-instruct-q4_k_m", Q4, 4096, KvType::F16, false),
    ("qwen2.5-0.5b-instruct-q4_k_m", Q4, 4096, KvType::Q8_0, true),
    ("qwen2.5-0.5b-instruct-q4_k_m", Q4, 1024, KvType::F16, true),
    ("qwen2.5-0.5b-instruct-q8_0", Q8, 4096, KvType::F16, true),
    ("qwen2.5-0.5b-instruct-q2_k", Q2, 4096, KvType::F16, true),
    ("smollm2-360m-instruct-q8_0", SM, 4096, KvType::F16, true),
    ("smollm2-360m-instruct-q8_0", SM, 4096, KvType::F16, false),
    ("smollm2-360m-instruct-q8_0", SM, 8192, KvType::F16, true),
    ("qwen2.5-1.5b-instruct-q4_k_m", Q15, 4096, KvType::F16, true),
];

const MIB: f64 = 1024.0 * 1024.0;

fn ucompute_home() -> PathBuf {
    std::env::var_os("UCOMPUTE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.ucompute"))
}

/// A missing asset: SKIP, or a failure under `UCOMPUTE_REQUIRE_FIXTURES=1` so a green evidence run
/// always measured.
fn skip(why: &str) {
    assert!(
        std::env::var_os("UCOMPUTE_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "UCOMPUTE_REQUIRE_FIXTURES=1: {why}"
    );
    eprintln!("SKIP: {why}");
}

fn blob(sha: &str) -> Option<PathBuf> {
    let p = ucompute_home().join("models/blobs").join(format!("sha256-{sha}"));
    if p.is_file() {
        Some(p)
    } else {
        skip(&format!("fixture {} not present", p.display()));
        None
    }
}

/// One HTTP/1.1 exchange on loopback; returns the body. `Connection: close` keeps this trivial.
fn http(port: u16, key: &str, method: &str, path: &str, body: &str, timeout: Duration) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().ok()?, Duration::from_secs(1)).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    out.split_once("\r\n\r\n").map(|(_, b)| b.to_string())
}

/// The kernel's peak physical footprint of `pid` (anonymous + compressed + graphics memory), via
/// macOS's `footprint` tool: `    phys_footprint_peak: 727 MB` (binary units despite the label).
fn footprint_peak_bytes(pid: u32) -> Option<u64> {
    let out = Command::new("/usr/bin/footprint")
        .args(["-p", &pid.to_string()])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let (_, v) = text
        .lines()
        .find(|l| l.contains("phys_footprint_peak:"))?
        .split_once(':')?;
    let mut it = v.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let unit = match it.next()? {
        "KB" => 1u64 << 10,
        "MB" => 1 << 20,
        "GB" => 1 << 30,
        _ => 1,
    };
    Some((n * unit as f64) as u64)
}

/// `VmHWM:   123456 kB` from `/proc/<pid>/status` (the kernel's peak resident set) in bytes.
fn parse_vmhwm(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmHWM:"))?
        .trim()
        .strip_suffix("kB")?
        .trim()
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

/// The kernel's peak memory for `pid`, or None on an OS without a reader here (Windows).
fn peak_bytes(pid: u32) -> Option<u64> {
    if cfg!(target_os = "macos") {
        footprint_peak_bytes(pid)
    } else if cfg!(target_os = "linux") {
        parse_vmhwm(&std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?)
    } else {
        None
    }
}

/// Same flags as ucompute-runtime's `process::build_args`, plus `--load-mode none`.
fn args(model: &Path, port: u16, key: &str, n_ctx: u32, kv: KvType, gpu: bool) -> Vec<String> {
    let mut args: Vec<String> = [
        "-m",
        &model.display().to_string(),
        "--host",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--api-key",
        key,
        "-c",
        &n_ctx.to_string(),
        "-t",
        "4",
        "-np",
        "1",
        "-ctk",
        kv.as_llama_arg(),
        "-ctv",
        kv.as_llama_arg(),
        "--metrics",
        "--no-webui",
        "--offline",
        "--log-timestamps",
        "--log-colors",
        "off",
        "-lv",
        "4",
        "--fit",
        "off",
        "--load-mode",
        "none",
    ]
    .map(String::from)
    .to_vec();
    if gpu {
        args.extend(["-ngl", "999"].map(String::from));
    } else {
        args.extend(["-ngl", "0", "--device", "none", "--no-kv-offload", "--no-op-offload"].map(String::from));
    }
    args
}

/// Same environment scrub as ucompute-runtime's `process::spawn`; stdout+stderr become the log.
fn spawn(engine: &Path, args: &[String], log: &Path) -> Child {
    let file = std::fs::File::create(log).unwrap();
    let env = std::env::vars_os().filter(|(k, _)| {
        let k = k.to_string_lossy();
        !["LLAMA_ARG_", "GGML_", "DYLD_", "LD_"].iter().any(|p| k.starts_with(p))
    });
    Command::new(engine)
        .args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(file.try_clone().unwrap())
        .stderr(file)
        .spawn()
        .expect("spawn llama-server")
}

/// `… size = 12.34 MiB …` → bytes; sums every matching line (ggml always logs buffer sizes in MiB).
fn log_mib_sum(log: &str, needle: &str) -> f64 {
    log.lines()
        .filter(|l| l.contains(needle))
        .filter_map(|l| {
            l.split_once("= ")
                .and_then(|(_, v)| v.split_whitespace().next())
                .and_then(|n| n.parse::<f64>().ok())
        })
        .sum::<f64>()
        * MIB
}

fn log_u64(log: &str, needle: &str) -> Option<u64> {
    log.lines()
        .find(|l| l.contains(needle))
        .and_then(|l| l.rsplit_once("= "))
        .and_then(|(_, v)| v.trim().parse().ok())
}

struct Row {
    label: String,
    est: ucompute_models::MemoryEstimate,
    cells: u64,
    weights: f64,
    kv: f64,
    compute: f64,
    peak: f64,
    prompt_n: u64,
}

fn measure(engine: &Path, id: &str, model: &Path, n_ctx: u32, kv: KvType, gpu: bool) -> Row {
    let meta = read_metadata(model).unwrap();
    let file_size = std::fs::metadata(model).unwrap().len();
    let est = estimate(&meta, file_size, n_ctx, kv);
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let key = format!(
        "{:x}{:x}",
        std::process::id(),
        Instant::now().elapsed().as_nanos() ^ port as u128
    );
    let label = format!(
        "{id} ctx={n_ctx} kv={} {}",
        kv.as_llama_arg(),
        if gpu { "metal" } else { "cpu" }
    );
    let log = std::env::temp_dir().join(format!(
        "ucompute-models-evidence-{}-{}.log",
        std::process::id(),
        label.replace([' ', '='], "-")
    ));
    let mut child = spawn(engine, &args(model, port, &key, n_ctx, kv, gpu), &log);
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            panic!(
                "{label}: llama-server exited with {status} before /health; log at {}",
                log.display()
            );
        }
        if http(port, &key, "GET", "/health", "", Duration::from_secs(1)).is_some_and(|b| b.contains("\"ok\"")) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: not ready within 180 s; log at {}",
            log.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Fill the context (the slot's usable size is the requested n_ctx) so KV and compute are touched.
    let ids = vec!["100"; n_ctx as usize - 40].join(",");
    let body = format!("{{\"prompt\":[{ids}],\"n_predict\":16,\"temperature\":0,\"seed\":42,\"cache_prompt\":false}}");
    let resp = http(port, &key, "POST", "/completion", &body, Duration::from_secs(600)).unwrap_or_default();
    let peak = peak_bytes(pid).unwrap_or_else(|| panic!("{label}: no peak memory reading for pid {pid}"));
    let _ = child.kill();
    let _ = child.wait();
    let prompt_n = resp
        .split_once("\"prompt_n\":")
        .and_then(|(_, r)| {
            r.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or_else(|| panic!("{label}: no timings in response: {}", &resp[..resp.len().min(300)]));
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let _ = std::fs::remove_file(&log);
    Row {
        label,
        est,
        cells: log_u64(&text, "llama_context: n_ctx ").unwrap_or(0),
        weights: log_mib_sum(&text, "model buffer size"),
        kv: log_mib_sum(&text, "llama_kv_cache: size"),
        compute: log_mib_sum(&text, "compute buffer size"),
        peak: peak as f64,
        prompt_n,
    }
}

fn pct(est: u64, measured: f64) -> f64 {
    (est as f64 - measured) / measured * 100.0
}

#[test]
fn estimate_matches_engine_log_and_peak_footprint() {
    let engine = ucompute_home().join("backends/llama.cpp/b11609/llama-server");
    if !engine.is_file() {
        return skip(&format!("pinned llama-server missing at {}", engine.display()));
    }
    if cfg!(target_os = "macos") && !Path::new("/usr/bin/footprint").is_file() {
        return skip("/usr/bin/footprint (macOS) is needed to read the kernel's peak footprint");
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        return skip("no peak-memory reader for this OS (macOS footprint, Linux VmHWM)");
    }
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &(id, sha, n_ctx, kv, gpu) in COMBOS {
        // Only the macOS pin has a GPU backend (Metal); elsewhere a GPU row is its CPU twin.
        let gpu = gpu && cfg!(target_os = "macos");
        if !seen.insert((id, n_ctx, kv.as_llama_arg(), gpu)) {
            continue;
        }
        let Some(model) = blob(sha) else { continue };
        rows.push(measure(&engine, id, &model, n_ctx, kv, gpu));
    }
    eprintln!("\nF-01-03/F-01-08 evidence (llama-server b11609, --load-mode none, context filled; MiB; delta = estimate vs measured)");
    eprintln!(
        "{:<52} {:>8} {:>8} {:>7} {:>8} {:>7} {:>8} {:>7} {:>8} {:>8} {:>7}",
        "combination", "w_est", "w_eng", "kv_est", "kv_eng", "c_est", "c_eng", "tot_est", "peak", "d_kv%", "d_tot%"
    );
    let mut failures = Vec::new();
    for r in &rows {
        let (d_w, d_kv, d_c, d_tot) = (
            pct(r.est.weights_bytes, r.weights),
            pct(r.est.kv_cache_bytes, r.kv),
            pct(r.est.compute_buffer_bytes, r.compute),
            pct(r.est.total_bytes, r.peak),
        );
        eprintln!(
            "{:<52} {:>8.1} {:>8.1} {:>7.1} {:>8.1} {:>7.1} {:>8.1} {:>7.1} {:>8.1} {:>+7.1} {:>+7.1}",
            r.label,
            r.est.weights_bytes as f64 / MIB,
            r.weights / MIB,
            r.est.kv_cache_bytes as f64 / MIB,
            r.kv / MIB,
            r.est.compute_buffer_bytes as f64 / MIB,
            r.compute / MIB,
            r.est.total_bytes as f64 / MIB,
            r.peak / MIB,
            d_kv,
            d_tot
        );
        let mut check = |ok: bool, what: String| {
            if !ok {
                failures.push(format!("{}: {what}", r.label))
            }
        };
        check(
            r.prompt_n + 40 == u64::from(r.est.n_ctx),
            format!("context not filled: prompt_n {}", r.prompt_n),
        );
        check(
            r.cells == u64::from(r.est.n_ctx).div_ceil(256) * 256,
            format!("engine allocated {} cells", r.cells),
        );
        check(d_w.abs() <= 3.0, format!("weights off by {d_w:+.1}%"));
        check(
            d_kv.abs() <= 10.0,
            format!("KV off by {d_kv:+.1}% (F-01-08 allows 10%)"),
        );
        // the compute term is a few percent of the total; it is printed as evidence, not asserted
        let _ = d_c;
        check(
            d_tot.abs() <= 15.0,
            format!("total off by {d_tot:+.1}% of the measured peak (F-01-03 allows 15%)"),
        );
    }
    if rows.len() < 5 {
        eprintln!(
            "WARNING: only {} of {} combinations could run (fixtures missing); F-01-03 needs >= 5",
            rows.len(),
            COMBOS.len()
        );
    }
    assert!(
        failures.is_empty(),
        "estimate outside tolerance:\n{}",
        failures.join("\n")
    );
}

#[test]
fn vmhwm_parses_proc_status() {
    let status = "Name:\tllama-server\nVmPeak:\t 2345678 kB\nVmHWM:\t  812345 kB\nVmRSS:\t  800000 kB\n";
    assert_eq!(parse_vmhwm(status), Some(812_345 * 1024));
    assert_eq!(parse_vmhwm("VmRSS:\t1 kB\n"), None);
    assert_eq!(parse_vmhwm("VmHWM:\t12 MB\n"), None, "the kernel always reports kB");
    #[cfg(target_os = "linux")]
    assert!(peak_bytes(std::process::id()).unwrap() > 1 << 20);
}

/// The refusal path with a real GGUF header: a budget below the fixed cost is a typed
/// `InsufficientMemory` with sizes in plain language, and `check_fit` names what is missing.
#[test]
fn real_fixture_refusal_reasons() {
    let Some(model) = blob(S15) else { return };
    let meta = read_metadata(&model).unwrap();
    let size = std::fs::metadata(&model).unwrap().len();
    let est = estimate(&meta, size, 128, KvType::F16);
    assert_eq!(
        est.kv_cache_bytes,
        256 * 6912,
        "stories15M reserves 256 cells for a 128-token context"
    );
    let FitVerdict::TooLarge {
        reason,
        needed_bytes,
        available_bytes,
    } = check_fit(&est, est.total_bytes - 1, 0.0)
    else {
        panic!("expected TooLarge")
    };
    assert_eq!((needed_bytes, available_bytes), (est.total_bytes, est.total_bytes - 1));
    assert!(
        reason.starts_with("This model needs about ") && reason.contains(" MB but only "),
        "{reason}"
    );
    let e = plan_context(&meta, size, Some(128), est.total_bytes / 2, true).unwrap_err();
    assert_eq!(e.code, ucompute_core::ErrorCode::InsufficientMemory);
    assert!(
        e.message.contains("minimum context of 128") && e.message.contains(" MB"),
        "{}",
        e.message
    );
    assert!(matches!(
        check_fit(&est, est.total_bytes * 2, 0.10),
        FitVerdict::Fits { .. }
    ));
}

/// The guard itself: with `UCOMPUTE_REQUIRE_FIXTURES=1` and no assets, a fixture test FAILS rather
/// than printing SKIP and passing; without it, the same test skips loudly. Re-runs this test binary
/// on `real_fixture_refusal_reasons` with UCOMPUTE_HOME pointing at an empty location.
#[test]
fn require_fixtures_turns_a_missing_asset_into_a_failure() {
    let empty = std::env::temp_dir().join(format!("ucompute-no-fixtures-{}", std::process::id()));
    let run = |require: &str| {
        let o = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "real_fixture_refusal_reasons",
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
    assert!(!ok && out.contains("UCOMPUTE_REQUIRE_FIXTURES=1: fixture"), "{out}");
    let (ok, out) = run("0");
    assert!(ok && out.contains("SKIP: fixture"), "{out}");
}
