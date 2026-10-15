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

/// The exact arguments the app starts the bundled daemon with, one whitespace-separated line.
/// `crates/common-cli/tests/desktop_sidecar.rs` and `scripts/build-sidecar.sh` read the same file and
/// fail when the CLI they build does not accept these arguments, so the app and its bundled daemon
/// cannot drift apart again (a stale sidecar rejected `--exit-with-parent` and never started).
const DAEMON_ARGS: &str = include_str!("../daemon-args.txt");
/// Basename of the `bundle.externalBin` entry in tauri.conf.json (`binaries/common`).
const SIDECAR: &str = "common"; // tauri resolves sidecars by basename next to the app executable
const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a quitting app waits for the daemon to stop its engine and exit after SIGTERM.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

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

/// Ask the daemon to stop (SIGTERM: it unloads the model and stops its llama.cpp engine), wait for
/// it, and only then kill it. A plain `kill()` is SIGKILL, which left the engine running orphaned.
fn stop_daemon(child: CommandChild, exited: &AtomicBool) {
    #[cfg(unix)]
    {
        // SAFETY: plain kill(2) on the pid of a child this process spawned and has not reaped.
        unsafe { libc::kill(child.pid() as libc::pid_t, libc::SIGTERM) };
        let deadline = Instant::now() + STOP_TIMEOUT;
        while !exited.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        if exited.load(Ordering::SeqCst) {
            eprintln!("common-desktop: daemon stopped");
            return;
        }
        eprintln!("common-desktop: daemon still running {STOP_TIMEOUT:?} after SIGTERM; killing it");
    }
    let _ = child.kill();
}

#[derive(serde::Serialize)]
struct DaemonInfo {
    base_url: String,
    spawned: bool,
}

/// GET /health over a raw TCP socket; true on a 200. No HTTP client dependency needed for one probe.
fn health_ok(addr: SocketAddr) -> bool {
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(800)));
    if s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut buf = Vec::new();
    let _ = s.read_to_end(&mut buf); // a timeout still leaves the status line in buf
    buf.starts_with(b"HTTP/1.1 200") || buf.starts_with(b"HTTP/1.0 200")
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
        .invoke_handler(tauri::generate_handler![daemon_info])
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
