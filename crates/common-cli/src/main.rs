//! `common` CLI: hardware, models, run, chat, serve, setup, doctor, bench, replay, and the mesh
//! commands peers, groups, privacy, audit, diagnostics (M3).
//!
//! Errors print `error[<code>]: <message>` plus the hint to stderr and exit 1; with `--json` the
//! error is printed as JSON instead (F-08-03). Ctrl-C, SIGTERM and SIGHUP print `stopped` and
//! exit 130 after the engine has been stopped (F-08-02).

mod mesh;
mod models;
mod run;

use clap::{Parser, Subcommand, ValueEnum};
use common_artifacts::backend::{self, BackendPin, BackendPins};
use common_artifacts::{Progress, Store};
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_hardware::HardwareReport;
use common_models::human_bytes;
use common_runtime::manager::ModelManager;
use common_runtime::DevicePreference;
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "common",
    version,
    about = "Common — run AI models on the best available compute"
)]
pub struct Cli {
    /// Machine-readable JSON output (errors are printed as JSON too)
    #[arg(long, global = true)]
    pub json: bool,
    /// Log to stderr (-v info, -vv debug); RUST_LOG overrides
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Show detected hardware and usable backends
    Hardware,
    /// List, search, pull, inspect, import, verify or remove models
    Models {
        #[command(subcommand)]
        cmd: models::ModelsCmd,
    },
    /// Generate text from a prompt
    Run(run::RunArgs),
    /// Interactive chat (type /quit to exit)
    Chat(run::ChatArgs),
    /// Serve the local HTTP API
    Serve(ServeArgs),
    /// Download and verify the pinned inference engine (and the test fixtures with --fixtures)
    Setup {
        /// Also pull the stories15M-q4_0 and stories260K fixture models
        #[arg(long)]
        fixtures: bool,
    },
    /// Check hardware, engine, disk and downloaded models
    Doctor,
    /// Benchmark a model and write the result to ./benchmarks/results/ (relative to the current directory)
    Bench(run::BenchArgs),
    /// Re-run a saved execution manifest and check that the same tokens come back (X-15)
    Replay(run::ReplayArgs),
    /// Paired devices: list, discover, pair, join, rm, revoke
    Peers {
        #[command(subcommand)]
        cmd: mesh::PeersCmd,
    },
    /// Trusted groups and what their members may run here
    Groups {
        #[command(subcommand)]
        cmd: mesh::GroupsCmd,
    },
    /// Show or change the privacy mode (local-only is the default and the only private mode)
    Privacy(mesh::PrivacyArgs),
    /// Print the audit trail (ids, codes and counts; never prompts)
    Audit(mesh::AuditArgs),
    /// Write the redacted diagnostics bundle
    Diagnostics(mesh::DiagnosticsArgs),
}

#[derive(clap::Args, Debug)]
pub struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    #[arg(long, default_value_t = 8192)]
    pub port: u16,
    /// Require this API key (mandatory for a non-loopback host)
    #[arg(long)]
    pub api_key: Option<String>,
    #[arg(long, default_value_t = 4)]
    pub max_concurrent: usize,
    /// Shut down when the parent process exits (used by the desktop app's sidecar spawn).
    #[arg(long, default_value_t = false)]
    exit_with_parent: bool,
    /// Also open the authenticated peer endpoint (or `network_enabled` in config.json)
    #[arg(long)]
    pub network: bool,
    /// Accept inference requests from paired devices in an allowing group (implies --network; or `serve_peers` in config.json)
    #[arg(long)]
    pub serve_peers: bool,
    #[command(flatten)]
    pub net: mesh::NetArgs,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    Auto,
    Cpu,
    Gpu,
}

impl From<Device> for DevicePreference {
    fn from(d: Device) -> Self {
        match d {
            Device::Auto => DevicePreference::Auto,
            Device::Cpu => DevicePreference::Cpu,
            Device::Gpu => DevicePreference::Gpu,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    reset_sigpipe();
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match dispatch(&cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e.code == ErrorCode::Cancelled => {
            eprintln!("stopped");
            ExitCode::from(130)
        }
        Err(e) => {
            report(&e, cli.json);
            ExitCode::from(1)
        }
    }
}

async fn dispatch(cli: &Cli) -> Result<()> {
    match &cli.cmd {
        Cmd::Hardware => hardware(cli.json),
        Cmd::Models { cmd } => models::dispatch(cmd, cli.json).await,
        Cmd::Run(a) => run::run(a, cli.json).await,
        Cmd::Chat(a) => run::chat(a).await,
        Cmd::Serve(a) => serve(a).await,
        Cmd::Setup { fixtures } => setup(*fixtures).await,
        Cmd::Doctor => doctor(cli.json).await,
        Cmd::Bench(a) => run::bench(a, cli.json).await,
        Cmd::Replay(a) => run::replay(a, cli.json).await,
        Cmd::Peers { cmd } => mesh::peers(cmd, cli.json).await,
        Cmd::Groups { cmd } => mesh::groups(cmd, cli.json).await,
        Cmd::Privacy(a) => mesh::privacy(a, cli.json).await,
        Cmd::Audit(a) => mesh::audit(a, cli.json),
        Cmd::Diagnostics(a) => mesh::diagnostics(a),
    }
}

/// Which stop signal arrived: Ctrl-C, or a terminate request (SIGTERM from a supervisor or
/// `kill`, SIGHUP from a closed terminal).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Interrupt,
    Terminate,
}

/// Every wait for Ctrl-C goes through here so SIGTERM and SIGHUP take the same path: left at
/// their default disposition they end the process without unwinding, so `unload()` never runs,
/// `kill_on_drop` never fires and the engine stays resident with the model.
pub async fn shutdown_signal() -> Stop {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        async fn wait(kind: SignalKind) {
            if let Ok(mut s) = signal(kind) {
                if s.recv().await.is_some() {
                    return;
                }
            }
            std::future::pending::<()>().await
        }
        tokio::select! {
            _ = tokio::signal::ctrl_c() => Stop::Interrupt,
            () = wait(SignalKind::terminate()) => Stop::Terminate,
            () = wait(SignalKind::hangup()) => Stop::Terminate,
        }
    }
    #[cfg(windows)]
    {
        // Closing the console window, logging off and shutting down arrive as their own console
        // events, not Ctrl+C; each gets the same graceful stop (unload the model, stop the engine).
        use tokio::signal::windows::{ctrl_break, ctrl_close, ctrl_logoff, ctrl_shutdown};
        let (mut close, mut logoff, mut shutdown, mut brk) =
            (ctrl_close(), ctrl_logoff(), ctrl_shutdown(), ctrl_break());
        tokio::select! {
            _ = tokio::signal::ctrl_c() => Stop::Interrupt,
            Some(()) = async { close.as_mut().ok()?.recv().await } => Stop::Terminate,
            Some(()) = async { logoff.as_mut().ok()?.recv().await } => Stop::Terminate,
            Some(()) = async { shutdown.as_mut().ok()?.recv().await } => Stop::Terminate,
            Some(()) = async { brk.as_mut().ok()?.recv().await } => Stop::Interrupt,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = tokio::signal::ctrl_c().await;
        Stop::Interrupt
    }
}

/// Stop `llama-server` processes started from `binary` whose parent is gone (ppid 1): a `common`
/// that died without unwinding (SIGKILL, crash) leaves its engine behind with the whole model
/// resident. Returns the pids signalled.
/// ponytail: `ps` scan matched on argv[0]; a user's own detached start of the pinned binary counts
/// as ours. Upgrade: pid registry under COMMON_HOME if that ever bites.
pub fn reap_orphaned_engines(binary: &Path) -> Vec<u32> {
    #[cfg(not(unix))]
    {
        let _ = binary;
        Vec::new()
    }
    #[cfg(unix)]
    {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid=,args="])
            .output()
        else {
            return Vec::new();
        };
        let killed: Vec<u32> = orphans(&String::from_utf8_lossy(&out.stdout), binary)
            .into_iter()
            // SAFETY: plain syscall on a pid read from the process table; worst case ESRCH.
            .filter(|&pid| unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) == 0 })
            .collect();
        if !killed.is_empty() {
            tracing::warn!(pids = ?killed, "stopped orphaned llama-server engines left by an earlier run");
        }
        killed
    }
}

/// Pids in `ps -axo pid=,ppid=,args=` output whose parent is 1 and whose argv[0] is `binary`.
#[cfg_attr(not(unix), allow(dead_code))]
fn orphans(ps: &str, binary: &Path) -> Vec<u32> {
    ps.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let pid: u32 = f.next()?.parse().ok()?;
            let ppid: u32 = f.next()?.parse().ok()?;
            (ppid == 1 && Path::new(f.next()?) == binary).then_some(pid)
        })
        .collect()
}

/// Die quietly on a closed stdout (`common models list | head`) like every other Unix CLI,
/// instead of Rust's default "failed printing to stdout: Broken pipe" panic.
/// A server must survive losing whoever reads its output (the desktop app holds the read end of the
/// daemon's stdout/stderr pipes): ignore SIGPIPE so a write fails with EPIPE instead of killing the
/// process before it has stopped its engine.
fn ignore_sigpipe() {
    // SAFETY: only changes the process-wide disposition of SIGPIPE; no handler, no shared state.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }
}

/// Point stdout and stderr at /dev/null. Called when the parent is gone: its end of our output pipes
/// went with it, and every later write (println!, eprintln!, the log) would fail or panic.
fn detach_stdio() {
    #[cfg(unix)]
    // SAFETY: open(2) and dup2(2) on fds 1 and 2; nothing else holds raw copies of them.
    unsafe {
        let fd = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        if fd >= 0 {
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
            libc::close(fd);
        }
    }
}

fn reset_sigpipe() {
    // SAFETY: only changes the process-wide disposition of SIGPIPE to the default; no handler, no shared state.
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        // A failed log write must not turn into an eprintln! that panics on a broken pipe.
        .log_internal_errors(false)
        .init();
}

/// F-08-03: code + message + hint on stderr, or the error JSON with `--json`.
fn report(e: &CommonError, json: bool) {
    if json {
        eprintln!(
            "{}",
            serde_json::to_string(e).unwrap_or_else(|_| format!("{{\"code\":\"{}\"}}", e.code))
        );
        return;
    }
    eprintln!("error[{}]: {}", e.code, e.message);
    if let Some(h) = &e.hint {
        eprintln!("  hint: {h}");
    }
}

/// The llama.cpp pin for this OS/arch.
pub fn pin() -> Result<BackendPin> {
    BackendPins::builtin()?
        .for_current_platform("llama.cpp")
        .cloned()
        .ok_or_else(|| {
            CommonError::new(
                ErrorCode::BackendUnavailable,
                format!("no pinned llama.cpp build for {}", backend::current_platform()),
            )
        })
}

/// Everything a model-running command needs: paths, hardware probe, verified engine, manager.
pub async fn context() -> Result<ModelManager> {
    let paths = Paths::from_env();
    paths.ensure_dirs()?;
    let hardware = common_hardware::probe()?;
    let pin = pin()?;
    // First run (fresh data dir, e.g. the packaged desktop app): provision the pinned engine automatically.
    // Download is hash-verified; nothing else is fetched without the user asking (models stay opt-in).
    let engine_dir = if backend::archive_path(&paths, &pin).exists() {
        backend::verify_backend(&paths, &pin)
            .map_err(|e| CommonError::new(ErrorCode::BackendUnavailable, e.message).with_hint("common setup"))?
    } else {
        eprintln!(
            "engine {} {} is not installed yet; downloading the pinned release",
            pin.name, pin.release
        );
        backend::ensure_backend(&paths, &pin, &mut progress("engine")).await?
    };
    reap_orphaned_engines(&backend::binary(&engine_dir, "llama-server"));
    let backend = common_runtime::backend_by_name("llama.cpp", engine_dir, paths.clone())
        .ok_or_else(|| CommonError::internal("llama.cpp adapter missing"))?;
    Ok(ModelManager::new(paths.clone(), Store::new(paths), backend, hardware))
}

/// Plain-text download progress on stderr, redrawn at most every 200 ms.
pub fn progress(label: &str) -> impl FnMut(Progress) {
    let label = label.to_string();
    let mut last: Option<Instant> = None;
    move |p: Progress| {
        let line = format!(
            "{label}: {} / {}",
            human_bytes(p.downloaded_bytes),
            human_bytes(p.total_bytes)
        );
        if p.done {
            eprintln!(
                "\r{line} done{}",
                if p.network_bytes == 0 {
                    " (already present, re-verified)"
                } else {
                    ""
                }
            );
        } else if last.is_none_or(|t| t.elapsed() >= Duration::from_millis(200)) {
            last = Some(Instant::now());
            eprint!("\r{line}");
        }
    }
}

fn hardware(json: bool) -> Result<()> {
    let r = common_hardware::probe()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print!("{}", hardware_text(&r));
    }
    Ok(())
}

pub fn hardware_text(r: &HardwareReport) -> String {
    let mut s = format!(
        "host     {} {} ({}, {})\n",
        r.host.os, r.host.os_version, r.host.arch, r.host.chip
    );
    s += &format!(
        "cpu      {} ({} physical / {} logical cores)\n",
        r.cpu.model, r.cpu.physical_cores, r.cpu.logical_cores
    );
    let unified = if r.memory.unified { ", unified" } else { "" };
    s += &format!(
        "memory   {} total, {} available{unified}\n",
        human_bytes(r.memory.total_bytes),
        human_bytes(r.memory.available_bytes)
    );
    for g in &r.gpus {
        let extra: Vec<String> = [g.cores.map(|c| format!("{c} cores")), g.memory_bytes.map(human_bytes)]
            .into_iter()
            .flatten()
            .collect();
        s += &format!(
            "gpu      {} ({}, {}, {})\n",
            g.name,
            g.vendor,
            g.backend,
            extra.join(", ")
        );
    }
    for b in &r.backends {
        let state = if b.available {
            "available".to_string()
        } else {
            format!("unavailable ({})", b.reason.as_deref().unwrap_or("unknown reason"))
        };
        s += &format!("backend  {:<7} {state}\n", b.name);
    }
    if let Some(d) = r.disk_free_bytes {
        s += &format!("disk     {} free\n", human_bytes(d));
    }
    s += &format!("probe    {:.1} ms\n", r.probe_ms);
    s
}

/// Resolves when the parent process goes away (ppid changes, typically to 1). Never resolves when
/// `enabled` is false. Polling once a second is enough for a sidecar that must not outlive its app.
async fn parent_exited(enabled: bool) {
    if !enabled {
        std::future::pending::<()>().await;
    }
    #[cfg(unix)]
    {
        let original = unsafe { libc::getppid() };
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = unsafe { libc::getppid() };
            if now != original || now == 1 {
                return;
            }
        }
    }
    #[cfg(not(unix))]
    {
        std::future::pending::<()>().await;
    }
}

/// `--network` / `--serve-peers` switch the persisted settings on (the same `config.json` the API's
/// `PUT /api/network` writes; the API reads it in `AppState::new`), so the flags and the desktop
/// toggle are one switch. Nothing listens for peers unless that switch is on (SD-04/SD-05).
// ponytail: persisted, not a run-only override: `AppState::set_settings` lasts only until
// config.json next changes (any CLI or desktop edit). A per-run override needs an API flag.
fn mesh_settings(a: &ServeArgs, paths: &Paths) -> Result<mesh::Settings> {
    let before = mesh::load_settings(paths)?;
    let after = mesh::Settings {
        network_enabled: before.network_enabled || a.network || a.serve_peers,
        serve_peers: before.serve_peers || a.serve_peers,
        ..before
    };
    if after != before {
        after.save(paths)?;
    }
    Ok(after)
}

async fn serve(a: &ServeArgs) -> Result<()> {
    ignore_sigpipe();
    let m = Arc::new(context().await?);
    let settings = mesh_settings(a, &m.paths)?;
    let config = common_api::ApiConfig {
        host: a.host.clone(),
        port: a.port,
        api_key: a.api_key.clone(),
        max_concurrent: a.max_concurrent,
        network: common_network::NetworkConfig {
            enabled: settings.network_enabled,
            ..a.net.config()
        },
        ..Default::default()
    };
    // PolicyDenied (non-loopback without a key) before anything is printed or bound.
    let addr = common_api::bind_addr(&config)?;
    let paths = m.paths.clone();
    let state = Arc::new(common_api::AppState::new(
        config.clone(),
        m.paths.clone(),
        common_artifacts::Catalog::builtin()?,
        m,
    )?);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CommonError::io(format!("cannot bind {addr}: {e}")))?;
    let addr = listener.local_addr()?;
    // CLI mesh mutations find this daemon here and go through its API for their live effects.
    mesh::write_serve_file(&paths, addr)?;
    println!("listening on http://{addr}");
    if settings.network_enabled {
        // serve_on() starts the peer endpoint before it accepts HTTP; a failure there is logged
        // (warn) and the API still serves with peer networking off.
        println!("peer id {}", state.identity.id);
        println!(
            "peer endpoint udp://{}:{} (serving peers: {}, mdns: {})",
            config.network.bind_addr,
            config.network.listen_port,
            if settings.serve_peers { "yes" } else { "no" },
            if config.network.mdns { "on" } else { "off" }
        );
    }
    // serve_on() stops the peer Node and unloads the model (stops the engine) once this future resolves.
    let exit_with_parent = a.exit_with_parent;
    let r = common_api::serve_on(state, listener, async move {
        tokio::select! {
            _ = shutdown_signal() => {}
            _ = parent_exited(exit_with_parent) => {
                // The app is gone and so is the reader of our stdout/stderr; detach before anything
                // writes, so the shutdown below (unload the model, stop the engine) always completes.
                detach_stdio();
                tracing::warn!("parent process exited; shutting down");
            }
        }
    })
    .await;
    mesh::remove_serve_file(&paths);
    r
}

async fn setup(fixtures: bool) -> Result<()> {
    let paths = Paths::from_env();
    paths.ensure_dirs()?;
    let pin = pin()?;
    let dir = backend::ensure_backend(&paths, &pin, &mut progress("engine")).await?;
    println!("engine   {} {} verified at {}", pin.name, pin.release, dir.display());
    if fixtures {
        let store = Store::new(paths);
        let catalog = common_artifacts::Catalog::builtin()?;
        for id in ["stories15M-q4_0", "stories260K"] {
            let entry = catalog.get(id).ok_or_else(|| {
                CommonError::new(
                    ErrorCode::ModelNotFound,
                    format!("{id} is missing from the built-in catalog"),
                )
            })?;
            let m = common_artifacts::pull(&store, entry, &mut progress(id)).await?;
            println!(
                "model    {} verified ({}, license {})",
                m.id,
                human_bytes(m.bytes),
                m.license
            );
        }
    }
    Ok(())
}

async fn doctor(json: bool) -> Result<()> {
    const LOW_DISK: u64 = 2_000_000_000;
    const LOW_MEMORY: u64 = 1_000_000_000;
    let paths = Paths::from_env();
    let hw = common_hardware::probe()?;
    let mut warnings = Vec::new();
    let engine = pin().and_then(|p| backend::verify_backend(&paths, &p).map(|dir| (p, dir)));
    let engine_json = match &engine {
        Ok((p, dir)) => {
            let version = common_runtime::backend_by_name(&p.name, dir.clone(), paths.clone())
                .map(|b| b.version())
                .unwrap_or_default();
            serde_json::json!({ "present": true, "verified": true, "release": p.release, "version": version, "dir": dir.display().to_string() })
        }
        Err(e) => {
            warnings.push("engine is not ready: run `common setup`".to_string());
            serde_json::json!({ "present": false, "verified": false, "error": e })
        }
    };
    let store = Store::new(paths.clone());
    let downloaded: Vec<String> = store
        .list()?
        .into_iter()
        .filter(|m| store.blob_path(&m.sha256).is_file())
        .map(|m| m.id)
        .collect();
    if hw.disk_free_bytes.is_some_and(|d| d < LOW_DISK) {
        warnings.push(format!(
            "low disk: {} free",
            human_bytes(hw.disk_free_bytes.unwrap_or(0))
        ));
    }
    if hw.memory.available_bytes < LOW_MEMORY {
        warnings.push(format!(
            "low available memory: {}; close other apps before loading large models",
            human_bytes(hw.memory.available_bytes)
        ));
    }
    if json {
        let v = serde_json::json!({
            "common_version": common_core::VERSION, "common_home": paths.root.display().to_string(),
            "hardware": hw, "engine": engine_json, "models_downloaded": downloaded, "warnings": warnings,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    println!(
        "common   {} (COMMON_HOME {})",
        common_core::VERSION,
        paths.root.display()
    );
    print!("{}", hardware_text(&hw));
    match &engine {
        Ok((p, _)) => println!(
            "engine   {} {} present, verified, reports version {}",
            p.name,
            p.release,
            engine_json["version"].as_str().unwrap_or("?")
        ),
        Err(e) => println!("engine   {}", e.message),
    }
    println!(
        "models   {} downloaded{}",
        downloaded.len(),
        if downloaded.is_empty() {
            String::new()
        } else {
            format!(": {}", downloaded.join(", "))
        }
    );
    for w in &warnings {
        println!("warning  {w}");
    }
    if warnings.is_empty() {
        println!("ok       no problems found");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_run_flags() {
        let cli = Cli::try_parse_from([
            "common",
            "run",
            "stories15M-q4_0",
            "-p",
            "hi",
            "-n",
            "32",
            "--temp",
            "0",
            "--seed",
            "42",
            "--device",
            "cpu",
            "--json",
            "--save-prompt",
            "--threads",
            "2",
            "--top-p",
            "0.5",
        ])
        .unwrap();
        assert!(cli.json);
        let Cmd::Run(a) = cli.cmd else { panic!("not run") };
        assert_eq!(
            (
                a.model.as_str(),
                a.prompt.as_str(),
                a.max_tokens,
                a.temp,
                a.seed,
                a.device
            ),
            ("stories15M-q4_0", "hi", 32, 0.0, Some(42), Device::Cpu)
        );
        assert_eq!((a.threads, a.top_p), (Some(2), 0.5));
        assert!(a.save_prompt && !a.chat && a.ctx.is_none());
    }

    #[test]
    fn run_defaults_and_required_prompt() {
        let cli = Cli::try_parse_from(["common", "run", "m", "-p", "x"]).unwrap();
        let Cmd::Run(a) = cli.cmd else { panic!("not run") };
        assert_eq!(
            (a.max_tokens, a.temp, a.top_p, a.device, a.seed, a.threads),
            (256, 0.7, 0.95, Device::Auto, None, None)
        );
        assert!(Cli::try_parse_from(["common", "run", "m"]).is_err(), "-p is required");
        let cli = Cli::try_parse_from(["common", "replay", "/tmp/r/manifest.json", "--json"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Replay(ref r) if r.manifest == Path::new("/tmp/r/manifest.json")));
    }

    #[test]
    fn orphans_match_ppid_1_and_argv0_only() {
        let bin = Path::new("/home/x/backends/llama.cpp/b11609/llama-server");
        let ps = "\
  1     0 /sbin/launchd
 10     1 /home/x/backends/llama.cpp/b11609/llama-server -m /tmp/a.gguf --port 1
 11   999 /home/x/backends/llama.cpp/b11609/llama-server -m /tmp/b.gguf --port 2
 12     1 /usr/local/bin/llama-server --port 3
 13     1 (llama-server)
garbage line
";
        assert_eq!(
            orphans(ps, bin),
            vec![10],
            "only the detached engine from our pinned dir"
        );
        assert!(orphans("", bin).is_empty());
    }

    /// A detached process running under our engine path (a symlink to `sleep`, so it is real and
    /// signed) is reparented to pid 1 and must be stopped by the reaper.
    #[cfg(unix)]
    #[test]
    fn reaper_stops_detached_engine() {
        let dir = std::env::temp_dir().join(format!("common-cli-reap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("llama-server");
        std::os::unix::fs::symlink("/bin/sleep", &bin).unwrap();
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("nohup '{}' 300 >/dev/null 2>&1 &", bin.display()))
            .status()
            .unwrap()
            .success();
        assert!(ok, "detached spawn");
        let deadline = Instant::now() + Duration::from_secs(10);
        let killed = loop {
            let k = reap_orphaned_engines(&bin);
            if !k.is_empty() || Instant::now() > deadline {
                break k;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(killed.len(), 1, "one orphan found and signalled");
        let pid = killed[0];
        while Instant::now() < deadline && common_runtime::llamacpp::process::alive(pid) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            !common_runtime::llamacpp::process::alive(pid),
            "pid {pid} still alive after SIGTERM"
        );
        assert!(reap_orphaned_engines(&bin).is_empty(), "nothing left to reap");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// SIGTERM to ourselves resolves the shared stop future as Terminate (the default disposition
    /// would have killed this test binary instead).
    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_signal_resolves_on_sigterm() {
        let waiter = tokio::spawn(shutdown_signal());
        tokio::time::sleep(Duration::from_millis(100)).await; // the handler registers on first poll
                                                              // SAFETY: signalling our own pid.
        unsafe { libc::kill(std::process::id() as libc::pid_t, libc::SIGTERM) };
        let stop = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("signal observed")
            .unwrap();
        assert_eq!(stop, Stop::Terminate);
    }

    #[test]
    fn parses_models_hardware_serve_bench() {
        assert!(matches!(
            Cli::try_parse_from(["common", "hardware", "--json"]).unwrap().cmd,
            Cmd::Hardware
        ));
        let cli = Cli::try_parse_from(["common", "models", "pull", "stories260K", "--yes"]).unwrap();
        assert!(
            matches!(cli.cmd, Cmd::Models { cmd: models::ModelsCmd::Pull { ref id, yes: true } } if id == "stories260K")
        );
        let cli = Cli::try_parse_from([
            "common",
            "models",
            "import",
            "/tmp/a.gguf",
            "--id",
            "a",
            "--license",
            "MIT",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Models {
                cmd: models::ModelsCmd::Import { .. }
            }
        ));
        let cli = Cli::try_parse_from(["common", "serve", "--port", "9000", "--api-key", "k"]).unwrap();
        let Cmd::Serve(s) = cli.cmd else { panic!("not serve") };
        assert_eq!(
            (s.host.as_str(), s.port, s.api_key.as_deref(), s.max_concurrent),
            ("127.0.0.1", 9000, Some("k"), 4)
        );
        let cli = Cli::try_parse_from(["common", "bench", "m", "--runs", "5", "-n", "16", "-vv"]).unwrap();
        assert_eq!(cli.verbose, 2);
        let Cmd::Bench(b) = cli.cmd else { panic!("not bench") };
        assert_eq!((b.runs, b.max_tokens), (5, 16));
    }

    /// Every mesh command parses, with its flags, into the expected variant (F-02-*, F-06-05, F-10-10).
    #[test]
    fn parses_mesh_commands() {
        use mesh::{GroupsCmd, PeersCmd, PrivacyChoice};
        let peers = |args: &[&str]| -> PeersCmd {
            let cli = Cli::try_parse_from([&["common", "peers"][..], args].concat()).unwrap();
            let Cmd::Peers { cmd } = cli.cmd else {
                panic!("not peers")
            };
            cmd
        };
        assert!(matches!(peers(&["list"]), PeersCmd::List));
        assert!(matches!(
            peers(&["discover", "--wait-secs", "5"]),
            PeersCmd::Discover { wait_secs: 5, .. }
        ));
        let PeersCmd::Pair { name, net } = peers(&["pair", "--name", "alpha", "--listen-port", "40001", "--no-mdns"])
        else {
            panic!("not pair")
        };
        assert_eq!(name.as_deref(), Some("alpha"));
        let cfg = net.config();
        assert!(cfg.enabled && !cfg.mdns && cfg.listen_port == 40001 && cfg.bind_addr == "0.0.0.0");
        let PeersCmd::Join {
            target,
            code,
            name,
            net,
        } = peers(&["join", "10.0.0.2:5000", "123456", "--listen-addr", "127.0.0.1"])
        else {
            panic!("not join")
        };
        assert_eq!(
            (target.as_str(), code.as_deref(), name),
            ("10.0.0.2:5000", Some("123456"), None)
        );
        assert_eq!((net.listen_port, net.listen_addr.as_str()), (8193, "127.0.0.1"));
        // A QR payload carries the secret, so the code is optional.
        assert!(matches!(
            peers(&["join", "common-pair:v1:ab:1.2.3.4:5:cd"]),
            PeersCmd::Join { code: None, .. }
        ));
        assert!(matches!(peers(&["rm", "ab12-cd34-ef56"]), PeersCmd::Rm { id } if id == "ab12-cd34-ef56"));
        assert!(matches!(peers(&["revoke", "ab12"]), PeersCmd::Revoke { id } if id == "ab12"));

        let groups = |args: &[&str]| -> GroupsCmd {
            let cli = Cli::try_parse_from([&["common", "groups"][..], args].concat()).unwrap();
            let Cmd::Groups { cmd } = cli.cmd else {
                panic!("not groups")
            };
            cmd
        };
        assert!(matches!(groups(&["list"]), GroupsCmd::List));
        assert!(matches!(
            groups(&["create", "home", "--allow-remote"]),
            GroupsCmd::Create { name, allow_remote: true, allow_pooled: false } if name == "home"
        ));
        assert!(matches!(
            groups(&["create", "lab", "--allow-pooled"]),
            GroupsCmd::Create {
                allow_remote: false,
                allow_pooled: true,
                ..
            }
        ));
        assert!(
            matches!(groups(&["add", "home", "ab12"]), GroupsCmd::Add { group, peer } if group == "home" && peer == "ab12")
        );
        assert!(matches!(groups(&["remove", "home", "ab12"]), GroupsCmd::Remove { .. }));
        assert!(matches!(
            groups(&["policy", "home", "--allow-remote=false", "--allow-pooled=true"]),
            GroupsCmd::Policy {
                allow_remote: Some(false),
                allow_pooled: Some(true),
                ..
            }
        ));
        assert!(matches!(
            groups(&["policy", "home"]),
            GroupsCmd::Policy {
                allow_remote: None,
                allow_pooled: None,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["common", "groups", "policy", "home", "--allow-remote", "true"]).is_err(),
            "the policy flags take `=value` only"
        );

        let cli = Cli::try_parse_from(["common", "privacy"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Privacy(ref p) if p.mode.is_none() && !p.yes));
        let cli = Cli::try_parse_from(["common", "privacy", "trusted-devices", "--yes"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Privacy(ref p) if p.mode == Some(PrivacyChoice::TrustedDevices) && p.yes));
        let cli = Cli::try_parse_from(["common", "privacy", "local-only"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Privacy(ref p) if p.mode == Some(PrivacyChoice::LocalOnly)));
        assert!(
            Cli::try_parse_from(["common", "privacy", "swarm"]).is_err(),
            "no swarm mode in M3"
        );

        let cli = Cli::try_parse_from(["common", "audit"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Audit(ref a) if a.n == 50));
        let cli = Cli::try_parse_from(["common", "audit", "--n", "7", "--json"]).unwrap();
        assert!(cli.json && matches!(cli.cmd, Cmd::Audit(ref a) if a.n == 7));
        let cli = Cli::try_parse_from(["common", "diagnostics", "--out", "/tmp/d.json"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Diagnostics(ref d) if d.out.as_deref() == Some(Path::new("/tmp/d.json"))));
        assert!(matches!(
            Cli::try_parse_from(["common", "diagnostics"]).unwrap().cmd,
            Cmd::Diagnostics(ref d) if d.out.is_none()
        ));
    }

    /// `serve` grows the peer flags without changing its defaults; `run --peer` excludes the
    /// local-engine knobs.
    #[test]
    fn parses_serve_network_and_run_peer() {
        let cli = Cli::try_parse_from(["common", "serve"]).unwrap();
        let Cmd::Serve(s) = cli.cmd else { panic!("not serve") };
        assert!(!s.network && !s.serve_peers && !s.net.no_mdns);
        assert_eq!((s.net.listen_port, s.net.listen_addr.as_str()), (8193, "0.0.0.0"));
        let cli = Cli::try_parse_from([
            "common",
            "serve",
            "--network",
            "--serve-peers",
            "--listen-port",
            "40002",
            "--listen-addr",
            "127.0.0.1",
            "--no-mdns",
        ])
        .unwrap();
        let Cmd::Serve(s) = cli.cmd else { panic!("not serve") };
        assert!(s.network && s.serve_peers && s.net.no_mdns);
        assert_eq!((s.net.listen_port, s.net.listen_addr.as_str()), (40002, "127.0.0.1"));

        let cli = Cli::try_parse_from(["common", "run", "m", "-p", "x", "--peer", "ab12-cd34-ef56"]).unwrap();
        let Cmd::Run(a) = cli.cmd else { panic!("not run") };
        assert_eq!(a.peer.as_deref(), Some("ab12-cd34-ef56"));
        assert!(
            Cli::try_parse_from(["common", "run", "m", "-p", "x", "--peer", "ab12", "--device", "cpu"]).is_err(),
            "--peer and --device are exclusive"
        );
        assert!(Cli::try_parse_from(["common", "run", "m", "-p", "x", "--peer", "ab12", "--threads", "2"]).is_err());
    }

    /// The persisted switch only ever moves towards "on" from the flags; no flag means no write.
    #[test]
    fn serve_flags_persist_settings_only_when_they_turn_something_on() {
        let dir = std::env::temp_dir().join(format!("common-cli-serve-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let paths = Paths::at(dir.clone());
        let parse = |args: &[&str]| -> ServeArgs {
            let cli = Cli::try_parse_from([&["common", "serve"][..], args].concat()).unwrap();
            let Cmd::Serve(s) = cli.cmd else { panic!("not serve") };
            s
        };
        let s = mesh_settings(&parse(&[]), &paths).unwrap();
        assert!(!s.network_enabled && !s.serve_peers);
        assert!(!mesh::Settings::path(&paths).exists(), "default serve writes nothing");
        let s = mesh_settings(&parse(&["--network"]), &paths).unwrap();
        assert!(s.network_enabled && !s.serve_peers);
        assert_eq!(mesh::load_settings(&paths).unwrap(), s, "persisted");
        let s = mesh_settings(&parse(&["--serve-peers"]), &paths).unwrap();
        assert!(s.network_enabled && s.serve_peers, "--serve-peers implies --network");
        assert_eq!(
            mesh::load_settings(&paths).unwrap().privacy,
            common_policy::PrivacyMode::LocalOnly
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hardware_text_lists_every_backend() {
        let r = HardwareReport {
            backends: vec![
                common_hardware::BackendInfo {
                    name: "cpu".into(),
                    available: true,
                    reason: None,
                },
                common_hardware::BackendInfo {
                    name: "cuda".into(),
                    available: false,
                    reason: Some("no NVIDIA GPU detected".into()),
                },
            ],
            disk_free_bytes: Some(15_089_623_040),
            ..Default::default()
        };
        let t = hardware_text(&r);
        assert!(t.contains("backend  cpu     available"), "{t}");
        assert!(
            t.contains("backend  cuda    unavailable (no NVIDIA GPU detected)"),
            "{t}"
        );
        assert!(t.contains("disk     15.1 GB free"), "{t}");
    }
}
