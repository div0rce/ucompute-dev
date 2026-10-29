//! llama.cpp adapter: supervises a pinned `llama-server` on 127.0.0.1 (ADR-0002).
//!
//! GPU proof comes from the engine log (`offloaded N/M layers to GPU`, `using device MTL0 (...)`),
//! never from the flags we passed (F-01-04). A GPU load that cannot be proven falls back to CPU
//! with `fallback_reason` set and a warning logged, never silently.
//!
//! Cancellation (F-08-02), verified empirically on b11609 on this Mac: closing the HTTP connection
//! mid-stream makes the server log `srv stop: cancel task` and release the slot ~12 ms later;
//! `/slots` reported `is_processing: false` 106 ms after the client went away and
//! `llamacpp:requests_processing` was 0. Dropping the reqwest response is therefore the cancel
//! mechanism; no slot-cancel endpoint is needed. `tests/llamacpp_real.rs` re-measures this through
//! the adapter: after `cancel()` the stream ended in ~10 µs and `/slots` was idle after ~1.2 ms
//! (stories15M, CPU).

pub mod logparse;
pub mod process;
pub mod rpc_server;
pub mod sockets;
pub mod split;
pub mod sse;

use crate::types::*;
use common_core::{CommonError, ErrorCode, Paths, Result, Timeouts};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use tokio::process::Child;
use tokio_util::sync::CancellationToken;

pub struct LlamaCppBackend {
    pub engine_dir: PathBuf,
    pub paths: Paths,
    version: OnceLock<String>,
}

/// A started engine that answered `/health`, plus the proof parsed from its log.
struct Engine {
    child: Child,
    port: u16,
    api_key: String,
    client: reqwest::Client,
    log_path: PathBuf,
    proof: logparse::Proof,
}

impl LlamaCppBackend {
    pub fn new(engine_dir: PathBuf, paths: Paths) -> Self {
        Self {
            engine_dir,
            paths,
            version: OnceLock::new(),
        }
    }

    fn server_binary(&self) -> PathBuf {
        common_artifacts::backend::binary(&self.engine_dir, "llama-server")
    }

    /// One engine start. Retries once with a fresh port when the engine lost the port race.
    async fn attempt(&self, req: &LoadRequest, gpu: bool, deadline: Instant) -> Result<Engine> {
        match self.attempt_once(req, gpu, deadline).await {
            Err(e) if e.code == ErrorCode::BackendFailed && logparse::bind_failed(&e.message) => {
                self.attempt_once(req, gpu, deadline).await
            }
            r => r,
        }
    }

    async fn attempt_once(&self, req: &LoadRequest, gpu: bool, deadline: Instant) -> Result<Engine> {
        let id = uuid::Uuid::new_v4();
        let log_path = self.paths.logs().join(format!("engine-{id}.log"));
        // The key reaches the engine through a 0600 file, never argv: argv is world-readable via ps.
        let key_path = self.paths.logs().join(format!("engine-{id}.key"));
        let api_key = uuid::Uuid::new_v4().to_string();
        process::write_key_file(&key_path, &api_key)?;
        let engine = self.start(req, gpu, deadline, api_key, &key_path, log_path).await;
        // The engine read the key at startup; nothing else may.
        let _ = std::fs::remove_file(&key_path);
        engine
    }

    /// Spawn, wait for `/health`, parse the proof. A failed start keeps its log for diagnostics.
    async fn start(
        &self,
        req: &LoadRequest,
        gpu: bool,
        deadline: Instant,
        api_key: String,
        key_path: &Path,
        log_path: PathBuf,
    ) -> Result<Engine> {
        let port = process::free_port().await?;
        let args = process::build_args(req, port, key_path, gpu)?;
        let env = process::engine_environment(req);
        let mut child = process::spawn(&self.server_binary(), &args, env, &log_path)?;
        tracing::info!(pid = child.id(), port, gpu, log = %log_path.display(), "llama-server started");
        // before the engine dials its `--rpc` ports: the bridge carries only this pid's connections
        if let (Some(split), Some(pid)) = (&req.split, child.id()) {
            split.engine_pid.set(pid);
        }
        let client = reqwest::Client::builder().no_proxy().build().map_err(transport)?;
        if let Err(e) = process::wait_ready(&mut child, &client, port, deadline, &log_path, &api_key).await {
            process::stop(child).await;
            return Err(e);
        }
        let proof = logparse::parse(&process::read_log(&log_path));
        Ok(Engine {
            child,
            port,
            api_key,
            client,
            log_path,
            proof,
        })
    }

    /// The one start of a split engine, kept only when its log proves the plan. Ok carries the
    /// devices as the log names them; a mismatch stops the engine and fails the load.
    async fn start_split(&self, req: &LoadRequest, block_count: u32, deadline: Instant) -> Result<(Engine, String)> {
        let plan = req.split.as_ref().expect("a split request");
        if plan.n_layer != block_count {
            return Err(CommonError::invalid(format!(
                "split plan is for {} blocks, the model has {block_count}",
                plan.n_layer
            )));
        }
        let e = self.attempt(req, true, deadline).await?;
        match prove_split(&process::read_log(&e.log_path), plan) {
            Ok(devices) => Ok((e, devices)),
            Err(err) => {
                process::stop(e.child).await;
                Err(err)
            }
        }
    }

    /// Typed `load` that keeps the concrete type (tests poll `/slots` through it).
    pub async fn load_llama(&self, req: LoadRequest) -> Result<Arc<LoadedLlama>> {
        let started = Instant::now();
        let deadline = started.checked_add(req.timeouts.load).unwrap_or(started);
        if !req.model_path.is_file() {
            return Err(CommonError::io(format!(
                "model file not found: {}",
                req.model_path.display()
            )));
        }
        // Header validation before any process spawn: a corrupt file is ModelFileInvalid here.
        let meta = common_models::gguf::read_metadata(&req.model_path)?;
        if req.n_ctx == 0 || req.parallel_slots == 0 {
            return Err(CommonError::invalid("n_ctx and parallel_slots must be >= 1"));
        }
        process::validate_extra_args(&req.extra_args)?;
        let version = self.version();
        if version.is_empty() {
            return Err(CommonError::new(
                ErrorCode::BackendUnavailable,
                format!("llama-server not runnable at {}", self.server_binary().display()),
            ));
        }
        std::fs::create_dir_all(self.paths.logs())?;

        let mut fallback_reason = None;
        let mut split_devices = None;
        let (engine, gpu) = match req.device {
            // ADR-0005 D2/D7: a split has one attempt, no CPU and no local fallback (no silent substitution).
            _ if req.split.is_some() => {
                let (e, devices) = self.start_split(&req, meta.block_count, deadline).await?;
                split_devices = Some(devices);
                (e, true)
            }
            DevicePreference::Cpu => (self.attempt(&req, false, deadline).await?, false),
            DevicePreference::Gpu => {
                let e = self.attempt(&req, true, deadline).await.map_err(|e| match e.code {
                    ErrorCode::BackendFailed => CommonError::new(ErrorCode::GpuUnavailable, e.message),
                    _ => e,
                })?;
                if !e.proof.gpu_proven() {
                    let why = e.proof.why_not_gpu();
                    process::stop(e.child).await;
                    return Err(CommonError::new(ErrorCode::GpuUnavailable, why));
                }
                (e, true)
            }
            DevicePreference::Auto => match self.attempt(&req, true, deadline).await {
                Ok(e) if e.proof.gpu_proven() => (e, true),
                Ok(e) => {
                    fallback_reason = Some(e.proof.why_not_gpu());
                    process::stop(e.child).await;
                    (self.attempt(&req, false, deadline).await?, false)
                }
                Err(e) if !auto_falls_back(&e) => return Err(e),
                Err(e) => {
                    fallback_reason = Some(format!("GPU engine start failed: {}", e.message));
                    (self.attempt(&req, false, deadline).await?, false)
                }
            },
        };
        if let Some(reason) = &fallback_reason {
            tracing::warn!(model = %req.model_id, reason = %reason, "GPU unavailable, running on CPU instead");
        }

        let info = LoadedModelInfo {
            model_id: req.model_id.clone(),
            model_sha256: req.model_sha256.clone(),
            backend_name: "llama.cpp".into(),
            backend_version: version,
            device_requested: match req.device {
                DevicePreference::Auto => "auto",
                DevicePreference::Cpu => "cpu",
                DevicePreference::Gpu => "gpu",
            }
            .into(),
            device_actual: if let Some(devices) = &split_devices {
                devices.clone()
            } else if gpu {
                engine.proof.device.clone().unwrap_or_default()
            } else {
                "cpu".into()
            },
            offloaded_layers: engine.proof.offloaded.map_or(0, |(n, _)| n),
            // GGUF block count; the engine's own "N/M" total counts the output layer as well.
            total_layers: meta.block_count,
            n_ctx: engine.proof.n_ctx.unwrap_or(req.n_ctx),
            threads: req.threads.unwrap_or(4),
            parallel_slots: req.parallel_slots,
            fallback_reason,
            // Filled by the ModelManager, which owns the context plan.
            context_note: None,
            load_ms: started.elapsed().as_secs_f64() * 1000.0,
            pid: engine.child.id(),
            // proved by `prove_split` above, or the load failed
            split: req.split.as_ref().map(|s| s.stages.clone()),
        };
        tracing::info!(model = %info.model_id, device = %info.device_actual, offloaded = info.offloaded_layers, total = info.total_layers, n_ctx = info.n_ctx, load_ms = info.load_ms, "model loaded");
        Ok(Arc::new(LoadedLlama {
            info,
            port: engine.port,
            api_key: engine.api_key,
            client: engine.client,
            child: tokio::sync::Mutex::new(Some(engine.child)),
            log_path: engine.log_path,
        }))
    }
}

/// The split engine's log must prove the plan (`split::check_buffers` at `-lv 4`: the devices in
/// `--device` order, each device's KV share its planned block count). Ok carries the devices as the
/// log names them, e.g. `RPC0 (127.0.0.1:50052), MTL0`. Err is `BackendFailed` naming the mismatch.
fn prove_split(log: &str, plan: &Split) -> Result<String> {
    let p = split::parse_buffers(log);
    split::check_buffers(&p, &split::stage_tuples(plan), plan.n_layer as usize).map_err(|why| {
        CommonError::new(
            ErrorCode::BackendFailed,
            format!("split placement does not match the plan: {why}"),
        )
    })?;
    Ok(p.devices
        .iter()
        .map(|(d, ep)| ep.as_ref().map_or(d.clone(), |ep| format!("{d} ({ep})")))
        .collect::<Vec<_>>()
        .join(", "))
}

/// Auto mode tries the CPU after any failed GPU start: the GGUF header was validated before the
/// spawn, so even an exit the log classifier calls `ModelFileInvalid` (a Metal init failure is
/// wrapped in "failed to load model") deserves a CPU try, which reports the real verdict. Only a
/// load timeout is final, because the CPU attempt shares the same deadline.
fn auto_falls_back(e: &CommonError) -> bool {
    e.code != ErrorCode::Timeout
}

#[async_trait::async_trait]
impl InferenceBackend for LlamaCppBackend {
    fn name(&self) -> &str {
        "llama.cpp"
    }

    /// `llama-server --version`, run once and cached. Empty when the binary is missing.
    fn version(&self) -> String {
        self.version
            .get_or_init(|| process::version(&self.server_binary()).unwrap_or_default())
            .clone()
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            formats: vec!["gguf".into()],
            devices: if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                vec!["cpu".into(), "metal".into()]
            } else if cfg!(windows) {
                // catalog/backends.json pins the Vulkan build on Windows (CPU backends included).
                vec!["cpu".into(), "vulkan".into()]
            } else {
                vec!["cpu".into()]
            },
            streaming: true,
            cancellation: true,
            embeddings: true,
            json_schema: true,
            tools: true,
            multimodal: false,
            lora: false,
            speculative: false,
        }
    }

    async fn load(&self, req: LoadRequest) -> Result<Arc<dyn LoadedModel>> {
        Ok(self.load_llama(req).await?)
    }
}

/// A running `llama-server` with one model. Dropping it kills the process (`kill_on_drop`).
pub struct LoadedLlama {
    info: LoadedModelInfo,
    port: u16,
    api_key: String,
    client: reqwest::Client,
    child: tokio::sync::Mutex<Option<Child>>,
    pub log_path: PathBuf,
}

impl LoadedLlama {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<reqwest::Response> {
        let resp = self
            .client
            .post(self.url(path))
            .bearer_auth(&self.api_key)
            .json(body)
            .send()
            .await
            .map_err(transport)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        Err(map_http_error(status, &body, &self.api_key))
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        self.post(path, body).await?.json().await.map_err(transport)
    }

    /// The per-process engine key. Tests use it to prove the key is in neither the log nor argv.
    #[doc(hidden)]
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Engine slots currently processing a task (`GET /slots`). Lets tests prove a cancel freed the slot.
    pub async fn slots_processing(&self) -> Result<u32> {
        let resp = self
            .client
            .get(self.url("/slots"))
            .bearer_auth(&self.api_key)
            .send()
            .await
            .map_err(transport)?;
        let v: Value = resp.json().await.map_err(transport)?;
        Ok(v.as_array().map_or(0, |slots| {
            slots.iter().filter(|s| s["is_processing"] == true).count() as u32
        }))
    }
}

#[async_trait::async_trait]
impl LoadedModel for LoadedLlama {
    fn info(&self) -> &LoadedModelInfo {
        &self.info
    }

    async fn generate(&self, req: GenerateRequest, cancel: CancellationToken) -> Result<GenerateStream> {
        if req.max_tokens == 0 {
            return Err(CommonError::invalid("max_tokens must be >= 1"));
        }
        let (path, body, chat) = match &req.input {
            PromptInput::Raw(prompt) => ("/completion", completion_body(&req, prompt), false),
            PromptInput::Chat(messages) => ("/v1/chat/completions", chat_body(&req, messages), true),
        };
        let sent = Instant::now();
        let resp = self.post(path, &body).await?;
        let state = StreamState {
            resp: Some(resp),
            parser: sse::SseParser::default(),
            pending: VecDeque::new(),
            chat,
            cancel,
            timeouts: req.timeouts,
            sent,
            first_token_at: None,
            chat_finish: None,
            done: false,
        };
        Ok(futures_util::stream::unfold(state, |mut st| async move {
            let ev = st.next().await?;
            Some((ev, st))
        })
        .boxed())
    }

    async fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        let v = self
            .post_json(
                "/tokenize",
                &json!({"content": text, "add_special": true, "with_pieces": false}),
            )
            .await?;
        v["tokens"]
            .as_array()
            .ok_or_else(|| {
                CommonError::new(
                    ErrorCode::BackendFailed,
                    "tokenize: engine response has no tokens array",
                )
            })?
            .iter()
            .map(|t| {
                t.as_u64()
                    .and_then(|t| u32::try_from(t).ok())
                    .ok_or_else(|| CommonError::new(ErrorCode::BackendFailed, "tokenize: non-integer token id"))
            })
            .collect()
    }

    async fn detokenize(&self, ids: &[u32]) -> Result<String> {
        let v = self.post_json("/detokenize", &json!({"tokens": ids})).await?;
        v["content"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| CommonError::new(ErrorCode::BackendFailed, "detokenize: engine response has no content"))
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        if inputs.is_empty() {
            return Ok(vec![]);
        }
        let v = self
            .post_json("/v1/embeddings", &json!({"input": inputs}))
            .await
            .map_err(|e| {
                if e.code == ErrorCode::InvalidRequest && e.message.contains("--embeddings") {
                    CommonError::invalid("this model was not loaded with embeddings=true; reload it in embeddings mode")
                } else {
                    e
                }
            })?;
        parse_embeddings(&v, inputs.len())
    }

    fn memory_rss_bytes(&self) -> Option<u64> {
        self.info.pid.and_then(process::rss_bytes)
    }

    fn exited(&self) -> Option<String> {
        // Locked only by `unload`, which takes the child: nothing to report then.
        let mut child = self.child.try_lock().ok()?;
        match child.as_mut()?.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(e) => Some(format!("cannot be polled: {e}")),
        }
    }

    async fn unload(&self) -> Result<()> {
        if let Some(child) = self.child.lock().await.take() {
            process::stop(child).await;
            // A clean stop leaves nothing to diagnose; failed starts keep their log (see `start`).
            let _ = std::fs::remove_file(&self.log_path);
        }
        Ok(())
    }
}

/// `data[i].embedding` goes to slot `data[i].index`; every input must get exactly one non-empty
/// vector, or the engine answer is a typed failure rather than a 200 with empty embeddings.
fn parse_embeddings(v: &Value, n: usize) -> Result<Vec<Vec<f32>>> {
    let data = v["data"].as_array().ok_or_else(|| {
        CommonError::new(
            ErrorCode::BackendFailed,
            "embeddings: engine response has no data array",
        )
    })?;
    let mut out = vec![Vec::new(); n];
    for (i, item) in data.iter().enumerate() {
        let idx = item["index"].as_u64().map_or(i, |x| x as usize);
        let vec: Vec<f32> = item["embedding"].as_array().map_or(vec![], |a| {
            a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()
        });
        if idx >= n || vec.is_empty() || !out[idx].is_empty() {
            return Err(CommonError::new(
                ErrorCode::BackendFailed,
                "embeddings: malformed engine response (bad, empty or duplicate index)",
            ));
        }
        out[idx] = vec;
    }
    if out.iter().any(Vec::is_empty) {
        return Err(CommonError::new(
            ErrorCode::BackendFailed,
            format!("embeddings: engine returned {} vectors for {n} inputs", data.len()),
        ));
    }
    Ok(out)
}

fn transport(e: reqwest::Error) -> CommonError {
    CommonError::new(ErrorCode::BackendFailed, format!("engine HTTP transport failed: {e}"))
}

/// Map an engine error reply. `err` is the `error` object of the body (or Null).
fn map_engine_error(status: u16, err: &Value, api_key: &str) -> CommonError {
    let kind = err["type"].as_str().unwrap_or("");
    let msg = err["message"].as_str().map_or_else(
        || format!("engine returned HTTP {status}"),
        |m| logparse::scrub(m, api_key),
    );
    let code = match (status, kind) {
        (400, "exceed_context_size_error") => ErrorCode::ContextTooLong,
        (400, _) | (501, _) => ErrorCode::InvalidRequest,
        (429, _) | (503, _) => ErrorCode::Overloaded,
        _ => ErrorCode::BackendFailed,
    };
    CommonError::new(code, msg)
}

fn map_http_error(status: u16, body: &str, api_key: &str) -> CommonError {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    map_engine_error(status, &v["error"], api_key)
}

fn completion_body(req: &GenerateRequest, prompt: &str) -> Value {
    let mut b = json!({
        "prompt": prompt,
        "n_predict": req.max_tokens,
        "temperature": req.temperature,
        "top_p": req.top_p,
        "seed": req.seed.map_or(-1, |s| s as i64),
        "stop": req.stop,
        "stream": true,
        "return_tokens": req.return_token_ids,
        "cache_prompt": false,
    });
    if let Some(s) = &req.json_schema {
        b["json_schema"] = s.clone();
    }
    if let Some(g) = &req.grammar {
        b["grammar"] = json!(g);
    }
    b
}

/// OpenAI-compatible chat body. Token ids are not available on this endpoint, so
/// `return_token_ids` is ignored for chat requests.
fn chat_body(req: &GenerateRequest, messages: &[ChatMessage]) -> Value {
    let mut b = json!({
        "messages": messages,
        "max_tokens": req.max_tokens,
        "temperature": req.temperature,
        "top_p": req.top_p,
        "stop": req.stop,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if let Some(s) = req.seed {
        b["seed"] = json!(s);
    }
    if let Some(t) = &req.tools {
        b["tools"] = t.clone();
    }
    if let Some(s) = &req.json_schema {
        b["response_format"] = json!({"type": "json_schema", "json_schema": {"name": "response", "schema": s}});
    }
    if let Some(g) = &req.grammar {
        b["grammar"] = json!(g);
    }
    b
}

fn timings_from(t: &Value) -> Timings {
    Timings {
        time_to_first_token_ms: None,
        prompt_ms: t["prompt_ms"].as_f64(),
        prompt_tokens_per_second: t["prompt_per_second"].as_f64(),
        decode_ms: t["predicted_ms"].as_f64(),
        decode_tokens_per_second: t["predicted_per_second"].as_f64(),
    }
}

fn no_tokens_error() -> CommonError {
    CommonError::new(ErrorCode::BackendFailed, "engine produced no tokens")
}

/// Events from one `/completion` SSE payload.
fn completion_events(payload: &str, api_key: &str) -> Vec<Result<GenerateEvent>> {
    let v: Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(e) => {
            return vec![Err(CommonError::new(
                ErrorCode::BackendFailed,
                format!("engine sent invalid JSON in the stream: {e}"),
            ))]
        }
    };
    if !v["error"].is_null() {
        return vec![Err(map_engine_error(
            v["error"]["code"].as_u64().unwrap_or(500) as u16,
            &v["error"],
            api_key,
        ))];
    }
    let mut out = Vec::new();
    let text = v["content"].as_str().unwrap_or("");
    let token_ids: Vec<u32> = v["tokens"].as_array().map_or(vec![], |a| {
        a.iter()
            .filter_map(|t| t.as_u64().and_then(|t| u32::try_from(t).ok()))
            .collect()
    });
    if !text.is_empty() || !token_ids.is_empty() {
        out.push(Ok(GenerateEvent::Token {
            text: text.into(),
            token_ids,
        }));
    }
    if v["stop"] == true {
        let finish_reason = match v["stop_type"].as_str() {
            Some("limit") => "length",
            Some("eos") | Some("word") => "stop",
            other => other.unwrap_or("unknown"),
        }
        .to_string();
        let usage = Usage {
            prompt_tokens: v["tokens_evaluated"].as_u64().unwrap_or(0) as u32,
            completion_tokens: v["tokens_predicted"].as_u64().unwrap_or(0) as u32,
        };
        if usage.completion_tokens == 0 && finish_reason != "stop" {
            out.push(Err(no_tokens_error()));
        } else {
            out.push(Ok(GenerateEvent::Done {
                finish_reason,
                usage,
                timings: timings_from(&v["timings"]),
            }));
        }
    }
    out
}

/// Events from one `/v1/chat/completions` SSE payload. `finish` carries the finish_reason from
/// the chunk that announced it to the later chunk that carries usage.
fn chat_events(payload: &str, finish: &mut Option<String>, api_key: &str) -> Vec<Result<GenerateEvent>> {
    if payload.trim() == "[DONE]" {
        // Normally unreachable: the usage chunk before it already produced Done.
        return vec![Ok(GenerateEvent::Done {
            finish_reason: finish.take().unwrap_or_else(|| "unknown".into()),
            usage: Usage::default(),
            timings: Timings::default(),
        })];
    }
    let v: Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(e) => {
            return vec![Err(CommonError::new(
                ErrorCode::BackendFailed,
                format!("engine sent invalid JSON in the stream: {e}"),
            ))]
        }
    };
    if !v["error"].is_null() {
        return vec![Err(map_engine_error(
            v["error"]["code"].as_u64().unwrap_or(500) as u16,
            &v["error"],
            api_key,
        ))];
    }
    let mut out = Vec::new();
    if let Some(choice) = v["choices"].get(0) {
        let delta = &choice["delta"];
        if let Some(t) = delta["content"].as_str().filter(|t| !t.is_empty()) {
            out.push(Ok(GenerateEvent::Token {
                text: t.into(),
                token_ids: vec![],
            }));
        }
        if !delta["tool_calls"].is_null() {
            out.push(Ok(GenerateEvent::ToolCallDelta { delta: delta.clone() }));
        }
        if let Some(f) = choice["finish_reason"].as_str() {
            *finish = Some(f.into());
        }
    }
    let usage = &v["usage"];
    if usage.is_object() {
        let usage = Usage {
            prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
            completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
        };
        let finish_reason = finish.take().unwrap_or_else(|| "unknown".into());
        if usage.completion_tokens == 0 && finish_reason != "stop" {
            out.push(Err(no_tokens_error()));
        } else {
            out.push(Ok(GenerateEvent::Done {
                finish_reason,
                usage,
                timings: timings_from(&v["timings"]),
            }));
        }
    }
    out
}

struct StreamState {
    /// Taken (connection closed) as soon as the stream is terminal, so a cancel frees the engine
    /// slot even if the consumer keeps the stream around.
    resp: Option<reqwest::Response>,
    parser: sse::SseParser,
    pending: VecDeque<Result<GenerateEvent>>,
    chat: bool,
    cancel: CancellationToken,
    timeouts: Timeouts,
    sent: Instant,
    first_token_at: Option<Instant>,
    chat_finish: Option<String>,
    done: bool,
}

impl StreamState {
    fn finish(&mut self) {
        self.done = true;
        self.resp = None;
    }

    async fn next(&mut self) -> Option<Result<GenerateEvent>> {
        loop {
            if self.done {
                return None;
            }
            if let Some(mut ev) = self.pending.pop_front() {
                match &mut ev {
                    Ok(GenerateEvent::Done { timings, .. }) => {
                        timings.time_to_first_token_ms =
                            self.first_token_at.map(|t| (t - self.sent).as_secs_f64() * 1000.0);
                        self.finish();
                    }
                    Err(_) => self.finish(),
                    _ => {}
                }
                return Some(ev);
            }
            let resp = self.resp.as_mut()?;
            let first = self.first_token_at.is_none();
            let per_item = if first {
                self.timeouts.first_token
            } else {
                self.timeouts.inter_token
            };
            let wait = per_item.min(self.timeouts.total.saturating_sub(self.sent.elapsed()));
            let chunk = tokio::select! {
                _ = self.cancel.cancelled() => Err(CommonError::new(ErrorCode::Cancelled, "generation cancelled")),
                r = tokio::time::timeout(wait, resp.chunk()) => match r {
                    Err(_) => Err(CommonError::new(ErrorCode::Timeout, if first { "no first token within the first-token timeout" } else { "no token within the inter-token timeout" })),
                    Ok(Err(e)) => Err(CommonError::new(ErrorCode::BackendFailed, format!("engine stream failed: {e}"))),
                    Ok(Ok(None)) => Err(CommonError::new(ErrorCode::BackendFailed, "engine closed the stream before signalling completion")),
                    Ok(Ok(Some(bytes))) => Ok(bytes),
                },
            };
            match chunk {
                Err(e) => self.pending.push_back(Err(e)),
                Ok(bytes) => {
                    for payload in self.parser.push(&bytes) {
                        let evs = if self.chat {
                            chat_events(&payload, &mut self.chat_finish, "")
                        } else {
                            completion_events(&payload, "")
                        };
                        // A tool call is the model's first output too: TTFT and the inter-token clock start here.
                        if self.first_token_at.is_none()
                            && evs.iter().any(|e| {
                                matches!(e, Ok(GenerateEvent::Token { .. } | GenerateEvent::ToolCallDelta { .. }))
                            })
                        {
                            self.first_token_at = Some(Instant::now());
                        }
                        self.pending.extend(evs);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const COMPLETION_FIXTURE: &str = include_str!("llamacpp/fixtures/completion_stream.sse");
    const CHAT_FIXTURE: &str = include_str!("llamacpp/fixtures/chat_stream.sse");
    const CHAT_TOOLS_FIXTURE: &str = include_str!("llamacpp/fixtures/chat_tools_stream.sse");

    fn payloads(fixture: &str) -> Vec<String> {
        sse::SseParser::default().push(fixture.as_bytes())
    }

    #[test]
    fn completion_fixture_maps_to_32_tokens_and_done() {
        let evs: Vec<GenerateEvent> = payloads(COMPLETION_FIXTURE)
            .iter()
            .flat_map(|p| completion_events(p, ""))
            .map(|e| e.unwrap())
            .collect();
        assert_eq!(evs.len(), 33);
        let mut text = String::new();
        let mut ids: Vec<u32> = vec![];
        for e in &evs[..32] {
            match e {
                GenerateEvent::Token { text: t, token_ids } => {
                    text.push_str(t);
                    ids.extend(token_ids);
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(text, " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big");
        assert_eq!(ids.len(), 32);
        assert_eq!(ids[..3], [4257, 4111, 1460]);
        match &evs[32] {
            GenerateEvent::Done {
                finish_reason,
                usage,
                timings,
            } => {
                assert_eq!(finish_reason, "length");
                assert_eq!(
                    *usage,
                    Usage {
                        prompt_tokens: 10,
                        completion_tokens: 32
                    }
                );
                assert_eq!(timings.decode_tokens_per_second, Some(399.8864838368463));
                assert_eq!(timings.prompt_ms, Some(9.951));
                assert_eq!(
                    timings.time_to_first_token_ms, None,
                    "filled by the stream, not the parser"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn chat_fixture_maps_to_tokens_and_done_with_usage() {
        let mut finish = None;
        let evs: Vec<GenerateEvent> = payloads(CHAT_FIXTURE)
            .iter()
            .flat_map(|p| chat_events(p, &mut finish, ""))
            .map(|e| e.unwrap())
            .collect();
        let tokens: Vec<&GenerateEvent> = evs
            .iter()
            .filter(|e| matches!(e, GenerateEvent::Token { .. }))
            .collect();
        assert_eq!(tokens.len(), 8);
        assert!(matches!(tokens[0], GenerateEvent::Token { text, .. } if text == "Hello"));
        let dones: Vec<&GenerateEvent> = evs.iter().filter(|e| matches!(e, GenerateEvent::Done { .. })).collect();
        assert_eq!(dones.len(), 2, "usage chunk + [DONE]; the stream stops after the first");
        match dones[0] {
            GenerateEvent::Done {
                finish_reason,
                usage,
                timings,
            } => {
                assert_eq!(finish_reason, "length");
                assert_eq!(
                    *usage,
                    Usage {
                        prompt_tokens: 31,
                        completion_tokens: 8
                    }
                );
                assert!(timings.decode_tokens_per_second.unwrap() > 0.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn chat_tool_call_deltas_pass_through() {
        let mut finish = None;
        let evs: Vec<GenerateEvent> = payloads(CHAT_TOOLS_FIXTURE)
            .iter()
            .flat_map(|p| chat_events(p, &mut finish, ""))
            .map(|e| e.unwrap())
            .collect();
        let deltas: Vec<&Value> = evs
            .iter()
            .filter_map(|e| match e {
                GenerateEvent::ToolCallDelta { delta } => Some(delta),
                _ => None,
            })
            .collect();
        assert!(deltas.len() >= 3);
        assert_eq!(deltas[0]["tool_calls"][0]["function"]["name"], "get_weather");
        assert!(matches!(evs.last().unwrap(), GenerateEvent::Done { .. }));
    }

    #[test]
    fn stream_errors_and_empty_output_are_typed() {
        let evs = completion_events(
            r#"{"error":{"code":503,"message":"slot unavailable","type":"unavailable_error"}}"#,
            "",
        );
        assert_eq!(evs[0].as_ref().unwrap_err().code, ErrorCode::Overloaded);
        let evs = completion_events(
            r#"{"content":"","tokens":[],"stop":true,"stop_type":"limit","tokens_predicted":0,"tokens_evaluated":5}"#,
            "",
        );
        assert_eq!(evs[0].as_ref().unwrap_err().message, "engine produced no tokens");
        let evs = completion_events(
            r#"{"content":"","stop":true,"stop_type":"eos","tokens_predicted":0,"tokens_evaluated":5}"#,
            "",
        );
        assert!(
            matches!(evs[0], Ok(GenerateEvent::Done { .. })),
            "immediate EOS is a legitimate stop"
        );
        assert_eq!(
            completion_events("{not json", "")[0].as_ref().unwrap_err().code,
            ErrorCode::BackendFailed
        );
        let mut finish = None;
        let evs = chat_events(
            r#"{"choices":[{"finish_reason":"length","index":0,"delta":{}}]}"#,
            &mut finish,
            "",
        );
        assert!(evs.is_empty());
        let evs = chat_events(
            r#"{"choices":[],"usage":{"completion_tokens":0,"prompt_tokens":3}}"#,
            &mut finish,
            "",
        );
        assert_eq!(evs[0].as_ref().unwrap_err().message, "engine produced no tokens");
    }

    #[test]
    fn http_error_mapping_scrubs_key() {
        const KEY: &str = "0d4b7a1e-7c7e-4a4b-9a1b-5e2c3f6d8a90";
        let ctx = r#"{"error":{"code":400,"message":"request (302 tokens) exceeds the available context size (128 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":302,"n_ctx":128}}"#;
        let e = map_http_error(400, ctx, KEY);
        assert_eq!(e.code, ErrorCode::ContextTooLong);
        assert!(e.message.contains("302 tokens"));
        assert_eq!(map_http_error(400, r#"{"error":{"code":400,"message":"Field 'n_predict': type must be number","type":"invalid_request_error"}}"#, KEY).code, ErrorCode::InvalidRequest);
        assert_eq!(
            map_http_error(
                503,
                r#"{"error":{"code":503,"message":"Loading model","type":"unavailable_error"}}"#,
                KEY
            )
            .code,
            ErrorCode::Overloaded
        );
        assert_eq!(map_http_error(501, r#"{"error":{"code":501,"message":"This server does not support embeddings. Start it with `--embeddings`","type":"not_supported_error"}}"#, KEY).code, ErrorCode::InvalidRequest);
        assert_eq!(map_http_error(500, "garbage", KEY).code, ErrorCode::BackendFailed);
        let e = map_http_error(500, r#"{"error":{"message":"bad key abc-123 here"}}"#, "abc-123");
        assert_eq!(e.message, "bad key [redacted] here");
    }

    #[test]
    fn embeddings_response_must_cover_every_input() {
        let ok = json!({"data": [{"index": 1, "embedding": [0.5]}, {"index": 0, "embedding": [0.1, 0.2]}]});
        assert_eq!(parse_embeddings(&ok, 2).unwrap(), vec![vec![0.1, 0.2], vec![0.5]]);
        let no_index = json!({"data": [{"embedding": [1.0]}]});
        assert_eq!(
            parse_embeddings(&no_index, 1).unwrap(),
            vec![vec![1.0]],
            "missing index means positional"
        );
        let short = json!({"data": [{"index": 0, "embedding": [0.1]}]});
        let e = parse_embeddings(&short, 2).unwrap_err();
        assert_eq!(e.code, ErrorCode::BackendFailed);
        assert!(e.message.contains("1 vectors for 2 inputs"), "{e}");
        let dup = json!({"data": [{"index": 0, "embedding": [0.1]}, {"index": 0, "embedding": [0.2]}]});
        assert!(parse_embeddings(&dup, 2).unwrap_err().message.contains("duplicate"));
        assert_eq!(
            parse_embeddings(&json!({"data": [{"index": 5, "embedding": [0.1]}]}), 1)
                .unwrap_err()
                .code,
            ErrorCode::BackendFailed
        );
        assert_eq!(
            parse_embeddings(&json!({"data": [{"index": 0, "embedding": []}]}), 1)
                .unwrap_err()
                .code,
            ErrorCode::BackendFailed
        );
        assert_eq!(
            parse_embeddings(&json!({}), 1).unwrap_err().code,
            ErrorCode::BackendFailed
        );
        assert!(auto_falls_back(&CommonError::new(ErrorCode::ModelFileInvalid, "x")));
        assert!(auto_falls_back(&CommonError::new(ErrorCode::BackendFailed, "x")));
        assert!(!auto_falls_back(&CommonError::new(ErrorCode::Timeout, "x")));
    }

    /// FAKE engine on 127.0.0.1 (not llama-server): answers every request with SSE headers,
    /// `tokens` token payloads, then holds the connection open without sending anything more.
    async fn stalled_engine(tokens: usize) -> u16 {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = l.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                tokio::spawn(async move {
                    let _ = sock.read(&mut [0u8; 4096]).await;
                    let mut body =
                        String::from("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n");
                    for i in 0..tokens {
                        body.push_str(&format!(
                            "data: {{\"content\":\" t{i}\",\"tokens\":[{i}],\"stop\":false}}\n\n"
                        ));
                    }
                    let _ = sock.write_all(body.as_bytes()).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });
        port
    }

    fn against_fake_engine(port: u16) -> LoadedLlama {
        LoadedLlama {
            info: LoadedModelInfo::default(),
            port,
            api_key: "k".into(),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            child: tokio::sync::Mutex::new(None),
            log_path: PathBuf::new(),
        }
    }

    // ---- split loads (WP-04 C2) against a FAKE engine ----

    /// SYNTHETIC GGUF header with stories15M's shape (6 blocks); not a model.
    #[cfg(unix)]
    fn synthetic_gguf(path: &Path) {
        fn s(v: &str) -> Vec<u8> {
            [(v.len() as u64).to_le_bytes().as_slice(), v.as_bytes()].concat()
        }
        fn kv_u32(k: &str, v: u32) -> Vec<u8> {
            [s(k), 4u32.to_le_bytes().to_vec(), v.to_le_bytes().to_vec()].concat()
        }
        let kvs = [
            [s("general.architecture"), 8u32.to_le_bytes().to_vec(), s("llama")].concat(),
            kv_u32("llama.block_count", 6),
            kv_u32("llama.context_length", 128),
            kv_u32("llama.embedding_length", 288),
            kv_u32("llama.attention.head_count", 6),
        ];
        let mut g = [b"GGUF".as_slice(), &3u32.to_le_bytes(), &0u64.to_le_bytes()].concat();
        g.extend((kvs.len() as u64).to_le_bytes());
        g.extend(kvs.concat());
        g.resize(g.len().next_multiple_of(32) + 64, 0);
        std::fs::write(path, g).unwrap();
    }

    /// FAKE `llama-server` (python3, not llama.cpp): appends `<pid> <argv>` to `starts` for every
    /// start, writes `log` as its engine log, then either exits 1 or answers `/health` with ok.
    #[cfg(unix)]
    fn fake_engine(dir: &Path, log: &str, serve: bool) -> (LlamaCppBackend, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let engine = dir.join("engine");
        std::fs::create_dir_all(&engine).unwrap();
        let fixture = dir.join("fixture.log");
        std::fs::write(&fixture, log).unwrap();
        let starts = dir.join("starts");
        let script = format!(
            r#"#!/usr/bin/env python3
import os, sys, http.server
a = sys.argv[1:]
if "--version" in a:
    print("version: 0 (FAKE engine)")
    sys.exit(0)
with open({starts:?}, "a") as f:
    f.write(str(os.getpid()) + " " + " ".join(a) + "\n")
sys.stderr.write(open({fixture:?}).read())
sys.stderr.flush()
if not {serve}:
    sys.exit(1)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'{{"status":"ok"}}')
    def log_message(self, *x):
        pass
http.server.HTTPServer(("127.0.0.1", int(a[a.index("--port") + 1])), H).serve_forever()
"#,
            starts = starts.display().to_string(),
            fixture = fixture.display().to_string(),
            serve = if serve { "True" } else { "False" },
        );
        let bin = engine.join("llama-server");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let model = dir.join("tiny.gguf");
        synthetic_gguf(&model);
        let backend = LlamaCppBackend::new(engine, Paths::at(dir.join("home")));
        (backend, model, starts)
    }

    #[cfg(unix)]
    fn split_req(model: &Path, stages: &[(&str, u32, u32)]) -> LoadRequest {
        LoadRequest {
            model_id: "tiny".into(),
            model_path: model.to_path_buf(),
            model_sha256: String::new(),
            device: DevicePreference::Auto,
            n_ctx: 128,
            n_gpu_layers: None,
            threads: Some(1),
            parallel_slots: 1,
            kv_type: common_models::KvType::F16,
            embeddings: false,
            timeouts: Timeouts {
                load: Duration::from_secs(20),
                ..Timeouts::default()
            },
            extra_args: vec![],
            split: Some(Split {
                rpc_ports: vec![49167],
                stages: stages
                    .iter()
                    .map(|&(d, a, b)| SplitStage {
                        device: d.into(),
                        first_block: a,
                        end_block: b,
                    })
                    .collect(),
                n_layer: 6,
                no_mmap: false,
                engine_pid: Default::default(),
            }),
        }
    }

    #[cfg(unix)]
    fn starts(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// ADR-0005 D2/D7: a split engine that fails to start is the load's error. The same failure on a
    /// local Auto load is retried on the CPU (the control), so the test would see a fallback.
    #[cfg(unix)]
    #[tokio::test]
    async fn split_never_falls_back_to_cpu() {
        let dir = tempfile::tempdir().unwrap();
        let (backend, model, log) = fake_engine(dir.path(), "error: failed to connect to RPC0\n", false);
        let e = backend
            .load_llama(split_req(&model, &[("RPC0", 0, 3), ("MTL0", 3, 6)]))
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::BackendFailed, "{e}");
        let s = starts(&log);
        assert_eq!(s.len(), 1, "one engine start, no CPU or local retry: {s:?}");
        assert!(
            s[0].contains("--rpc 127.0.0.1:49167") && !s[0].contains("--device none"),
            "{s:?}"
        );
        // control: without the split, Auto retries on the CPU after the same failure
        std::fs::remove_file(&log).unwrap();
        let mut local = split_req(&model, &[]);
        local.split = None;
        assert!(backend.load_llama(local).await.is_err());
        let s = starts(&log);
        assert_eq!(s.len(), 2, "{s:?}");
        assert!(s[1].contains("--device none"), "{s:?}");
    }

    /// The RPC bridge's owner check and per-load byte cap (ADR-0005 D4, D6) read the split
    /// engine's pid from `Split::engine_pid`: it is set at every start, including the one retry
    /// after a lost port race (`couldn't bind`), which is a second start against the same RPC
    /// servers and so a second load. FAKE engine.
    #[cfg(unix)]
    #[tokio::test]
    async fn split_publishes_each_engine_start_pid() {
        let dir = tempfile::tempdir().unwrap();
        let (backend, model, log) = fake_engine(dir.path(), "couldn't bind HTTP server socket\n", false);
        let req = split_req(&model, &[("RPC0", 0, 3), ("MTL0", 3, 6)]);
        let sink = req.split.as_ref().unwrap().engine_pid.clone();
        assert_eq!(sink.get(), None);
        assert!(backend.load_llama(req).await.is_err());
        let s = starts(&log);
        assert_eq!(s.len(), 2, "the port-race retry: {s:?}");
        let pid: u32 = s[1].split(' ').next().unwrap().parse().unwrap();
        assert_eq!(
            sink.get(),
            Some((pid, 2)),
            "the second start, counted as the second load"
        );
    }

    /// A ready split engine whose `-lv 4` log (real b11609 lines: RPC0 holds [0,3), MTL0 [3,6))
    /// does not prove the plan is stopped and the load fails naming the mismatch; the matching plan
    /// loads and reports the stages and devices as proved.
    #[cfg(unix)]
    #[tokio::test]
    async fn split_load_fails_on_a_placement_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let (backend, model, log) = fake_engine(
            dir.path(),
            include_str!("llamacpp/fixtures/b11609-rpc-split-lv4-A.log"),
            true,
        );
        let e = backend
            .load_llama(split_req(&model, &[("RPC0", 0, 2), ("MTL0", 2, 6)]))
            .await
            .err()
            .unwrap();
        assert_eq!(e.code, ErrorCode::BackendFailed, "{e}");
        assert!(e.message.contains("split placement does not match the plan"), "{e}");
        assert!(
            e.message.contains("RPC0 holds 2.98 layers of KV, the plan gives it 2"),
            "{e}"
        );
        let s = starts(&log);
        assert_eq!(s.len(), 1, "no fallback after a placement mismatch: {s:?}");
        let pid: u32 = s[0].split(' ').next().unwrap().parse().unwrap();
        assert!(!process::alive(pid), "the mismatched engine was stopped");

        let stages = [("RPC0", 0, 3), ("MTL0", 3, 6)];
        let loaded = backend.load_llama(split_req(&model, &stages)).await.unwrap();
        let info = loaded.info();
        assert_eq!(info.device_actual, "RPC0 (127.0.0.1:49167), MTL0");
        assert_eq!(info.split, split_req(&model, &stages).split.map(|s| s.stages));
        assert_eq!(info.fallback_reason, None);
        loaded.unload().await.unwrap();
        // a plan for another model is refused before any start
        let mut other = split_req(&model, &[("RPC0", 0, 3), ("MTL0", 3, 7)]);
        other.split.as_mut().unwrap().n_layer = 7;
        let e = backend.load_llama(other).await.err().unwrap();
        assert!(e.message.contains("plan is for 7 blocks, the model has 6"), "{e}");
        assert_eq!(starts(&log).len(), 2);
    }

    /// F-08-01: a backend that stops sending is a typed Timeout within the configured bound.
    #[tokio::test]
    async fn stalled_engine_is_a_typed_timeout_within_bound() {
        let timeouts = Timeouts {
            first_token: Duration::from_millis(500),
            inter_token: Duration::from_millis(500),
            ..Timeouts::default()
        };
        let req = GenerateRequest {
            input: PromptInput::Raw("p".into()),
            max_tokens: 4,
            timeouts,
            ..Default::default()
        };

        let model = against_fake_engine(stalled_engine(1).await);
        let t = Instant::now();
        let mut s = model.generate(req.clone(), CancellationToken::new()).await.unwrap();
        assert!(matches!(s.next().await, Some(Ok(GenerateEvent::Token { ref text, .. })) if text == " t0"));
        let e = s.next().await.unwrap().unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout, "{e}");
        assert!(e.message.contains("inter-token"), "{e}");
        assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
        assert!(s.next().await.is_none(), "terminal after the error");

        let model = against_fake_engine(stalled_engine(0).await);
        let t = Instant::now();
        let mut s = model.generate(req, CancellationToken::new()).await.unwrap();
        let e = s.next().await.unwrap().unwrap_err();
        assert_eq!(e.code, ErrorCode::Timeout, "{e}");
        assert!(e.message.contains("first-token"), "{e}");
        assert!(t.elapsed() < Duration::from_millis(1500), "{:?}", t.elapsed());
    }

    #[test]
    fn request_bodies() {
        let req = GenerateRequest {
            input: PromptInput::Raw("p".into()),
            max_tokens: 32,
            temperature: 0.0,
            top_p: 0.9,
            seed: Some(42),
            stop: vec!["\n".into()],
            json_schema: Some(json!({"type": "object"})),
            return_token_ids: true,
            ..Default::default()
        };
        let b = completion_body(&req, "p");
        assert_eq!(b["n_predict"], 32);
        assert_eq!(b["seed"], 42);
        assert_eq!(b["stream"], true);
        assert_eq!(b["cache_prompt"], false);
        assert_eq!(b["return_tokens"], true);
        assert_eq!(b["json_schema"]["type"], "object");
        assert_eq!(b["stop"][0], "\n");
        assert_eq!(completion_body(&GenerateRequest::default(), "")["seed"], -1);
        let msgs = vec![ChatMessage {
            role: "user".into(),
            content: "hi".into(),
        }];
        let c = chat_body(&req, &msgs);
        assert_eq!(c["messages"][0]["role"], "user");
        assert_eq!(c["stream_options"]["include_usage"], true);
        assert_eq!(c["response_format"]["type"], "json_schema");
        assert_eq!(c["response_format"]["json_schema"]["schema"]["type"], "object");
        assert_eq!(c["max_tokens"], 32);
    }
}
