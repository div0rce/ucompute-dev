//! Model manager: resolves a catalog id to a verified blob, checks feasibility, plans context,
//! loads through the backend (with GPU→CPU fallback) and keeps the loaded model in a single slot.
//! Shared by the CLI (`common run`) and the API server.

use crate::types::*;
use common_core::{CommonError, ErrorCode, Paths, Result, Timeouts};
use common_hardware::HardwareReport;
use common_models::FitVerdict;
use futures_util::StreamExt;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct ModelManager {
    pub paths: Paths,
    pub store: common_artifacts::Store,
    pub backend: Arc<dyn InferenceBackend>,
    pub hardware: HardwareReport,
    /// ponytail: one loaded model at a time (8 GB dev machine). Upgrade: LRU map keyed by id.
    pub loaded: Mutex<Option<Slot>>,
    /// What status endpoints show without waiting on `loaded`, which a load holds for its whole
    /// hash check and engine start ([`ModelManager::snapshot`]).
    view: std::sync::Mutex<EngineView>,
    /// Memory usable right now, sampled before every load (F-01-03): the probe in `hardware` is
    /// a snapshot from daemon start. `None` falls back to that snapshot. Tests swap in a fixed figure.
    pub available_memory: fn() -> Option<u64>,
    /// Unexpected engine exits in the last [`RESTART_WINDOW`], for the restart bound.
    exits: std::sync::Mutex<Vec<Exit>>,
}

/// One unexpected engine exit, keyed by model and requested device: three failed `--device gpu`
/// starts say nothing about the CPU engine. `code` and `hint` are the failure's own, so a model
/// whose file breaks every start is still refused as `ModelFileInvalid`.
struct Exit {
    model_id: String,
    device: DevicePreference,
    at: Instant,
    code: ErrorCode,
    hint: Option<String>,
}

/// At most this many unexpected engine exits of one model on one requested device within
/// [`RESTART_WINDOW`], counting exits during a load (before the engine was ready) and after it;
/// after that a load of that model on that device is refused until the window has passed, instead
/// of restarting in a loop.
pub const MAX_RESTARTS: usize = 3;
pub const RESTART_WINDOW: Duration = Duration::from_secs(60);

/// Engine threads when the request does not pin them: the probe's performance cores where it
/// reports them (Apple Silicon), else its physical cores, at most this. A request (`--threads`)
/// overrides it.
// Measured on the M2 (4P+4E, CPU, qwen2.5-0.5b q4_k_m, round 6): -t 8 lost 38% on prompt and
// 15-34% on decode against -t 4, and three engines at -t 8 ran about 9x slower than at -t 4.
// llama.cpp's own default there is also the performance-core count.
// ponytail: hybrid x86 (the Windows laptop's 4P+4LPE) still counts every physical core; read
// EfficiencyClass from GetLogicalProcessorInformationEx if a measurement there says it loses.
pub const MAX_DEFAULT_THREADS: u32 = 16;

/// What is loaded, the options it was loaded with (needed to judge "compatible opts"), and who
/// caused the load and when.
pub struct Slot {
    pub model_id: String,
    pub opts: LoadOptions,
    pub model: Arc<dyn LoadedModel>,
    /// `"local"`, or `"peer:<hex>"` when a paired device's request loaded it.
    pub loaded_for: String,
    pub loaded_at_utc: String,
}

/// The engine as status endpoints show it ([`ModelManager::snapshot`]).
#[derive(Debug, Clone, Default)]
pub struct EngineView {
    /// The loaded model as of the last moment the slot was free.
    pub loaded: Option<LoadedView>,
    /// The model id a load in progress is for, and when it started.
    pub loading: Option<(String, Instant)>,
    /// The last engine that exited on its own (killed, crashed); its slot was freed.
    pub last_exit: Option<EngineExit>,
}

/// An engine that exited on its own and why, as the OS reported it.
#[derive(Debug, Clone, Serialize)]
pub struct EngineExit {
    pub model_id: String,
    pub pid: Option<u32>,
    /// E.g. `signal: 9 (SIGKILL)` or `exit status: 1`.
    pub reason: String,
    /// Streams and calls the exit cut off.
    pub in_flight: usize,
    pub at_utc: String,
}

/// `LoadedModelInfo` plus what only the manager knows.
#[derive(Debug, Clone, Serialize)]
pub struct LoadedView {
    #[serde(flatten)]
    pub info: LoadedModelInfo,
    /// Streams and calls using the model.
    pub in_flight: usize,
    pub loaded_for: String,
    pub loaded_at_utc: String,
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Marks a load in progress in the view until dropped, so a cancelled load (the caller's future
/// dropped) never leaves "loading" behind.
struct LoadingMark<'a>(&'a std::sync::Mutex<EngineView>);

impl<'a> LoadingMark<'a> {
    fn new(view: &'a std::sync::Mutex<EngineView>, model_id: &str) -> Self {
        lock(view).loading = Some((model_id.to_string(), Instant::now()));
        Self(view)
    }
}

impl Drop for LoadingMark<'_> {
    fn drop(&mut self) {
        lock(self.0).loading = None;
    }
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    pub device: Option<DevicePreference>,
    pub n_ctx: Option<u32>,
    pub threads: Option<u32>,
    pub parallel_slots: Option<u32>,
    pub embeddings: bool,
    /// Load as one stage of a layer split (WP-04): planned with this device's stages only, a
    /// longer load timeout ([`split_load_timeout`]), and never compatible with a load without it,
    /// so a local request never runs on the split engine (I-05).
    pub split: Option<Split>,
}

/// A split load waits this long plus the remote stages' upload at [`SPLIT_UPLOAD_BYTES_PER_S`].
pub const SPLIT_LOAD_BASE: Duration = Duration::from_secs(300);
// ponytail: 5 MB/s is a slow Wi-Fi guess; tune from P1's measured upload.
pub const SPLIT_UPLOAD_BYTES_PER_S: u64 = 5_000_000;

/// The load timeout of a split whose remote stages hold `remote_bytes`.
pub fn split_load_timeout(remote_bytes: u64) -> Duration {
    SPLIT_LOAD_BASE + Duration::from_secs(remote_bytes / SPLIT_UPLOAD_BYTES_PER_S)
}

/// The stages of `split` on this device (not `RPCk`) as one range: the head always keeps the input
/// embedding and the tokenizer, and the output layer when the last stage is local.
pub fn local_stage(split: &Split) -> common_models::StageRange {
    let local: Vec<&SplitStage> = split.stages.iter().filter(|s| !s.device.starts_with("RPC")).collect();
    common_models::StageRange {
        first: local.iter().map(|s| s.first_block).min().unwrap_or(0),
        end: local.iter().map(|s| s.end_block).max().unwrap_or(0),
        input: true,
        output: split.stages.last().is_some_and(|s| !s.device.starts_with("RPC")),
    }
}

/// Weight bytes the remote stages of `split` hold (uploaded at every load).
pub fn remote_bytes(lb: &common_models::LayerBytes, split: &Split) -> u64 {
    let blocks: u64 = split
        .stages
        .iter()
        .filter(|s| s.device.starts_with("RPC"))
        .map(|s| lb.range(s.first_block, s.end_block))
        .sum();
    let output = split.stages.last().is_some_and(|s| s.device.starts_with("RPC"));
    blocks + if output { lb.output } else { 0 }
}

impl ModelManager {
    pub fn new(
        paths: Paths,
        store: common_artifacts::Store,
        backend: Arc<dyn InferenceBackend>,
        hardware: HardwareReport,
    ) -> Self {
        Self {
            paths,
            store,
            backend,
            hardware,
            loaded: Mutex::new(None),
            view: std::sync::Mutex::new(EngineView::default()),
            available_memory: common_hardware::available_memory,
            exits: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The engine now, without waiting: the slot when it is free (it is the truth), else the view
    /// recorded the last time it was, plus any load in progress. Never blocks on a load.
    // ponytail: while a load or a request's brief check holds the slot, `in_flight` is the count
    // last seen with the slot free.
    pub fn snapshot(&self) -> EngineView {
        if let Ok(mut slot) = self.loaded.try_lock() {
            self.reap(&mut slot);
            let mut v = lock(&self.view);
            v.loaded = slot.as_ref().map(view_of);
            return v.clone();
        }
        lock(&self.view).clone()
    }

    /// Memory usable right now: a fresh sample, or the startup probe when it cannot be read.
    pub fn available_memory_now(&self) -> u64 {
        (self.available_memory)().unwrap_or(self.hardware.memory.available_bytes)
    }

    /// Free the slot of an engine that exited on its own and record why. Its streams and calls
    /// end with a typed `BackendFailed` (see [`Managed`]); the next request loads a fresh engine.
    // ponytail: detected when the slot is next touched (every request, status read, unload), not by
    // a watcher task; the kernel already freed the dead engine's memory.
    fn reap(&self, slot: &mut Option<Slot>) {
        let Some(reason) = slot.as_ref().and_then(|s| s.model.exited()) else {
            return;
        };
        let Some(s) = slot.take() else { return };
        let info = s.model.info();
        let exit = EngineExit {
            model_id: s.model_id.clone(),
            pid: info.pid,
            reason,
            in_flight: s.model.in_flight(),
            at_utc: common_artifacts::store::utc_now(),
        };
        tracing::warn!(model = %exit.model_id, pid = ?exit.pid, reason = %exit.reason, in_flight = exit.in_flight, "engine exited unexpectedly; slot freed, the next request starts it again");
        let device = s.opts.device.unwrap_or_else(|| self.default_device());
        self.record_exit(exit, device, ErrorCode::BackendFailed, None);
        lock(&self.view).loaded = None;
    }

    /// Count `exit` toward the restart bound and show it as the last exit.
    fn record_exit(&self, exit: EngineExit, device: DevicePreference, code: ErrorCode, hint: Option<String>) {
        lock(&self.exits).push(Exit {
            model_id: exit.model_id.clone(),
            device,
            at: Instant::now(),
            code,
            hint,
        });
        lock(&self.view).last_exit = Some(exit);
    }

    /// Refused once `model_id`'s engine on `device` has exited [`MAX_RESTARTS`] times within
    /// [`RESTART_WINDOW`], so a model that crashes on every start is not restarted in a loop:
    /// the last exit's own code and hint when it named a cause (`ModelFileInvalid` for a broken
    /// model file, `GpuUnavailable`), else `BackendFailed`.
    fn restart_allowed(&self, model_id: &str, device: DevicePreference) -> Result<()> {
        let mut exits = lock(&self.exits);
        exits.retain(|x| x.at.elapsed() < RESTART_WINDOW);
        let recent: Vec<&Exit> = exits
            .iter()
            .filter(|x| x.model_id == model_id && x.device == device)
            .collect();
        let (Some(first), Some(last)) = (recent.first(), recent.last()) else {
            return Ok(());
        };
        if recent.len() < MAX_RESTARTS {
            return Ok(());
        }
        let wait = RESTART_WINDOW.saturating_sub(first.at.elapsed()).as_secs() + 1;
        let message = format!(
            "the engine for {model_id} stopped unexpectedly {} times in the last {} s; not starting it again for {wait} s",
            recent.len(),
            RESTART_WINDOW.as_secs()
        );
        if last.code != ErrorCode::BackendFailed {
            let e = CommonError::new(last.code, message);
            return Err(match &last.hint {
                Some(h) => e.with_hint(h.clone()),
                None => e,
            });
        }
        Err(CommonError::new(ErrorCode::BackendFailed, message).with_hint(format!(
            "check free memory and the engine logs in {}, or load a smaller model",
            self.paths.logs().display()
        )))
    }

    /// Return the loaded model for `model_id`, loading it first if needed. Errors:
    /// ModelNotFound (not in catalog or store), ModelNotDownloaded, IntegrityMismatch (blob bytes
    /// differ from the manifest hash), ModelFileInvalid, InsufficientMemory (with plain-language
    /// reason), ContextTooLong, BackendFailed, Overloaded (the loaded model is still in use and
    /// would have to be replaced).
    pub async fn ensure_loaded(&self, model_id: &str, opts: LoadOptions) -> Result<Arc<dyn LoadedModel>> {
        self.ensure_loaded_for(model_id, opts, "local").await
    }

    /// [`ModelManager::ensure_loaded`], recording `loaded_for` (`"local"` or `"peer:<hex>"`) when
    /// this call is the one that loads.
    pub async fn ensure_loaded_for(
        &self,
        model_id: &str,
        opts: LoadOptions,
        loaded_for: &str,
    ) -> Result<Arc<dyn LoadedModel>> {
        if !common_core::paths::safe_name(model_id) {
            return Err(CommonError::invalid(
                "model id may only contain letters, digits, '-', '_' and '.'",
            ));
        }
        // Held across the load: one slot, so loads are serialized and status() waits for the result.
        let mut slot = self.loaded.lock().await;
        self.reap(&mut slot);
        if let Some(s) = slot.as_ref() {
            // WP-04: the split engine is this device's own pooled run; a paired device's Mode B
            // request neither uses it nor replaces it.
            if s.opts.split.is_some() && opts.split.is_none() && loaded_for != "local" {
                return Err(CommonError::new(
                    ErrorCode::Overloaded,
                    "this device is running a model split with another device; it serves no requests from other devices until that is unloaded",
                ));
            }
            if s.model_id == model_id && compatible(&s.opts, &opts) {
                return Ok(s.model.clone());
            }
            // F-08-08: admitted requests complete. A caller still holding the model (between
            // ensure_loaded and generate) or a live stream/call never has its engine killed.
            refuse_if_busy(s, Arc::strong_count(&s.model) > 1)?;
        }
        let device = opts.device.unwrap_or_else(|| self.default_device());
        self.restart_allowed(model_id, device)?;
        let _loading = LoadingMark::new(&self.view, model_id);
        let (path, sha256) = self.resolve(model_id)?;
        // F-05-01: the bytes handed to the engine are the bytes the manifest promised. Hashing a
        // 1 GB blob takes ~1 s on an M2, well under the engine load time, off the async threads.
        // ponytail: re-hashes on every load; cache a verified (len, mtime) per blob if it ever matters.
        let (store, id) = (self.store.clone(), model_id.to_string());
        tokio::task::spawn_blocking(move || store.verify(&id))
            .await
            .map_err(|e| CommonError::internal(format!("hash task failed: {e}")))??;
        // F-01-03: a sample taken while the engine this load replaces still runs counts that engine
        // as used. So with one to replace, this first plan only rules out a model that could not fit
        // even if nothing else ran (the old engine stays loaded); the plan that decides runs on a
        // fresh sample once the old engine has stopped.
        // ponytail: a model that fits an idle machine but not this one still stops the old engine
        // before it is refused; the refusal is then true of what is free.
        let first = if slot.is_some() {
            self.hardware.memory.total_bytes.max(self.available_memory_now())
        } else {
            self.available_memory_now()
        };
        let mut planned = self.plan(model_id, &path, &sha256, &opts, first)?;
        if let Some(prev) = slot.take() {
            lock(&self.view).loaded = None;
            prev.model.unload().await?;
            planned = self.plan(model_id, &path, &sha256, &opts, self.available_memory_now())?;
        }
        let (req, context_note) = planned;
        let loaded = match self.backend.load(req).await {
            Ok(m) => m,
            Err(e) => {
                // An engine that exits before it is ready counts toward the restart bound like one
                // that exits after, so a model that crashes on every start is refused too.
                let marker = format!(" {}", crate::llamacpp::process::EXITED_BEFORE_READY);
                if let Some((head, _)) = e.message.split_once(&marker) {
                    let exit = EngineExit {
                        model_id: model_id.to_string(),
                        pid: None,
                        reason: head.rsplit_once("exited with ").map_or(head, |(_, r)| r).to_string(),
                        in_flight: 0,
                        at_utc: common_artifacts::store::utc_now(),
                    };
                    self.record_exit(exit, device, e.code, e.hint.clone());
                }
                return Err(e);
            }
        };
        let model: Arc<dyn LoadedModel> = Arc::new(Managed::new(loaded, context_note));
        let s = Slot {
            model_id: model_id.into(),
            opts,
            model: model.clone(),
            loaded_for: loaded_for.into(),
            loaded_at_utc: common_artifacts::store::utc_now(),
        };
        // Before `_loading` drops: the view never shows idle between the load and its result.
        lock(&self.view).loaded = Some(view_of(&s));
        *slot = Some(s);
        Ok(model)
    }

    /// Stop the loaded engine. `Overloaded` while streams or calls are still using it (F-08-08);
    /// a caller merely holding the handle (the CLI unloads before dropping it) does not block this.
    pub async fn unload(&self) -> Result<()> {
        let mut slot = self.loaded.lock().await;
        self.reap(&mut slot);
        if let Some(s) = slot.as_ref() {
            refuse_if_busy(s, false)?;
        }
        if let Some(prev) = slot.take() {
            // As the swap path does: a load that takes the slot before any snapshot saw it free must
            // not report this model as loaded.
            lock(&self.view).loaded = None;
            prev.model.unload().await?;
        }
        Ok(())
    }

    /// Stop the split engine whose `--rpc` ports are `rpc_ports`, in use or not: its pooled session
    /// has ended, so its requests fail with the session's reason anyway (ADR-0005 D7). Leaves any
    /// other loaded model alone.
    pub async fn unload_split(&self, rpc_ports: &[u16]) -> Result<()> {
        let mut slot = self.loaded.lock().await;
        self.reap(&mut slot);
        if slot
            .as_ref()
            .and_then(|s| s.opts.split.as_ref())
            .is_some_and(|s| s.rpc_ports == rpc_ports)
        {
            if let Some(prev) = slot.take() {
                lock(&self.view).loaded = None;
                prev.model.unload().await?;
            }
        }
        Ok(())
    }

    /// Currently loaded model info, if any.
    pub async fn status(&self) -> Option<LoadedModelInfo> {
        let mut slot = self.loaded.lock().await;
        self.reap(&mut slot);
        slot.as_ref().map(|s| s.model.info().clone())
    }

    /// Blob path + sha256 for `id`, or the typed reason it cannot be used.
    pub fn resolve(&self, id: &str) -> Result<(PathBuf, String)> {
        match self.store.resolve(id) {
            Ok((m, path)) => Ok((path, m.sha256)),
            Err(e) if e.code == ErrorCode::ModelNotFound => match common_artifacts::Catalog::builtin()?.get(id) {
                Some(_) => Err(CommonError::new(
                    ErrorCode::ModelNotDownloaded,
                    format!("{id} is in the catalog but has not been downloaded"),
                )
                .with_hint(format!("common models pull {id}"))),
                None => Err(CommonError::new(
                    ErrorCode::ModelNotFound,
                    format!("no model named {id} in the catalog or the local store"),
                )
                .with_hint("common models list")),
            },
            Err(e) => Err(e),
        }
    }

    /// The load request plus the planner's note when it changed what was asked for (F-01-08:
    /// context capped to the trained context, reduced to fit memory, or KV cache downgraded).
    fn plan(
        &self,
        id: &str,
        path: &Path,
        sha256: &str,
        opts: &LoadOptions,
        available: u64,
    ) -> Result<(LoadRequest, Option<String>)> {
        let meta = common_models::gguf::read_metadata(path)?;
        let requested = opts.n_ctx.unwrap_or(meta.context_length.min(4096));
        let budget = (available as f64 * 0.9) as u64;
        let size = file_size(path)?;
        // A split head plans for its own stages only: their weights, their KV share (WP-04).
        let stage = match &opts.split {
            Some(split) => {
                meta.uniform_kv().map_err(CommonError::invalid)?;
                if split.n_layer != meta.block_count {
                    return Err(CommonError::invalid(format!(
                        "split plan is for {} blocks, the model has {}",
                        split.n_layer, meta.block_count
                    )));
                }
                Some(local_stage(split))
            }
            None => None,
        };
        let estimate = |n_ctx, kv| match stage {
            Some(s) => common_models::estimate_stage(&meta, size, n_ctx, kv, s),
            None => common_models::feasibility::estimate(&meta, size, n_ctx, kv),
        };
        let plan = common_models::planning::plan_context_with(&meta, Some(requested), budget, true, estimate)?;
        if let Some(reason) = &plan.reason {
            tracing::warn!(
                model = id,
                n_ctx = plan.n_ctx,
                kv = plan.kv_type.as_llama_arg(),
                "{reason}"
            );
        }
        let est = estimate(plan.n_ctx, plan.kv_type);
        if let FitVerdict::TooLarge { reason, .. } = common_models::feasibility::check_fit(&est, available, 0.10) {
            return Err(CommonError::new(ErrorCode::InsufficientMemory, reason));
        }
        let req = LoadRequest {
            model_id: id.into(),
            model_path: path.to_path_buf(),
            model_sha256: sha256.to_string(),
            device: opts.device.unwrap_or_else(|| self.default_device()),
            n_ctx: plan.n_ctx,
            n_gpu_layers: None,
            threads: Some(opts.threads.unwrap_or_else(|| self.default_threads())),
            parallel_slots: opts.parallel_slots.unwrap_or(1),
            kv_type: plan.kv_type,
            embeddings: opts.embeddings,
            timeouts: match &opts.split {
                Some(split) => Timeouts {
                    load: split_load_timeout(remote_bytes(&meta.layer_bytes(size), split)),
                    ..Timeouts::default()
                },
                None => Timeouts::default(),
            },
            extra_args: vec![],
            split: opts.split.clone(),
        };
        Ok((req, plan.reason))
    }

    /// The probe's performance cores, else its physical cores, at most [`MAX_DEFAULT_THREADS`];
    /// 4 when the probe has no count.
    fn default_threads(&self) -> u32 {
        let cpu = &self.hardware.cpu;
        match cpu.performance_cores.unwrap_or(cpu.physical_cores) {
            0 => 4,
            n => n.min(MAX_DEFAULT_THREADS),
        }
    }

    /// Auto (GPU first, proven by the engine log) unless the probe says only the CPU is usable.
    /// An empty probe means unknown, so Auto lets the adapter find out.
    fn default_device(&self) -> DevicePreference {
        if self.hardware.backends.is_empty() || self.hardware.backends.iter().any(|b| b.available && b.name != "cpu") {
            DevicePreference::Auto
        } else {
            DevicePreference::Cpu
        }
    }
}

fn view_of(s: &Slot) -> LoadedView {
    LoadedView {
        info: s.model.info().clone(),
        in_flight: s.model.in_flight(),
        loaded_for: s.loaded_for.clone(),
        loaded_at_utc: s.loaded_at_utc.clone(),
    }
}

fn file_size(path: &Path) -> Result<u64> {
    Ok(std::fs::metadata(path)?.len())
}

/// `Overloaded` (429, retryable) when the slot's model has work in flight, or when `held` says a
/// caller still holds the handle.
fn refuse_if_busy(s: &Slot, held: bool) -> Result<()> {
    let n = s.model.in_flight();
    if n > 0 || held {
        return Err(CommonError::new(
            ErrorCode::Overloaded,
            format!(
                "model {} is in use by {} request(s); retry when they finish",
                s.model_id,
                n.max(1)
            ),
        ));
    }
    Ok(())
}

/// Same options = same engine. `None` in `wanted` means "don't care"; a pinned value must equal
/// the value the slot was loaded with (an unpinned default never matches a pin, so the first pinned
/// request reloads once and then sticks).
fn compatible(loaded: &LoadOptions, wanted: &LoadOptions) -> bool {
    loaded.embeddings == wanted.embeddings
        && (wanted.device.is_none() || wanted.device == loaded.device)
        && (wanted.n_ctx.is_none() || wanted.n_ctx == loaded.n_ctx)
        && (wanted.threads.is_none() || wanted.threads == loaded.threads)
        && (wanted.parallel_slots.is_none() || wanted.parallel_slots == loaded.parallel_slots)
        && wanted.split == loaded.split
}

/// The slot's view of a loaded model: the backend's model, its info completed with the planner's
/// `context_note`, and a live count of the streams and calls using it. A wrapper, rather than new
/// fields on `LoadRequest`/`Slot`, keeps those public shapes stable for downstream crates.
struct Managed {
    inner: Arc<dyn LoadedModel>,
    info: LoadedModelInfo,
    /// One clone per live stream or call; `strong_count - 1` is the in-flight count.
    in_use: Arc<()>,
}

impl Managed {
    fn new(inner: Arc<dyn LoadedModel>, context_note: Option<String>) -> Self {
        let info = LoadedModelInfo {
            context_note,
            ..inner.info().clone()
        };
        Self {
            inner,
            info,
            in_use: Arc::new(()),
        }
    }
}

#[async_trait::async_trait]
impl LoadedModel for Managed {
    fn info(&self) -> &LoadedModelInfo {
        &self.info
    }

    async fn generate(&self, req: GenerateRequest, cancel: CancellationToken) -> Result<GenerateStream> {
        let guard = self.in_use.clone();
        let stream = match self.inner.generate(req, cancel).await {
            Ok(s) => s,
            Err(e) => return Err(explain_exit(self.inner.as_ref(), e).await),
        };
        let inner = self.inner.clone();
        // The guard lives inside the stream: dropping the stream releases the model.
        Ok(stream
            .then(move |ev| {
                let _keep = &guard;
                let inner = inner.clone();
                async move {
                    match ev {
                        Err(e) => Err(explain_exit(inner.as_ref(), e).await),
                        ev => ev,
                    }
                }
            })
            .boxed())
    }

    async fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        let _guard = self.in_use.clone();
        match self.inner.tokenize(text).await {
            Err(e) => Err(explain_exit(self.inner.as_ref(), e).await),
            r => r,
        }
    }

    async fn detokenize(&self, ids: &[u32]) -> Result<String> {
        let _guard = self.in_use.clone();
        match self.inner.detokenize(ids).await {
            Err(e) => Err(explain_exit(self.inner.as_ref(), e).await),
            r => r,
        }
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let _guard = self.in_use.clone();
        match self.inner.embed(inputs).await {
            Err(e) => Err(explain_exit(self.inner.as_ref(), e).await),
            r => r,
        }
    }

    fn memory_rss_bytes(&self) -> Option<u64> {
        self.inner.memory_rss_bytes()
    }

    async fn unload(&self) -> Result<()> {
        self.inner.unload().await
    }

    fn in_flight(&self) -> usize {
        Arc::strong_count(&self.in_use) - 1
    }

    fn exited(&self) -> Option<String> {
        self.inner.exited()
    }
}

/// How long a failed call waits for the OS to report the engine's exit: a killed engine's sockets
/// close a moment before the process can be reaped.
const EXIT_GRACE: Duration = Duration::from_millis(250);

/// A `BackendFailed` from an engine that has exited says so, with the exit status and a hint that
/// a retry starts it again ([`ModelManager::reap`]). Other errors pass through unchanged.
async fn explain_exit(model: &dyn LoadedModel, e: CommonError) -> CommonError {
    if e.code != ErrorCode::BackendFailed {
        return e;
    }
    let t = Instant::now();
    loop {
        if let Some(how) = model.exited() {
            return CommonError::new(
                ErrorCode::BackendFailed,
                format!("the engine stopped unexpectedly ({how}): {}", e.message),
            )
            .with_hint("retry: the next request starts the engine again");
        }
        if t.elapsed() >= EXIT_GRACE {
            return e;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_hardware::BackendInfo;

    #[test]
    fn compatibility_rules() {
        let loaded = LoadOptions {
            device: Some(DevicePreference::Cpu),
            n_ctx: Some(2048),
            threads: None,
            parallel_slots: None,
            embeddings: false,
            split: None,
        };
        assert!(compatible(&loaded, &LoadOptions::default()), "no pins = compatible");
        assert!(compatible(&loaded, &loaded.clone()));
        assert!(compatible(
            &loaded,
            &LoadOptions {
                n_ctx: Some(2048),
                device: Some(DevicePreference::Cpu),
                ..Default::default()
            }
        ));
        assert!(!compatible(
            &loaded,
            &LoadOptions {
                n_ctx: Some(4096),
                ..Default::default()
            }
        ));
        assert!(!compatible(
            &loaded,
            &LoadOptions {
                device: Some(DevicePreference::Gpu),
                ..Default::default()
            }
        ));
        assert!(!compatible(
            &loaded,
            &LoadOptions {
                embeddings: true,
                ..Default::default()
            }
        ));
        assert!(
            !compatible(
                &loaded,
                &LoadOptions {
                    threads: Some(8),
                    ..Default::default()
                }
            ),
            "a pin never matches an unpinned default"
        );
        assert!(!compatible(
            &LoadOptions::default(),
            &LoadOptions {
                embeddings: true,
                ..Default::default()
            }
        ));
    }

    fn manager(hardware: HardwareReport) -> ModelManager {
        let paths = Paths::at(std::env::temp_dir().join(format!("common-mgr-{}", uuid::Uuid::new_v4())));
        let backend = crate::backend_by_name("llama.cpp", paths.backends(), paths.clone()).unwrap();
        ModelManager::new(paths.clone(), common_artifacts::Store::new(paths), backend, hardware)
    }

    #[test]
    fn default_device_follows_probe() {
        assert_eq!(
            manager(HardwareReport::default()).default_device(),
            DevicePreference::Auto,
            "unknown probe: let the adapter find out"
        );
        let cpu_only = HardwareReport {
            backends: vec![BackendInfo {
                name: "cpu".into(),
                available: true,
                reason: None,
            }],
            ..Default::default()
        };
        assert_eq!(manager(cpu_only).default_device(), DevicePreference::Cpu);
        let metal = HardwareReport {
            backends: vec![
                BackendInfo {
                    name: "cpu".into(),
                    available: true,
                    reason: None,
                },
                BackendInfo {
                    name: "metal".into(),
                    available: true,
                    reason: None,
                },
            ],
            ..Default::default()
        };
        assert_eq!(manager(metal).default_device(), DevicePreference::Auto);
    }

    #[tokio::test]
    async fn bad_ids_and_empty_state() {
        let m = manager(HardwareReport::default());
        assert_eq!(
            m.ensure_loaded("../etc/passwd", LoadOptions::default())
                .await
                .err()
                .unwrap()
                .code,
            ErrorCode::InvalidRequest
        );
        assert!(m.status().await.is_none());
        m.unload().await.unwrap();
    }

    #[tokio::test]
    async fn unknown_and_undownloaded_ids_are_typed() {
        let m = manager(HardwareReport::default());
        let e = m
            .ensure_loaded("no-such-model-xyz", LoadOptions::default())
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::ModelNotFound, "{e}");
        let e = m
            .ensure_loaded("stories15M-q4_0", LoadOptions::default())
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::ModelNotDownloaded, "{e}");
        assert_eq!(e.hint.as_deref(), Some("common models pull stories15M-q4_0"));
    }

    /// F-05-01: a blob whose bytes no longer match the manifest is refused before anything runs.
    #[tokio::test]
    async fn tampered_blob_is_integrity_mismatch_before_any_spawn() {
        let m = manager(HardwareReport::default());
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("tiny.gguf");
        std::fs::write(&f, b"GGUF not really a model").unwrap();
        let manifest = m.store.import(&f, Some("tiny"), None).unwrap();
        // import hard-links the original: break the link, then flip one byte.
        let blob = m.store.blob_path(&manifest.sha256);
        std::fs::remove_file(&blob).unwrap();
        std::fs::write(&blob, b"GGUF not really a modeL").unwrap();
        let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::IntegrityMismatch, "{e}");
        assert!(!m.paths.logs().exists(), "no engine log: nothing was spawned");
        assert!(m.status().await.is_none());
    }

    /// FAKE in-test model (no engine): a stream from it never yields, so it stays in flight until
    /// dropped. `unload` runs the hook (a test's stand-in for the memory a stopped engine frees).
    struct Idle(LoadedModelInfo, fn());

    #[async_trait::async_trait]
    impl LoadedModel for Idle {
        fn info(&self) -> &LoadedModelInfo {
            &self.0
        }
        async fn generate(&self, _: GenerateRequest, _: CancellationToken) -> Result<GenerateStream> {
            Ok(futures_util::stream::pending().boxed())
        }
        async fn tokenize(&self, _: &str) -> Result<Vec<u32>> {
            Ok(vec![1])
        }
        async fn detokenize(&self, _: &[u32]) -> Result<String> {
            Ok(String::new())
        }
        async fn embed(&self, _: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![])
        }
        fn memory_rss_bytes(&self) -> Option<u64> {
            None
        }
        async fn unload(&self) -> Result<()> {
            (self.1)();
            Ok(())
        }
    }

    fn managed(note: Option<&str>) -> Arc<dyn LoadedModel> {
        Arc::new(Managed::new(
            Arc::new(Idle(
                LoadedModelInfo {
                    model_id: "a".into(),
                    ..Default::default()
                },
                || {},
            )),
            note.map(str::to_string),
        ))
    }

    fn slot(id: &str, model: Arc<dyn LoadedModel>) -> Slot {
        Slot {
            model_id: id.into(),
            opts: LoadOptions::default(),
            model,
            loaded_for: "local".into(),
            loaded_at_utc: "2026-10-22T00:00:00Z".into(),
        }
    }

    /// E2: status reads never wait on the slot. A held slot (a load, a request's check) leaves the
    /// last view; a load in progress is marked and the mark goes when the load ends however it ends.
    #[tokio::test]
    async fn snapshot_never_waits_and_marks_a_load_in_progress() {
        let m = manager(HardwareReport::default());
        let e = m.snapshot();
        assert!(e.loaded.is_none() && e.loading.is_none());
        *m.loaded.lock().await = Some(Slot {
            loaded_for: "peer:ab".into(),
            ..slot("a", managed(None))
        });
        let model = m.ensure_loaded("a", LoadOptions::default()).await.unwrap();
        let stream = model
            .generate(GenerateRequest::default(), CancellationToken::new())
            .await
            .unwrap();
        let v = m.snapshot().loaded.unwrap();
        assert_eq!(
            (v.info.model_id.as_str(), v.loaded_for.as_str(), v.in_flight),
            ("a", "peer:ab", 1)
        );
        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["model_id"], "a", "info fields are flattened: {json}");
        assert_eq!(json["loaded_for"], "peer:ab");

        let held = m.loaded.lock().await;
        {
            let _mark = LoadingMark::new(&m.view, "b");
            let e = m.snapshot();
            assert_eq!(e.loading.as_ref().map(|(id, _)| id.as_str()), Some("b"));
            assert_eq!(
                e.loaded.unwrap().info.model_id,
                "a",
                "the last view while the slot is held"
            );
        }
        assert!(m.snapshot().loading.is_none(), "the mark goes with the load");
        drop(held);

        // A load that fails clears its mark too.
        drop(stream);
        drop(model);
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("tiny.gguf");
        std::fs::write(&f, b"GGUF not really a model").unwrap();
        m.store.import(&f, Some("tiny"), None).unwrap();
        let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::ModelFileInvalid, "{e}");
        let e = m.snapshot();
        assert!(e.loading.is_none());
        assert_eq!(
            e.loaded.unwrap().info.model_id,
            "a",
            "a swap that fails before loading keeps the old model"
        );
    }

    /// FAKE engine (no process): "exits" with SIGKILL's status once `dead` is set. Its streams fail
    /// the way a killed engine's do, with a transport error.
    struct Mortal(LoadedModelInfo, Arc<std::sync::atomic::AtomicBool>);

    #[async_trait::async_trait]
    impl LoadedModel for Mortal {
        fn info(&self) -> &LoadedModelInfo {
            &self.0
        }
        async fn generate(&self, _: GenerateRequest, _: CancellationToken) -> Result<GenerateStream> {
            Ok(futures_util::stream::once(async {
                Err(CommonError::new(
                    ErrorCode::BackendFailed,
                    "engine stream failed: connection reset",
                ))
            })
            .boxed())
        }
        async fn tokenize(&self, _: &str) -> Result<Vec<u32>> {
            Ok(vec![1])
        }
        async fn detokenize(&self, _: &[u32]) -> Result<String> {
            Ok(String::new())
        }
        async fn embed(&self, _: &[String]) -> Result<Vec<Vec<f32>>> {
            Ok(vec![])
        }
        fn memory_rss_bytes(&self) -> Option<u64> {
            None
        }
        async fn unload(&self) -> Result<()> {
            Ok(())
        }
        fn exited(&self) -> Option<String> {
            self.1
                .load(std::sync::atomic::Ordering::SeqCst)
                .then(|| "signal: 9 (SIGKILL)".into())
        }
    }

    fn mortal(id: &str) -> (Slot, Arc<std::sync::atomic::AtomicBool>) {
        let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let info = LoadedModelInfo {
            model_id: id.into(),
            pid: Some(4242),
            ..Default::default()
        };
        let model: Arc<dyn LoadedModel> = Arc::new(Managed::new(Arc::new(Mortal(info, dead.clone())), None));
        (slot(id, model), dead)
    }

    /// F-08-04: an engine that dies after loading frees its slot even with a stream in flight, the
    /// stream ends with a typed error naming the exit, the view records it, and the next request
    /// loads again; a model whose engine keeps dying is refused after MAX_RESTARTS exits.
    #[tokio::test]
    async fn dead_engine_frees_the_slot_and_restarts_are_bounded() {
        let m = manager(HardwareReport::default());
        let (s, dead) = mortal("a");
        *m.loaded.lock().await = Some(s);
        let model = m.ensure_loaded("a", LoadOptions::default()).await.unwrap();
        let mut stream = model
            .generate(GenerateRequest::default(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            m.ensure_loaded("b", LoadOptions::default()).await.err().unwrap().code,
            ErrorCode::Overloaded,
            "alive and busy: never replaced"
        );

        dead.store(true, std::sync::atomic::Ordering::SeqCst);
        let v = m.snapshot();
        assert!(v.loaded.is_none(), "the dead engine's slot is freed");
        let exit = v.last_exit.unwrap();
        assert_eq!((exit.model_id.as_str(), exit.pid, exit.in_flight), ("a", Some(4242), 1));
        assert!(exit.reason.contains("signal: 9"), "{}", exit.reason);
        let e = stream.next().await.unwrap().unwrap_err();
        assert_eq!(e.code, ErrorCode::BackendFailed);
        assert!(e.message.contains("stopped unexpectedly (signal: 9"), "{e}");
        assert!(e.hint.as_deref().unwrap().contains("retry"), "{e:?}");
        drop((stream, model));
        assert_eq!(
            m.ensure_loaded("a", LoadOptions::default()).await.err().unwrap().code,
            ErrorCode::ModelNotFound,
            "the next request loads afresh (here: nothing in the store to load)"
        );

        for _ in 1..MAX_RESTARTS {
            let (s, dead) = mortal("a");
            *m.loaded.lock().await = Some(s);
            dead.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(m.status().await.is_none());
        }
        let e = m.ensure_loaded("a", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::BackendFailed, "{e}");
        assert!(e.message.contains("3 times in the last 60 s"), "{e}");
        assert_eq!(
            m.ensure_loaded("b", LoadOptions::default()).await.err().unwrap().code,
            ErrorCode::ModelNotFound,
            "the bound is per model"
        );
        m.unload().await.unwrap();
    }

    /// FAKE backend (no engine): `load` either fails with `fail`'s message or returns an [`Idle`]
    /// model; `loads` counts the calls.
    struct FakeBackend {
        loads: std::sync::atomic::AtomicUsize,
        fail: Option<&'static str>,
        /// The last request `load` was given.
        last: std::sync::Mutex<Option<LoadRequest>>,
    }

    #[async_trait::async_trait]
    impl InferenceBackend for FakeBackend {
        fn name(&self) -> &str {
            "fake"
        }
        fn version(&self) -> String {
            String::new()
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
        async fn load(&self, req: LoadRequest) -> Result<Arc<dyn LoadedModel>> {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            *lock(&self.last) = Some(req.clone());
            match self.fail {
                Some(msg) => Err(CommonError::new(ErrorCode::BackendFailed, msg)),
                None => Ok(Arc::new(Idle(
                    LoadedModelInfo {
                        model_id: req.model_id,
                        ..Default::default()
                    },
                    || {},
                ))),
            }
        }
    }

    /// A manager over the FAKE backend with a SYNTHETIC GGUF (not a real model: llama arch,
    /// stories15M's shape, 64 bytes of tensor data) imported as `tiny`.
    fn fake_manager(hardware: HardwareReport, fail: Option<&'static str>) -> (ModelManager, Arc<FakeBackend>) {
        fn s(v: &str) -> Vec<u8> {
            [(v.len() as u64).to_le_bytes().as_slice(), v.as_bytes()].concat()
        }
        fn kv_u32(k: &str, v: u32) -> Vec<u8> {
            [s(k), 4u32.to_le_bytes().to_vec(), v.to_le_bytes().to_vec()].concat()
        }
        fn kv_str(k: &str, v: &str) -> Vec<u8> {
            [s(k), 8u32.to_le_bytes().to_vec(), s(v)].concat()
        }
        let kvs = [
            kv_str("general.architecture", "llama"),
            kv_u32("llama.block_count", 6),
            kv_u32("llama.context_length", 128),
            kv_u32("llama.embedding_length", 288),
            kv_u32("llama.feed_forward_length", 768),
            kv_u32("llama.attention.head_count", 6),
        ];
        let mut gguf = [b"GGUF".as_slice(), &3u32.to_le_bytes(), &0u64.to_le_bytes()].concat();
        gguf.extend((kvs.len() as u64).to_le_bytes());
        gguf.extend(kvs.concat());
        while !gguf.len().is_multiple_of(32) {
            gguf.push(0);
        }
        gguf.extend([0u8; 64]);
        let mut m = manager(hardware);
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("tiny.gguf");
        std::fs::write(&f, gguf).unwrap();
        m.store.import(&f, Some("tiny"), None).unwrap();
        let backend = Arc::new(FakeBackend {
            loads: Default::default(),
            fail,
            last: Default::default(),
        });
        m.backend = backend.clone();
        (m, backend)
    }

    fn split3(port: u16) -> Split {
        Split {
            rpc_ports: vec![port],
            stages: vec![
                SplitStage {
                    device: "RPC0".into(),
                    first_block: 0,
                    end_block: 3,
                },
                SplitStage {
                    device: "MTL0".into(),
                    first_block: 3,
                    end_block: 6,
                },
            ],
            n_layer: 6,
            no_mmap: false,
            engine_pid: Default::default(),
        }
    }

    /// WP-04 C6 (ADR-0005 D2, I-05): the split engine is this device's own run. A paired device's
    /// Mode B request is refused while it is loaded, and a local request reloads locally instead
    /// of running on it. FAKE engines, SYNTHETIC model.
    #[tokio::test]
    async fn mode_b_refused_while_split_loaded() {
        use std::sync::atomic::Ordering::SeqCst;
        let (mut m, backend) = fake_manager(HardwareReport::default(), None);
        m.available_memory = || Some(8 << 30);
        let split = LoadOptions {
            split: Some(split3(50052)),
            ..Default::default()
        };
        m.ensure_loaded("tiny", split.clone()).await.unwrap();
        assert_eq!(backend.loads.load(SeqCst), 1);
        let peer = format!("peer:{}", "ab".repeat(32));
        let e = m
            .ensure_loaded_for("tiny", LoadOptions::default(), &peer)
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::Overloaded, "{e}");
        assert!(e.message.contains("split with another device"), "{e}");
        assert_eq!(backend.loads.load(SeqCst), 1, "the split engine stays");
        // the same split reuses the engine; another session's ports do not
        m.ensure_loaded("tiny", split).await.unwrap();
        assert_eq!(backend.loads.load(SeqCst), 1);
        // a local request never runs on the split engine: it reloads without the split
        m.ensure_loaded("tiny", LoadOptions::default()).await.unwrap();
        assert_eq!(backend.loads.load(SeqCst), 2);
        assert_eq!(lock(&backend.last).as_ref().unwrap().split, None);
        // and a stale session's unload leaves the local engine alone
        m.unload_split(&[50052]).await.unwrap();
        assert!(m.status().await.is_some());
        m.ensure_loaded(
            "tiny",
            LoadOptions {
                split: Some(split3(50053)),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        m.unload_split(&[50053]).await.unwrap();
        assert!(
            m.status().await.is_none(),
            "the split engine of the ended session is stopped"
        );
    }

    /// WP-04 C6 (lab watch item): a split head plans memory and context for its own stages (its
    /// blocks, its KV share), never the whole model, and waits longer for the load. FAKE engine,
    /// SYNTHETIC model: the budget sits between the whole model's Q8_0 and F16 estimates, above the
    /// head's F16 one, so only a whole-model plan has to downgrade its KV cache.
    #[tokio::test]
    async fn a_split_head_plans_with_its_stage_estimate() {
        use common_models::KvType;
        static AVAIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let (mut m, backend) = fake_manager(HardwareReport::default(), None);
        let (_, blob) = m.store.resolve("tiny").unwrap();
        let meta = common_models::read_metadata(&blob).unwrap();
        let size = std::fs::metadata(&blob).unwrap().len();
        let split = split3(50052);
        let head = common_models::estimate_stage(&meta, size, 128, KvType::F16, local_stage(&split)).total_bytes;
        let whole_q8 = common_models::estimate(&meta, size, 128, KvType::Q8_0).total_bytes;
        let whole_f16 = common_models::estimate(&meta, size, 128, KvType::F16).total_bytes;
        assert!(head < whole_q8 && whole_q8 < whole_f16, "{head} {whole_q8} {whole_f16}");
        AVAIL.store(
            ((whole_q8 + whole_f16) / 2) * 10 / 9 + 1,
            std::sync::atomic::Ordering::SeqCst,
        );
        m.available_memory = || Some(AVAIL.load(std::sync::atomic::Ordering::SeqCst));
        m.ensure_loaded("tiny", LoadOptions::default()).await.unwrap();
        assert_eq!(lock(&backend.last).as_ref().unwrap().kv_type, KvType::Q8_0);
        m.ensure_loaded(
            "tiny",
            LoadOptions {
                split: Some(split.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let req = lock(&backend.last).clone().unwrap();
        assert_eq!(req.kv_type, KvType::F16, "the head's own stages fit as F16");
        assert_eq!(req.split, Some(split));
        assert_eq!(req.timeouts.load, SPLIT_LOAD_BASE, "64 bytes upload in no time");
        assert_eq!(
            split_load_timeout(50_000_000),
            SPLIT_LOAD_BASE + Duration::from_secs(10)
        );
    }

    /// WP-04 C6: the split load waits for the remote stages' upload, which is their blocks and,
    /// in configuration C, the output layer.
    #[test]
    fn remote_bytes_counts_the_remote_stages_and_output() {
        let lb = common_models::LayerBytes {
            blocks: vec![10, 20, 30, 40, 50, 60],
            input: 5,
            output: 7,
        };
        assert_eq!(remote_bytes(&lb, &split3(1)), 10 + 20 + 30, "P: RPC0 holds [0,3)");
        let mut c = split3(1);
        c.stages[0].device = "CPU".into();
        c.stages[1].device = "RPC0".into();
        assert_eq!(
            remote_bytes(&lb, &c),
            40 + 50 + 60 + 7,
            "C: RPC0 holds [3,6) and the output"
        );
        assert_eq!(
            local_stage(&split3(1)),
            common_models::StageRange {
                first: 3,
                end: 6,
                input: true,
                output: true
            }
        );
        assert_eq!(
            local_stage(&c),
            common_models::StageRange {
                first: 0,
                end: 3,
                input: true,
                output: false
            }
        );
    }

    /// F-01-03: memory sampled while the engine a load replaces still runs counts that engine as
    /// used. A swap is planned on a sample taken after the old engine stopped, so a model that fits
    /// once it is gone loads; a model too large for the whole machine is refused with the old
    /// engine still loaded. FAKE engines; the "free memory" figure rises when the old one unloads.
    #[tokio::test]
    async fn a_swap_is_planned_on_memory_sampled_after_the_old_engine_stopped() {
        use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
        static FREE: AtomicU64 = AtomicU64::new(0);
        const GIB: u64 = 1 << 30;
        let machine = |total_bytes| HardwareReport {
            memory: common_hardware::MemoryInfo {
                total_bytes,
                ..Default::default()
            },
            ..Default::default()
        };
        let old = || {
            slot(
                "old",
                Arc::new(Idle(LoadedModelInfo::default(), || FREE.store(4 * GIB, SeqCst))),
            )
        };

        let (mut m, backend) = fake_manager(machine(8 * GIB), None);
        m.available_memory = || Some(FREE.load(SeqCst));
        *m.loaded.lock().await = Some(old());
        FREE.store(1 << 20, SeqCst); // 1 MiB free while the old engine runs
        let model = m.ensure_loaded("tiny", LoadOptions::default()).await.unwrap();
        assert_eq!(model.info().model_id, "tiny");
        assert_eq!(backend.loads.load(SeqCst), 1);
        assert_eq!(m.status().await.unwrap().model_id, "tiny");

        // Stopping the old engine freed nothing: the plan on the fresh sample refuses.
        let (mut m, backend) = fake_manager(machine(8 * GIB), None);
        m.available_memory = || Some(FREE.load(SeqCst));
        *m.loaded.lock().await = Some(slot("old", managed(None)));
        FREE.store(1 << 20, SeqCst);
        let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::InsufficientMemory, "{e}");
        assert_eq!(backend.loads.load(SeqCst), 0, "the machine's total never admits a load");
        assert!(m.status().await.is_none(), "the old engine was stopped to measure");

        // Nothing to replace: the sample decides at once.
        let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::InsufficientMemory, "{e}");
        assert_eq!(backend.loads.load(SeqCst), 0);

        let (mut m, backend) = fake_manager(machine(1 << 20), None);
        m.available_memory = || Some(FREE.load(SeqCst));
        *m.loaded.lock().await = Some(old());
        FREE.store(1 << 20, SeqCst);
        let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::InsufficientMemory, "{e}");
        assert_eq!(FREE.load(SeqCst), 1 << 20, "the old engine was never stopped");
        assert_eq!(backend.loads.load(SeqCst), 0);
        assert!(
            m.status().await.is_some(),
            "a load that cannot fit the machine keeps the old engine"
        );
    }

    /// F-08-04: an engine that exits during its load counts toward [`MAX_RESTARTS`] like one that
    /// exits after it: a model that crashes on every start is refused without starting it again.
    /// A load failure that is not an exit (here a timeout) does not count. FAKE backend.
    #[tokio::test]
    async fn exits_during_load_count_toward_the_restart_bound() {
        use std::sync::atomic::Ordering::SeqCst;
        let (mut m, backend) = fake_manager(
            HardwareReport::default(),
            Some("llama-server exited with signal: 6 (SIGABRT) before becoming ready:\nGGML_ASSERT"),
        );
        m.available_memory = || Some(1 << 30);
        for n in 1..=MAX_RESTARTS {
            let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
            assert!(e.message.contains("SIGABRT"), "start {n}: {e}");
            assert_eq!(backend.loads.load(SeqCst), n);
        }
        let e = m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::BackendFailed, "{e}");
        assert!(e.message.contains("3 times in the last 60 s"), "{e}");
        assert_eq!(backend.loads.load(SeqCst), MAX_RESTARTS, "not started again");
        let exit = m.snapshot().last_exit.expect("an exit during load is the last exit");
        assert_eq!(
            (exit.model_id.as_str(), exit.reason.as_str(), exit.pid, exit.in_flight),
            ("tiny", "signal: 6 (SIGABRT)", None, 0)
        );

        let (mut m, backend) = fake_manager(
            HardwareReport::default(),
            Some("llama-server did not become ready within the load timeout"),
        );
        m.available_memory = || Some(1 << 30);
        for _ in 0..=MAX_RESTARTS {
            m.ensure_loaded("tiny", LoadOptions::default()).await.err().unwrap();
        }
        assert_eq!(backend.loads.load(SeqCst), MAX_RESTARTS + 1, "timeouts are not exits");
    }

    /// Round 6b regression (F-08-04): failed `--device gpu` starts count only against GPU loads of
    /// that model, so a CPU or Auto load right after still loads; a model whose file breaks every
    /// start trips the bound as `ModelFileInvalid` with its hint. FAKE backend, real error shapes.
    #[tokio::test]
    async fn failed_gpu_starts_leave_cpu_loads_alone() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        struct Broken {
            loads: AtomicUsize,
            code: ErrorCode,
            gpu_only: bool,
        }
        #[async_trait::async_trait]
        impl InferenceBackend for Broken {
            fn name(&self) -> &str {
                "fake"
            }
            fn version(&self) -> String {
                String::new()
            }
            fn capabilities(&self) -> Capabilities {
                Capabilities::default()
            }
            async fn load(&self, req: LoadRequest) -> Result<Arc<dyn LoadedModel>> {
                self.loads.fetch_add(1, SeqCst);
                if self.gpu_only && req.device != DevicePreference::Gpu {
                    let info = LoadedModelInfo {
                        model_id: req.model_id,
                        ..Default::default()
                    };
                    return Ok(Arc::new(Idle(info, || {})));
                }
                Err(CommonError::new(
                    self.code,
                    "llama-server exited with exit status: 1 before becoming ready:\nggml_metal_init: error",
                )
                .with_hint("the hint of the failure"))
            }
        }
        let broken = |code, gpu_only| {
            let (mut m, _) = fake_manager(HardwareReport::default(), None);
            m.available_memory = || Some(1 << 30);
            let b = Arc::new(Broken {
                loads: AtomicUsize::new(0),
                code,
                gpu_only,
            });
            m.backend = b.clone();
            (m, b)
        };
        let on = |d| LoadOptions {
            device: Some(d),
            ..Default::default()
        };

        let (m, b) = broken(ErrorCode::GpuUnavailable, true);
        for _ in 0..=MAX_RESTARTS {
            let e = m.ensure_loaded("tiny", on(DevicePreference::Gpu)).await.err().unwrap();
            assert_eq!(e.code, ErrorCode::GpuUnavailable, "{e}");
        }
        assert_eq!(b.loads.load(SeqCst), MAX_RESTARTS, "the 4th GPU start is refused");
        m.ensure_loaded("tiny", on(DevicePreference::Cpu))
            .await
            .expect("a CPU load after three failed GPU starts");
        assert_eq!(b.loads.load(SeqCst), MAX_RESTARTS + 1);
        m.unload().await.unwrap();
        m.ensure_loaded("tiny", LoadOptions::default())
            .await
            .expect("an Auto load too");

        let (m, b) = broken(ErrorCode::ModelFileInvalid, false);
        for _ in 0..MAX_RESTARTS {
            m.ensure_loaded("tiny", on(DevicePreference::Cpu)).await.err().unwrap();
        }
        let e = m.ensure_loaded("tiny", on(DevicePreference::Cpu)).await.err().unwrap();
        assert_eq!(b.loads.load(SeqCst), MAX_RESTARTS, "not started again");
        assert_eq!(e.code, ErrorCode::ModelFileInvalid, "{e}");
        assert_eq!(e.hint.as_deref(), Some("the hint of the failure"));
        assert!(e.message.contains("3 times in the last 60 s"), "{e}");
    }

    #[test]
    fn default_threads_follow_performance_then_physical_cores_within_bounds() {
        let with = |physical_cores, performance_cores| {
            manager(HardwareReport {
                cpu: common_hardware::CpuInfo {
                    physical_cores,
                    performance_cores,
                    ..Default::default()
                },
                ..Default::default()
            })
            .default_threads()
        };
        assert_eq!(
            with(8, Some(4)),
            4,
            "Apple Silicon M2: 4 performance + 4 efficiency cores"
        );
        assert_eq!(
            with(24, Some(64)),
            MAX_DEFAULT_THREADS,
            "performance cores are bounded too"
        );
        assert_eq!(
            (with(8, None), with(3, None), with(64, None), with(0, None)),
            (8, 3, MAX_DEFAULT_THREADS, 4),
            "no core types reported: physical cores"
        );
    }

    /// An unload empties the view at once: a load that holds the slot before any snapshot saw it
    /// free never reports the unloaded model as loaded.
    #[tokio::test]
    async fn unload_clears_the_view_before_the_next_load_holds_the_slot() {
        let m = manager(HardwareReport::default());
        *m.loaded.lock().await = Some(slot("a", managed(None)));
        assert_eq!(m.snapshot().loaded.unwrap().info.model_id, "a");
        m.unload().await.unwrap();
        let _next_load = m.loaded.lock().await;
        assert!(
            m.snapshot().loaded.is_none(),
            "the slot is held: the view is all a status read sees"
        );
    }

    /// F-08-08 / F-01-08: the planner's note reaches `info()`, and a model with work in flight is
    /// neither replaced nor unloaded.
    #[tokio::test]
    async fn busy_model_is_never_replaced_or_unloaded() {
        let m = manager(HardwareReport::default());
        *m.loaded.lock().await = Some(slot("a", managed(Some("capped to 128"))));
        assert_eq!(m.status().await.unwrap().context_note.as_deref(), Some("capped to 128"));
        let model = m.ensure_loaded("a", LoadOptions::default()).await.unwrap();
        assert_eq!(model.in_flight(), 0);
        assert_eq!(
            model.tokenize("x").await.unwrap(),
            [1],
            "calls release the model when they return"
        );
        assert_eq!(model.in_flight(), 0);

        let stream = model
            .generate(GenerateRequest::default(), CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(model.in_flight(), 1);
        let e = m.ensure_loaded("b", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::Overloaded, "{e}");
        assert!(e.message.contains("1 request"), "{e}");
        assert_eq!(
            m.ensure_loaded(
                "a",
                LoadOptions {
                    embeddings: true,
                    ..Default::default()
                }
            )
            .await
            .err()
            .unwrap()
            .code,
            ErrorCode::Overloaded,
            "incompatible opts = reload = refused"
        );
        assert_eq!(m.unload().await.err().unwrap().code, ErrorCode::Overloaded);
        assert!(m.status().await.is_some(), "still loaded");

        drop(stream);
        assert_eq!(model.in_flight(), 0);
        assert_eq!(
            m.ensure_loaded("b", LoadOptions::default()).await.err().unwrap().code,
            ErrorCode::Overloaded,
            "a held handle still blocks a swap"
        );
        m.unload().await.unwrap();
        assert!(
            m.status().await.is_none(),
            "an explicit unload ignores a merely held handle"
        );

        *m.loaded.lock().await = Some(slot("a", managed(None)));
        assert_eq!(
            m.ensure_loaded("b", LoadOptions::default()).await.err().unwrap().code,
            ErrorCode::ModelNotFound,
            "gate open: the swap proceeds to resolve"
        );
        assert!(
            m.status().await.is_some(),
            "a swap that fails before loading keeps the old model"
        );
    }
}
