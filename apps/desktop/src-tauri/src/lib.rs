//! Thin shell: make sure the Common daemon is serving on loopback, tell the UI where it is, restart the
//! daemon this app started when it dies, keep its output in a capped log, and stop it again on exit.
//! Everything else is HTTP from the webview.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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
/// The app restarts its own daemon after an unexpected exit at most this many times within any
/// [`RESTART_WINDOW`], so a daemon that crashes at every start is not restarted for ever.
const RESTART_MAX: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
/// Size cap of each daemon log file (see [`LogFile`]): at most two files, about twice this on disk.
const LOG_CAP: u64 = 5 * 1024 * 1024;

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

/// The daemon this app started, across restarts. One lock over the child, its run and `quitting`, so a restart
/// and Quit never race: Quit sets `quitting` and takes the child under it, and a restart starts a daemon under it
/// only while `quitting` is unset.
#[derive(Default)]
struct Supervised {
    child: Option<CommandChild>,
    /// The current daemon's run; `Run::default()` (not spawned) for a daemon this app did not start.
    run: Arc<Run>,
    quitting: bool,
    restarts: Restarts,
    /// Automatic restarts so far, for the UI.
    restarted: u32,
    /// It exited unexpectedly and its restart waits for the backoff.
    restarting: bool,
    /// It exited unexpectedly once more after [`RESTART_MAX`] restarts within [`RESTART_WINDOW`]: not restarted.
    gave_up: bool,
    /// The app could not start it at launch (sidecar missing, fork failing) and does not try again.
    start_failed: bool,
}

/// When the app restarted its daemon, within the last [`RESTART_WINDOW`].
#[derive(Default)]
struct Restarts(Vec<Instant>);

impl Restarts {
    /// The wait before restarting a daemon that exited at `now` (1, 2, then 4 s), counted as a restart; `None`
    /// once [`RESTART_MAX`] restarts happened within the [`RESTART_WINDOW`] before `now`.
    fn next(&mut self, now: Instant) -> Option<Duration> {
        self.0.retain(|t| now.saturating_duration_since(*t) < RESTART_WINDOW);
        if self.0.len() >= RESTART_MAX {
            return None;
        }
        let wait = Duration::from_secs(1 << self.0.len());
        self.0.push(now);
        Some(wait)
    }
}

/// The daemon's output as this app keeps it: appended to `path` until the next line would take it past `cap`
/// bytes, then `path` becomes `path.1` (replacing the older one) and a new `path` starts.
struct LogFile {
    path: PathBuf,
    file: File,
    len: u64,
    cap: u64,
}

/// Opens `path` for appending; owner-only on unix (it holds device names and addresses).
fn append_to(path: &Path) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    o.open(path)
}

impl LogFile {
    fn open(path: &Path, cap: u64) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let file = append_to(path)?;
        let len = file.metadata()?.len();
        Ok(LogFile {
            path: path.to_path_buf(),
            file,
            len,
            cap,
        })
    }

    /// Appends `line`, with a newline when it has none.
    // ponytail: a line longer than `cap` is kept whole, in a file of its own.
    fn write_line(&mut self, line: &[u8]) -> std::io::Result<()> {
        let nl = !line.ends_with(b"\n");
        let n = line.len() as u64 + u64::from(nl);
        if self.len > 0 && self.len + n > self.cap {
            let mut older = self.path.clone().into_os_string();
            older.push(".1");
            if fs::rename(&self.path, &older).is_ok() {
                self.file = append_to(&self.path)?;
            } else {
                // Held open elsewhere without delete sharing (a log viewer on Windows): the cap matters more than
                // the older lines.
                self.file.set_len(0)?;
            }
            self.len = 0;
        }
        self.file.write_all(line)?;
        if nl {
            self.file.write_all(b"\n")?;
        }
        self.len += n;
        Ok(())
    }
}

struct Daemon {
    sup: Mutex<Supervised>,
    /// `None` when it could not be opened, or after a write failed.
    log: Mutex<Option<LogFile>>,
    log_path: Option<PathBuf>,
}

impl Daemon {
    fn new(log_path: Option<PathBuf>) -> Self {
        let log = log_path.as_deref().and_then(|p| {
            LogFile::open(p, LOG_CAP)
                .map_err(|e| eprintln!("common-desktop: cannot open the daemon log {}: {e}", p.display()))
                .ok()
        });
        Daemon {
            sup: Mutex::default(),
            log: Mutex::new(log),
            log_path,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Supervised> {
        self.sup.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Appends one line of the daemon's output to the log; after a failed write it logs nothing more.
    fn log(&self, line: &[u8]) {
        let mut log = self.log.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Err(e)) = log.as_mut().map(|f| f.write_line(line)) {
            eprintln!("common-desktop: cannot write the daemon log: {e}; not writing it any more");
            *log = None;
        }
    }

    /// This shell's own line about the daemon: on stderr and in the log.
    fn note(&self, msg: &str) {
        eprintln!("common-desktop: {msg}");
        self.log(format!("common-desktop: {msg}").as_bytes());
    }

    /// The daemon this app started exited with `code`: the wait before restarting it, or `None` when it is not
    /// restarted: the app is quitting, the daemon stopped on request (0: SIGTERM, `/api/shutdown`) or rejected
    /// its command line (2: a restart cannot help), or it was restarted too often ([`Restarts::next`]).
    fn after_exit(&self, code: Option<i32>, now: Instant) -> Option<Duration> {
        if matches!(code, Some(0 | 2)) {
            return None;
        }
        self.start_again(now, false)
    }

    /// One more start of the daemon after it exited, or after a restart could not start it (`spawn_failed`): the
    /// wait before it, or `None` while quitting or once the restarts are used up ([`Restarts::next`]).
    fn start_again(&self, now: Instant, spawn_failed: bool) -> Option<Duration> {
        let mut s = self.lock();
        if s.quitting {
            return None;
        }
        let wait = s.restarts.next(now);
        s.restarting = wait.is_some();
        s.gave_up = wait.is_none();
        drop(s);
        let tries = format!("{RESTART_MAX} restart attempts within {RESTART_WINDOW:?}");
        match wait {
            Some(w) => self.note(&format!("restarting the daemon in {w:?}")),
            None if spawn_failed => self.note(&format!("could not start the daemon in {tries}; not trying again")),
            None => self.note(&format!("the daemon exited again after {tries}; not restarting it")),
        }
        wait
    }

    /// At launch: a daemon already serving `addr` is used (and never restarted by this app); otherwise `spawn`
    /// starts ours into the state. When it cannot, nothing tries again (a missing sidecar is a packaging defect),
    /// and `start_failed` tells the UI so instead of letting it promise a retry.
    fn launch(&self, addr: SocketAddr, spawn: impl FnOnce(&mut Supervised) -> bool) {
        if health_ok(addr) {
            self.note(&format!("daemon already serving on {addr}; not spawning the sidecar"));
            return;
        }
        let mut s = self.lock();
        let started = spawn(&mut s);
        s.start_failed = !started;
    }

    fn info(&self) -> DaemonInfo {
        let log_open = self.log.lock().map(|l| l.is_some()).unwrap_or(false);
        let s = self.lock();
        DaemonInfo {
            base_url: format!("http://{}", daemon_addr()),
            spawned: s.run.spawned,
            restarts: s.restarted,
            restarting: s.restarting,
            gave_up: s.gave_up,
            start_failed: s.start_failed,
            log_path: self
                .log_path
                .as_ref()
                .filter(|_| log_open)
                .map(|p| p.display().to_string()),
        }
    }

    /// The restart [`Daemon::after_exit`] waited for. Never while quitting, and never while a daemon serves `addr`:
    /// that one is used instead, as at launch, so there are never two. `spawn` starts the sidecar into the state.
    /// When it cannot (the sidecar missing, fork failing), that counts as one more unexpected exit: the wait
    /// before the next try, or `None` once the restarts are used up ([`Restarts::next`]) and `gave_up` is set.
    // ponytail: a daemon adopted here is not supervised again, like one adopted at launch; if it stops, the UI
    // says the service is not running until the app is reopened. Watch its /health if that race matters.
    fn restart(&self, addr: SocketAddr, spawn: impl FnOnce(&mut Supervised) -> bool) -> Option<Duration> {
        // Probed outside the lock: `daemon_info` runs on the main thread. A daemon that takes the port after the
        // probe makes ours fail to bind and exit, which comes back here.
        let serving = health_ok(addr);
        let mut s = self.lock();
        s.restarting = false;
        if s.quitting {
            return None;
        }
        if serving {
            s.child = None;
            s.run = Arc::new(Run::default());
            drop(s);
            self.note(&format!(
                "a daemon is already serving on {addr}; using it instead of restarting ours"
            ));
            return None;
        }
        if spawn(&mut s) {
            s.restarted += 1;
            return None;
        }
        drop(s);
        self.start_again(Instant::now(), true)
    }
}

/// The bundled daemon this app started, as its sidecar event loop reports it.
#[derive(Default)]
struct Run {
    spawned: bool,
    /// It printed its listening line for [`daemon_addr`] (see [`is_listening_line`]): it, not another
    /// daemon, holds the port. Only the child writes to its own stdout, so nothing else can set this.
    bound: AtomicBool,
    /// The process has exited.
    exited: AtomicBool,
}

impl Run {
    /// The service on the port is the bundled daemon this app started: it bound the port and has not exited
    /// since. Until it binds, or once it exited (crashed, or lost the port to another daemon), whatever
    /// serves is not ours (see [`stop_daemon`]).
    fn ours(&self) -> bool {
        self.spawned && self.bound.load(Ordering::SeqCst) && !self.exited.load(Ordering::SeqCst)
    }

    /// [`Run::ours`] once it is known: waits up to `timeout` while the daemon is still starting (neither
    /// bound nor exited), so a read at app start does not call a daemon still starting "not ours".
    // ponytail: bounded by the caller's timeout; a first start that downloads the engine for longer reads "not
    // ours" until the UI reads again (next mount of Devices, Settings or Dashboard).
    fn ours_once_started(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while self.spawned
            && !self.bound.load(Ordering::SeqCst)
            && !self.exited.load(Ordering::SeqCst)
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        self.ours()
    }
}

/// `common serve` prints `listening on http://<addr>` once it has bound its port, before it starts
/// peer networking or anything else (crates/common-cli `serve`). Changing that line makes the app
/// treat its own daemon as another one: fail closed (no firewall check, Quit kills it on Windows).
fn is_listening_line(line: &[u8], addr: SocketAddr) -> bool {
    String::from_utf8_lossy(line).trim_end() == format!("listening on http://{addr}")
}

/// Ask the daemon to stop, wait for it, and only then kill it. Stopping gracefully unloads the model,
/// stops the llama.cpp engine and sends the mDNS goodbye, so other devices drop this one at once
/// instead of listing it as nearby for minutes. A plain `kill()` (SIGKILL / TerminateProcess) did
/// none of that and, before the job object, left the engine running orphaned.
fn stop_daemon(child: CommandChild, run: &Run) {
    let exited = &run.exited;
    // Our daemon already exited (crashed, or lost the port to another daemon): whatever answers on the port now
    // is not ours, and on unix its pid may have been reused. Ask nothing, kill nothing.
    if exited.load(Ordering::SeqCst) {
        return;
    }
    // SAFETY: plain kill(2) on the pid of a child this process spawned and has not reaped. The signal reaches
    // our child whether or not it bound the port yet.
    #[cfg(unix)]
    let asked = unsafe { libc::kill(child.pid() as libc::pid_t, libc::SIGTERM) } == 0;
    // Windows has no SIGTERM: `POST /api/shutdown` takes the daemon down the same path, but only when the port
    // is our child's (a child still starting while another daemon holds the port is killed instead).
    #[cfg(not(unix))]
    let asked = ask_to_stop(run.bound.load(Ordering::SeqCst), daemon_addr());
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
    /// The serving daemon is (or, until it exits, was) the one this app started.
    spawned: bool,
    /// Times this app restarted its daemon after it exited unexpectedly.
    restarts: u32,
    restarting: bool,
    gave_up: bool,
    /// It could not be started at launch and is not tried again.
    start_failed: bool,
    /// Where this app writes the daemon's output; `None` when it could not.
    log_path: Option<String>,
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

/// [`request_shutdown`] to `addr` only when our daemon bound it (`bound`); before that whatever answers
/// there is another daemon. Used on Windows only; built everywhere so the tests run on every platform.
#[cfg_attr(unix, allow(dead_code))]
fn ask_to_stop(bound: bool, addr: SocketAddr) -> bool {
    bound && request_shutdown(addr)
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
    state.info()
}

/// Windows Firewall state for the bundled daemon; `supported: false` elsewhere, `adopted` when the
/// serving daemon is not the one this app started (or that one exited). Read-only; `Err` when the
/// rules could not be read. Waits while the app's own daemon is still starting.
// ponytail: during a restart's backoff the exited run reads "not ours" (adopted) until the UI reads again.
#[tauri::command]
async fn firewall_status(state: tauri::State<'_, Daemon>) -> Result<firewall::Status, String> {
    let run = state.lock().run.clone();
    tauri::async_runtime::spawn_blocking(move || firewall::status_for(run.ours_once_started(HEALTH_TIMEOUT)))
        .await
        .map_err(|e| e.to_string())?
}

/// Adds the rule through the Windows elevation prompt (the user's click there is the consent);
/// `Err("cancelled")` when the user declines it. Refused for a daemon this app did not start, or
/// once the one it started exited.
#[tauri::command]
async fn allow_through_firewall(state: tauri::State<'_, Daemon>) -> Result<(), String> {
    let run = state.lock().run.clone();
    tauri::async_runtime::spawn_blocking(move || firewall::allow_for(run.ours_once_started(HEALTH_TIMEOUT)))
        .await
        .map_err(|e| e.to_string())?
}

/// Starts the bundled daemon into `s` (the caller holds the lock) and watches it: its output goes to stderr and
/// the log, its listening line marks the bind, and its exit goes to [`Daemon::after_exit`] and, when that says
/// so, a restart through here again. False when it could not be started.
fn spawn_daemon(app: &tauri::AppHandle, s: &mut Supervised) -> bool {
    let daemon = app.state::<Daemon>();
    let addr = daemon_addr();
    // The sidecar may be absent in `pnpm dev` without a packaged binary; the UI then shows
    // "service not running" and the developer starts `common serve` by hand.
    let started = match app.shell().sidecar(SIDECAR) {
        Ok(cmd) => cmd
            .args(daemon_args())
            .spawn()
            .map_err(|e| format!("could not start the daemon sidecar: {e}")),
        Err(e) => Err(format!("daemon sidecar not available: {e}")),
    };
    let (mut rx, child) = match started {
        Ok(x) => x,
        Err(e) => {
            daemon.note(&e);
            return false;
        }
    };
    #[cfg(windows)]
    end_with_this_app(child.pid());
    daemon.note(&format!("started the daemon, pid {}", child.pid()));
    let run = Arc::new(Run {
        spawned: true,
        ..Run::default()
    });
    s.child = Some(child);
    s.run = run.clone();
    let code = Arc::new(AtomicI32::new(i32::MIN));
    let (ev_run, ev_code, ev_app) = (run.clone(), code.clone(), app.clone());
    tauri::async_runtime::spawn(async move {
        let daemon = ev_app.state::<Daemon>();
        while let Some(ev) = rx.recv().await {
            match ev {
                CommandEvent::Stdout(l) | CommandEvent::Stderr(l) => {
                    if is_listening_line(&l, addr) {
                        ev_run.bound.store(true, Ordering::SeqCst);
                    }
                    eprint!("common: {}", String::from_utf8_lossy(&l));
                    daemon.log(&l);
                }
                CommandEvent::Terminated(p) => {
                    daemon.note(&format!("daemon exited with code {:?}, signal {:?}", p.code, p.signal));
                    ev_code.store(p.code.unwrap_or(-1), Ordering::SeqCst);
                    ev_run.exited.store(true, Ordering::SeqCst);
                    let app = ev_app.clone();
                    std::thread::spawn(move || {
                        let daemon = app.state::<Daemon>();
                        let mut next = daemon.after_exit(p.code, Instant::now());
                        while let Some(wait) = next {
                            std::thread::sleep(wait);
                            next = daemon.restart(daemon_addr(), |s| spawn_daemon(&app, s));
                        }
                    });
                }
                _ => {}
            }
        }
    });
    let started = app.clone();
    std::thread::spawn(move || {
        let daemon = started.state::<Daemon>();
        let start = Instant::now();
        while start.elapsed() < HEALTH_TIMEOUT {
            // A 200 before our daemon bound the port is another daemon's.
            if run.ours() && health_ok(addr) {
                daemon.note(&format!("daemon healthy on {addr}"));
                return;
            }
            if run.exited.load(Ordering::SeqCst) {
                // Exit code 2 is a command-line usage error: the bundled daemon
                // does not accept the arguments this app passes.
                let c = code.load(Ordering::SeqCst);
                let why = if c == 2 {
                    " (it rejected its command line: the app and its bundled daemon come from different builds)"
                } else {
                    ""
                };
                daemon.note(&format!("daemon exited with code {c} before becoming healthy{why}"));
                return;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        daemon.note(&format!("daemon did not become healthy within {HEALTH_TIMEOUT:?}"));
    });
    true
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
            // macOS: ~/Library/Logs/<identifier>; Windows and Linux: <local data>/<identifier>/logs.
            let log_path = app.path().app_log_dir().ok().map(|d| d.join("daemon.log"));
            app.manage(Daemon::new(log_path));
            app.state::<Daemon>().launch(addr, |s| spawn_daemon(app.handle(), s));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_info,
            firewall_status,
            allow_through_firewall
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            let daemon = app.state::<Daemon>();
            // Under the lock: a restart still waiting for its backoff then starts nothing.
            let (child, run) = {
                let mut s = daemon.lock();
                s.quitting = true;
                (s.child.take(), s.run.clone())
            };
            if let Some(child) = child {
                stop_daemon(child, &run);
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

    fn run(spawned: bool) -> Arc<Run> {
        Arc::new(Run {
            spawned,
            ..Run::default()
        })
    }

    /// The finding: the firewall commands read the startup-only `spawned`, so after the app's own
    /// daemon exited (and another took the port) they still checked and added the bundled path's rule.
    #[test]
    fn a_daemon_that_exited_is_no_longer_ours() {
        let r = run(true);
        r.bound.store(true, Ordering::SeqCst);
        assert!(r.ours());
        r.exited.store(true, Ordering::SeqCst);
        assert!(!r.ours());
        assert!(!run(false).ours(), "adopted");
    }

    /// Open after round 2: the app's daemon still starting while another daemon holds the port counted
    /// as ours, so the firewall commands checked the bundled program and, on Windows, Quit sent
    /// `/api/shutdown` to the other daemon. Ours only once our child said it bound the port.
    #[test]
    fn a_daemon_still_starting_is_not_ours_until_it_binds_the_port() {
        let r = run(true);
        assert!(
            !r.ours(),
            "spawned but not bound: whatever answers on the port is not ours yet"
        );
        assert!(!r.ours_once_started(Duration::from_millis(100)), "never bound");

        let r = run(true);
        let later = r.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            later.bound.store(true, Ordering::SeqCst);
        });
        assert!(r.ours_once_started(Duration::from_secs(5)), "waits for the bind");
        t.join().unwrap();

        let r = run(true);
        let later = r.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            later.exited.store(true, Ordering::SeqCst); // lost the port to the other daemon
        });
        let start = Instant::now();
        assert!(!r.ours_once_started(Duration::from_secs(5)));
        assert!(start.elapsed() < Duration::from_secs(2), "stops waiting once it exited");
        t.join().unwrap();

        let start = Instant::now();
        assert!(!run(false).ours_once_started(Duration::from_secs(5)), "adopted");
        assert!(start.elapsed() < Duration::from_secs(1), "nothing to wait for");
    }

    /// Only our child writes to its stdout: its listening line for our address marks the bind.
    #[test]
    fn the_listening_line_for_our_address_marks_the_bind() {
        let addr: SocketAddr = "127.0.0.1:8192".parse().unwrap();
        assert!(is_listening_line(b"listening on http://127.0.0.1:8192\n", addr));
        assert!(is_listening_line(b"listening on http://127.0.0.1:8192\r\n", addr));
        assert!(!is_listening_line(b"listening on http://127.0.0.1:8193\n", addr));
        assert!(!is_listening_line(
            b"cannot bind 127.0.0.1:8192: address in use\n",
            addr
        ));
        assert!(!is_listening_line(
            b"peer id listening on http://127.0.0.1:8192\n",
            addr
        ));
        // The CLI's own line (`serve`, printed right after the bind): a change there fails here instead of
        // silently making the app's own daemon "not ours".
        assert!(include_str!("../../../../crates/common-cli/src/main.rs")
            .contains(r#"println!("listening on http://{addr}");"#));
    }

    /// Windows Quit: `/api/shutdown` goes to whatever serves the port, so it is sent only when our
    /// child bound it; otherwise the caller kills the child, which has started nothing yet.
    #[test]
    fn quit_asks_the_port_to_stop_only_when_our_daemon_bound_it() {
        let (addr, req) = answer("202 Accepted");
        assert!(!ask_to_stop(false, addr), "another daemon's port");
        assert!(ask_to_stop(true, addr), "the one request goes to our own daemon");
        assert!(req.join().unwrap().starts_with("POST /api/shutdown "));
    }

    /// F-08-04: a daemon that dies is restarted after 1, 2, then 4 s; a fourth exit within a minute of the
    /// first restart is not restarted (a daemon that crashes at every start); after a quiet minute it is again.
    #[test]
    fn restarts_back_off_and_stop_after_three_within_a_minute() {
        let s = Duration::from_secs;
        let mut r = Restarts::default();
        let t = Instant::now();
        assert_eq!(r.next(t), Some(s(1)));
        assert_eq!(r.next(t + s(1)), Some(s(2)));
        assert_eq!(r.next(t + s(3)), Some(s(4)));
        assert_eq!(r.next(t + s(10)), None);
        assert_eq!(r.next(t + s(59)), None, "the first restart is still within the minute");
        assert_eq!(r.next(t + s(63)), Some(s(1)), "all three are older than a minute");
    }

    /// Only an unexpected exit is restarted: not one the app asked for while quitting, not a daemon stopped on
    /// request (exit 0) and not one that rejected its command line (exit 2). A signal or an error exit is.
    #[test]
    fn only_an_unexpected_exit_is_restarted() {
        let s = Duration::from_secs;
        let t = Instant::now();
        let d = Daemon::new(None);
        assert_eq!(d.after_exit(Some(0), t), None, "stopped on request");
        assert_eq!(d.after_exit(Some(2), t), None, "rejected its command line");
        assert!(!d.lock().restarting);
        assert_eq!(d.after_exit(None, t), Some(s(1)), "killed by a signal");
        assert!(d.lock().restarting);
        assert_eq!(d.after_exit(Some(101), t), Some(s(2)), "panicked");
        assert_eq!(d.after_exit(Some(1), t), Some(s(4)), "failed");
        assert_eq!(d.after_exit(Some(1), t), None, "a fourth time within the minute");
        let st = d.lock();
        assert!(st.gave_up && !st.restarting);
        drop(st);

        let q = Daemon::new(None);
        q.lock().quitting = true;
        assert_eq!(q.after_exit(None, t), None, "the app is quitting");
        assert!(!q.lock().restarting);
    }

    /// Never two daemons: a restart that finds a daemon serving the port uses it (not ours, as at launch) and
    /// starts none; Quit during the backoff starts none either.
    #[test]
    fn a_restart_never_starts_a_second_daemon_nor_one_while_quitting() {
        let d = Daemon::new(None);
        d.lock().run = run(true);
        d.lock().restarting = true;
        let (other, req) = answer("200 OK");
        d.restart(other, |_| panic!("a daemon already serves the port"));
        assert!(req.join().unwrap().starts_with("GET /health "));
        let st = d.lock();
        assert!(!st.run.spawned, "the serving daemon is adopted, not ours");
        assert!(!st.restarting && st.restarted == 0);
        drop(st);

        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        d.restart(free, |s| {
            s.run = run(true);
            true
        });
        assert_eq!(d.lock().restarted, 1);

        d.lock().quitting = true;
        d.restart(free, |_| panic!("the app is quitting"));
        assert_eq!(d.lock().restarted, 1);
    }

    /// Round 6: a sidecar that could not be started during a restart stopped supervision silently (no retry, no
    /// gave_up) while the UI said "Retrying automatically". It counts as one more exit, within the same bounds.
    #[test]
    fn a_restart_that_cannot_start_the_sidecar_retries_then_gives_up() {
        let s = Duration::from_secs;
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let (dir, log) = temp_log("spawn-failed");
        let d = Daemon::new(Some(log.clone()));
        assert_eq!(d.after_exit(None, Instant::now()), Some(s(1)), "killed");
        assert_eq!(d.restart(free, |_| false), Some(s(2)), "could not start: tried again");
        let st = d.lock();
        assert!(st.restarting && !st.gave_up);
        drop(st);
        assert_eq!(d.restart(free, |_| false), Some(s(4)));
        assert_eq!(d.restart(free, |_| false), None, "three tries within the minute");
        let st = d.lock();
        assert!(st.gave_up && !st.restarting && st.restarted == 0);
        drop(st);
        // Round 6b: the give-up line said "exited again after 3 restarts" although no restart had started a daemon.
        let text = fs::read_to_string(&log).unwrap();
        assert!(
            text.ends_with(
                "common-desktop: could not start the daemon in 3 restart attempts within 60s; not trying again\n"
            ),
            "{text}"
        );
        assert!(!text.contains("exited again"), "{text}");
        fs::remove_dir_all(dir).unwrap();
    }

    /// The give-up line after real exits says the daemon exited again.
    #[test]
    fn the_give_up_line_after_exits_says_it_exited_again() {
        let t = Instant::now();
        let (dir, log) = temp_log("exited-again");
        let d = Daemon::new(Some(log.clone()));
        for _ in 0..RESTART_MAX {
            assert!(d.after_exit(None, t).is_some());
        }
        assert_eq!(d.after_exit(None, t), None);
        let text = fs::read_to_string(&log).unwrap();
        assert!(
            text.ends_with(
                "common-desktop: the daemon exited again after 3 restart attempts within 60s; not restarting it\n"
            ),
            "{text}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    /// Round 6b: a sidecar that could not be started at launch left the UI saying "Retrying automatically", but
    /// nothing retried. The shell now reports `start_failed` (and retries nothing); a launch that starts the daemon,
    /// or finds one serving, does not.
    #[test]
    fn a_launch_that_cannot_start_the_sidecar_reports_start_failed() {
        let free = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let d = Daemon::new(None);
        d.launch(free, |_| false);
        let st = d.lock();
        assert!(st.start_failed && !st.restarting && !st.gave_up);
        drop(st);
        let info = serde_json::to_value(d.info()).unwrap();
        assert_eq!(info["start_failed"], true, "{info}");
        assert_eq!(info["spawned"], false, "{info}");

        let d = Daemon::new(None);
        d.launch(free, |s| {
            s.run = run(true);
            true
        });
        assert_eq!(serde_json::to_value(d.info()).unwrap()["start_failed"], false);

        let (other, req) = answer("200 OK");
        let d = Daemon::new(None);
        d.launch(other, |_| panic!("a daemon already serves the port"));
        assert!(req.join().unwrap().starts_with("GET /health "));
        assert!(!d.lock().start_failed);
    }

    /// A fresh `<temp>/common-desktop-<name>-<pid>/daemon.log` path; returns the folder and the path.
    fn temp_log(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("common-desktop-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let log = dir.join("daemon.log");
        (dir, log)
    }

    /// F-10-06: the daemon's output is kept, capped: past the cap `daemon.log` becomes `daemon.log.1` (replacing
    /// the older one), so two files at most, the newest lines in `daemon.log`; a restarted app appends.
    #[test]
    fn the_daemon_log_rotates_at_its_cap_and_keeps_one_older_file() {
        let dir = std::env::temp_dir().join(format!("common-desktop-log-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("logs").join("daemon.log");
        let older = dir.join("logs").join("daemon.log.1");
        let read = |p: &Path| fs::read_to_string(p).unwrap();
        let line = |c: &str| format!("{}\n", c.repeat(59)); // 60 bytes

        let mut log = LogFile::open(&path, 100).unwrap();
        log.write_line(line("a").as_bytes()).unwrap();
        log.write_line("b".repeat(59).as_bytes()).unwrap(); // no newline: one is added
        assert_eq!(read(&path), line("b"));
        assert_eq!(read(&older), line("a"));
        drop(log);

        let mut log = LogFile::open(&path, 100).unwrap();
        log.write_line(line("c").as_bytes()).unwrap();
        assert_eq!(read(&path), line("c"));
        assert_eq!(read(&older), line("b"), "the oldest lines are gone");
        log.write_line(b"d\n").unwrap();
        assert_eq!(read(&path), line("c") + "d\n", "under the cap: appended");
        assert_eq!(fs::read_dir(dir.join("logs")).unwrap().count(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(&dir).unwrap();
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
