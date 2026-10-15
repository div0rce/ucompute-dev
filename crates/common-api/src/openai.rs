//! OpenAI-compatible endpoints (F-09-01, F-09-02, F-01-12): `/v1/models`, `/v1/chat/completions`,
//! `/v1/completions`, `/v1/embeddings`. Request fields the engine cannot honour are rejected
//! (`n > 1`) or ignored (`logprobs`, `user`, ...); unknown fields are ignored like OpenAI does.
//!
//! Common extension `target` (body field or `X-Common-Target` header): `"local"` (default) or
//! `"peer:<peer id or short label>"` runs the whole request on one paired device (Mode B, remote
//! execution, never pooled inference). `PolicyEngine::evaluate` decides before admission, load or
//! any network activity (I-05); every response carries `"target"` so nothing runs elsewhere silently.

use crate::admission::Rejected;
use crate::{ApiError, AppState, Json};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use common_core::{CommonError, ErrorCode, Result};
use common_identity::PeerId;
use common_policy::{ExecutionTarget, PrivacyMode, WorkloadKind};
use common_runtime::manager::LoadOptions;
use common_runtime::{ChatMessage, GenerateEvent, GenerateRequest, GenerateStream, PromptInput, Timings, Usage};
use futures_util::stream::{self, BoxStream, Stream};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;
use tokio::sync::OwnedSemaphorePermit;
use tokio_util::sync::CancellationToken;

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
            return Err(CommonError::invalid("n must be 1"));
        }
        let max_tokens = self
            .max_completion_tokens
            .or(self.max_tokens)
            .unwrap_or(DEFAULT_MAX_TOKENS);
        if max_tokens == 0 {
            return Err(CommonError::invalid("max_tokens must be >= 1"));
        }
        let temperature = self.temperature.unwrap_or(DEFAULT_TEMPERATURE);
        let top_p = self.top_p.unwrap_or(DEFAULT_TOP_P);
        if !(0.0..=2.0).contains(&temperature) || !(0.0..=1.0).contains(&top_p) {
            return Err(CommonError::invalid(
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
                return Err(CommonError::invalid(
                    "response_format.json_schema.schema must be a JSON schema object",
                ));
            }
            check_schema(schema)?;
            Ok(Some(schema.clone()))
        }
        Some(other) => Err(CommonError::invalid(format!(
            "unsupported response_format type {other:?}"
        ))),
    }
}

/// Bounds on an untrusted JSON schema before it reaches the engine's schema->grammar converter
/// (F-01-12: pathological grammars are refused here, in microseconds, not compiled there).
const SCHEMA_MAX_BYTES: usize = 64 * 1024;
const SCHEMA_MAX_DEPTH: usize = 32;
/// Largest `{n,m}` quantifier, `minItems`/`maxLength`/... value and `pattern` length accepted.
const SCHEMA_MAX_REPEAT: u64 = 1024;

fn check_schema(schema: &Value) -> Result<()> {
    if schema.to_string().len() > SCHEMA_MAX_BYTES {
        return Err(CommonError::invalid(format!(
            "response_format schema exceeds {SCHEMA_MAX_BYTES} bytes"
        )));
    }
    check_schema_node(schema, 0)
}

// ponytail: keyword checks apply to any object key, so a *property* literally named `pattern`
// with a long string value is refused too; use a real JSON-schema walker if that ever bites.
fn check_schema_node(v: &Value, depth: usize) -> Result<()> {
    if depth > SCHEMA_MAX_DEPTH {
        return Err(CommonError::invalid(format!(
            "response_format schema nests deeper than {SCHEMA_MAX_DEPTH} levels"
        )));
    }
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                match (k.as_str(), val) {
                    ("$ref", Value::String(r)) if !r.starts_with('#') => return Err(CommonError::invalid("response_format schema: only local $ref (#/...) is supported")),
                    ("pattern", Value::String(p)) if p.len() as u64 > SCHEMA_MAX_REPEAT || pattern_repeat_too_large(p) => {
                        return Err(CommonError::invalid(format!("response_format schema: pattern longer than {SCHEMA_MAX_REPEAT} or with a repetition above {SCHEMA_MAX_REPEAT}")))
                    }
                    ("minItems" | "maxItems" | "minLength" | "maxLength" | "minProperties" | "maxProperties" | "minContains" | "maxContains", Value::Number(n)) if n.as_u64().is_none_or(|n| n > SCHEMA_MAX_REPEAT) => {
                        return Err(CommonError::invalid(format!("response_format schema: {k} must be an integer in 0..={SCHEMA_MAX_REPEAT}")))
                    }
                    _ => check_schema_node(val, depth + 1)?,
                }
            }
        }
        Value::Array(items) => {
            for i in items {
                check_schema_node(i, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// True when a regex `{n}` / `{n,}` / `{n,m}` quantifier names a count above `SCHEMA_MAX_REPEAT`.
fn pattern_repeat_too_large(p: &str) -> bool {
    p.split('{').skip(1).filter_map(|rest| rest.split('}').next()).any(|q| {
        q.split(',')
            .any(|n| n.trim().parse::<u64>().is_ok_and(|n| n > SCHEMA_MAX_REPEAT))
    })
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
                            return Err(CommonError::invalid(format!(
                                "unsupported content part type {:?}; only text is supported",
                                other.unwrap_or("")
                            )))
                        }
                    }
                }
                s
            }
            Some(_) => {
                return Err(CommonError::invalid(
                    "message content must be a string or an array of text parts",
                ))
            }
        };
        if !matches!(
            self.role.as_str(),
            "system" | "user" | "assistant" | "tool" | "developer"
        ) {
            return Err(CommonError::invalid(format!("unknown message role {:?}", self.role)));
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
    /// Common extension: `"local"` or `"peer:<id>"` (see the module docs).
    pub target: Option<String>,
}

#[derive(Deserialize)]
pub struct CompletionRequest {
    pub model: Option<String>,
    pub prompt: Option<Value>,
    #[serde(flatten)]
    pub sampling: Sampling,
    pub target: Option<String>,
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
        _ => Err(CommonError::invalid("\"model\" is required (a catalog or store id)")),
    }
}

/// Where a request runs: this device or one paired peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Local,
    Peer(PeerId),
}

impl Target {
    /// The value echoed as `"target"` in every response.
    fn label(&self) -> String {
        match self {
            Target::Local => "local".into(),
            Target::Peer(id) => format!("peer:{}", id.to_hex()),
        }
    }
}

/// Body field first, then the `X-Common-Target` header; absent means local. A peer must be named
/// by its 64-hex id or the short label of a pinned peer (`PeerUnauthorized` for an unknown label).
fn parse_target(st: &AppState, body: Option<&str>, headers: &HeaderMap) -> Result<Target> {
    let s = body.or_else(|| headers.get("x-common-target").and_then(|v| v.to_str().ok()));
    match s.map(str::trim) {
        None | Some("") | Some("local") => Ok(Target::Local),
        Some(s) => match s.strip_prefix("peer:") {
            Some(id) => Ok(Target::Peer(st.resolve_peer(id)?)),
            None => Err(CommonError::invalid(
                "target must be \"local\" or \"peer:<peer id or short label>\"",
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
            error: CommonError::new(ErrorCode::Overloaded, msg),
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
    /// `"local"` or `"peer:<hex>"`, in every envelope.
    target: String,
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
        }
    }

    fn envelope(&self, choices: Value) -> Value {
        let object = match self.kind {
            Kind::Chat => "chat.completion.chunk",
            Kind::Text => "text_completion",
        };
        json!({"id": self.id, "object": object, "created": self.created, "model": self.model, "choices": choices, "target": self.target})
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
    let chunks = gen
        .flat_map(move |ev| {
            if let Ok(GenerateEvent::Done { usage, timings, .. }) = &ev {
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

async fn run(
    st: Arc<AppState>,
    fmt: Format,
    model_id: &str,
    input: Input,
    sampling: &Sampling,
    tools: Option<Value>,
    target: Target,
) -> std::result::Result<Response, ApiError> {
    // I-05: the routing decision comes first, before admission, any load or any network activity.
    // Local is always allowed; a peer needs trusted_group privacy, networking on and a pinned,
    // non-revoked peer in a group allowing remote inference (typed denial with the plain reason).
    let privacy = st.settings().privacy;
    let exec = match target {
        Target::Local => ExecutionTarget::Local,
        Target::Peer(id) => ExecutionTarget::Peer(id),
    };
    st.policy()
        .evaluate(privacy, exec, WorkloadKind::RemoteInference)
        .into_result()?;
    // Untrusted schema is checked before admission: a grammar bomb never takes a slot or a load.
    json_schema_of(sampling.response_format.as_ref())?;
    if let Target::Peer(id) = target {
        return run_remote(st, fmt, model_id, input, sampling, tools, id, privacy).await;
    }
    let permit = admit(&st).await?;
    let model = st.manager.ensure_loaded(model_id, st.load_options()).await?;
    let input = match input {
        Input::Raw(p) => PromptInput::Raw(p),
        Input::Chat(msgs) => {
            let (manifest, blob) = st.store.resolve(model_id)?;
            if st.metadata(&manifest.sha256, &blob)?.has_chat_template {
                PromptInput::Chat(msgs)
            } else {
                PromptInput::Raw(plain_prompt(&msgs))
            }
        }
    };
    let req = sampling.to_generate(input, tools)?;
    // Child of the server's shutdown token: Ctrl-C ends this generation too (see `serve_on`).
    let cancel = st.shutdown.child_token();
    let gen = model.generate(req, cancel.clone()).await?;
    if sampling.stream {
        return Ok(sse_response(st, fmt, gen, cancel, Some(permit)));
    }
    let collected = collect(gen).await?;
    st.metrics.record_done(&collected.usage, &collected.timings);
    drop(permit);
    Ok(axum::Json(fmt.final_json(&collected)).into_response())
}

/// Mode B: the whole request on `peer` over the authenticated transport, streamed back exactly
/// like a local run. Called only after `evaluate` allowed it; `generate_remote` re-checks the
/// privacy string before opening a stream. The prompt travels inside TLS only and is never logged.
#[allow(clippy::too_many_arguments)]
async fn run_remote(
    st: Arc<AppState>,
    fmt: Format,
    model_id: &str,
    input: Input,
    sampling: &Sampling,
    tools: Option<Value>,
    peer: PeerId,
    privacy: PrivacyMode,
) -> std::result::Result<Response, ApiError> {
    if !common_core::paths::safe_name(model_id) {
        return Err(CommonError::invalid("model id may only contain letters, digits, '-', '_' and '.'").into());
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
        .map_err(CommonError::from)?
        .as_str()
        .unwrap_or("local_only")
        .to_string();
    let started = Instant::now();
    let gen = common_network::remote::generate_remote(conn, model_id, &sha, req, &privacy, cancel.clone()).await?;
    st.metrics
        .remote_requests
        .with_label_values(&[&peer.short_label()])
        .inc();
    let gen = audited(st.clone(), gen, peer, model_id.to_string(), started);
    if sampling.stream {
        return Ok(sse_response(st, fmt, gen, cancel, None));
    }
    let collected = collect(gen).await?;
    st.metrics.record_done(&collected.usage, &collected.timings);
    Ok(axum::Json(fmt.final_json(&collected)).into_response())
}

/// Audits the outcome of a remote request on this side: peer, model id, outcome code, counts and
/// duration. Never the text. (ponytail: a client that disconnects before Done leaves no entry here;
/// the serving side audits every request regardless.)
fn audited(st: Arc<AppState>, gen: GenerateStream, peer: PeerId, model_id: String, started: Instant) -> GenerateStream {
    gen.inspect(move |ev| {
        let outcome = match ev {
            Ok(GenerateEvent::Done { usage, .. }) => Some(("done", usage.completion_tokens)),
            Err(e) => Some((e.code.as_str(), 0)),
            Ok(_) => None,
        };
        if let Some((outcome, completion_tokens)) = outcome {
            st.audit(
                "remote_request",
                Some(&peer),
                json!({
                    "role": "client",
                    "peer": peer.short_label(),
                    "model_id": model_id,
                    "outcome": outcome,
                    "completion_tokens": completion_tokens,
                    "duration_ms": started.elapsed().as_millis() as u64,
                }),
            );
        }
    })
    .boxed()
}

pub async fn chat(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChatRequest>,
) -> std::result::Result<Response, ApiError> {
    let model_id = require_model(&req.model)?;
    let target = parse_target(&st, req.target.as_deref(), &headers)?;
    if req.messages.is_empty() {
        return Err(CommonError::invalid("messages must not be empty").into());
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
        _ => return Err(CommonError::invalid("prompt must be a string or a single-element array of strings").into()),
    };
    let fmt = Format::new(Kind::Text, &model_id, req.sampling.include_usage(), &target);
    run(st, fmt, &model_id, Input::Raw(prompt), &req.sampling, None, target).await
}

pub async fn embeddings(
    State(st): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<EmbeddingsRequest>,
) -> std::result::Result<axum::Json<Value>, ApiError> {
    let model_id = require_model(&req.model)?;
    // Embeddings run here only; asking for a peer is refused rather than quietly run locally.
    if parse_target(&st, None, &headers)? != Target::Local {
        return Err(CommonError::invalid("embeddings run on this device only; remove the target").into());
    }
    let inputs = req.input.map(OneOrMany::into_vec).unwrap_or_default();
    if inputs.is_empty() || inputs.iter().any(String::is_empty) {
        return Err(CommonError::invalid("input must be a non-empty string or array of non-empty strings").into());
    }
    let base64 = match req.encoding_format.as_deref() {
        None | Some("float") => false,
        Some("base64") => true,
        Some(other) => return Err(CommonError::invalid(format!("unsupported encoding_format {other:?}")).into()),
    };
    let _permit = admit(&st).await?;
    let model = st
        .manager
        .ensure_loaded(
            &model_id,
            LoadOptions {
                embeddings: true,
                ..st.load_options()
            },
        )
        .await?;
    let vectors = model.embed(&inputs).await?;
    let mut prompt_tokens = 0u32;
    for s in &inputs {
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
        "owned_by": "common",
        "common": {"downloaded": downloaded, "license": license, "bytes": bytes, "redistribution": redistribution, "loaded": loaded},
    })
}

/// Every store manifest plus catalog entries not downloaded yet, in the OpenAI list shape with a
/// `common` extension object.
pub async fn list_models(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    let loaded = st.manager.status().await.map(|i| i.model_id);
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
    if !common_core::paths::safe_name(&id) {
        return Err(CommonError::invalid("model id may only contain letters, digits, '-', '_' and '.'").into());
    }
    let loaded = st.manager.status().await.is_some_and(|i| i.model_id == id);
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
        None => Err(CommonError::new(
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
        assert!(!pattern_repeat_too_large("^\\d{3}-\\d{4}$"));
        assert!(pattern_repeat_too_large("(ab){1,5000}"));
        assert!(!pattern_repeat_too_large("{unterminated"));
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
        let err = f.stream_events(Err(CommonError::new(ErrorCode::Timeout, "slow")));
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
    }

    #[test]
    fn model_object_shape() {
        let v = model_object("m", false, "MIT", 10, "allowed", false);
        assert_eq!(v["object"], "model");
        assert_eq!(v["common"]["downloaded"], false);
        assert_eq!(v["common"]["license"], "MIT");
    }
}
