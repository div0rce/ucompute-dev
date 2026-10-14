//! Thin shell: make sure the Common daemon is serving on loopback, tell the UI where it is, and stop the
//! daemon again on exit if this app started it. Everything else is HTTP from the webview.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;

const HOST: &str = "127.0.0.1";
const PORT: u16 = 8192;
/// Basename of the `bundle.externalBin` entry in tauri.conf.json (`binaries/common`).
const SIDECAR: &str = "common"; // tauri resolves sidecars by basename next to the app executable
const HEALTH_TIMEOUT: Duration = Duration::from_secs(20);

struct Daemon {
    child: Mutex<Option<CommandChild>>,
    spawned: bool,
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

fn wait_for_health(addr: SocketAddr, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if health_ok(addr) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

#[tauri::command]
fn daemon_info(state: tauri::State<'_, Daemon>) -> DaemonInfo {
    DaemonInfo {
        base_url: format!("http://{HOST}:{PORT}"),
        spawned: state.spawned,
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let addr: SocketAddr = format!("{HOST}:{PORT}").parse()?;
            let mut child = None;
            let mut spawned = false;
            if health_ok(addr) {
                eprintln!("common-desktop: daemon already serving on {addr}; not spawning the sidecar");
            } else {
                // The sidecar may be absent in `pnpm dev` without a packaged binary; the UI then shows
                // "service not running" and the developer starts `common serve` by hand.
                match app.shell().sidecar(SIDECAR) {
                    Ok(cmd) => match cmd
                        .args([
                            "serve",
                            "--host",
                            HOST,
                            "--port",
                            &PORT.to_string(),
                            "--exit-with-parent",
                        ])
                        .spawn()
                    {
                        Ok((mut rx, c)) => {
                            child = Some(c);
                            spawned = true;
                            tauri::async_runtime::spawn(async move {
                                while let Some(ev) = rx.recv().await {
                                    match ev {
                                        CommandEvent::Stdout(l) | CommandEvent::Stderr(l) => {
                                            eprint!("common: {}", String::from_utf8_lossy(&l));
                                        }
                                        CommandEvent::Terminated(p) => {
                                            eprintln!("common-desktop: daemon exited with {:?}", p.code);
                                        }
                                        _ => {}
                                    }
                                }
                            });
                            std::thread::spawn(move || {
                                if wait_for_health(addr, HEALTH_TIMEOUT) {
                                    eprintln!("common-desktop: daemon healthy on {addr}");
                                } else {
                                    eprintln!(
                                        "common-desktop: daemon did not become healthy within {HEALTH_TIMEOUT:?}"
                                    );
                                }
                            });
                        }
                        Err(e) => eprintln!("common-desktop: could not start the daemon sidecar: {e}"),
                    },
                    Err(e) => eprintln!("common-desktop: daemon sidecar not available: {e}"),
                }
            }
            app.manage(Daemon {
                child: Mutex::new(child),
                spawned,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![daemon_info])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            if let Some(child) = app.state::<Daemon>().child.lock().ok().and_then(|mut c| c.take()) {
                let _ = child.kill();
            }
        }
    });
}
