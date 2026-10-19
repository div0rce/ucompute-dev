//! Engine process plumbing: argument building, environment scrubbing, port choice, spawn,
//! readiness polling and graceful stop. Nothing here sees prompts or tokens.

use super::logparse;
use crate::types::LoadRequest;
use common_core::{CommonError, ErrorCode, Result};
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

/// Test hook: when set to `1`, the GPU attempt names a device that does not exist, so the engine
/// really fails to start (exit code 1, usage text in the log). Exercises the Auto→CPU fallback and
/// the `Gpu` → `GpuUnavailable` path with a real process failure instead of a mock.
pub const FORCE_GPU_FAIL_ENV: &str = "COMMON_TEST_FORCE_GPU_FAIL";

/// Env prefixes that could redirect the engine's dynamic libraries, devices or arguments.
const SCRUBBED_ENV_PREFIXES: [&str; 4] = ["LLAMA_ARG_", "GGML_", "DYLD_", "LD_"];

/// Flags Common owns; `extra_args` may not override them (loopback bind, secret, model, log).
const RESERVED_FLAGS: [&str; 7] = [
    "--host",
    "--port",
    "--api-key",
    "--api-key-file",
    "--log-file",
    "-m",
    "--model",
];

pub fn force_gpu_fail() -> bool {
    std::env::var_os(FORCE_GPU_FAIL_ENV).is_some_and(|v| v == "1")
}

/// Each extra arg must be one shell-free token and must not touch a reserved flag.
pub fn validate_extra_args(args: &[String]) -> Result<()> {
    for a in args {
        if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '\0') {
            return Err(CommonError::invalid(format!(
                "extra engine arg must be one non-empty token without whitespace: {a:?}"
            )));
        }
        let flag = a.split_once('=').map_or(a.as_str(), |(f, _)| f);
        if RESERVED_FLAGS.contains(&flag) {
            return Err(CommonError::invalid(format!(
                "extra engine arg {flag} is managed by Common"
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
pub fn build_args(req: &LoadRequest, port: u16, api_key_file: &Path, gpu: bool) -> Vec<String> {
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
        // Common plans n_ctx itself; never let the engine silently resize it (no silent substitution).
        "--fit".into(),
        "off".into(),
    ];
    if gpu {
        a.extend(["-ngl".to_string(), req.n_gpu_layers.unwrap_or(999).to_string()]);
        if force_gpu_fail() {
            a.extend(["--device".to_string(), "COMMON_TEST_NO_SUCH_DEVICE".to_string()]);
        }
    } else {
        a.extend(["-ngl", "0", "--device", "none", "--no-kv-offload", "--no-op-offload"].map(String::from));
    }
    if req.embeddings {
        // Pooling comes from the model file (verified: nomic-embed reports pooling_type=1 without --pooling).
        a.push("--embeddings".into());
    }
    a.extend(req.extra_args.iter().cloned());
    a
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

/// Spawn the engine with stdout+stderr redirected to `log_path` and a scrubbed environment.
/// The engine writes its whole log to stderr, so one redirected handle is the log; `--log-file`
/// would be a second writer on the same file.
pub fn spawn(binary: &Path, args: &[String], log_path: &Path) -> Result<Child> {
    let log = std::fs::File::create(log_path)?;
    let stderr = log.try_clone()?;
    let mut cmd = Command::new(binary);
    // Windows: the desktop app starts the daemon without a console; a console child would then get a
    // console window of its own popping up over the app.
    #[cfg(windows)]
    cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    cmd.args(args)
        .env_clear()
        .envs(clean_environment(std::env::vars_os()))
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
            CommonError::new(
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
            return Err(CommonError::new(
                logparse::exit_error_code(&tail),
                format!("llama-server exited with {status} before becoming ready:\n{tail}"),
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
            return Err(CommonError::new(
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
            CommonError::new(
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
        return Err(CommonError::new(
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
    use common_core::Timeouts;
    use std::path::PathBuf;

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
            kv_type: common_models::KvType::Q8_0,
            embeddings: false,
            timeouts: Timeouts::default(),
            extra_args: vec![],
        }
    }

    fn has(args: &[String], pair: &[&str]) -> bool {
        args.windows(pair.len())
            .any(|w| w.iter().zip(pair).all(|(a, b)| a == b))
    }

    const KEY_FILE: &str = "/tmp/engine.key";

    #[test]
    fn cpu_args() {
        let a = build_args(&req(), 4242, Path::new(KEY_FILE), false);
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
        let a = build_args(&r, 1, Path::new(KEY_FILE), true);
        assert!(has(&a, &["-ngl", "999"]), "all layers by default");
        assert!(!a.iter().any(|x| x == "--device"), "no device override unless forced");
        assert!(has(&a, &["-t", "6"]));
        assert!(a.iter().any(|x| x == "--embeddings"));
        r.n_gpu_layers = Some(3);
        assert!(has(&build_args(&r, 1, Path::new(KEY_FILE), true), &["-ngl", "3"]));
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
        r.extra_args = vec!["--rpc".into(), "127.0.0.1:50052".into()];
        assert!(validate_extra_args(&r.extra_args).is_ok());
        let a = build_args(&r, 1, Path::new(KEY_FILE), false);
        assert_eq!(&a[a.len() - 2..], &["--rpc", "127.0.0.1:50052"]);
        for bad in ["--rpc 1.2.3.4:1", "a\nb", "", "--host", "--api-key=x", "--port"] {
            let e = validate_extra_args(&[bad.to_string()]).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest, "{bad:?}");
        }
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

    #[tokio::test]
    async fn free_port_is_bindable_and_nonzero() {
        let p = free_port().await.unwrap();
        assert!(p > 0);
        tokio::net::TcpListener::bind(("127.0.0.1", p)).await.unwrap();
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
