//! `common run`, `common chat`, `common bench`, `common replay`: load through the ModelManager,
//! stream tokens, write the execution manifest (X-15), stop on Ctrl-C / SIGTERM / SIGHUP (F-08-02).
//! `run --peer` sends the whole request to one paired device instead (Mode B, remote execution):
//! the manifest records `scope = remote_single_peer` and the peer id; it never claims more than
//! one physical device, because this CLI cannot know whether the peer is another machine. The
//! sending side is audited like the daemon's `target: peer:<id>` (`remote_request`, `sent` then
//! `finished`; ids and counts only). `run --split <peer> --split-at k` splits the model with one
//! paired device (WP-04 pooled inference) through the running `common serve`, which owns the
//! session: the daemon's disclosure is shown and needs an explicit yes, and the manifest records
//! `scope = pooled_with_peer`, still one physical device claimed.

use crate::mesh::Mesh;
use crate::{context, shutdown_signal, Device, Stop};
use clap::Args;
use common_core::manifest::RunMetrics;
use common_core::{CommonError, ErrorCode, ExecutionManifest, HostSummary, Paths, Result};
use common_identity::PeerId;
use common_network::NetworkConfig;
use common_policy::{ExecutionTarget, PrivacyMode, WorkloadKind};
use common_runtime::manager::{LoadOptions, ModelManager};
use common_runtime::*;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::io::AsyncBufReadExt;
use tokio_util::sync::CancellationToken;

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Model id (see `common models list`)
    pub model: String,
    #[arg(short, long)]
    pub prompt: String,
    /// Run on this paired device (id or short label) instead of here; needs `common privacy trusted-devices`
    #[arg(long, conflicts_with_all = ["device", "threads", "ctx"])]
    pub peer: Option<String>,
    /// Split the model with this paired device (pooled inference through the running `common serve`); it receives its part of the weights and data from which your prompt and the reply can be rebuilt
    #[arg(long, conflicts_with_all = ["peer", "device", "threads", "ctx"], requires = "split_at")]
    pub split: Option<String>,
    /// With --split: the first block of the second stage (1 to the model's block count minus 1)
    #[arg(long, requires = "split")]
    pub split_at: Option<u32>,
    /// With --split: this device's CPU holds the first blocks and the other device the rest and the output layer (default: the other device holds the first blocks, this device's GPU the rest)
    #[arg(long, requires = "split")]
    pub same_backend: bool,
    /// Maximum tokens to generate
    #[arg(short = 'n', long = "max-tokens", default_value_t = 256)]
    pub max_tokens: u32,
    #[arg(long, value_enum, default_value_t = Device::Auto)]
    pub device: Device,
    /// Context length (capped to the model's trained context with a reason)
    #[arg(long)]
    pub ctx: Option<u32>,
    /// Engine threads (default: the performance core count on Apple Silicon, else the physical core count, at most 16; the manifest records what ran)
    #[arg(long)]
    pub threads: Option<u32>,
    #[arg(long, default_value_t = 0.7)]
    pub temp: f32,
    /// Nucleus sampling cutoff
    #[arg(long, default_value_t = 0.95)]
    pub top_p: f32,
    /// Sampling seed (random when omitted; the manifest records the seed that ran)
    #[arg(long)]
    pub seed: Option<u64>,
    /// Store the prompt text in the execution manifest (needed by `common replay`)
    #[arg(long)]
    pub save_prompt: bool,
    /// Send the prompt as a chat user message so the model's chat template applies
    #[arg(long)]
    pub chat: bool,
}

#[derive(Args, Debug)]
pub struct ChatArgs {
    pub model: String,
    #[arg(long, value_enum, default_value_t = Device::Auto)]
    pub device: Device,
    #[arg(long)]
    pub ctx: Option<u32>,
}

#[derive(Args, Debug)]
pub struct BenchArgs {
    pub model: String,
    #[arg(long, value_enum, default_value_t = Device::Auto)]
    pub device: Device,
    #[arg(long, default_value_t = 3)]
    pub runs: u32,
    /// Tokens to generate per run
    #[arg(short = 'n', long = "max-tokens", default_value_t = 64)]
    pub max_tokens: u32,
}

#[derive(Args, Debug)]
pub struct ReplayArgs {
    /// A manifest.json written by `common run --save-prompt`
    pub manifest: PathBuf,
}

/// Fixed prompt for `bench` (~50 llama tokens so it also fits the 128-token fixture models).
const BENCH_PROMPT: &str = "Once upon a time, a little rabbit lived at the edge of a wide green meadow. Every morning she hopped down to the river to watch the fish swim by, and every evening she told her friends what she had seen.";

/// Largest manifest `replay` will read (a 1M-token run is ~8 MB).
const MAX_MANIFEST_BYTES: u64 = 64 << 20;

/// The X-15 manifest plus what a replay needs that `ExecutionManifest` does not carry yet.
/// ponytail: superset by flatten so common-core's schema (shared with the API) stays untouched;
/// upgrade: move these fields into ExecutionManifest and bump SCHEMA_VERSION.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunManifest {
    #[serde(flatten)]
    pub base: ExecutionManifest,
    pub top_p: f32,
    /// `raw` (prompt sent as-is) or `chat` (prompt wrapped as one user message, template applied).
    pub input_mode: String,
    pub parallel_slots: u32,
    /// The device that executed the request when it was not this one (`scope = remote_single_peer`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_peer_id: Option<PeerId>,
}

impl RunManifest {
    /// `local` or `peer:<64 hex>`, for the summary line.
    pub fn target(&self) -> String {
        self.remote_peer_id
            .map(|p| format!("peer:{p}"))
            .unwrap_or_else(|| "local".into())
    }
}

fn opts(device: Device, ctx: Option<u32>) -> LoadOptions {
    LoadOptions {
        device: Some(device.into()),
        n_ctx: ctx,
        ..Default::default()
    }
}

/// Load with stop-signal support: interrupting the (seconds-long) load drops the load future,
/// which kills the half-started engine, and surfaces as Cancelled.
async fn load(m: &ModelManager, id: &str, o: LoadOptions) -> Result<Arc<dyn LoadedModel>> {
    tokio::select! {
        r = m.ensure_loaded(id, o) => r,
        _ = shutdown_signal() => Err(CommonError::new(ErrorCode::Cancelled, "interrupted during load")),
    }
}

#[derive(Default)]
struct Outcome {
    text: String,
    ids: Vec<u32>,
    usage: Usage,
    timings: Timings,
    cancelled: bool,
    /// Which signal cancelled the generation, when one did.
    stop: Option<Stop>,
}

/// Drain the stream, echoing tokens to stdout as they arrive. A stop signal cancels the token;
/// the adapter then yields one Cancelled item and ends, which is reported as `cancelled` (F-08-02).
async fn stream(model: &dyn LoadedModel, req: GenerateRequest, echo: bool) -> Result<Outcome> {
    let cancel = CancellationToken::new();
    drain(cancel.clone(), echo, model.generate(req, cancel)).await
}

/// `open` yields the event stream (local engine or remote peer); a stop signal cancels `cancel` at
/// any point, including while `open` is still pending.
async fn drain(
    cancel: CancellationToken,
    echo: bool,
    open: impl Future<Output = Result<GenerateStream>>,
) -> Result<Outcome> {
    let token = cancel.clone();
    let watcher = tokio::spawn(async move {
        let stop = shutdown_signal().await;
        token.cancel();
        stop
    });
    let result = async {
        let mut s = open.await?;
        let mut out = Outcome::default();
        while let Some(ev) = s.next().await {
            match ev {
                Ok(GenerateEvent::Token { text, token_ids }) => {
                    if echo {
                        print!("{text}");
                        let _ = std::io::stdout().flush();
                    }
                    out.text.push_str(&text);
                    out.ids.extend(token_ids);
                }
                Ok(GenerateEvent::ToolCallDelta { .. }) => {}
                Ok(GenerateEvent::Done { usage, timings, .. }) => {
                    out.usage = usage;
                    out.timings = timings;
                }
                Err(e) if e.code == ErrorCode::Cancelled => {
                    out.cancelled = true;
                    // No Done event after a cancel: record what arrived (ids come only with raw prompts).
                    out.usage.completion_tokens = out.ids.len() as u32;
                    break;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(out)
    }
    .await;
    watcher.abort();
    // Ok only when the watcher finished before the abort, i.e. a signal fired.
    let stop = watcher.await.ok();
    result.map(|mut out| {
        out.stop = stop;
        out
    })
}

/// Load, generate, unload, write the manifest. Shared by `run` and `replay`.
async fn execute(a: &RunArgs, echo: bool) -> Result<(RunManifest, PathBuf, Outcome)> {
    if let Some(peer) = &a.peer {
        return execute_remote(a, peer, echo).await;
    }
    if let Some(peer) = &a.split {
        return execute_split(a, peer, echo).await;
    }
    let started = Instant::now();
    let m = context().await?;
    let model = load(
        &m,
        &a.model,
        LoadOptions {
            threads: a.threads,
            ..opts(a.device, a.ctx)
        },
    )
    .await?;
    let seed = a.seed.unwrap_or_else(random_seed);
    let out = stream(model.as_ref(), request(a, seed), echo).await;
    let rss = model.memory_rss_bytes();
    m.unload().await?;
    let out = out?;
    let info = model.info();
    let metrics = RunMetrics {
        load_ms: Some(info.load_ms),
        time_to_first_token_ms: out.timings.time_to_first_token_ms,
        prefill_tokens_per_second: out.timings.prompt_tokens_per_second,
        decode_tokens_per_second: out.timings.decode_tokens_per_second,
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        peak_rss_bytes: rss,
    };
    let manifest = manifest(m.hardware.host_summary(), info, a, seed, &out, metrics, None);
    let path = write_manifest(&m.paths, &manifest)?;
    Ok((manifest, path, out))
}

/// Mode B: the whole request runs on one paired device. Order (I-05): privacy, then the policy
/// engine for this peer, then the model hash from the local store or catalog (the peer verifies
/// integrity against it; nothing is downloaded), then the running daemon's hold on that device
/// (E5, `Mesh::held_by_daemon`), then a client-only endpoint on an ephemeral port.
async fn execute_remote(a: &RunArgs, peer: &str, echo: bool) -> Result<(RunManifest, PathBuf, Outcome)> {
    let started = Instant::now();
    let mesh = Mesh::open()?;
    if mesh.settings.privacy == PrivacyMode::LocalOnly {
        return Err(
            CommonError::new(ErrorCode::PolicyDenied, "your privacy settings require local execution")
                .with_hint("`common privacy trusted-devices` allows paired devices to run your requests"),
        );
    }
    let rec = mesh.resolve_peer(peer)?;
    mesh.policy(mesh.settings.network_enabled, false)
        .evaluate(
            mesh.settings.privacy,
            ExecutionTarget::Peer(rec.id),
            WorkloadKind::RemoteInference,
        )
        .into_result()?;
    let sha = model_sha256(&mesh.paths, &a.model)?;
    let held = mesh.held_by_daemon(&rec.id).await?;
    let addr = mesh.peer_addr(&rec).await?;
    let node = mesh
        .node(
            NetworkConfig {
                enabled: true,
                listen_port: 0,
                mdns: false,
                ..NetworkConfig::default()
            },
            false,
        )
        .await?;
    // E5: the daemon's hold, enforced by this Node's transport (`connect` refuses a held peer).
    if held {
        node.hold(&rec.id);
    }
    let result = async {
        let conn = node.connect(addr, rec.id).await?;
        let engine = conn
            .capabilities
            .lock()
            .ok()
            .and_then(|c| c.as_ref().and_then(|c| c.engines.first().cloned()))
            .unwrap_or_default();
        let seed = a.seed.unwrap_or_else(random_seed);
        let req = request(a, seed);
        let cancel = CancellationToken::new();
        let privacy = crate::mesh::privacy_str(&mesh.settings);
        let out = drain(
            cancel.clone(),
            echo,
            // F-06-10: `remote_request` phase `sent` (with the request id the peer audits too) as the
            // frame leaves; if it cannot be written the request is cancelled at once and this fails
            // with `io`. Phase `finished` when the stream ends, fails or is dropped.
            common_api::mesh::generate_remote_audited(mesh.audit.clone(), conn, &a.model, &sha, req, &privacy, cancel),
        )
        .await?;
        Ok::<_, CommonError>((engine, seed, out))
    }
    .await;
    node.shutdown().await;
    let (engine, seed, out) = result?;
    let info = LoadedModelInfo {
        model_id: a.model.clone(),
        model_sha256: sha,
        backend_name: "remote".into(),
        backend_version: engine,
        device_requested: "peer".into(),
        device_actual: format!("peer:{}", rec.id.short_label()),
        ..Default::default()
    };
    let metrics = RunMetrics {
        load_ms: None,
        // Client-side wall clock (request written -> first token), includes the network and any load on the peer.
        time_to_first_token_ms: out.timings.time_to_first_token_ms,
        prefill_tokens_per_second: out.timings.prompt_tokens_per_second,
        decode_tokens_per_second: out.timings.decode_tokens_per_second,
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        peak_rss_bytes: None,
    };
    let host = common_hardware::probe()?.host_summary();
    let manifest = manifest(host, &info, a, seed, &out, metrics, Some(rec.id));
    let path = write_manifest(&mesh.paths, &manifest)?;
    Ok((manifest, path, out))
}

/// A split through the running daemon (WP-04): the daemon checks the policy, shows the disclosure
/// (ADR-0005 D8) until it is accepted here, opens the session and loads the split engine. Nothing
/// runs without the daemon, and nothing falls back to a local run.
async fn execute_split(a: &RunArgs, peer: &str, echo: bool) -> Result<(RunManifest, PathBuf, Outcome)> {
    let started = Instant::now();
    let paths = Paths::from_env();
    let daemon = crate::mesh::Daemon::find(&paths).await?.ok_or_else(|| {
        CommonError::new(
            ErrorCode::PolicyDenied,
            "pooled inference runs through `common serve`, which is not running",
        )
        .with_hint("start `common serve`, then retry")
    })?;
    let seed = a.seed.unwrap_or_else(random_seed);
    let mut body = serde_json::json!({
        "model": a.model, "max_tokens": a.max_tokens, "temperature": a.temp, "top_p": a.top_p, "seed": seed,
        "target": format!("split:{peer}"), "split_at": a.split_at,
        "split_mode": if a.same_backend { "same_backend" } else { "pooled" }, "acknowledged": false,
    });
    let path = if a.chat {
        body["messages"] = serde_json::json!([{"role": "user", "content": a.prompt}]);
        "/v1/chat/completions"
    } else {
        body["prompt"] = a.prompt.clone().into();
        "/v1/completions"
    };
    // a split load uploads the other device's part first: allow the daemon's whole load timeout
    let wait = common_runtime::manager::SPLIT_LOAD_BASE * 2;
    let v = match daemon
        .call_for(reqwest::Method::POST, path, Some(body.clone()), wait)
        .await
    {
        Err(e)
            if e.code == ErrorCode::PolicyDenied && e.hint.as_deref().is_some_and(|h| h.contains("acknowledged")) =>
        {
            eprintln!("{}", e.message);
            eprint!("Split this model with that device? [y/N] ");
            let mut answer = String::new();
            std::io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim(), "y" | "Y" | "yes" | "Yes") {
                return Err(CommonError::new(
                    ErrorCode::PolicyDenied,
                    "pooling was not accepted; nothing was sent",
                ));
            }
            body["acknowledged"] = true.into();
            daemon.call_for(reqwest::Method::POST, path, Some(body), wait).await?
        }
        r => r?,
    };
    let choice = &v["choices"][0];
    let text = choice["text"]
        .as_str()
        .or_else(|| choice["message"]["content"].as_str())
        .unwrap_or_default()
        .to_string();
    if echo {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
    let out = Outcome {
        text,
        usage: serde_json::from_value(v["usage"].clone()).unwrap_or_default(),
        timings: serde_json::from_value(v["timings"].clone()).unwrap_or_default(),
        ..Default::default()
    };
    let remote = v["target"]
        .as_str()
        .and_then(|t| t.strip_prefix("split:"))
        .and_then(|h| PeerId::parse_hex(h).ok());
    let status = daemon.call(reqwest::Method::GET, "/api/status", None).await?;
    let info: LoadedModelInfo = serde_json::from_value(status["loaded"].clone()).unwrap_or_default();
    let metrics = RunMetrics {
        load_ms: Some(info.load_ms),
        time_to_first_token_ms: out.timings.time_to_first_token_ms,
        prefill_tokens_per_second: out.timings.prompt_tokens_per_second,
        decode_tokens_per_second: out.timings.decode_tokens_per_second,
        total_ms: started.elapsed().as_secs_f64() * 1000.0,
        peak_rss_bytes: None,
    };
    let host = common_hardware::probe()?.host_summary();
    let mut manifest = manifest(host, &info, a, seed, &out, metrics, remote);
    // one model across this device and the peer's stage; this CLI cannot prove the peer is another machine
    manifest.base.scope = "pooled_with_peer".into();
    let path = write_manifest(&paths, &manifest)?;
    Ok((manifest, path, out))
}

/// The hash the peer must serve: the local manifest's, else the catalog's. Never a substitute.
fn model_sha256(paths: &Paths, model: &str) -> Result<String> {
    if let Some(m) = common_artifacts::Store::new(paths.clone()).get(model)? {
        return Ok(m.sha256);
    }
    common_artifacts::Catalog::builtin()?
        .get(model)
        .map(|e| e.sha256.clone())
        .ok_or_else(|| {
            CommonError::new(
                ErrorCode::ModelNotFound,
                format!("{model} is neither in the local store nor in the catalog"),
            )
            .with_hint("common models list")
        })
}

fn request(a: &RunArgs, seed: u64) -> GenerateRequest {
    let input = if a.chat {
        PromptInput::Chat(vec![ChatMessage {
            role: "user".into(),
            content: a.prompt.clone(),
        }])
    } else {
        PromptInput::Raw(a.prompt.clone())
    };
    GenerateRequest {
        input,
        max_tokens: a.max_tokens,
        temperature: a.temp,
        top_p: a.top_p,
        seed: Some(seed),
        return_token_ids: true,
        ..Default::default()
    }
}

pub async fn run(a: &RunArgs, json: bool) -> Result<()> {
    let (manifest, path, out) = execute(a, !json).await?;
    if json {
        let mut v = serde_json::to_value(&manifest)?;
        v["completion"] = out.text.into();
        v["manifest_path"] = path.display().to_string().into();
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        if !out.text.ends_with('\n') {
            println!();
        }
        eprintln!("{}", summary(&manifest, &path));
    }
    if out.cancelled {
        return Err(CommonError::new(ErrorCode::Cancelled, "generation stopped"));
    }
    Ok(())
}

/// X-15: re-run a saved manifest with its recorded model, device, context, threads and sampling
/// parameters, then compare the generated token ids with the recorded ones.
pub async fn replay(a: &ReplayArgs, json: bool) -> Result<()> {
    let recorded = read_run_manifest(&a.manifest)?;
    let args = replay_args(&recorded)?;
    let (replayed, path, out) = execute(&args, false).await?;
    if out.cancelled {
        return Err(CommonError::new(ErrorCode::Cancelled, "replay stopped"));
    }
    if replayed.base.model_sha256 != recorded.base.model_sha256 {
        return Err(CommonError::new(
            ErrorCode::IntegrityMismatch,
            format!(
                "model {} on this host (sha256 {}) is not the file the manifest ran (sha256 {})",
                args.model, replayed.base.model_sha256, recorded.base.model_sha256
            ),
        ));
    }
    let first_mismatch = divergence(&recorded.base.completion_token_ids, &replayed.base.completion_token_ids);
    let same_backend = replayed.base.backend_version == recorded.base.backend_version;
    if json {
        let v = serde_json::json!({
            "match": first_mismatch.is_none(), "tokens": recorded.base.completion_token_ids.len(), "first_mismatch": first_mismatch,
            "backend_version_matches": same_backend, "device_actual": replayed.base.device_actual, "manifest_path": path.display().to_string(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        let verdict = first_mismatch.map_or("match".to_string(), |i| format!("MISMATCH at token {i}"));
        println!(
            "replay: {verdict} ({} recorded tokens, {} replayed) device={} backend={} manifest={}",
            recorded.base.completion_token_ids.len(),
            replayed.base.completion_token_ids.len(),
            replayed.base.device_actual,
            replayed.base.backend_version,
            path.display()
        );
    }
    match first_mismatch {
        None => Ok(()),
        Some(i) => {
            let e = CommonError::new(
                ErrorCode::IntegrityMismatch,
                format!("replay diverged from the manifest at token {i}"),
            );
            Err(if same_backend {
                e
            } else {
                e.with_hint(format!(
                    "the manifest ran on backend {}, this host has {}",
                    recorded.base.backend_version, replayed.base.backend_version
                ))
            })
        }
    }
}

/// Trust boundary: bytes from disk become a typed manifest or an InvalidRequest; size-capped.
fn read_run_manifest(path: &Path) -> Result<RunManifest> {
    if std::fs::metadata(path)?.len() > MAX_MANIFEST_BYTES {
        return Err(CommonError::invalid(format!(
            "{} is larger than {} bytes; not an execution manifest",
            path.display(),
            MAX_MANIFEST_BYTES
        )));
    }
    serde_json::from_slice(&std::fs::read(path)?).map_err(|e| {
        CommonError::invalid(format!(
            "{} is not a replayable execution manifest: {e}",
            path.display()
        ))
    })
}

/// The `run` invocation that reproduces `m`. Device is what actually ran (not what was asked for):
/// a GPU run replays as `--device gpu` so a fallback to CPU is an error, never a silent change.
fn replay_args(m: &RunManifest) -> Result<RunArgs> {
    let prompt = m.base.prompt.clone().ok_or_else(|| {
        CommonError::invalid("manifest has no prompt text (only its sha256), so it cannot be replayed")
            .with_hint("record a replayable manifest with `common run --save-prompt`")
    })?;
    Ok(RunArgs {
        model: m.base.model_id.clone(),
        prompt,
        // A replay always runs here: the point is to check this host against the record.
        peer: None,
        split: None,
        split_at: None,
        same_backend: false,
        max_tokens: m.base.max_tokens,
        device: if m.base.device_actual == "cpu" {
            Device::Cpu
        } else {
            Device::Gpu
        },
        ctx: Some(m.base.n_ctx),
        threads: Some(m.base.threads),
        temp: m.base.temperature,
        top_p: m.top_p,
        seed: Some(m.base.seed),
        save_prompt: true,
        chat: m.input_mode == "chat",
    })
}

/// Index of the first differing token (a missing tail counts as a difference); None when equal.
fn divergence(expected: &[u32], actual: &[u32]) -> Option<usize> {
    if expected == actual {
        return None;
    }
    Some(
        expected
            .iter()
            .zip(actual)
            .position(|(e, a)| e != a)
            .unwrap_or(expected.len().min(actual.len())),
    )
}

/// One line for stderr: F-01-04 wants device, offloaded layers and decode tok/s visible; `target`
/// says which device ran it (`local` or `peer:<id>`).
fn summary(r: &RunManifest, path: &Path) -> String {
    let m = &r.base;
    let f = |v: Option<f64>| v.map(|x| format!("{x:.1}")).unwrap_or_else(|| "-".into());
    format!(
        "target={} device={} offloaded={}/{} prompt_tokens={} completion_tokens={} ttft_ms={} prefill_tok_s={} decode_tok_s={} load_ms={} manifest={}",
        r.target(),
        m.device_actual,
        m.offloaded_layers,
        m.total_layers,
        m.prompt_tokens,
        m.completion_tokens,
        f(m.metrics.time_to_first_token_ms),
        f(m.metrics.prefill_tokens_per_second),
        f(m.metrics.decode_tokens_per_second),
        f(m.metrics.load_ms),
        path.display()
    )
}

/// `remote_peer` set = Mode B: scope `remote_single_peer`, still one physical device claimed
/// (this CLI cannot prove the peer is another machine; the id is the evidence, F-02-10 honesty).
fn manifest(
    host: HostSummary,
    info: &LoadedModelInfo,
    a: &RunArgs,
    seed: u64,
    out: &Outcome,
    metrics: RunMetrics,
    remote_peer: Option<PeerId>,
) -> RunManifest {
    let base = ExecutionManifest {
        schema_version: ExecutionManifest::SCHEMA_VERSION,
        common_version: common_core::VERSION.into(),
        timestamp_utc: common_artifacts::store::utc_now(),
        host,
        model_id: info.model_id.clone(),
        model_sha256: info.model_sha256.clone(),
        backend_name: info.backend_name.clone(),
        backend_version: info.backend_version.clone(),
        device_requested: info.device_requested.clone(),
        device_actual: info.device_actual.clone(),
        offloaded_layers: info.offloaded_layers,
        total_layers: info.total_layers,
        n_ctx: info.n_ctx,
        threads: info.threads,
        seed,
        temperature: a.temp,
        max_tokens: a.max_tokens,
        prompt_tokens: out.usage.prompt_tokens,
        completion_tokens: out.usage.completion_tokens,
        prompt_sha256: sha256(a.prompt.as_bytes()),
        prompt: a.save_prompt.then(|| a.prompt.clone()),
        completion_token_ids: out.ids.clone(),
        completion_sha256: sha256(out.text.as_bytes()),
        metrics,
        scope: if remote_peer.is_some() {
            "remote_single_peer"
        } else {
            "single_physical_host"
        }
        .into(),
        physical_device_count: 1,
    };
    RunManifest {
        base,
        top_p: a.top_p,
        input_mode: if a.chat { "chat" } else { "raw" }.into(),
        parallel_slots: info.parallel_slots,
        remote_peer_id: remote_peer,
    }
}

/// `runs/<compact utc timestamp>-<pid>/manifest.json`; returns the file path.
fn write_manifest(paths: &Paths, m: &RunManifest) -> Result<PathBuf> {
    let dir = paths
        .runs()
        .join(format!("{}-{}", compact(&m.base.timestamp_utc), std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("manifest.json");
    std::fs::write(&path, serde_json::to_vec_pretty(m)?)?;
    Ok(path)
}

/// `2026-10-14T08:38:27Z` -> `20261014T083827Z` (safe in file names on every OS).
fn compact(utc: &str) -> String {
    utc.replace([':', '-'], "")
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A seed that fits llama.cpp's u32 when the user gives none, so every manifest is replayable.
fn random_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_nanos() % u128::from(u32::MAX)) as u64)
        .unwrap_or(42)
}

pub async fn chat(a: &ChatArgs) -> Result<()> {
    let m = context().await?;
    let model = load(&m, &a.model, opts(a.device, a.ctx)).await?;
    let info = model.info();
    eprintln!(
        "{} on {} (ctx {}); type /quit or Ctrl-D to exit, Ctrl-C stops a reply",
        info.model_id, info.device_actual, info.n_ctx
    );
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    loop {
        eprint!("> ");
        let line = tokio::select! {
            l = lines.next_line() => l?,
            _ = shutdown_signal() => None,
        };
        let Some(line) = line else { break };
        let line = line.trim();
        if line == "/quit" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        messages.push(ChatMessage {
            role: "user".into(),
            content: line.into(),
        });
        let req = GenerateRequest {
            input: PromptInput::Chat(messages.clone()),
            ..Default::default()
        };
        // ponytail: a ContextTooLong after many turns ends the session; upgrade: drop the oldest turns and retry.
        let out = stream(model.as_ref(), req, true).await?;
        println!();
        // Ctrl-C only stops the reply; a terminate request ends the session.
        if out.stop == Some(Stop::Terminate) {
            break;
        }
        messages.push(ChatMessage {
            role: "assistant".into(),
            content: out.text,
        });
    }
    m.unload().await
}

#[derive(Serialize)]
struct Sample {
    ttft_ms: Option<f64>,
    prefill_tok_s: Option<f64>,
    decode_tok_s: Option<f64>,
    prompt_tokens: u32,
    completion_tokens: u32,
    total_ms: f64,
    rss_bytes: Option<u64>,
}

pub async fn bench(a: &BenchArgs, json: bool) -> Result<()> {
    let m = context().await?;
    let model = load(&m, &a.model, opts(a.device, None)).await?;
    let mut samples = Vec::new();
    for i in 1..=a.runs.max(1) {
        let t = Instant::now();
        let req = GenerateRequest {
            input: PromptInput::Raw(BENCH_PROMPT.into()),
            max_tokens: a.max_tokens,
            temperature: 0.0,
            seed: Some(42),
            ..Default::default()
        };
        let out = match stream(model.as_ref(), req, false).await {
            Ok(o) if !o.cancelled => o,
            Ok(_) => {
                m.unload().await?;
                return Err(CommonError::new(ErrorCode::Cancelled, "benchmark stopped"));
            }
            Err(e) => {
                m.unload().await?;
                return Err(e);
            }
        };
        let s = Sample {
            ttft_ms: out.timings.time_to_first_token_ms,
            prefill_tok_s: out.timings.prompt_tokens_per_second,
            decode_tok_s: out.timings.decode_tokens_per_second,
            prompt_tokens: out.usage.prompt_tokens,
            completion_tokens: out.usage.completion_tokens,
            total_ms: t.elapsed().as_secs_f64() * 1000.0,
            rss_bytes: model.memory_rss_bytes(),
        };
        eprintln!(
            "run {i}/{}: ttft {} ms, prefill {} tok/s, decode {} tok/s, {} tokens",
            a.runs,
            fmt(s.ttft_ms),
            fmt(s.prefill_tok_s),
            fmt(s.decode_tok_s),
            s.completion_tokens
        );
        samples.push(s);
    }
    let info = model.info().clone();
    m.unload().await?;
    let med = |f: fn(&Sample) -> Option<f64>| median(samples.iter().filter_map(f).collect());
    let (ttft, prefill, decode) = (med(|s| s.ttft_ms), med(|s| s.prefill_tok_s), med(|s| s.decode_tok_s));
    let peak_rss = samples.iter().filter_map(|s| s.rss_bytes).max();
    let timestamp = common_artifacts::store::utc_now();
    let report = serde_json::json!({
        "schema_version": 1,
        "common_version": common_core::VERSION,
        "timestamp_utc": timestamp,
        "command": std::env::args().collect::<Vec<_>>(),
        "hardware": m.hardware,
        "model": info,
        "prompt_sha256": sha256(BENCH_PROMPT.as_bytes()),
        "max_tokens": a.max_tokens,
        "temperature": 0.0,
        "seed": 42,
        "runs": samples,
        "median": { "ttft_ms": ttft, "prefill_tok_s": prefill, "decode_tok_s": decode },
        "peak_rss_bytes": peak_rss,
    });
    let device = if info.device_actual == "cpu" { "cpu" } else { "gpu" };
    let path =
        PathBuf::from("benchmarks")
            .join("results")
            .join(format!("{}-{}-{device}.json", compact(&timestamp), a.model));
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} on {} ({}/{} layers offloaded): median ttft {} ms, prefill {} tok/s, decode {} tok/s, peak rss {}; {} runs; wrote {}",
            info.model_id,
            info.device_actual,
            info.offloaded_layers,
            info.total_layers,
            fmt(ttft),
            fmt(prefill),
            fmt(decode),
            peak_rss.map(common_models::human_bytes).unwrap_or_else(|| "-".into()),
            samples.len(),
            path.display()
        );
    }
    Ok(())
}

fn fmt(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.1}")).unwrap_or_else(|| "-".into())
}

/// Median (mean of the two middle values when even); None when empty.
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> RunArgs {
        RunArgs {
            model: "m".into(),
            prompt: "secret".into(),
            peer: None,
            split: None,
            split_at: None,
            same_backend: false,
            max_tokens: 8,
            device: Device::Cpu,
            ctx: None,
            threads: None,
            temp: 0.0,
            top_p: 0.95,
            seed: None,
            save_prompt: false,
            chat: false,
        }
    }

    #[test]
    fn median_odd_even_empty() {
        assert_eq!(median(vec![3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(vec![4.0, 1.0, 3.0, 2.0]), Some(2.5));
        assert_eq!(median(vec![]), None);
    }

    #[test]
    fn sha256_known_vector_and_compact_timestamp() {
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(compact("2026-10-14T08:38:27Z"), "20261014T083827Z");
        assert!(random_seed() <= u64::from(u32::MAX));
    }

    #[test]
    fn manifest_omits_prompt_unless_saved_and_writes_to_runs() {
        let args = args();
        let info = LoadedModelInfo {
            model_id: "m".into(),
            total_layers: 6,
            offloaded_layers: 7,
            parallel_slots: 1,
            ..Default::default()
        };
        let out = Outcome {
            text: "hi".into(),
            ids: vec![1, 2],
            usage: Usage {
                prompt_tokens: 3,
                completion_tokens: 2,
            },
            ..Default::default()
        };
        let m = manifest(
            HostSummary::default(),
            &info,
            &args,
            42,
            &out,
            RunMetrics::default(),
            None,
        );
        assert!(m.base.prompt.is_none());
        assert_eq!(m.base.prompt_sha256, sha256(b"secret"));
        assert_eq!(
            (
                m.base.seed,
                m.base.completion_token_ids.clone(),
                m.base.offloaded_layers,
                m.base.total_layers
            ),
            (42, vec![1, 2], 7, 6)
        );
        assert_eq!((m.top_p, m.input_mode.as_str(), m.parallel_slots), (0.95, "raw", 1));
        let saved = manifest(
            HostSummary::default(),
            &info,
            &RunArgs {
                save_prompt: true,
                chat: true,
                ..args
            },
            42,
            &out,
            RunMetrics::default(),
            None,
        );
        assert_eq!(
            (saved.base.prompt.as_deref(), saved.input_mode.as_str()),
            (Some("secret"), "chat")
        );

        let root = std::env::temp_dir().join(format!("common-cli-run-{}", std::process::id()));
        let path = write_manifest(&Paths::at(&root), &m).unwrap();
        assert!(path.starts_with(root.join("runs")) && path.ends_with("manifest.json"));
        let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            (
                json["schema_version"].as_u64(),
                json["top_p"].as_f64(),
                json["input_mode"].as_str()
            ),
            (Some(1), Some(0.95), Some("raw")),
            "flattened: base and extra fields side by side"
        );
        assert_eq!(read_run_manifest(&path).unwrap(), m);
        assert!(summary(&m, &path).contains("target=local device=") && summary(&m, &path).contains("offloaded=7/6"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn replay_args_pin_what_ran_and_need_the_prompt() {
        let info = LoadedModelInfo {
            model_id: "m".into(),
            device_actual: "Metal0 (Apple M2)".into(),
            n_ctx: 128,
            threads: 6,
            ..Default::default()
        };
        let out = Outcome::default();
        let m = manifest(
            HostSummary::default(),
            &info,
            &args(),
            42,
            &out,
            RunMetrics::default(),
            None,
        );
        let e = replay_args(&m).unwrap_err();
        assert_eq!(e.code, ErrorCode::InvalidRequest);
        assert!(e.hint.as_deref().unwrap_or_default().contains("--save-prompt"));

        let m = manifest(
            HostSummary::default(),
            &info,
            &RunArgs {
                save_prompt: true,
                chat: true,
                top_p: 0.5,
                ..args()
            },
            42,
            &out,
            RunMetrics::default(),
            None,
        );
        let r = replay_args(&m).unwrap();
        assert_eq!(
            (
                r.model.as_str(),
                r.prompt.as_str(),
                r.max_tokens,
                r.device,
                r.ctx,
                r.threads
            ),
            ("m", "secret", 8, Device::Gpu, Some(128), Some(6))
        );
        assert_eq!(
            (r.temp, r.top_p, r.seed, r.save_prompt, r.chat),
            (0.0, 0.5, Some(42), true, true)
        );
        let cpu = manifest(
            HostSummary::default(),
            &LoadedModelInfo {
                device_actual: "cpu".into(),
                ..info
            },
            &RunArgs {
                save_prompt: true,
                ..args()
            },
            1,
            &out,
            RunMetrics::default(),
            None,
        );
        assert_eq!(replay_args(&cpu).unwrap().device, Device::Cpu);
    }

    #[test]
    fn divergence_reports_first_difference() {
        assert_eq!(divergence(&[1, 2, 3], &[1, 2, 3]), None);
        assert_eq!(divergence(&[1, 2, 3], &[1, 9, 3]), Some(1));
        assert_eq!(divergence(&[1, 2, 3], &[1, 2]), Some(2), "missing tail");
        assert_eq!(divergence(&[1, 2], &[1, 2, 3]), Some(2), "extra tail");
        assert_eq!(divergence(&[], &[7]), Some(0));
    }

    #[test]
    fn unreadable_manifests_are_invalid_request() {
        let dir = std::env::temp_dir().join(format!("common-cli-replay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("manifest.json");
        std::fs::write(&p, b"{\"schema_version\": 1}").unwrap();
        assert_eq!(read_run_manifest(&p).unwrap_err().code, ErrorCode::InvalidRequest);
        assert_eq!(
            read_run_manifest(&dir.join("missing.json")).unwrap_err().code,
            ErrorCode::Io
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
