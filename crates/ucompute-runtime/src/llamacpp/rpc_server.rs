//! The stage host's `ggml-rpc-server`, supervised (WP-04 C3; ADR-0005 D3). One server per split
//! session, on a loopback port the daemon picks, proved to be the child's own listener before
//! anything connects to it; the daemon's bridge (C5) is its only client.
//!
//! The launch is fixed: `-H 127.0.0.1 -p <port> -t <threads>`. Never `-c`: the local file cache
//! keeps weights on R's disk and lets a client load tensors by hash. Never `-d`: every device a
//! server exposes becomes an `RPCk` on H, and the split maps `RPCk` to the k-th endpoint, so a
//! server must expose exactly one device ([`start`] refuses more). The one exception is a
//! SINGLE-HOST test's `-d CPU` ([`TEST_STAGE_CPU_ENV`]), still one device.
//!
//! Does not claim a sandbox (F-06-08): a same-user process stays ADR-0005 R6.
//!
//! The server obeys whoever reaches its port. At b11609 (fcac19079, `rpc_server::comm_init`)
//! `COMM_INIT` (command 20) with rank 0 makes it listen on `0.0.0.0:<client's port>`, and with rank 1
//! connect to any `host:port` the client names. So the bridge (C5) must refuse `COMM_INIT`,
//! `COMM_ALLREDUCE` and `COMM_FREE` (20..=22) from the head before forwarding a byte; the
//! `-H 127.0.0.1` here does not cover that listener.
//!
//! Ported from the UCompute Core Lab (Apache-2.0); the banner fixture is real b11609 output from
//! this Mac (2026-10-24).

use super::{process, sockets};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use ucompute_core::{ErrorCode, Result, UComputeError};

/// A ready `ggml-rpc-server`. Dropping it kills the process (`kill_on_drop`); [`process::stop`]
/// on `child` is the graceful path.
#[derive(Debug)]
pub struct RpcServer {
    pub child: Child,
    pub port: u16,
    /// The one device the server exposes, as its banner names it, e.g. `MTL0 (Apple M2)`.
    pub device: String,
    /// Its stdout and stderr; the caller removes it after a clean stop, as for llama-server's.
    pub log_path: PathBuf,
}

/// The SINGLE-HOST test flag under which a non-Linux device hosts a stage, and only for a loopback
/// peer (`ucompute_network::rpc::single_host_pool_test`).
pub const TEST_POOL_ENV: &str = "UCOMPUTE_TEST_SINGLE_HOST_POOL";
/// SINGLE-HOST test only (plan S1-M, configuration C with ggml-cpu on both stages): with
/// [`TEST_POOL_ENV`] also `1`, `-d CPU` holds the server to its CPU device instead of this Mac's
/// Metal. It still exposes exactly one device. Production sets neither, so its line stays [`args`].
pub const TEST_STAGE_CPU_ENV: &str = "UCOMPUTE_TEST_STAGE_CPU";

/// The extra arguments of a SINGLE-HOST CPU stage ([`TEST_STAGE_CPU_ENV`]), else none.
pub fn test_device_args(pool: Option<OsString>, cpu: Option<OsString>) -> &'static [&'static str] {
    if pool.is_some_and(|v| v == "1") && cpu.is_some_and(|v| v == "1") {
        &["-d", "CPU"]
    } else {
        &[]
    }
}

/// The banner and the listener appear within a second on this Mac (b11609, Metal).
const READY_WITHIN: Duration = Duration::from_secs(30);

/// `-t` for a stage host: its physical cores less two for the daemon and the OS, at least one.
pub fn stage_threads(physical_cores: u32) -> u32 {
    physical_cores.saturating_sub(2).max(1)
}

/// The whole command line after the binary.
pub fn args(port: u16, threads: u32) -> Vec<String> {
    ["-H", "127.0.0.1", "-p", &port.to_string(), "-t", &threads.to_string()]
        .map(String::from)
        .to_vec()
}

/// The parent's environment scrubbed as for the engine ([`process::clean_environment`]), without
/// `LLAMA_CACHE`, with RPC kept on TCP and glibc's malloc arenas capped (fewer reserved address
/// ranges under `RLIMIT_AS`).
pub fn environment<I>(parent: I) -> Vec<(OsString, OsString)>
where
    I: IntoIterator<Item = (OsString, OsString)>,
{
    let mut env: Vec<_> = process::clean_environment(parent)
        .into_iter()
        .filter(|(k, _)| k != "LLAMA_CACHE" && k != "MALLOC_ARENA_MAX")
        .collect();
    env.push(("GGML_RPC_NO_RDMA".into(), "1".into()));
    env.push(("MALLOC_ARENA_MAX".into(), "2".into()));
    env
}

/// The devices under the banner's `Devices:` heading, as `NAME (description)` without the memory
/// figures: `  MTL0: Apple M2 (5461 MiB, 5460 MiB free)` gives `MTL0 (Apple M2)`. None until the
/// banner's last line (`  transport      : ...`) is in the log.
pub fn banner_devices(log: &str) -> Option<Vec<String>> {
    let banner = log.split_once("\nDevices:\n")?.1;
    let (devices, _) = banner.split_once("\n  transport ")?;
    Some(
        devices
            .lines()
            .filter_map(|l| l.strip_prefix("  ")?.split_once(": "))
            .filter(|(name, _)| !name.is_empty() && !name.contains(char::is_whitespace))
            .map(|(name, desc)| {
                let desc = desc.rsplit_once(" (").map_or(desc, |(d, _)| d);
                format!("{name} ({desc})")
            })
            .collect(),
    )
}

/// Start the server on a free loopback port; if another listener squats the port first, once more
/// on a new one, then `BackendFailed`.
///
/// Any task or thread may call it: on Linux the fork happens on one thread that never exits
/// ([`spawn`]), so `PR_SET_PDEATHSIG` fires only when the daemon dies.
pub async fn start(engine_dir: &Path, logs: &Path, threads: u32, mem_cap: Option<u64>) -> Result<RpcServer> {
    match start_at(engine_dir, logs, threads, mem_cap, process::free_port().await?).await {
        Err(e) if e.message.contains(SQUATTED) => {
            tracing::warn!(error = %e.message, "rpc-server port squatted, retrying on a new port");
            start_at(engine_dir, logs, threads, mem_cap, process::free_port().await?).await
        }
        r => r,
    }
}

const SQUATTED: &str = "another listener holds";

/// [`start`] on a given port, without the retry. Ready means the banner is complete and every
/// listener on `port` is the child's own, on 127.0.0.1; nothing connects to the port before that.
pub async fn start_at(
    engine_dir: &Path,
    logs: &Path,
    threads: u32,
    mem_cap: Option<u64>,
    port: u16,
) -> Result<RpcServer> {
    let binary = ucompute_artifacts::backend::binary(engine_dir, "ggml-rpc-server");
    std::fs::create_dir_all(logs)?;
    let log_path = logs.join(format!("rpc-server-{}.log", uuid::Uuid::new_v4()));
    let log = std::fs::File::create(&log_path)?;
    let mut cmd = Command::new(&binary);
    cmd.args(args(port, threads))
        .args(test_device_args(
            std::env::var_os(TEST_POOL_ENV),
            std::env::var_os(TEST_STAGE_CPU_ENV),
        ))
        .env_clear()
        .envs(environment(std::env::vars_os()))
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .kill_on_drop(true);
    #[cfg(target_os = "linux")]
    confine(&mut cmd, mem_cap);
    #[cfg(not(target_os = "linux"))]
    let _ = mem_cap; // ponytail: no RLIMIT_AS off Linux; R is Linux-only in this build (OD-11)
    let mut child = spawn(cmd).await.map_err(|e| {
        UComputeError::new(
            ErrorCode::BackendUnavailable,
            format!("cannot start {}: {e}", binary.display()),
        )
    })?;
    match wait_ready(&mut child, port, &log_path).await {
        Ok(device) => Ok(RpcServer {
            child,
            port,
            device,
            log_path,
        }),
        Err(e) => {
            process::stop(child).await;
            // a squatted port is in the message and [`start`] retries: its log holds nothing
            if e.message.contains(SQUATTED) {
                let _ = std::fs::remove_file(&log_path);
            }
            Err(e)
        }
    }
}

async fn wait_ready(child: &mut Child, port: u16, log_path: &Path) -> Result<String> {
    let failed = |why: String| UComputeError::new(ErrorCode::BackendFailed, why);
    let pid = child.id().ok_or_else(|| failed("rpc-server exited at once".into()))?;
    let deadline = Instant::now() + READY_WITHIN;
    loop {
        // the owner check first: a squatter that made the child's bind fail is the reason it exited
        match listener_state(pid, port)? {
            sockets::Listener::Squatted => {
                return Err(failed(format!(
                    "{SQUATTED} 127.0.0.1:{port} (or the port on another address); nothing was sent to it"
                )))
            }
            sockets::Listener::Ours => {
                if let Some(devices) = banner_devices(&process::read_log(log_path)) {
                    return match devices.as_slice() {
                        [one] => Ok(one.clone()),
                        _ => Err(failed(format!(
                            "rpc-server exposes {} devices ({}); a split needs exactly one per server",
                            devices.len(),
                            devices.join(", ")
                        ))),
                    };
                }
            }
            sockets::Listener::Nobody => {}
        }
        if let Some(status) = child.try_wait()? {
            let tail = super::logparse::tail(&process::read_log(log_path), 20);
            return Err(failed(format!(
                "rpc-server exited ({status}) before becoming ready: {tail}"
            )));
        }
        if Instant::now() >= deadline {
            return Err(UComputeError::new(
                ErrorCode::Timeout,
                format!("rpc-server not ready on 127.0.0.1:{port} within {READY_WITHIN:?}"),
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Linux: fork on a thread that lives as long as the daemon. `PR_SET_PDEATHSIG` fires when the
/// *thread* that forked the child exits, and no tokio thread is safe for that: workers are
/// blocking-pool threads, `block_in_place` hands a worker's core to a new thread, and idle pool
/// threads retire after 10 s. Seen on sentinel (b11609, tokio 1.53): a server spawned from a
/// `tokio::spawn` task in a multi-thread runtime was SIGKILLed 10 s later, when the pool thread
/// that had polled the task at the fork exited.
#[cfg(target_os = "linux")]
async fn spawn(cmd: Command) -> std::io::Result<Child> {
    use std::sync::{mpsc, OnceLock};
    type Job = (
        Command,
        tokio::runtime::Handle,
        tokio::sync::oneshot::Sender<std::io::Result<Child>>,
    );
    static FORKER: OnceLock<mpsc::Sender<Job>> = OnceLock::new();
    let forker = FORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("rpc-server-forker".into())
            .spawn(move || {
                for (mut cmd, rt, reply) in rx {
                    let _rt = rt.enter(); // the child registers with the caller's runtime
                    let _ = reply.send(cmd.spawn());
                }
            })
            .expect("start the rpc-server forker thread");
        tx
    });
    let (reply, child) = tokio::sync::oneshot::channel();
    let gone = || std::io::Error::other("the rpc-server forker thread is gone");
    forker
        .send((cmd, tokio::runtime::Handle::current(), reply))
        .map_err(|_| gone())?;
    child.await.map_err(|_| gone())?
}

#[cfg(not(target_os = "linux"))]
async fn spawn(mut cmd: Command) -> std::io::Result<Child> {
    cmd.spawn()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn listener_state(pid: u32, port: u16) -> Result<sockets::Listener> {
    Ok(sockets::live::listener_state_of(pid, port)?)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn listener_state(_pid: u32, _port: u16) -> Result<sockets::Listener> {
    Err(UComputeError::new(
        ErrorCode::NotImplemented,
        "a stage host needs Linux (ADR-0005 OD-11)",
    ))
}

/// Linux, between fork and exec (async-signal-safe calls only): die with the daemon, refuse to run
/// if the daemon already died, cap the address space, and yield the CPU to the daemon. The cap and
/// the nice only ever tighten what the daemon has: an unprivileged process can lower its RLIMIT_AS
/// and raise its nice, never the reverse, so a daemon already niced above 10 or under a lower hard
/// limit keeps those instead of failing every spawn.
#[cfg(target_os = "linux")]
fn confine(cmd: &mut Command, mem_cap: Option<u64>) {
    let daemon = std::process::id() as libc::pid_t;
    // SAFETY: the closure runs in the forked child before exec and makes only async-signal-safe
    // syscalls (prctl, getppid, getrlimit, setrlimit, getpriority, setpriority, _exit); it
    // allocates nothing.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // the daemon died between fork and prctl: PDEATHSIG will never fire
            if libc::getppid() != daemon {
                libc::_exit(1);
            }
            if let Some(cap) = mem_cap {
                let mut now = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                if libc::getrlimit(libc::RLIMIT_AS, &mut now) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // rlim_cur <= rlim_max; RLIM_INFINITY is the largest value
                let cap = cap.min(now.rlim_cur);
                let limit = libc::rlimit {
                    rlim_cur: cap,
                    rlim_max: cap,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            // a failed getpriority reads as -1: then 10 is set, or the spawn fails as before
            if libc::getpriority(libc::PRIO_PROCESS, 0) < 10 && libc::setpriority(libc::PRIO_PROCESS, 0, 10) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_server_args_are_fixed() {
        assert_eq!(
            args(50113, 6),
            ["-H", "127.0.0.1", "-p", "50113", "-t", "6"].map(String::from)
        );
        assert_eq!([0, 1, 3, 8].map(stage_threads), [1, 1, 1, 6]);
        let a = args(1, 1);
        assert!(!a
            .iter()
            .any(|x| x == "-c" || x == "--cache" || x == "-d" || x == "--device"));
    }

    #[test]
    fn rpc_server_cpu_stage_only_under_both_test_flags() {
        let on = || Some(OsString::from("1"));
        assert_eq!(test_device_args(on(), on()), ["-d", "CPU"]);
        assert!(
            test_device_args(None, on()).is_empty(),
            "never without the pool test flag"
        );
        assert!(test_device_args(on(), None).is_empty());
        assert!(test_device_args(Some("0".into()), on()).is_empty());
        assert!(test_device_args(on(), Some("true".into())).is_empty());
    }

    #[test]
    fn rpc_server_env_has_no_rdma_no_cache() {
        let parent = [
            ("PATH", "/usr/bin"),
            ("LLAMA_CACHE", "/tmp/cache"),
            ("GGML_RPC_NO_RDMA", "0"),
            ("GGML_RPC_DEBUG", "1"),
            ("MALLOC_ARENA_MAX", "64"),
            ("LD_PRELOAD", "/tmp/x.so"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let env = environment(parent);
        let get = |k: &str| {
            env.iter()
                .filter(|(x, _)| x == k)
                .map(|(_, v)| v.to_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(get("GGML_RPC_NO_RDMA"), ["1"]);
        assert_eq!(get("MALLOC_ARENA_MAX"), ["2"]);
        assert_eq!(get("PATH"), ["/usr/bin"]);
        assert!(get("LLAMA_CACHE").is_empty() && get("GGML_RPC_DEBUG").is_empty() && get("LD_PRELOAD").is_empty());
    }

    // Real b11609 banner from this Mac (`ggml-rpc-server -H 127.0.0.1 -p 65425 -t 4`, 2026-10-24).
    const BANNER_MTL0: &str = include_str!("fixtures/rpc-server-banner-b11609-mtl0.txt");

    #[test]
    fn banner_names_the_one_device() {
        assert_eq!(banner_devices(BANNER_MTL0), Some(vec!["MTL0 (Apple M2)".to_string()]));
        // not complete until the transport line
        let cut = BANNER_MTL0.split("  transport").next().unwrap();
        assert_eq!(banner_devices(cut), None);
        assert_eq!(banner_devices(""), None);
        // SYNTHETIC: a CPU server prints its transport line right after its devices, and a server
        // with two devices lists both (start refuses it)
        let cpu =
            "Starting RPC server v8.0.0\n  endpoint       : 127.0.0.1:50113\n  local cache    : n/a\nDevices:\n  \
                   CPU: Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz (14000 MiB, 13000 MiB free)\n  \
                   transport      : TCP (RDMA auto-negotiate enabled)\n";
        assert_eq!(
            banner_devices(cpu),
            Some(vec!["CPU (Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz)".to_string()])
        );
        let two = cpu.replace("Devices:\n", "Devices:\n  CUDA0: NVIDIA X (8000 MiB, 7000 MiB free)\n");
        assert_eq!(banner_devices(&two).map(|d| d.len()), Some(2));
    }
}
