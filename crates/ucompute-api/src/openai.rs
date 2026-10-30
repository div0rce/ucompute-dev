//! OpenAI-compatible endpoints (F-09-01, F-09-02, F-01-12): `/v1/models`, `/v1/chat/completions`,
//! `/v1/completions`, `/v1/embeddings`. Request fields the engine cannot honour are rejected
//! (`n > 1`) or ignored (`logprobs`, `user`, ...); unknown fields are ignored like OpenAI does.
//!
//! UCompute extension `target` (body field or `X-UCompute-Target` header): `"local"` (default) or
//! `"peer:<peer id or short label>"` runs the whole request on one paired device (Mode B, remote
//! execution, never pooled inference). `"split:<peer>"` splits the model's blocks between this
//! device and that one (WP-04 pooled inference, [`crate::split`]; body fields `split_at`,
//! `split_mode`, `acknowledged`), and its responses also carry `"placement"` as the engine log
//! proved it. `PolicyEngine::evaluate` decides before admission, load or any network activity
//! (I-05); every response carries `"target"` so nothing runs elsewhere silently.

use crate::admission::Rejected;
use crate::{ApiError, AppState, Json};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{self, BoxStream, Stream};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;
use ucompute_core::{ErrorCode, Result, UComputeError};
use ucompute_identity::PeerId;
use ucompute_observability::jobs::{Finished, JobHandle};
use ucompute_policy::{ExecutionTarget, PrivacyMode, WorkloadKind};
use ucompute_runtime::manager::LoadOptions;
use ucompute_runtime::{ChatMessage, GenerateEvent, GenerateRequest, GenerateStream, PromptInput, Timings, Usage};

/// Engine defaults (llama.cpp) when the request leaves sampling unset.
const DEFAULT_MAX_TOKENS: u32 = 256;
const DEFAULT_TEMPERATURE: f32 = 0.8;
const DEFAULT_TOP_P: f32 = 0.95;

#[derive(Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

#[derive(Deserialize, Default)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

/// Sampling and output options shared by chat and text completions.
#[derive(Deserialize, Default)]
pub struct Sampling {
    pub max_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub seed: Option<i64>,
    pub stop: Option<OneOrMany>,
    #[serde(default)]
    pub stream: bool,
    pub stream_options: Option<StreamOptions>,
    pub response_format: Option<Value>,
    pub n: Option<u32>,
}

impl Sampling {
    fn to_generate(&self, input: PromptInput, tools: Option<Value>) -> Result<GenerateRequest> {
        if self.n.is_some_and(|n| n != 1) {
            return Err(UComputeError::invalid("n must be 1"));
        }
        let max_tokens = self
            .max_completion_tokens
            .or(self.max_tokens)
            .unwrap_or(DEFAULT_MAX_TOKENS);
        if max_tokens == 0 {
            return Err(UComputeError::invalid("max_tokens must be >= 1"));
        }
        let temperature = self.temperature.unwrap_or(DEFAULT_TEMPERATURE);
        let top_p = self.top_p.unwrap_or(DEFAULT_TOP_P);
        if !(0.0..=2.0).contains(&temperature) || !(0.0..=1.0).contains(&top_p) {
            return Err(UComputeError::invalid(
                "temperature must be in [0, 2] and top_p in [0, 1]",
            ));
        }
        Ok(GenerateRequest {
            input,
            max_tokens,
            temperature,
            top_p,
            seed: self.seed.and_then(|s| u64::try_from(s).ok()),
            stop: self.stop.as_ref().map_or_else(Vec::new, |s| match s {
                OneOrMany::One(x) => vec![x.clone()],
                OneOrMany::Many(v) => v.clone(),
            }),
            json_schema: json_schema_of(self.response_format.as_ref())?,
            grammar: None,
            tools,
            return_token_ids: false,
            timeouts: Default::default(),
        })
    }

    fn include_usage(&self) -> bool {
        self.stream_options.as_ref().is_some_and(|o| o.include_usage)
    }
}

/// `response_format` -> schema for constrained decoding: `json_schema` uses the given schema,
/// `json_object` any JSON object, `text`/absent none.
fn json_schema_of(rf: Option<&Value>) -> Result<Option<Value>> {
    let Some(rf) = rf else { return Ok(None) };
    match rf["type"].as_str() {
        None | Some("text") => Ok(None),
        Some("json_object") => Ok(Some(json!({"type": "object"}))),
        Some("json_schema") => {
            let schema = &rf["json_schema"]["schema"];
            if !schema.is_object() {
                return Err(UComputeError::invalid(
                    "response_format.json_schema.schema must be a JSON schema object",
                ));
            }
            // F-01-12 bounds, shared with what a peer may send (`ucompute_runtime::schema`).
            ucompute_runtime::schema::check_schema(schema)?;
            Ok(Some(schema.clone()))
        }
        Some(other) => Err(UComputeError::invalid(format!(
            "unsupported response_format type {other:?}"
        ))),
    }
}

#[derive(Deserialize)]
pub struct InMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
}

impl InMessage {
    /// OpenAI content is a string or an array of parts; only text parts are supported.
    fn into_message(self) -> Result<ChatMessage> {
        let content = match self.content {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(s)) => s,
            Some(Value::Array(parts)) => {
                let mut s = String::new();
                for p in parts {
                    match p["type"].as_str() {
                        Some("text") => s.push_str(p["text"].as_str().unwrap_or("")),
                        other => {
                            return Err(UComputeError::invalid(format!(
                                "unsupported content part type {:?}; only text is supported",
                                other.unwrap_or("")
                            )))
                        }
                    }
                }
                s
            }
            Some(_) => {
                return Err(UComputeError::invalid(
                    "message content must be a string or an array of text parts",
                ))
            }
        };
        if !matches!(
            self.role.as_str(),
            "system" | "user" | "assistant" | "tool" | "developer"
        ) {
            return Err(UComputeError::invalid(format!("unknown message role {:?}", self.role)));
        }
        Ok(ChatMessage {
            role: self.role,
            content,
        })
    }
}

#[derive(Deserialize)]
pub struct ChatRequest {
    pub model: Option<String>,
    #[serde(default)]
    pub messages: Vec<InMessage>,
    #[serde(flatten)]
    pub sampling: Sampling,
    pub tools: Option<Value>,
    /// Accepted for compatibility; the runtime has no field for it yet, so it is not forwarded.
    #[allow(dead_code)]
    pub tool_choice: Option<Value>,
    /// UCompute extension: `"local"`, `"peer:<id>"` or `"split:<id>"` (see the module docs).
    pub target: Option<String>,
    #[serde(flatten)]
    pub split: crate::split::SplitParams,
}

#[derive(Deserialize)]
pub struct CompletionRequest {
    pub model: Option<String>,
    pub prompt: Option<Value>,
    #[serde(flatten)]
    pub sampling: Sampling,
    pub target: Option<String>,
    #[serde(flatten)]
    pub split: crate::split::SplitParams,
}

#[derive(Deserialize)]
pub struct EmbeddingsRequest {
    pub model: Option<String>,
    pub input: Option<OneOrMany>,
    pub encoding_format: Option<String>,
}

fn require_model(model: &Option<String>) -> Result<String> {
    match model {
        Some(m) if !m.is_empty() => Ok(m.clone()),
        _ => Err(UComputeError::invalid("\"model\" is required (a catalog or store id)")),
    }
}

/// Where a request runs: this device, one paired peer, or split between the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Local,
    Peer(PeerId),
    Split(PeerId),
}

impl Target {
    /// The value echoed as `"target"` in every response.
    fn label(&self) -> String {
        match self {
            Target::Local => "local".into(),
            Target::Peer(id) => format!("peer:{}", id.to_hex()),
            Target::Split(id) => format!("split:{}", id.to_hex()),
        }
    }
}

/// Body field first, then the `X-UCompute-Target` header; absent means local. A peer must be named
/// by its 64-hex id or the short label of a pinned peer (`PeerUnauthorized` for an unknown label).
fn parse_target(st: &AppState, body: Option<&str>, headers: &HeaderMap) -> Result<Target> {
    // D-029/D-030: a client naming its target in any other `X-*-Target` header (an outdated or foreign one) is
    // refused cleanly instead of having its request quietly run here.
    if let Some(h) = headers.keys().map(|k| k.as_str()).find(|k| {
        *k != "x-ucompute-target" && k.starts_with("x-") && k.ends_with("-target") && k.len() > "x--target".len()
    }) {
        return Err(UComputeError::invalid(format!(
            "header {h} is not read; UCompute reads X-UCompute-Target"
        )));
    }
    let s = body.or_else(|| headers.get("x-ucompute-target").and_then(|v| v.to_str().ok()));
    match s.map(str::trim) {
        None | Some("") | Some("local") => Ok(Target::Local),
        Some(s) => match (s.strip_prefix("peer:"), s.strip_prefix("split:")) {
            (Some(id), _) => Ok(Target::Peer(st.resolve_peer(id)?)),
            (_, Some(id)) => Ok(Target::Split(st.resolve_peer(id)?)),
            _ => Err(UComputeError::invalid(
                "target must be \"local\", \"peer:<peer id or short label>\" or \"split:<peer id or short label>\"",
            )),
        },
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Wait for a generation slot or turn the request away (429 queue full, 503 wait timed out).
async fn admit(st: &AppState) -> std::result::Result<OwnedSemaphorePermit, ApiError> {
    st.admission.acquire().await.map_err(|r| {
        st.metrics.rejected.with_label_values(&[r.reason()]).inc();
        let (status, msg) = match r {
            Rejected::QueueFull => (
                StatusCode::TOO_MANY_REQUESTS,
                format!("request queue is full ({} waiting)", st.admission.max_queue),
            ),
            Rejected::Timeout => (
                StatusCode::SERVICE_UNAVAILABLE,
                format!(
                    "no generation slot became free within {} ms",
                    st.admission.timeout.as_millis()
                ),
            ),
        };
        ApiError {
            error: UComputeError::new(ErrorCode::Overloaded, msg),
            status,
        }
    })
}

/// `role: content` lines for models without a chat template (see crate docs).
pub fn plain_prompt(messages: &[ChatMessage]) -> String {
    let mut s = String::new();
    for m in messages {
        s.push_str(&m.role);
        s.push_str(": ");
        s.push_str(&m.content);
        s.push('\n');
    }
    s.push_str("assistant:");
    s
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Chat,
    Text,
}

/// Formats one generation as OpenAI JSON: `chat.completion[.chunk]` or `text_completion`.
#[derive(Clone)]
struct Format {
    kind: Kind,
    id: String,
    model: String,
    created: u64,
    include_usage: bool,
    /// `"local"`, `"peer:<hex>"` or `"split:<hex>"`, in every envelope.
    target: String,
    /// A split's placement as its engine log proved it, in every envelope (WP-04).
    placement: Option<Value>,
}

fn usage_json(u: &Usage) -> Value {
    json!({"prompt_tokens": u.prompt_tokens, "completion_tokens": u.completion_tokens, "total_tokens": u.prompt_tokens + u.completion_tokens})
}

impl Format {
    fn new(kind: Kind, model: &str, include_usage: bool, target: &Target) -> Self {
        let prefix = match kind {
            Kind::Chat => "chatcmpl",
            Kind::Text => "cmpl",
        };
        Self {
            kind,
            id: format!("{prefix}-{}", uuid::Uuid::new_v4()),
            model: model.to_string(),
            created: now_secs(),
            include_usage,
            target: target.label(),
            placement: None,
        }
    }

    fn envelope(&self, choices: Value) -> Value {
        let object = match self.kind {
            Kind::Chat => "chat.completion.chunk",
            Kind::Text => "text_completion",
        };
        let mut v = json!({"id": self.id, "object": object, "created": self.created, "model": self.model, "choices": choices, "target": self.target});
        if let Some(p) = &self.placement {
            v["placement"] = p.clone();
        }
        v
    }

    fn chunk(&self, delta: Value, finish: Option<&str>) -> Value {
        let choice = match self.kind {
            Kind::Chat => json!({"index": 0, "delta": delta, "finish_reason": finish}),
            Kind::Text => {
                json!({"index": 0, "text": delta["content"].as_str().unwrap_or(""), "logprobs": Value::Null, "finish_reason": finish})
            }
        };
        self.envelope(json!([choice]))
    }

    /// Stream events for one runtime event (several when a `Done` carries usage).
    fn stream_events(&self, ev: Result<GenerateEvent>) -> Vec<Value> {
        match ev {
            Ok(GenerateEvent::Token { text, .. }) => vec![self.chunk(json!({"content": text}), None)],
            Ok(GenerateEvent::ToolCallDelta { delta }) => vec![self.chunk(delta, None)],
            Ok(GenerateEvent::Done {
                finish_reason,
                usage,
                timings,
            }) => {
                let mut out = vec![self.chunk(json!({}), Some(&finish_reason))];
                if self.include_usage {
                    let mut u = self.envelope(json!([]));
                    u["usage"] = usage_json(&usage);
                    u["timings"] = json!(timings);
                    out.push(u);
                }
                out
            }
            Err(e) => vec![e.to_openai_json()],
        }
    }

    fn final_json(&self, c: &Collected) -> Value {
        let mut v = match self.kind {
            Kind::Chat => {
                let mut message = json!({"role": "assistant", "content": c.text});
                if !c.tool_calls.is_empty() {
                    message["tool_calls"] = json!(c.tool_calls);
                    if c.text.is_empty() {
                        message["content"] = Value::Null;
                    }
                }
                self.envelope(
                    json!([{"index": 0, "message": message, "logprobs": Value::Null, "finish_reason": c.finish}]),
                )
            }
            Kind::Text => {
                self.envelope(json!([{"index": 0, "text": c.text, "logprobs": Value::Null, "finish_reason": c.finish}]))
            }
        };
        if self.kind == Kind::Chat {
            v["object"] = json!("chat.completion");
        }
        v["usage"] = usage_json(&c.usage);
        v["timings"] = json!(c.timings);
        v
    }
}

#[derive(Default)]
struct Collected {
    text: String,
    tool_calls: Vec<Value>,
    finish: String,
    usage: Usage,
    timings: Timings,
}

/// Drain a generation; tool-call deltas are merged by `index` into whole OpenAI tool calls.
async fn collect(mut stream: GenerateStream) -> Result<Collected> {
    let mut c = Collected::default();
    while let Some(ev) = stream.next().await {
        match ev? {
            GenerateEvent::Token { text, .. } => c.text.push_str(&text),
            GenerateEvent::ToolCallDelta { delta } => merge_tool_calls(&mut c.tool_calls, &delta),
            GenerateEvent::Done {
                finish_reason,
                usage,
                timings,
            } => {
                c.finish = finish_reason;
                c.usage = usage;
                c.timings = timings;
            }
        }
    }
    Ok(c)
}

fn merge_tool_calls(calls: &mut Vec<Value>, delta: &Value) {
    for d in delta["tool_calls"].as_array().into_iter().flatten() {
        let idx = d["index"].as_u64().map_or(calls.len(), |i| i as usize);
        while calls.len() <= idx {
            calls.push(json!({"id": "", "type": "function", "function": {"name": "", "arguments": ""}}));
        }
        let call = &mut calls[idx];
        for key in ["id", "type"] {
            if let Some(s) = d[key].as_str() {
                call[key] = json!(s);
            }
        }
        if let Some(name) = d["function"]["name"].as_str() {
            call["function"]["name"] = json!(name);
        }
        if let Some(args) = d["function"]["arguments"].as_str() {
            let joined = format!("{}{}", call["function"]["arguments"].as_str().unwrap_or(""), args);
            call["function"]["arguments"] = json!(joined);
        }
    }
}

/// Body stream that owns the admission permit (local runs only; a remote run takes no engine
/// slot here) and cancels the generation when dropped (client disconnect) or finished.
struct Guard {
    inner: BoxStream<'static, std::result::Result<Event, Infallible>>,
    cancel: CancellationToken,
    _permit: Option<OwnedSemaphorePermit>,
}

impl Stream for Guard {
    type Item = std::result::Result<Event, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

fn sse_response(
    st: Arc<AppState>,
    fmt: Format,
    gen: GenerateStream,
    cancel: CancellationToken,
    permit: Option<OwnedSemaphorePermit>,
) -> Response {
    // Only a local run holds a permit; tokens a peer decoded are not this device's generation
    // metrics (`ucompute_remote_requests_total` counts those requests).
    let local = permit.is_some();
    let chunks = gen
        .flat_map(move |ev| {
            if let (true, Ok(GenerateEvent::Done { usage, timings, .. })) = (local, &ev) {
                st.metrics.record_done(usage, timings);
            }
            stream::iter(fmt.stream_events(ev))
        })
        .map(|v| Event::default().data(v.to_string()))
        .chain(stream::once(async { Event::default().data("[DONE]") }))
        .map(Ok::<_, Infallible>)
        .boxed();
    Sse::new(Guard {
        inner: chunks,
        cancel,
        _permit: permit,
    })
    .into_response()
}

/// What the caller gave us; chat is resolved to Raw or Chat only once the model is known.
enum Input {
    Raw(String),
    Chat(Vec<ChatMessage>),
}

/// A generation under way: the stream, its cancel token, and the admission permit of a local run.
type Started = (GenerateStream, CancellationToken, Option<OwnedSemaphorePermit>);

#[allow(clippy::too_many_arguments)]
async fn run(
    st: Arc<AppState>,
    mut fmt: Format,
    model_id: &str,
    input: Input,
    sampling: &Sampling,
    tools: Option<Value>,
    target: Target,
    split: &crate::split::SplitParams,
    client_ref: Option<String>,
) -> std::result::Result<Response, ApiError> {
    // I-05: the routing decision comes first, before admission, any load or any network activity.
    // Local is always allowed; a peer needs trusted_group privacy, networking on and a pinned,
    // non-revoked peer in a group allowing remote inference (typed denial with the plain reason);
    // a split needs a group allowing pooled inference, which remote inference does not grant.
    let privacy = st.settings().privacy;
    let (exec, workload) = match target {
        Target::Local => (ExecutionTarget::Local, WorkloadKind::RemoteInference),
        Target::Peer(id) => (ExecutionTarget::Peer(id), WorkloadKind::RemoteInference),
        Target::Split(id) => (ExecutionTarget::Peer(id), WorkloadKind::PooledInference),
    };
    st.policy().evaluate(privacy, exec, workload).into_result()?;
    // Untrusted schema is checked before admission: a grammar bomb never takes a slot or a load.
    json_schema_of(sampling.response_format.as_ref())?;
    // Listed from here on (E3), so a request waiting for a slot or a load shows with its client_ref.
    let kind = match fmt.kind {
        Kind::Chat => "chat",
        Kind::Text => "completion",
    };
    let job = st.jobs.start(
        fmt.id.clone(),
        kind,
        model_id,
        target.label(),
        "local".into(),
        client_ref,
    );
    let started = match target {
        Target::Peer(id) => start_remote(&st, model_id, input, sampling, tools, id, privacy, &job).await,
        Target::Local => start_local(&st, model_id, input, sampling, tools, &job).await,
        Target::Split(id) => start_split(&st, model_id, input, sampling, tools, id, split, &job, &mut fmt).await,
    };
    let (gen, cancel, permit) = started.inspect_err(|e| job.fail(e.error.code.as_str()))?;
    let gen = crate::jobs::track(gen, job);
    if sampling.stream {
        return Ok(sse_response(st, fmt, gen, cancel, permit));
    }
    let collected = collect(gen).await?;
    if permit.is_some() {
        st.metrics.record_done(&collected.usage, &collected.timings);
    }
    drop(permit);
    Ok(axum::Json(fmt.final_json(&collected)).into_response())
}

/// Admission, the model, then the generation on this device.
async fn start_local(
    st: &Arc<AppState>,
    model_id: &str,
    input: Input,
    sampling: &Sampling,
    tools: Option<Value>,
    job: &JobHandle,
) -> std::result::Result<Started, ApiError> {
    let permit = admit(st).await?;
    job.started();
    let model = st.manager.ensure_loaded(model_id, st.load_options()).await?;
    job.loaded(&model.info().device_actual);
    let req = sampling.to_generate(local_input(st, model_id, input)?, tools)?;
    // D-026: local work on a CPU engine holds peers off (and, under `preempt`, cancels them) until
    // this stream is dropped, then for `local_priority::WINDOW`.
    let local = ucompute_network::local_priority::on_cpu(model.info()).then(|| st.local_work.local_started());
    // Child of the server's shutdown token: Ctrl-C ends this generation too (see `serve_on`).
    let cancel = st.shutdown.child_token();
    let gen = model.generate(req, cancel.clone()).await?;
    let gen = match local {
        Some(guard) => gen
            .map(move |ev| {
                let _ = &guard;
                ev
            })
            .boxed(),
        None => gen,
    };
    Ok((gen, cancel, Some(permit)))
}

/// Chat with the model's template when its file has one, else a plain prompt.
fn local_input(st: &AppState, model_id: &str, input: Input) -> Result<PromptInput> {
    Ok(match input {
        Input::Raw(p) => PromptInput::Raw(p),
        Input::Chat(msgs) => {
            let (manifest, blob) = st.store.resolve(model_id)?;
            if st.metadata(&manifest.sha256, &blob)?.has_chat_template {
                PromptInput::Chat(msgs)
            } else {
                PromptInput::Raw(plain_prompt(&msgs))
            }
        }
    })
}

/// WP-04 pooled inference: admission (the split engine is this device's), the split engine with
/// `peer` ([`crate::split::ensure`]), then the generation. When the session ends mid-request its
/// split engine is stopped and the generation fails with the session's reason (`peer_lost`,
/// `policy_denied`, ...), never a local rerun (ADR-0005 D7). `fmt` gets the placement the engine
/// log proved.
#[allow(clippy::too_many_arguments)]
async fn start_split(
    st: &Arc<AppState>,
    model_id: &str,
    input: Input,
    sampling: &Sampling,
    tools: Option<Value>,
    peer: PeerId,
    split: &crate::split::SplitParams,
    job: &JobHandle,
    fmt: &mut Format,
) -> std::result::Result<Started, ApiError> {
    // shape and consent before a slot is taken
    if split.split_at.is_none() || !split.acknowledged {
        crate::split::ensure(st, model_id, peer, split).await?;
    }
    let permit = admit(st).await?;
    job.started();
    // `users` is this request's hold on the split, kept until the stream below ends
    let (model, session, users) = crate::split::ensure(st, model_id, peer, split).await?;
    let info = model.info();
    job.loaded(&info.device_actual);
    fmt.placement = Some(json!({"devices": info.device_actual, "stages": info.split, "session": session.session_id()}));
    let req = sampling.to_generate(local_input(st, model_id, input)?, tools)?;
    // The session's end stops the split engine (`split::ensure`'s watcher), which ends this stream.
    let cancel = st.shutdown.child_token();
    let gen = match model.generate(req, cancel.clone()).await {
        Ok(g) => g,
        Err(e) => return Err(session.failure().unwrap_or(e).into()),
    };
    // An error while the session is ending (the engine's RPC connections reset a little before the
    // session's reason is known) is the session's reason.
    let gen = gen
        .then(move |ev| {
            let _ = &users;
            let session = session.clone();
            async move {
                match ev {
                    Err(e) => Err(match tokio::time::timeout(SESSION_END_GRACE, session.ended()).await {
                        Ok(reason) => reason,
                        Err(_) => e,
                    }),
                    ok => ok,
                }
            }
        })
        .boxed();
    Ok((gen, cancel, Some(permit)))
}

/// How long a split generation's error waits for its session's reason (ADR-0005 D7).
const SESSION_END_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Mode B: the whole request on `peer` over the authenticated transport, streamed back exactly
/// like a local run. Called only after `evaluate` allowed it; `generate_remote` re-checks the
/// privacy string before opening a stream. The prompt travels inside TLS only and is never logged.
#[allow(clippy::too_many_arguments)]
async fn start_remote(
    st: &Arc<AppState>,
    model_id: &str,
    input: Input,
    sampling: &Sampling,
    tools: Option<Value>,
    peer: PeerId,
    privacy: PrivacyMode,
    job: &JobHandle,
) -> std::result::Result<Started, ApiError> {
    if !ucompute_core::paths::safe_name(model_id) {
        return Err(UComputeError::invalid("model id may only contain letters, digits, '-', '_' and '.'").into());
    }
    let node = st.node().await?;
    let sha = st.model_sha256(model_id)?;
    let input = match input {
        Input::Raw(p) => PromptInput::Raw(p),
        // The chat-template decision needs the GGUF header; without the file here the peer's engine
        // applies the template (ponytail: ask the peer's Capabilities for it if that ever misroutes).
        Input::Chat(msgs) => match st.store.resolve(model_id) {
            Ok((m, blob)) if !st.metadata(&m.sha256, &blob)?.has_chat_template => PromptInput::Raw(plain_prompt(&msgs)),
            _ => PromptInput::Chat(msgs),
        },
    };
    let req = sampling.to_generate(input, tools)?;
    let conn = st.peer_connection(&node, peer).await?;
    let cancel = st.shutdown.child_token();
    // The serde string of the mode that was just evaluated ("trusted_group" is the only one that passes).
    let privacy = serde_json::to_value(privacy)
        .map_err(UComputeError::from)?
        .as_str()
        .unwrap_or("local_only")
        .to_string();
    // Audited here before the request frame leaves, and again with the outcome (F-06-10).
    let (request_id, gen) = crate::mesh::generate_remote_audited_with_id(
        st.audit.clone(),
        conn,
        model_id,
        &sha,
        req,
        &privacy,
        cancel.clone(),
    )
    .await?;
    job.sent(&request_id);
    st.metrics
        .remote_requests
        .with_label_values(&[&peer.short_label()])
        .inc();
    Ok((gen, cancel, None))
}

pub async fn chat(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> std::result::Result<Response, ApiError> {
    let model_id = require_model(&req.model)?;
    let target = parse_target(&st, req.target.as_deref(), &headers)?;
    if req.messages.is_empty() {
        return Err(UComputeError::invalid("messages must not be empty").into());
    }
    let messages = req
        .messages
        .into_iter()
        .map(InMessage::into_message)
        .collect::<Result<Vec<_>>>()?;
    let fmt = Format::new(Kind::Chat, &model_id, req.sampling.include_usage(), &target);
    run(
        st,
        fmt,
        &model_id,
        Input::Chat(messages),
        &req.sampling,
        req.tools,
        target,
        &req.split,
        crate::jobs::client_ref(&headers),
    )
    .await
}

pub async fn completions(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CompletionRequest>,
) -> std::result::Result<Response, ApiError> {
    let model_id = require_model(&req.model)?;
    let target = parse_target(&st, req.target.as_deref(), &headers)?;
    let prompt = match req.prompt {
        Some(Value::String(s)) => s,
        Some(Value::Array(a)) if a.len() == 1 && a[0].is_string() => a[0].as_str().unwrap_or("").to_string(),
        _ => return Err(UComputeError::invalid("prompt must be a string or a single-element array of strings").into()),
    };
    let fmt = Format::new(Kind::Text, &model_id, req.sampling.include_usage(), &target);
    let client_ref = crate::jobs::client_ref(&headers);
    run(
        st,
        fmt,
        &model_id,
        Input::Raw(prompt),
        &req.sampling,
        None,
        target,
        &req.split,
        client_ref,
    )
    .await
}

pub async fn embeddings(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingsRequest>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    let model_id = require_model(&req.model)?;
    // Embeddings run here only; asking for a peer is refused rather than quietly run locally.
    if parse_target(&st, None, &headers)? != Target::Local {
        return Err(UComputeError::invalid("embeddings run on this device only; remove the target").into());
    }
    let inputs = req.input.map(OneOrMany::into_vec).unwrap_or_default();
    if inputs.is_empty() || inputs.iter().any(String::is_empty) {
        return Err(UComputeError::invalid("input must be a non-empty string or array of non-empty strings").into());
    }
    let base64 = match req.encoding_format.as_deref() {
        None | Some("float") => false,
        Some("base64") => true,
        Some(other) => return Err(UComputeError::invalid(format!("unsupported encoding_format {other:?}")).into()),
    };
    let id = format!("embd-{}", uuid::Uuid::new_v4());
    let job = st
        .jobs
        .start(id, "embeddings", &model_id, "local".into(), "local".into(), None);
    let r = embed(&st, &model_id, &inputs, base64, &job).await;
    match &r {
        Ok(v) => job.done(Finished {
            prompt_tokens: v["usage"]["prompt_tokens"].as_u64().map(|n| n as u32),
            ..Default::default()
        }),
        Err(e) => job.fail(e.error.code.as_str()),
    }
    r
}

async fn embed(
    st: &Arc<AppState>,
    model_id: &str,
    inputs: &[String],
    base64: bool,
    job: &JobHandle,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    let _permit = admit(st).await?;
    job.started();
    let model = st
        .manager
        .ensure_loaded(
            model_id,
            LoadOptions {
                embeddings: true,
                ..st.load_options()
            },
        )
        .await?;
    job.loaded(&model.info().device_actual);
    let vectors = model.embed(inputs).await?;
    let mut prompt_tokens = 0u32;
    for s in inputs {
        prompt_tokens += model.tokenize(s).await?.len() as u32;
    }
    st.metrics.prompt_tokens.inc_by(u64::from(prompt_tokens));
    // ponytail: f32 values widen to f64 inside serde_json::Value ("0.10000000149011612"); exact as f32,
    // ~2x the bytes. Serialize a typed struct instead if embedding payload size ever matters.
    let data: Vec<Value> = vectors
        .iter()
        .enumerate()
        .map(|(i, v)| json!({"object": "embedding", "index": i, "embedding": if base64 { json!(base64_f32(v)) } else { json!(v) }}))
        .collect();
    Ok(axum::Json(
        json!({"object": "list", "data": data, "model": model_id, "usage": {"prompt_tokens": prompt_tokens, "total_tokens": prompt_tokens}}),
    ))
}

/// Little-endian f32 bytes as standard base64 (the OpenAI SDKs' default `encoding_format`).
fn base64_f32(v: &[f32]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = v.iter().flat_map(|f| f.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn model_object(id: &str, downloaded: bool, license: &str, bytes: u64, redistribution: &str, loaded: bool) -> Value {
    json!({
        "id": id,
        "object": "model",
        "created": 0,
        "owned_by": "ucompute",
        "ucompute": {"downloaded": downloaded, "license": license, "bytes": bytes, "redistribution": redistribution, "loaded": loaded},
    })
}

/// Every store manifest plus catalog entries not downloaded yet, in the OpenAI list shape with a
/// `ucompute` extension object.
pub async fn list_models(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    let loaded = st.manager.snapshot().loaded.map(|v| v.info.model_id);
    let manifests = st.store.list()?;
    let mut data: Vec<Value> = manifests
        .iter()
        .map(|m| {
            model_object(
                &m.id,
                true,
                &m.license,
                m.bytes,
                &m.redistribution,
                loaded.as_deref() == Some(m.id.as_str()),
            )
        })
        .collect();
    data.extend(
        st.catalog
            .models
            .iter()
            .filter(|e| !manifests.iter().any(|m| m.id == e.id))
            .map(|e| {
                model_object(
                    &e.id,
                    false,
                    &e.license,
                    e.bytes,
                    &e.redistribution,
                    loaded.as_deref() == Some(e.id.as_str()),
                )
            }),
    );
    Ok(axum::Json(json!({"object": "list", "data": data})))
}

pub async fn get_model(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    if !ucompute_core::paths::safe_name(&id) {
        return Err(UComputeError::invalid("model id may only contain letters, digits, '-', '_' and '.'").into());
    }
    let loaded = st.manager.snapshot().loaded.is_some_and(|v| v.info.model_id == id);
    if let Some(m) = st.store.get(&id)? {
        return Ok(axum::Json(model_object(
            &m.id,
            true,
            &m.license,
            m.bytes,
            &m.redistribution,
            loaded,
        )));
    }
    match st.catalog.get(&id) {
        Some(e) => Ok(axum::Json(model_object(
            &e.id,
            false,
            &e.license,
            e.bytes,
            &e.redistribution,
            loaded,
        ))),
        None => Err(UComputeError::new(
            ErrorCode::ModelNotFound,
            format!("no model named {id} in the catalog or the local store"),
        )
        .with_hint("GET /v1/models")
        .into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_defaults_and_bounds() {
        let s: Sampling = serde_json::from_str("{}").unwrap();
        let g = s.to_generate(PromptInput::Raw("x".into()), None).unwrap();
        assert_eq!(
            (g.max_tokens, g.temperature, g.top_p, g.seed),
            (DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE, DEFAULT_TOP_P, None)
        );
        let s: Sampling = serde_json::from_str(
            r#"{"max_completion_tokens": 7, "max_tokens": 99, "seed": 42, "stop": "END", "temperature": 0}"#,
        )
        .unwrap();
        let g = s.to_generate(PromptInput::Raw("x".into()), None).unwrap();
        assert_eq!(
            (g.max_tokens, g.seed, g.stop.clone(), g.temperature),
            (7, Some(42), vec!["END".to_string()], 0.0)
        );
        let s: Sampling = serde_json::from_str(r#"{"stop": ["a", "b"], "seed": -1}"#).unwrap();
        let g = s.to_generate(PromptInput::Raw("x".into()), None).unwrap();
        assert_eq!((g.stop.len(), g.seed), (2, None));
        for bad in [
            r#"{"n": 2}"#,
            r#"{"max_tokens": 0}"#,
            r#"{"temperature": 3}"#,
            r#"{"top_p": 1.5}"#,
        ] {
            let s: Sampling = serde_json::from_str(bad).unwrap();
            assert_eq!(
                s.to_generate(PromptInput::Raw("x".into()), None).unwrap_err().code,
                ErrorCode::InvalidRequest,
                "{bad}"
            );
        }
    }

    #[test]
    fn response_format_to_schema() {
        assert_eq!(json_schema_of(None).unwrap(), None);
        assert_eq!(json_schema_of(Some(&json!({"type": "text"}))).unwrap(), None);
        assert_eq!(
            json_schema_of(Some(&json!({"type": "json_object"}))).unwrap(),
            Some(json!({"type": "object"}))
        );
        let schema = json!({"type": "object", "properties": {"a": {"type": "integer"}}, "required": ["a"]});
        assert_eq!(
            json_schema_of(Some(
                &json!({"type": "json_schema", "json_schema": {"name": "x", "schema": schema}})
            ))
            .unwrap(),
            Some(schema)
        );
        assert_eq!(
            json_schema_of(Some(&json!({"type": "json_schema"}))).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        assert_eq!(
            json_schema_of(Some(&json!({"type": "xml"}))).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }

    /// F-01-12: grammar bombs are refused before the engine sees them, each in well under a second.
    #[test]
    fn schema_bombs_are_rejected_quickly() {
        let wrap = |schema: Value| json!({"type": "json_schema", "json_schema": {"name": "x", "schema": schema}});
        let mut deep = json!({"type": "string"});
        for _ in 0..40 {
            deep = json!({"type": "object", "properties": {"a": deep}});
        }
        let huge =
            json!({"type": "object", "properties": {"a": {"type": "string", "enum": vec!["x".repeat(1000); 80]}}});
        let bombs = [
            (deep, "nests deeper"),
            (json!({"type": "string", "pattern": "^(a{1,100000})$"}), "repetition"),
            (json!({"type": "string", "pattern": "a{2000}"}), "repetition"),
            (json!({"type": "string", "pattern": "x".repeat(2000)}), "pattern longer"),
            (
                json!({"type": "array", "items": {"type": "string"}, "minItems": 1000000}),
                "minItems",
            ),
            (json!({"type": "string", "maxLength": -1}), "maxLength"),
            (json!({"$ref": "https://evil.example/schema.json"}), "local $ref"),
            (
                json!({"type": "object", "properties": {"a": {"$ref": "file:///etc/passwd"}}}),
                "local $ref",
            ),
            (huge, "exceeds"),
        ];
        for (schema, why) in bombs {
            let t = std::time::Instant::now();
            let e = json_schema_of(Some(&wrap(schema))).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest);
            assert!(e.message.contains(why), "{}", e.message);
            assert!(t.elapsed() < std::time::Duration::from_secs(1));
        }
        // Reasonable schemas pass: local refs, bounded repetition, nested but shallow.
        let ok = json!({"type": "object", "$defs": {"s": {"type": "string", "pattern": "^[a-z]{1,64}$", "maxLength": 64}}, "properties": {"a": {"$ref": "#/$defs/s"}, "b": {"type": "array", "items": {"type": "integer"}, "minItems": 0, "maxItems": 10}}, "required": ["a"]});
        assert_eq!(json_schema_of(Some(&wrap(ok.clone()))).unwrap(), Some(ok));
    }

    #[test]
    fn message_content_forms() {
        let m: InMessage = serde_json::from_str(r#"{"role": "user", "content": "hi"}"#).unwrap();
        assert_eq!(m.into_message().unwrap().content, "hi");
        let m: InMessage = serde_json::from_str(
            r#"{"role": "user", "content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}"#,
        )
        .unwrap();
        assert_eq!(m.into_message().unwrap().content, "ab");
        let m: InMessage = serde_json::from_str(r#"{"role": "assistant", "content": null}"#).unwrap();
        assert_eq!(m.into_message().unwrap().content, "");
        let m: InMessage =
            serde_json::from_str(r#"{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "x"}}]}"#)
                .unwrap();
        assert_eq!(m.into_message().unwrap_err().code, ErrorCode::InvalidRequest);
        let m: InMessage = serde_json::from_str(r#"{"role": "robot", "content": "x"}"#).unwrap();
        assert_eq!(m.into_message().unwrap_err().code, ErrorCode::InvalidRequest);
    }

    #[test]
    fn plain_prompt_lines() {
        let msgs = vec![
            ChatMessage {
                role: "system".into(),
                content: "Be brief.".into(),
            },
            ChatMessage {
                role: "user".into(),
                content: "Hi".into(),
            },
        ];
        assert_eq!(plain_prompt(&msgs), "system: Be brief.\nuser: Hi\nassistant:");
    }

    #[test]
    fn chat_request_parses_openai_shape() {
        let r: ChatRequest = serde_json::from_str(r#"{"model": "m", "messages": [{"role": "user", "content": "hi"}], "stream": true, "stream_options": {"include_usage": true}, "tools": [], "tool_choice": "auto", "unknown_field": 1}"#).unwrap();
        assert_eq!(r.model.as_deref(), Some("m"));
        assert!(r.sampling.stream && r.sampling.include_usage());
        assert!(r.tools.is_some());
        assert_eq!(require_model(&None).unwrap_err().code, ErrorCode::InvalidRequest);
        assert_eq!(
            require_model(&Some(String::new())).unwrap_err().code,
            ErrorCode::InvalidRequest
        );
    }

    #[test]
    fn stream_chunks_and_final_shapes() {
        let f = Format {
            kind: Kind::Chat,
            id: "chatcmpl-1".into(),
            model: "m".into(),
            created: 5,
            include_usage: true,
            target: "local".into(),
            placement: None,
        };
        let evs = f.stream_events(Ok(GenerateEvent::Token {
            text: "Hi".into(),
            token_ids: vec![],
        }));
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0]["object"], "chat.completion.chunk");
        assert_eq!(evs[0]["target"], "local");
        assert_eq!(evs[0]["choices"][0]["delta"]["content"], "Hi");
        assert!(evs[0]["choices"][0]["finish_reason"].is_null());
        let done = GenerateEvent::Done {
            finish_reason: "length".into(),
            usage: Usage {
                prompt_tokens: 3,
                completion_tokens: 4,
            },
            timings: Timings::default(),
        };
        let evs = f.stream_events(Ok(done.clone()));
        assert_eq!(evs.len(), 2, "finish chunk + usage chunk");
        assert_eq!(evs[0]["choices"][0]["finish_reason"], "length");
        assert_eq!(evs[1]["choices"], json!([]));
        assert_eq!(evs[1]["usage"]["total_tokens"], 7);
        let no_usage = Format {
            include_usage: false,
            ..f.clone()
        };
        assert_eq!(no_usage.stream_events(Ok(done)).len(), 1);
        let err = f.stream_events(Err(UComputeError::new(ErrorCode::Timeout, "slow")));
        assert_eq!(err[0]["error"]["code"], "timeout");

        let text = Format {
            kind: Kind::Text,
            ..f.clone()
        };
        let evs = text.stream_events(Ok(GenerateEvent::Token {
            text: "Hi".into(),
            token_ids: vec![],
        }));
        assert_eq!(evs[0]["object"], "text_completion");
        assert_eq!(evs[0]["choices"][0]["text"], "Hi");

        let c = Collected {
            text: "Hello".into(),
            finish: "stop".into(),
            usage: Usage {
                prompt_tokens: 1,
                completion_tokens: 2,
            },
            ..Default::default()
        };
        let v = f.final_json(&c);
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["choices"][0]["message"]["content"], "Hello");
        assert_eq!(v["choices"][0]["message"]["role"], "assistant");
        assert_eq!(v["usage"]["total_tokens"], 3);
        assert!(v["timings"].is_object());
        let v = text.final_json(&c);
        assert_eq!(v["object"], "text_completion");
        assert_eq!(v["choices"][0]["text"], "Hello");
    }

    #[test]
    fn tool_call_deltas_merge_by_index() {
        let mut calls = vec![];
        merge_tool_calls(
            &mut calls,
            &json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "get_weather", "arguments": "{\"ci"}}]}),
        );
        merge_tool_calls(
            &mut calls,
            &json!({"tool_calls": [{"index": 0, "function": {"arguments": "ty\": \"Paris\"}"}}]}),
        );
        merge_tool_calls(
            &mut calls,
            &json!({"tool_calls": [{"index": 1, "id": "call_2", "function": {"name": "other", "arguments": "{}"}}]}),
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["id"], "call_1");
        assert_eq!(calls[0]["function"]["name"], "get_weather");
        assert_eq!(calls[0]["function"]["arguments"], "{\"city\": \"Paris\"}");
        assert_eq!(calls[1]["function"]["name"], "other");
        let c = Collected {
            tool_calls: calls,
            finish: "tool_calls".into(),
            ..Default::default()
        };
        let v = Format {
            kind: Kind::Chat,
            id: "x".into(),
            model: "m".into(),
            created: 0,
            include_usage: false,
            target: "local".into(),
            placement: None,
        }
        .final_json(&c);
        assert!(v["choices"][0]["message"]["content"].is_null());
        assert_eq!(v["choices"][0]["message"]["tool_calls"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn base64_matches_known_vectors() {
        // 1.0f32 LE = 00 00 80 3f -> "AACAPw=="; two floats -> 8 bytes -> 12 chars.
        assert_eq!(base64_f32(&[1.0]), "AACAPw==");
        assert_eq!(base64_f32(&[1.0, -2.0]), "AACAPwAAAMA=");
        assert_eq!(base64_f32(&[0.0, 0.0, 0.0]), "AAAAAAAAAAAAAAAA");
        assert_eq!(base64_f32(&[]), "");
    }

    #[test]
    fn target_labels_and_remote_envelope() {
        assert_eq!(Target::Local.label(), "local");
        let id = PeerId([0xab; 32]);
        assert_eq!(Target::Peer(id).label(), format!("peer:{}", "ab".repeat(32)));
        let f = Format::new(Kind::Text, "m", false, &Target::Peer(id));
        let v = f.final_json(&Collected::default());
        assert_eq!(v["target"], format!("peer:{}", "ab".repeat(32)));
        assert_eq!(f.envelope(json!([]))["target"], v["target"]);
        assert!(v.get("placement").is_none(), "only a split carries a placement");
        let mut f = Format::new(Kind::Text, "m", false, &Target::Split(id));
        f.placement = Some(json!({"devices": "RPC0 (127.0.0.1:1), MTL0"}));
        let v = f.final_json(&Collected::default());
        assert_eq!(v["target"], format!("split:{}", "ab".repeat(32)));
        assert_eq!(v["placement"]["devices"], "RPC0 (127.0.0.1:1), MTL0");
        assert_eq!(f.chunk(json!({}), None)["placement"], v["placement"], "every chunk too");
    }

    #[test]
    fn model_object_shape() {
        let v = model_object("m", false, "MIT", 10, "allowed", false);
        assert_eq!(v["object"], "model");
        assert_eq!(v["ucompute"]["downloaded"], false);
        assert_eq!(v["ucompute"]["license"], "MIT");
    }
}
