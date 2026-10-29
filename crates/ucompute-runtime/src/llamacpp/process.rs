//! Engine process plumbing: argument building, environment scrubbing, port choice, spawn,
//! readiness polling and graceful stop. Nothing here sees prompts or tokens.

use super::{logparse, split};
use crate::types::LoadRequest;
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use ucompute_core::{ErrorCode, Result, UComputeError};

/// Test hook: when set to `1`, the GPU attempt names a device that does not exist, so the engine
/// really fails to start (exit code 1, usage text in the log). Exercises the Auto→CPU fallback and
/// the `Gpu` → `GpuUnavailable` path with a real process failure instead of a mock.
pub const FORCE_GPU_FAIL_ENV: &str = "UCOMPUTE_TEST_FORCE_GPU_FAIL";

/// Env prefixes that could redirect the engine's dynamic libraries, devices or arguments.
const SCRUBBED_ENV_PREFIXES: [&str; 4] = ["LLAMA_ARG_", "GGML_", "DYLD_", "LD_"];

/// Flags UCompute owns; `extra_args` may not set them. Every spelling b11609's `llama-server --help` lists:
/// loopback bind, secret, log; the model and every other way to load or download one (integrity); device,
/// offload, split and RPC placement (WP-04 / ADR-0005 D1: `--rpc` alone would reach a raw rpc-server on a LAN
/// address, and `-ot` can place tensors on an RPC device); verbosity (the placement and GPU proof lines need
/// UCompute's level, and unbounded verbosity may log prompt text).
const RESERVED_FLAGS: &[&str] = &[
    // bind, secret, log
    "--host",
    "--port",
    "--api-key",
    "--api-key-file",
    "--log-file",
    // model sources
    "-m",
    "--model",
    "-mu",
    "--model-url",
    "-hf",
    "-hfr",
    "--hf-repo",
    "-hff",
    "--hf-file",
    "-hft",
    "--hf-token",
    "-dr",
    "--docker-repo",
    "-mm",
    "--mmproj",
    "-md",
    "--model-draft",
    "--spec-draft-model",
    "-hfd",
    "-hfrd",
    "--hf-repo-draft",
    "--spec-draft-hf",
    // placement: devices, offload, split, RPC
    "--rpc",
    "-dev",
    "--device",
    "--list-devices",
    "-ts",
    "--tensor-split",
    "-sm",
    "--split-mode",
    "-mg",
    "--main-gpu",
    "-ngl",
    "--gpu-layers",
    "--n-gpu-layers",
    "-ot",
    "--override-tensor",
    "-cmoe",
    "--cpu-moe",
    "-ncmoe",
    "--n-cpu-moe",
    "--moe-cache-mib",
    "-ncffn",
    "--n-cpu-ffn",
    "--override-kv",
    "-fit",
    "--fit",
    "-fitt",
    "--fit-target",
    "-fitc",
    "--fit-ctx",
    "-lm",
    "--load-mode",
    "-kvo",
    "--kv-offload",
    "-nkvo",
    "--no-kv-offload",
    "--op-offload",
    "--no-op-offload",
    "-mmdev",
    "--mmproj-device",
    "-devd",
    "--device-draft",
    "--spec-draft-device",
    "-ngld",
    "--gpu-layers-draft",
    "--n-gpu-layers-draft",
    "--spec-draft-ngl",
    "-otd",
    "--override-tensor-draft",
    "--spec-draft-override-tensor",
    "-cmoed",
    "--cpu-moe-draft",
    "--spec-draft-cpu-moe",
    "-ncmoed",
    "--n-cpu-moe-draft",
    "--spec-draft-ncmoe",
    "--spec-draft-n-cpu-moe",
    // verbosity
    "-lv",
    "--verbosity",
    "--log-verbosity",
    "-v",
    "--verbose",
    "--log-verbose",
];

pub fn force_gpu_fail() -> bool {
    std::env::var_os(FORCE_GPU_FAIL_ENV).is_some_and(|v| v == "1")
}

/// Each extra arg must be one shell-free token and must not touch a reserved flag.
pub fn validate_extra_args(args: &[String]) -> Result<()> {
    for a in args {
        if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '\0') {
            return Err(UComputeError::invalid(format!(
                "extra engine arg must be one non-empty token without whitespace: {a:?}"
            )));
        }
        let flag = a.split_once('=').map_or(a.as_str(), |(f, _)| f);
        if RESERVED_FLAGS.contains(&flag) {
            return Err(UComputeError::invalid(format!(
                "extra engine arg {flag} is managed by UCompute"
            )));
        }
    }
    Ok(())
}

/// Command line for one `llama-server` start. `-lv 4` is required: the GPU proof lines
/// (`offloaded N/M layers to GPU`, `using device MTL0 (...)`) are not printed at the default
/// verbosity. Verified on b11609 that level 4 never writes prompt text to the log.
/// The API key goes through `--api-key-file` (a 0600 file, see [`write_key_file`]), never argv:
/// argv is readable by every local user through the process table.
/// A split load (`req.split`) takes the split branch instead of the GPU or CPU one, so none of
/// `--device none`, `--no-kv-offload`, `--no-op-offload` or the force-gpu-fail device reach it.
pub fn build_args(req: &LoadRequest, port: u16, api_key_file: &Path, gpu: bool) -> Result<Vec<String>> {
    let kv = req.kv_type.as_llama_arg();
    let mut a: Vec<String> = vec![
        "-m".into(),
        req.model_path.display().to_string(),
        "--host".into(),
        "127.0.0.1".into(),
        "--port".into(),
        port.to_string(),
        "--api-key-file".into(),
        api_key_file.display().to_string(),
        "-c".into(),
        req.n_ctx.to_string(),
        "-t".into(),
        req.threads.unwrap_or(4).to_string(),
        "-np".into(),
        req.parallel_slots.to_string(),
        "-ctk".into(),
        kv.into(),
        "-ctv".into(),
        kv.into(),
        "--metrics".into(),
        "--no-webui".into(),
        "--offline".into(),
        "--log-timestamps".into(),
        "--log-colors".into(),
        "off".into(),
        "-lv".into(),
        "4".into(),
        // UCompute plans n_ctx itself; never let the engine silently resize it (no silent substitution).
        "--fit".into(),
        "off".into(),
    ];
    if let Some(s) = &req.split {
        a.extend(split::launch_args(s).map_err(|e| UComputeError::invalid(format!("split plan: {e}")))?);
    } else if gpu {
        a.extend(["-ngl".to_string(), req.n_gpu_layers.unwrap_or(999).to_string()]);
        if force_gpu_fail() {
            a.extend(["--device".to_string(), "UCOMPUTE_TEST_NO_SUCH_DEVICE".to_string()]);
        }
    } else {
        a.extend(["-ngl", "0", "--device", "none", "--no-kv-offload", "--no-op-offload"].map(String::from));
    }
    if req.embeddings {
        // Pooling comes from the model file (verified: nomic-embed reports pooling_type=1 without --pooling).
        a.push("--embeddings".into());
    }
    a.extend(req.extra_args.iter().cloned());
    Ok(a)
}

/// The engine's environment: the scrubbed parent environment and, for a split load,
/// `GGML_RPC_NO_RDMA=1` so RPC stays on TCP (ADR-0005 D2). The scrub drops every inherited `GGML_`
/// variable, so this is the only one the engine sees.
pub fn engine_environment(req: &LoadRequest) -> Vec<(OsString, OsString)> {
    let mut env = clean_environment(std::env::vars_os());
    if req.split.is_some() {
        env.push(("GGML_RPC_NO_RDMA".into(), "1".into()));
    }
    env
}

/// Drop env vars that could redirect the engine's libraries, devices or arguments.
pub fn clean_environment<I>(env: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    env.into_iter()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            !SCRUBBED_ENV_PREFIXES.iter().any(|p| k.starts_with(p))
        })
        .collect()
}

/// Write the per-process API key to `path`, readable by the owner only (0600 on unix). The engine
/// reads it once at startup, so the caller deletes it as soon as readiness is known.
pub fn write_key_file(path: &Path, key: &str) -> Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    std::io::Write::write_all(&mut opts.open(path)?, format!("{key}\n").as_bytes())?;
    Ok(())
}

/// Bind 127.0.0.1:0, read the port, release it.
/// ponytail: tiny race until the engine binds; `attempt` retries once when the log says the bind failed.
pub async fn free_port() -> Result<u16> {
    let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    Ok(l.local_addr()?.port())
}

/// Spawn the engine with stdout+stderr redirected to `log_path` and exactly the environment `env`
/// (see [`engine_environment`]).
/// The engine writes its whole log to stderr, so one redirected handle is the log; `--log-file`
/// would be a second writer on the same file.
pub fn spawn(binary: &Path, args: &[String], env: Vec<(OsString, OsString)>, log_path: &Path) -> Result<Child> {
    let log = std::fs::File::create(log_path)?;
    let stderr = log.try_clone()?;
    let mut cmd = Command::new(binary);
    // Windows: the desktop app starts the daemon without a console; a console child would then get a
    // console window of its own popping up over the app.
    #[cfg(windows)]
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    cmd.args(args)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr)
        .kill_on_drop(true)
        .spawn()
        .inspect(|_child| {
            #[cfg(windows)]
            end_with_this_process(_child);
        })
        .map_err(|e| {
            UComputeError::new(
                ErrorCode::BackendUnavailable,
                format!("cannot start {}: {e}", binary.display()),
            )
        })
}

/// Windows: put the engine in a job object that ends every process in it when its last handle
/// closes. Only this process holds that handle, and Windows closes it when this process exits for
/// any reason, including TerminateProcess, so a killed daemon can no longer leave `llama-server.exe`
/// running with the model resident (observed on the HP test laptop). The Unix counterpart is the
/// graceful stop path plus the orphan reaper at the next start.
#[cfg(windows)]
fn end_with_this_process(child: &Child) {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // HANDLE is a raw pointer (not Sync); the job lives for the whole process, so keep it as usize.
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| {
        // SAFETY: plain Win32 calls; `info` is a zeroed POD struct passed with its exact size.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return 0;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return 0;
            }
            job as usize
        }
    });
    let assigned = job != 0
        && child.raw_handle().is_some_and(|h| {
            // SAFETY: `h` is the live process handle tokio owns for this child; `job` is ours.
            unsafe { AssignProcessToJobObject(job as _, h as _) != 0 }
        });
    if !assigned {
        tracing::warn!("could not tie the engine to this process; it may outlive a killed daemon");
    }
}

/// In the error of an engine that exited during its load; the manager counts these toward its
/// restart bound.
pub const EXITED_BEFORE_READY: &str = "before becoming ready";

/// Poll `/health` every 100 ms until `{"status":"ok"}`. Exit before readiness → BackendFailed or
/// ModelFileInvalid carrying the scrubbed log tail; `deadline` passed → Timeout. The caller stops
/// the child on Err.
pub async fn wait_ready(
    child: &mut Child,
    client: &reqwest::Client,
    port: u16,
    deadline: Instant,
    log_path: &Path,
    api_key: &str,
) -> Result<()> {
    let url = format!("http://127.0.0.1:{port}/health");
    loop {
        if let Some(status) = child.try_wait()? {
            let tail = logparse::scrub(&logparse::tail(&read_log(log_path), 40), api_key);
            return Err(UComputeError::new(
                logparse::exit_error_code(&tail),
                format!("llama-server exited with {status} {EXITED_BEFORE_READY}:\n{tail}"),
            ));
        }
        if let Ok(resp) = client.get(&url).timeout(Duration::from_secs(1)).send().await {
            if resp.status().is_success() {
                if let Ok(v) = resp.json::<serde_json::Value>().await {
                    if v["status"] == "ok" {
                        return Ok(());
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(UComputeError::new(
                ErrorCode::Timeout,
                "llama-server did not become ready within the load timeout",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The engine log, decoded leniently: at `-lv 4` the engine echoes GGUF strings, which need not be
/// UTF-8, and one bad byte must not erase the GPU proof or the error tail. Missing file = empty.
pub fn read_log(path: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).into_owned()
}

/// SIGTERM, wait up to 2 s, then SIGKILL. Consumes the child, so it cannot run twice.
pub async fn stop(mut child: Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: plain syscall on a pid we spawned; worst case ESRCH.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(2), child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
}

/// True when a process with `pid` still exists (zombies count until reaped).
pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks existence.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// Resident set size via `ps` (KiB → bytes). None off unix or when `ps` fails.
pub fn rss_bytes(pid: u32) -> Option<u64> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("ps")
            .args(["-o", "rss=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .ok()
            .map(|kib| kib * 1024)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        None
    }
}

/// `llama-server --version`, e.g. `0.6.0-dev (build 11609, commit fcac19079)`.
pub fn version(binary: &Path) -> Result<String> {
    let out = std::process::Command::new(binary)
        .arg("--version")
        .env_clear()
        .envs(clean_environment(std::env::vars_os()))
        .output()
        .map_err(|e| {
            UComputeError::new(
                ErrorCode::BackendUnavailable,
                format!("cannot run {}: {e}", binary.display()),
            )
        })?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let line = text
        .lines()
        .find(|l| l.contains("version"))
        .or_else(|| text.lines().next())
        .unwrap_or("")
        .trim();
    let v = line.strip_prefix("version:").unwrap_or(line).trim().to_string();
    if v.is_empty() {
        return Err(UComputeError::new(
            ErrorCode::BackendUnavailable,
            format!("{} --version printed nothing", binary.display()),
        ));
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::DevicePreference;
    use std::path::PathBuf;
    use ucompute_core::Timeouts;

    fn req() -> LoadRequest {
        LoadRequest {
            model_id: "m".into(),
            model_path: PathBuf::from("/tmp/m.gguf"),
            model_sha256: String::new(),
            device: DevicePreference::Auto,
            n_ctx: 128,
            n_gpu_layers: None,
            threads: None,
            parallel_slots: 2,
            kv_type: ucompute_models::KvType::Q8_0,
            embeddings: false,
            timeouts: Timeouts::default(),
            extra_args: vec![],
            split: None,
        }
    }

    fn has(args: &[String], pair: &[&str]) -> bool {
        args.windows(pair.len())
            .any(|w| w.iter().zip(pair).all(|(a, b)| a == b))
    }

    const KEY_FILE: &str = "/tmp/engine.key";

    #[test]
    fn cpu_args() {
        let a = build_args(&req(), 4242, Path::new(KEY_FILE), false).unwrap();
        assert!(has(&a, &["-m", "/tmp/m.gguf"]));
        assert!(has(&a, &["--host", "127.0.0.1"]));
        assert!(has(&a, &["--port", "4242"]));
        assert!(has(&a, &["--api-key-file", KEY_FILE]));
        assert!(!a.iter().any(|x| x == "--api-key"), "the secret never goes on argv");
        assert!(has(&a, &["-c", "128"]));
        assert!(has(&a, &["-t", "4"]), "threads default to 4");
        assert!(has(&a, &["-np", "2"]));
        assert!(has(&a, &["-ctk", "q8_0", "-ctv", "q8_0"]));
        assert!(has(
            &a,
            &["-ngl", "0", "--device", "none", "--no-kv-offload", "--no-op-offload"]
        ));
        assert!(has(&a, &["-lv", "4"]) && has(&a, &["--fit", "off"]) && has(&a, &["--log-colors", "off"]));
        for f in ["--metrics", "--no-webui", "--offline", "--log-timestamps"] {
            assert!(a.iter().any(|x| x == f), "{f}");
        }
        assert!(!a.iter().any(|x| x == "--embeddings"));
    }

    #[test]
    fn gpu_and_embeddings_args() {
        let mut r = req();
        r.threads = Some(6);
        r.embeddings = true;
        let a = build_args(&r, 1, Path::new(KEY_FILE), true).unwrap();
        assert!(has(&a, &["-ngl", "999"]), "all layers by default");
        assert!(!a.iter().any(|x| x == "--device"), "no device override unless forced");
        assert!(has(&a, &["-t", "6"]));
        assert!(a.iter().any(|x| x == "--embeddings"));
        r.n_gpu_layers = Some(3);
        assert!(has(
            &build_args(&r, 1, Path::new(KEY_FILE), true).unwrap(),
            &["-ngl", "3"]
        ));
    }

    #[test]
    fn key_file_is_owner_only_and_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("engine.key");
        write_key_file(&p, "s3cret").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "s3cret\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert_eq!(
            write_key_file(&p, "other").unwrap_err().code,
            ErrorCode::Io,
            "create_new: an existing file is never clobbered"
        );
    }

    #[test]
    fn extra_args_are_appended_after_validation() {
        let mut r = req();
        r.extra_args = vec!["-ub".into(), "256".into()];
        assert!(validate_extra_args(&r.extra_args).is_ok());
        let a = build_args(&r, 1, Path::new(KEY_FILE), false).unwrap();
        assert_eq!(&a[a.len() - 2..], &["-ub", "256"]);
        for bad in ["--rpc 1.2.3.4:1", "a\nb", "", "--host", "--api-key=x", "--port"] {
            let e = validate_extra_args(&[bad.to_string()]).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest, "{bad:?}");
        }
    }

    /// WP-04 C1 (ADR-0005 D1): no split, RPC, placement, model-source or verbosity flag reaches the engine
    /// through `extra_args`, in the two-token or the `--flag=value` form.
    #[test]
    fn extra_args_cannot_set_split_flags() {
        let split = [
            "--rpc",
            "-dev",
            "--device",
            "-ts",
            "--tensor-split",
            "-sm",
            "--split-mode",
            "-mg",
            "--main-gpu",
            "-ngl",
            "--gpu-layers",
            "--n-gpu-layers",
            "-ot",
            "--override-tensor",
            "-fit",
            "--fit",
            "--list-devices",
            "-lm",
            "--load-mode",
            "-lv",
            "--verbosity",
            "-v",
            "-md",
            "--model-draft",
            "-hf",
            "--model-url",
            "-devd",
            "-ngld",
            "-otd",
            "-cmoe",
            "--cpu-moe",
            "-ncmoe",
            "--n-cpu-moe",
            "--moe-cache-mib",
            "-ncffn",
            "--n-cpu-ffn",
            "--override-kv",
        ];
        for flag in split {
            for args in [vec![flag.to_string(), "x".to_string()], vec![format!("{flag}=x")]] {
                let e = validate_extra_args(&args).unwrap_err();
                assert_eq!(e.code, ErrorCode::InvalidRequest, "{args:?}");
            }
        }
        assert!(RESERVED_FLAGS.iter().all(|f| f.starts_with('-') && !f.contains('=')));
    }

    /// Configuration P on stories15M (6 blocks): the RPC stage holds [0,3), MTL0 [3,6) plus output.
    fn split_req(ports: Vec<u16>, stages: &[(&str, u32, u32)]) -> LoadRequest {
        let mut r = req();
        r.split = Some(crate::types::Split {
            rpc_ports: ports,
            stages: stages
                .iter()
                .map(|&(d, a, b)| crate::types::SplitStage {
                    device: d.into(),
                    first_block: a,
                    end_block: b,
                })
                .collect(),
            n_layer: 6,
            no_mmap: false,
            engine_pid: Default::default(),
        });
        r
    }

    fn value_of<'a>(args: &'a [String], flag: &str) -> Vec<&'a str> {
        args.windows(2)
            .filter(|w| w[0] == flag)
            .map(|w| w[1].as_str())
            .collect()
    }

    /// WP-04 C2: the split branch carries the lab's mapping unchanged (`split::rpc_split_args`, whose
    /// placement the engine's own `-lv 5` lines confirmed on b11609), with `-ngl 999` and `--fit off`.
    #[test]
    fn split_args_follow_the_lab_mapping() {
        let mut r = split_req(vec![50052], &[("RPC0", 0, 3), ("MTL0", 3, 6)]);
        r.n_gpu_layers = Some(3);
        let a = build_args(&r, 1, Path::new(KEY_FILE), false).unwrap();
        let lab = split::rpc_split_args(&[("RPC0", 0, 3), ("MTL0", 3, 6)], 6).unwrap();
        assert!(has(&a, &lab.iter().map(String::as_str).collect::<Vec<_>>()), "{a:?}");
        assert_eq!(value_of(&a, "--device"), ["RPC0,MTL0"]);
        assert_eq!(value_of(&a, "--split-mode"), ["layer"]);
        assert_eq!(
            value_of(&a, "-ngl"),
            ["999"],
            "every layer offloaded; n_gpu_layers is not a split input"
        );
        assert_eq!(
            value_of(&a, "--fit"),
            ["off"],
            "with fit on, llama.cpp may move layers itself"
        );
        let w: Vec<f32> = value_of(&a, "--tensor-split")[0]
            .split(',')
            .map(|x| x.parse().unwrap())
            .collect();
        assert_eq!(
            split::llama_assign(&w, 6),
            [0, 0, 0, 1, 1, 1, 1],
            "output layer with MTL0"
        );
        assert!(!a.iter().any(|x| x == "-lm"), "mmap unless asked");
        r.split.as_mut().unwrap().no_mmap = true;
        let a = build_args(&r, 1, Path::new(KEY_FILE), false).unwrap();
        assert_eq!(value_of(&a, "-lm"), ["none"], "b11609 has no --no-mmap");
        assert!(!a.iter().any(|x| x == "--no-mmap"));
    }

    #[test]
    fn rpc_endpoints_render_loopback_only() {
        let r = split_req(vec![50052, 50053], &[("RPC0", 0, 2), ("RPC1", 2, 4), ("MTL0", 4, 6)]);
        let a = build_args(&r, 1, Path::new(KEY_FILE), true).unwrap();
        assert_eq!(value_of(&a, "--rpc"), ["127.0.0.1:50052,127.0.0.1:50053"]);
        for ep in value_of(&a, "--rpc")[0].split(',') {
            let addr: std::net::SocketAddr = ep.parse().unwrap();
            assert!(addr.ip().is_loopback() && addr.ip().is_ipv4(), "{ep}");
        }
        // the type has no address to give: a host in the plan does not even deserialize
        let host = serde_json::json!({"rpc_ports": ["10.0.0.7:50052"], "stages": [], "n_layer": 6});
        assert!(serde_json::from_value::<crate::types::Split>(host).is_err());
        // plans that would reach a port without a stage, a stage without a port, or smuggle a device list
        for (ports, stages, why) in [
            (vec![], vec![("MTL0", 0, 6)], "at least one RPC port"),
            (vec![0], vec![("RPC0", 0, 3), ("MTL0", 3, 6)], "none of them 0"),
            (
                vec![7, 7],
                vec![("RPC0", 0, 2), ("RPC1", 2, 4), ("MTL0", 4, 6)],
                "given twice",
            ),
            (
                vec![7, 8],
                vec![("RPC0", 0, 3), ("MTL0", 3, 6)],
                "RPC1 (port 8) has no stage",
            ),
            (
                vec![7],
                vec![("RPC0", 0, 2), ("RPC1", 2, 4), ("MTL0", 4, 6)],
                "RPC1 has no RPC port",
            ),
            (
                vec![7],
                vec![("RPC0", 0, 3), ("MTL0,CPU", 3, 6)],
                "not a llama.cpp device name",
            ),
            (vec![7], vec![("RPC0", 0, 3), ("MTL0", 4, 6)], "contiguously"),
        ] {
            let e = build_args(&split_req(ports, &stages), 1, Path::new(KEY_FILE), true).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest);
            assert!(e.message.contains(why), "{why}: {e}");
        }
    }

    /// No CPU-branch flag and no second device or offload choice reaches a split engine, whichever
    /// branch the caller would otherwise take.
    #[test]
    fn split_skips_cpu_branch_flags() {
        let r = split_req(vec![50052], &[("RPC0", 0, 3), ("MTL0", 3, 6)]);
        for gpu in [false, true] {
            let a = build_args(&r, 1, Path::new(KEY_FILE), gpu).unwrap();
            for f in [
                "none",
                "--no-kv-offload",
                "--no-op-offload",
                "UCOMPUTE_TEST_NO_SUCH_DEVICE",
            ] {
                assert!(!a.iter().any(|x| x == f), "{f} with gpu={gpu}: {a:?}");
            }
            assert_eq!(value_of(&a, "--device").len(), 1, "{a:?}");
            assert_eq!(value_of(&a, "-ngl"), ["999"]);
        }
    }

    #[test]
    fn split_sets_no_rdma_and_keeps_lv4() {
        let r = split_req(vec![50052], &[("RPC0", 0, 3), ("MTL0", 3, 6)]);
        let env = engine_environment(&r);
        let ggml: Vec<_> = env
            .iter()
            .filter(|(k, _)| k.to_string_lossy().starts_with("GGML_"))
            .collect();
        assert_eq!(ggml, [&(OsString::from("GGML_RPC_NO_RDMA"), OsString::from("1"))]);
        assert!(!engine_environment(&req())
            .iter()
            .any(|(k, _)| k.to_string_lossy().starts_with("GGML_")));
        // -lv 5 logs every prompt (lab rpc-dataflow, b11609): the split proof works at UCompute's -lv 4
        let a = build_args(&r, 1, Path::new(KEY_FILE), true).unwrap();
        assert_eq!(value_of(&a, "-lv"), ["4"]);
    }

    #[test]
    fn environment_is_scrubbed() {
        let env = [
            "PATH",
            "HOME",
            "LLAMA_ARG_PORT",
            "GGML_METAL_PATH",
            "DYLD_LIBRARY_PATH",
            "LD_PRELOAD",
            "LANG",
        ]
        .map(|k| (OsString::from(k), OsString::from("v")));
        let kept: Vec<String> = clean_environment(env)
            .into_iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        assert_eq!(kept, ["PATH", "HOME", "LANG"]);
    }

    /// `free_port` binds and drops, so another socket in this parallel test binary can take the port
    /// before we bind it again (AddrInUse seen once in a full run); a port that is never bindable in
    /// three tries is a real failure. The engine start itself retries once on "couldn't bind".
    #[tokio::test]
    async fn free_port_is_bindable_and_nonzero() {
        let mut last = None;
        for _ in 0..3 {
            let p = free_port().await.unwrap();
            assert!(p > 0);
            match tokio::net::TcpListener::bind(("127.0.0.1", p)).await {
                Ok(_) => return,
                Err(e) => last = Some(e),
            }
        }
        panic!("free_port gave three ports that could not be bound: {last:?}");
    }

    #[test]
    fn log_read_is_lossy_and_tolerates_absence() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("engine.log");
        std::fs::write(&p, b"offloaded 7/7 layers to GPU\nname: \xff\xfe junk\n").unwrap();
        let log = read_log(&p);
        assert!(log.contains("offloaded 7/7 layers to GPU") && log.contains("junk"));
        assert_eq!(read_log(&dir.path().join("missing.log")), "");
    }

    #[test]
    fn alive_and_rss_on_self() {
        let pid = std::process::id();
        assert!(alive(pid));
        if cfg!(unix) {
            assert!(rss_bytes(pid).unwrap() > 1024 * 1024);
        }
    }
}
