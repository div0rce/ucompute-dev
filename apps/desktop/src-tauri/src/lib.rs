//! Thin shell: make sure the Common daemon is serving on loopback, tell the UI where it is, and stop the
//! daemon again on exit if this app started it. Everything else is HTTP from the webview.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

mod firewall;

/// The exact arguments the app starts the bundled daemon with, one whitespace-separated line.
/// `crates/common-cli/tests/desktop_sidecar.rs` and `scripts/build-sidecar.mjs` read the same file and
/// fail when the CLI they build does not accept these arguments, so the app and its bundled daemon
/// cannot drift apart again (a stale sidecar rejected `--exit-with-parent` and never started).
const DAEMON_ARGS: &str = include_str!("../daemon-args.txt");
/// Basename of the `bundle.externalBin` entry in tauri.conf.json (`binaries/common`).
const SIDECAR: &str = "common"; // tauri resolves sidecars by basename next to the app executable
const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a quitting app waits for the daemon to stop its engine and exit after it was asked to.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Windows has no `--exit-with-parent` (no parent pid to poll) and no SIGTERM: put the daemon in a job
/// object that ends its processes when its last handle closes. Only this app holds that handle and
/// Windows closes it when the app exits for any reason (crash, Task Manager), so the daemon goes with
/// it; the daemon's own job then ends its engine.
#[cfg(windows)]
fn end_with_this_app(pid: u32) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE};
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
    // SAFETY: OpenProcess on our own child's pid; the handle is closed right after the assignment.
    let assigned = job != 0
        && unsafe {
            let h = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
            let ok = !h.is_null() && AssignProcessToJobObject(job as _, h) != 0;
            if !h.is_null() {
                CloseHandle(h);
            }
            ok
        };
    if !assigned {
        eprintln!("common-desktop: could not tie the daemon to this app; it may outlive a crash");
    }
}

fn daemon_args() -> Vec<&'static str> {
    DAEMON_ARGS.split_whitespace().collect()
}

/// The value after `flag` in [`DAEMON_ARGS`]; the file is fixed at compile time, so a missing value
/// is a build defect and fails at startup.
fn daemon_arg(flag: &str) -> &'static str {
    let args = daemon_args();
    args.iter()
        .position(|a| *a == flag)
        .and_then(|i| args.get(i + 1))
        .copied()
        .unwrap_or_else(|| panic!("daemon-args.txt has no value for {flag}"))
}

fn daemon_addr() -> SocketAddr {
    format!("{}:{}", daemon_arg("--host"), daemon_arg("--port"))
        .parse()
        .expect("daemon-args.txt --host/--port form a socket address")
}

struct Daemon {
    child: Mutex<Option<CommandChild>>,
    /// Set by the sidecar event loop when the daemon process has exited.
    exited: Arc<AtomicBool>,
    spawned: bool,
}

impl Daemon {
    /// The service on the port is the bundled daemon this app started: spawned, and not exited since. Once it exited
    /// (crashed, or lost the port to another daemon), whatever serves is not ours (see [`stop_daemon`]).
    fn ours(&self) -> bool {
        self.spawned && !self.exited.load(Ordering::SeqCst)
    }
}

/// Ask the daemon to stop, wait for it, and only then kill it. Stopping gracefully unloads the model,
/// stops the llama.cpp engine and sends the mDNS goodbye, so other devices drop this one at once
/// instead of listing it as nearby for minutes. A plain `kill()` (SIGKILL / TerminateProcess) did
/// none of that and, before the job object, left the engine running orphaned.
fn stop_daemon(child: CommandChild, exited: &AtomicBool) {
    // Our daemon already exited (crashed, or lost the port to another daemon): whatever answers on the port now
    // is not ours, and on unix its pid may have been reused. Ask nothing, kill nothing.
    // ponytail: a child still starting while another daemon holds the port is not caught; a per-launch token
    // checked by /api/shutdown (or a pid reported by /health) would close that.
    if exited.load(Ordering::SeqCst) {
        return;
    }
    // SAFETY: plain kill(2) on the pid of a child this process spawned and has not reaped.
    #[cfg(unix)]
    let asked = unsafe { libc::kill(child.pid() as libc::pid_t, libc::SIGTERM) } == 0;
    // Windows has no SIGTERM: `POST /api/shutdown` takes the daemon down the same path.
    #[cfg(not(unix))]
    let asked = request_shutdown(daemon_addr());
    if asked {
        let deadline = Instant::now() + STOP_TIMEOUT;
        while !exited.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if exited.load(Ordering::SeqCst) {
            eprintln!("common-desktop: daemon stopped");
            return;
        }
        eprintln!("common-desktop: daemon still running {STOP_TIMEOUT:?} after the stop request; killing it");
    } else {
        eprintln!("common-desktop: could not ask the daemon to stop; killing it");
    }
    // The daemon's own job object ends its engine on Windows.
    let _ = child.kill();
}

#[derive(serde::Serialize)]
struct DaemonInfo {
    base_url: String,
    spawned: bool,
}

/// One HTTP/1.1 request over a raw TCP socket; true when the answer has `status`. No HTTP client
/// dependency needed for two loopback calls. No Origin header: the daemon's browser guards let a
/// local non-browser client through, as they do for the CLI.
fn http_status_is(addr: SocketAddr, request: &[u8], status: &str) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    if s.write_all(request).is_err() {
        return false;
    }
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf); // a timeout still leaves the status line in buf
    buf.starts_with(format!("HTTP/1.1 {status}").as_bytes()) || buf.starts_with(format!("HTTP/1.0 {status}").as_bytes())
}

/// GET /health; true on a 200.
fn health_ok(addr: SocketAddr) -> bool {
    http_status_is(
        addr,
        b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        "200",
    )
}

/// POST /api/shutdown; true on a 202 (the daemon is shutting down). False on an older daemon
/// without the route, which the caller then kills. Used on Windows only; built everywhere so the
/// tests run on every platform.
#[cfg_attr(unix, allow(dead_code))]
fn request_shutdown(addr: SocketAddr) -> bool {
    http_status_is(
        addr,
        b"POST /api/shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        "202",
    )
}

/// Set by SIGTERM/SIGINT/SIGHUP. The handler only stores a flag (async-signal-safe); a thread turns
/// it into `AppHandle::exit`, so `kill <app pid>` takes the same path as Quit: RunEvent::Exit stops
/// the daemon gracefully. Without this a SIGTERM ended the app at once and left the daemon to notice
/// via `--exit-with-parent` a second later.
#[cfg(unix)]
static QUIT_SIGNAL: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_quit_signal(_: libc::c_int) {
    QUIT_SIGNAL.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
fn quit_on_signals(handle: tauri::AppHandle) {
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: installs a handler that only stores to an atomic.
        unsafe { libc::signal(sig, on_quit_signal as extern "C" fn(libc::c_int) as libc::sighandler_t) };
    }
    std::thread::spawn(move || loop {
        if QUIT_SIGNAL.load(Ordering::SeqCst) {
            eprintln!("common-desktop: quit signal received");
            handle.exit(0);
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    });
}

#[tauri::command]
fn daemon_info(state: tauri::State<'_, Daemon>) -> DaemonInfo {
    DaemonInfo {
        base_url: format!("http://{}", daemon_addr()),
        spawned: state.spawned,
    }
}

/// Windows Firewall state for the bundled daemon; `supported: false` elsewhere, `adopted` when the
/// serving daemon is not the one this app started (or that one exited). Read-only; `Err` when the
/// rules could not be read.
#[tauri::command]
async fn firewall_status(state: tauri::State<'_, Daemon>) -> Result<firewall::Status, String> {
    let ours = state.ours();
    tauri::async_runtime::spawn_blocking(move || firewall::status_for(ours))
        .await
        .map_err(|e| e.to_string())?
}

/// Adds the rule through the Windows elevation prompt (the user's click there is the consent);
/// `Err("cancelled")` when the user declines it. Refused for a daemon this app did not start, or
/// once the one it started exited.
#[tauri::command]
async fn allow_through_firewall(state: tauri::State<'_, Daemon>) -> Result<(), String> {
    let ours = state.ours();
    tauri::async_runtime::spawn_blocking(move || firewall::allow_for(ours))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            #[cfg(unix)]
            quit_on_signals(app.handle().clone());
            let addr = daemon_addr();
            let mut child = None;
            let mut spawned = false;
            let exited = Arc::new(AtomicBool::new(false));
            if health_ok(addr) {
                eprintln!("common-desktop: daemon already serving on {addr}; not spawning the sidecar");
            } else {
                // The sidecar may be absent in `pnpm dev` without a packaged binary; the UI then shows
                // "service not running" and the developer starts `common serve` by hand.
                match app.shell().sidecar(SIDECAR) {
                    Ok(cmd) => match cmd
                        .args(daemon_args())
                        .spawn()
                    {
                        Ok((mut rx, c)) => {
                            #[cfg(windows)]
                            end_with_this_app(c.pid());
                            child = Some(c);
                            spawned = true;
                            let code = Arc::new(AtomicI32::new(i32::MIN));
                            let (ev_exited, ev_code) = (exited.clone(), code.clone());
                            tauri::async_runtime::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    match ev {
                                        CommandEvent::Stdout(l) | CommandEvent::Stderr(l) => {
                                            eprint!("common: {}", String::from_utf8_lossy(&l));
                                        }
                                        CommandEvent::Terminated(p) => {
                                            eprintln!("common-desktop: daemon exited with {:?}", p.code);
                                            ev_code.store(p.code.unwrap_or(-1), Ordering::SeqCst);
                                            ev_exited.store(true, Ordering::SeqCst);
                                        }
                                        _ => {}
                                    }
                                }
                            });
                            let exited = exited.clone();
                            std::thread::spawn(move || {
                                let start = Instant::now();
                                while start.elapsed() < HEALTH_TIMEOUT {
                                    if health_ok(addr) {
                                        eprintln!("common-desktop: daemon healthy on {addr}");
                                        return;
                                    }
                                    if exited.load(Ordering::SeqCst) {
                                        // Exit code 2 is a command-line usage error: the bundled daemon
                                        // does not accept the arguments this app passes.
                                        let c = code.load(Ordering::SeqCst);
                                        let why = if c == 2 {
                                            " (it rejected its command line: the app and its bundled daemon come from different builds)"
                                        } else {
                                            ""
                                        };
                                        eprintln!("common-desktop: daemon exited with code {c} before becoming healthy{why}");
                                        return;
                                    }
                                    std::thread::sleep(Duration::from_millis(200));
                                }
                                eprintln!("common-desktop: daemon did not become healthy within {HEALTH_TIMEOUT:?}");
                            });
                        }
                        Err(e) => eprintln!("common-desktop: could not start the daemon sidecar: {e}"),
                    },
                    Err(e) => eprintln!("common-desktop: daemon sidecar not available: {e}"),
                }
            }
            app.manage(Daemon {
                child: Mutex::new(child),
                exited,
                spawned,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![daemon_info, firewall_status, allow_through_firewall])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            let daemon = app.state::<Daemon>();
            if let Some(child) = daemon.child.lock().ok().and_then(|mut c| c.take()) {
                stop_daemon(child, &daemon.exited);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A one-shot loopback server answering `status`; returns its address and the request it got.
    fn answer(status: &'static str) -> (SocketAddr, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = l.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 512];
            while !req.ends_with(b"\r\n\r\n") {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&buf[..n]);
            }
            s.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes())
                .unwrap();
            String::from_utf8(req).unwrap()
        });
        (addr, t)
    }

    /// The finding: the firewall commands read the startup-only `spawned`, so after the app's own
    /// daemon exited (and another took the port) they still checked and added the bundled path's rule.
    #[test]
    fn a_daemon_that_exited_is_no_longer_ours() {
        let daemon = |spawned| Daemon {
            child: Mutex::new(None),
            exited: Arc::new(AtomicBool::new(false)),
            spawned,
        };
        let d = daemon(true);
        assert!(d.ours());
        d.exited.store(true, Ordering::SeqCst);
        assert!(!d.ours());
        assert!(!daemon(false).ours(), "adopted");
    }

    /// The Windows quit path: POST /api/shutdown to the loopback daemon; only a 202 counts as
    /// stopping, anything else (an older daemon's 404, nothing listening) makes the caller kill it.
    #[test]
    fn request_shutdown_posts_and_accepts_only_202() {
        let (addr, req) = answer("202 Accepted");
        assert!(request_shutdown(addr));
        let req = req.join().unwrap();
        assert!(
            req.starts_with("POST /api/shutdown HTTP/1.1\r\nHost: 127.0.0.1\r\n"),
            "{req:?}"
        );
        assert!(!req.contains("Origin:"), "{req:?}");

        let (addr, req) = answer("404 Not Found");
        assert!(!request_shutdown(addr));
        req.join().unwrap();

        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        assert!(!request_shutdown(free), "nothing listening");
    }
}
