//! Backend-neutral request/response types and the adapter traits.

use common_core::{Result, Timeouts};
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DevicePreference {
    /// GPU if the probe says one is usable, else CPU. GPU failure falls back to CPU with a reason.
    Auto,
    Cpu,
    /// GPU required; failure is an error, no fallback.
    Gpu,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoadRequest {
    pub model_id: String,
    pub model_path: PathBuf,
    pub model_sha256: String,
    pub device: DevicePreference,
    pub n_ctx: u32,
    /// None = all layers when on GPU, 0 when on CPU.
    pub n_gpu_layers: Option<u32>,
    pub threads: Option<u32>,
    /// Concurrent sequences the engine should serve.
    pub parallel_slots: u32,
    pub kv_type: common_models::KvType,
    /// Start the engine in embeddings mode (pooling enabled, generation disabled).
    #[serde(default)]
    pub embeddings: bool,
    pub timeouts: Timeouts,
    /// Extra engine-specific args. Validated by the adapter; split and RPC flags are reserved (use `split`).
    #[serde(default)]
    pub extra_args: Vec<String>,
    /// Split this model across devices with llama.cpp RPC (WP-04, ADR-0005 D2). The placement comes
    /// from `stages` alone: `device` and `n_gpu_layers` are ignored, and a split never falls back to
    /// the CPU or to a local load.
    #[serde(default)]
    pub split: Option<Split>,
}

/// A static layer split. `--rpc` is rendered from `rpc_ports` only, as `127.0.0.1:<port>`: an RPC
/// endpoint is always a loopback port the daemon's bridge owns, never an address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Split {
    /// Loopback ports in `--rpc` order: the first is llama.cpp's `RPC0`, the next `RPC1`, ...
    pub rpc_ports: Vec<u16>,
    /// The planner's stages in order, covering blocks `0..n_layer`; the last also takes the output layer.
    pub stages: Vec<SplitStage>,
    /// The model's block count (GGUF `block_count`); the load refuses a plan for another model.
    pub n_layer: u32,
    /// Load without mmap (`-lm none`; b11609 has no `--no-mmap`).
    #[serde(default)]
    pub no_mmap: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SplitStage {
    /// llama.cpp device name: `RPC0`, `RPC1`, ... or a local device such as `MTL0`.
    pub device: String,
    /// First block of the stage.
    pub first_block: u32,
    /// One past the stage's last block.
    pub end_block: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LoadedModelInfo {
    pub model_id: String,
    pub model_sha256: String,
    pub backend_name: String,
    pub backend_version: String,
    pub device_requested: String,
    /// Proven device, e.g. `Metal0 (Apple M2)` parsed from the engine log, or `cpu`.
    pub device_actual: String,
    pub offloaded_layers: u32,
    pub total_layers: u32,
    /// Context one request can use (the engine's per-slot n_ctx, parsed from its log), not the
    /// total across `parallel_slots`.
    pub n_ctx: u32,
    pub threads: u32,
    pub parallel_slots: u32,
    /// Set when the adapter fell back from GPU to CPU (F-01-04). Never silent.
    pub fallback_reason: Option<String>,
    /// Set by the planner when the requested context was capped to the trained context, reduced
    /// to fit memory, or the KV cache was downgraded to Q8_0 (F-01-08). Never silent.
    #[serde(default)]
    pub context_note: Option<String>,
    pub load_ms: f64,
    /// Engine process id for memory measurement.
    pub pid: Option<u32>,
    /// A split load's stages, proved from the engine log (`split::check_buffers`). None for a local load.
    #[serde(default)]
    pub split: Option<Vec<SplitStage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PromptInput {
    /// Raw text; no chat template applied.
    Raw(String),
    /// Chat messages; the engine applies the model's chat template.
    Chat(Vec<ChatMessage>),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GenerateRequest {
    pub input: PromptInput,
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
    pub seed: Option<u64>,
    #[serde(default)]
    pub stop: Vec<String>,
    /// JSON schema for constrained decoding (F-01-12); converted to grammar by the engine.
    #[serde(default)]
    pub json_schema: Option<serde_json::Value>,
    #[serde(default)]
    pub grammar: Option<String>,
    /// OpenAI `tools` array passed through for chat requests.
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    /// Request token ids in the stream (needed by golden tests; costs a little bandwidth).
    #[serde(default)]
    pub return_token_ids: bool,
    pub timeouts: Timeouts,
}

impl Default for GenerateRequest {
    fn default() -> Self {
        Self {
            input: PromptInput::Raw(String::new()),
            max_tokens: 256,
            temperature: 0.7,
            top_p: 0.95,
            seed: None,
            stop: vec![],
            json_schema: None,
            grammar: None,
            tools: None,
            return_token_ids: false,
            timeouts: Timeouts::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Timings {
    pub time_to_first_token_ms: Option<f64>,
    pub prompt_ms: Option<f64>,
    pub prompt_tokens_per_second: Option<f64>,
    pub decode_ms: Option<f64>,
    pub decode_tokens_per_second: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum GenerateEvent {
    /// A piece of generated text (one or more tokens).
    Token {
        text: String,
        #[serde(default)]
        token_ids: Vec<u32>,
    },
    /// Streamed tool-call delta for chat requests (raw OpenAI delta JSON).
    ToolCallDelta { delta: serde_json::Value },
    Done {
        finish_reason: String,
        usage: Usage,
        timings: Timings,
    },
}

pub type GenerateStream = BoxStream<'static, Result<GenerateEvent>>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Capabilities {
    pub formats: Vec<String>,
    pub devices: Vec<String>,
    pub streaming: bool,
    pub cancellation: bool,
    pub embeddings: bool,
    pub json_schema: bool,
    pub tools: bool,
    pub multimodal: bool,
    pub lora: bool,
    pub speculative: bool,
}

#[async_trait::async_trait]
pub trait InferenceBackend: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> String;
    fn capabilities(&self) -> Capabilities;
    /// Validate the file, start the engine, wait for readiness. Must not hang past `timeouts.load`.
    async fn load(&self, req: LoadRequest) -> Result<Arc<dyn LoadedModel>>;
}

#[async_trait::async_trait]
pub trait LoadedModel: Send + Sync {
    fn info(&self) -> &LoadedModelInfo;
    /// Streaming generation. Cancelling the token (or dropping the stream) must stop the engine's
    /// work for this request within 200 ms (F-08-02).
    async fn generate(&self, req: GenerateRequest, cancel: CancellationToken) -> Result<GenerateStream>;
    async fn tokenize(&self, text: &str) -> Result<Vec<u32>>;
    async fn detokenize(&self, ids: &[u32]) -> Result<String>;
    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
    /// Current resident set size of the engine process, if measurable.
    fn memory_rss_bytes(&self) -> Option<u64>;
    /// Stop the engine and free everything. Idempotent.
    async fn unload(&self) -> Result<()>;
    /// Requests (streams or calls) currently using this model. The `ModelManager` refuses to
    /// unload or replace a model while this is non-zero (F-08-08). Backends may leave the default.
    fn in_flight(&self) -> usize {
        0
    }
    /// `Some(how it ended)` once the engine has exited on its own (killed, crashed, out of memory),
    /// never after `unload`. Non-blocking. The `ModelManager` frees the slot of an exited engine
    /// and reloads on the next request. Backends without a process leave the default.
    fn exited(&self) -> Option<String> {
        None
    }
}

/// Collect a whole generation into text (+ ids) — convenience for tests and the CLI.
pub async fn collect(stream: GenerateStream) -> Result<(String, Vec<u32>, Usage, Timings, String)> {
    use futures_util::StreamExt;
    let mut text = String::new();
    let mut ids = Vec::new();
    let mut usage = Usage::default();
    let mut timings = Timings::default();
    let mut finish = String::from("unknown");
    let mut s = stream;
    while let Some(ev) = s.next().await {
        match ev? {
            GenerateEvent::Token { text: t, token_ids } => {
                text.push_str(&t);
                ids.extend(token_ids);
            }
            GenerateEvent::ToolCallDelta { .. } => {}
            GenerateEvent::Done {
                finish_reason,
                usage: u,
                timings: t,
            } => {
                usage = u;
                timings = t;
                finish = finish_reason;
            }
        }
    }
    Ok((text, ids, usage, timings, finish))
}
