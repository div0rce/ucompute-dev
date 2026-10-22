//! Shared helpers for the API tests: an in-process server on `127.0.0.1:0`, fixture discovery
//! (COMMON_HOME or the repo's `.common`), and a `LoadedModel` test double labelled FAKE that never
//! touches an engine (admission, SSE framing and disconnect handling do not depend on one).
#![allow(dead_code)]

use common_api::{serve_on, ApiConfig, AppState};
use common_artifacts::Store;
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_hardware::HardwareReport;
use common_runtime::manager::{LoadOptions, ModelManager, Slot};
use common_runtime::*;
use futures_util::stream::{self, StreamExt};
use serde_json::Value;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const MODEL_ID: &str = "stories15M-q4_0";
pub const MODEL_SHA: &str = "66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739";
pub const CHAT_MODEL_ID: &str = "qwen2.5-0.5b-instruct-q4_k_m";
pub const PROMPT: &str = "Once upon a time, a little rabbit";
/// Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal).
pub const GOLDEN: &str =
    " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big";
pub const ENGINE_RELEASE: &str = "b11609";

pub fn common_home() -> PathBuf {
    std::env::var_os("COMMON_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.common"))
}

pub fn engine_dir(paths: &Paths) -> PathBuf {
    paths.backends().join("llama.cpp").join(ENGINE_RELEASE)
}

/// COMMON_HOME when the pinned engine and the stories15M fixture are present, else None (printed).
/// With `COMMON_REQUIRE_FIXTURES=1` (the evidence/CI job) a missing asset fails the test instead,
/// so a green run can never mean "nothing was generated".
pub fn fixture_home() -> Option<PathBuf> {
    let home = common_home();
    let paths = Paths::at(&home);
    let missing = if !common_artifacts::backend::binary(&engine_dir(&paths), "llama-server").is_file() {
        format!("pinned llama-server missing at {}", engine_dir(&paths).display())
    } else if Store::new(paths).resolve(MODEL_ID).is_err() {
        format!("fixture model {MODEL_ID} missing under {}", home.display())
    } else {
        return Some(home);
    };
    assert!(
        std::env::var_os("COMMON_REQUIRE_FIXTURES").is_none_or(|v| v != "1"),
        "COMMON_REQUIRE_FIXTURES=1: {missing}"
    );
    eprintln!("SKIP: {missing}");
    None
}

pub fn has_model(home: &std::path::Path, id: &str) -> bool {
    Store::new(Paths::at(home)).resolve(id).is_ok()
}

/// AppState over `home` with the real llama.cpp adapter. `HardwareReport::default()` reports zero
/// available memory, so pass `common_hardware::probe()` when a load must succeed.
pub fn app(config: ApiConfig, home: PathBuf, hardware: HardwareReport) -> Arc<AppState> {
    let paths = Paths::at(home);
    paths.ensure_dirs().unwrap();
    let backend = common_runtime::backend_by_name("llama.cpp", engine_dir(&paths), paths.clone()).unwrap();
    let manager = ModelManager::new(paths.clone(), Store::new(paths.clone()), backend, hardware);
    app_with(config, paths, Arc::new(manager))
}

/// AppState whose identity, peers, audit and settings live under `paths` while `manager` (and so
/// the model store) may point elsewhere, e.g. the real COMMON_HOME for a peer that serves models.
pub fn app_with(config: ApiConfig, paths: Paths, manager: Arc<ModelManager>) -> Arc<AppState> {
    paths.ensure_dirs().unwrap();
    Arc::new(AppState::new(config, paths, common_artifacts::Catalog::builtin().unwrap(), manager).unwrap())
}

pub struct Server {
    pub addr: SocketAddr,
    pub state: Arc<AppState>,
    task: tokio::task::JoinHandle<Result<()>>,
}

impl Server {
    pub async fn start(state: Arc<AppState>) -> Server {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(serve_on(state.clone(), listener, std::future::pending()));
        Server { addr, state, task }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Wait up to `limit` for `serve_on` to return by itself; its result, or None if still serving.
    pub async fn finished(&mut self, limit: Duration) -> Option<Result<()>> {
        tokio::time::timeout(limit, &mut self.task)
            .await
            .ok()
            .map(|r| r.unwrap())
    }

    /// Unload whatever is loaded (kills the engine), close the peer endpoint and stop serving.
    pub async fn stop(self) {
        let _ = self.state.manager.unload().await;
        self.state.stop_network().await;
        self.task.abort();
    }
}

/// Put a model straight into the manager's single slot (no store, no engine), loaded with the
/// options the API pins (`AppState::load_options`), so a request finds it compatible.
pub async fn seed_slot(state: &AppState, model_id: &str, model: Arc<dyn LoadedModel>) {
    *state.manager.loaded.lock().await = Some(slot(model_id, state.load_options(), model));
}

/// A slot as a local load fills it.
pub fn slot(model_id: &str, opts: LoadOptions, model: Arc<dyn LoadedModel>) -> Slot {
    Slot {
        model_id: model_id.into(),
        opts,
        model,
        loaded_for: "local".into(),
        loaded_at_utc: common_identity::utc_now(),
    }
}

/// reqwest without proxies, so HTTP_PROXY in the environment cannot divert loopback traffic.
pub fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

/// `data:` payloads of an SSE body, in order.
pub fn sse_data(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|l| l.strip_prefix("data: ").or_else(|| l.strip_prefix("data:")))
        .map(str::to_string)
        .collect()
}

/// Minimal Prometheus text-format lexer: every line is a comment, blank, or `name[{labels}] value`.
pub fn check_prometheus_text(text: &str) {
    for line in text.lines() {
        if line.is_empty() || line.starts_with("# HELP ") || line.starts_with("# TYPE ") {
            continue;
        }
        let (series, value) = line
            .rsplit_once(' ')
            .unwrap_or_else(|| panic!("no value in line {line:?}"));
        assert!(
            value.parse::<f64>().is_ok() || matches!(value, "+Inf" | "-Inf" | "NaN"),
            "bad value in {line:?}"
        );
        let name = series.split('{').next().unwrap();
        assert!(
            name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_'),
            "bad name in {line:?}"
        );
        assert!(
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'),
            "bad name in {line:?}"
        );
        assert!(
            !series.contains('{') || series.ends_with('}'),
            "unterminated labels in {line:?}"
        );
    }
}

/// Value of a metric line (`name{labels} value`) in Prometheus text.
pub fn metric(text: &str, series: &str) -> Option<f64> {
    text.lines().find_map(|l| {
        l.strip_prefix(series)
            .and_then(|rest| rest.strip_prefix(' '))
            .and_then(|v| v.parse().ok())
    })
}

/// FAKE `LoadedModel`: emits up to `tokens` text pieces `delay` apart, stops at the cancellation
/// token, and remembers the last token it was given so a test can prove a disconnect cancelled it.
pub struct FakeModel {
    info: LoadedModelInfo,
    pub tokens: u32,
    pub delay: Duration,
    last_cancel: Mutex<Option<CancellationToken>>,
}

impl FakeModel {
    pub fn new(tokens: u32, delay: Duration) -> Arc<FakeModel> {
        let info = LoadedModelInfo {
            model_id: MODEL_ID.into(),
            backend_name: "fake".into(),
            device_actual: "cpu".into(),
            ..Default::default()
        };
        Arc::new(FakeModel {
            info,
            tokens,
            delay,
            last_cancel: Mutex::new(None),
        })
    }

    pub fn last_cancel(&self) -> Option<CancellationToken> {
        self.last_cancel.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl LoadedModel for FakeModel {
    fn info(&self) -> &LoadedModelInfo {
        &self.info
    }

    async fn generate(&self, req: GenerateRequest, cancel: CancellationToken) -> Result<GenerateStream> {
        *self.last_cancel.lock().unwrap() = Some(cancel.clone());
        let n = req.max_tokens.min(self.tokens);
        let delay = self.delay;
        let prompt_tokens = match &req.input {
            PromptInput::Raw(s) => s.split_whitespace().count() as u32,
            PromptInput::Chat(m) => m.len() as u32,
        };
        let s = stream::unfold(0u32, move |i| {
            let cancel = cancel.clone();
            async move {
                if i > n {
                    return None;
                }
                if i == n {
                    let timings = Timings {
                        time_to_first_token_ms: Some(1.0),
                        decode_tokens_per_second: Some(100.0),
                        ..Default::default()
                    };
                    return Some((
                        Ok(GenerateEvent::Done {
                            finish_reason: "length".into(),
                            usage: Usage {
                                prompt_tokens,
                                completion_tokens: n,
                            },
                            timings,
                        }),
                        i + 1,
                    ));
                }
                tokio::select! {
                    _ = cancel.cancelled() => Some((Err(CommonError::new(ErrorCode::Cancelled, "cancelled")), n + 1)),
                    _ = tokio::time::sleep(delay) => Some((Ok(GenerateEvent::Token { text: format!("tok{i} "), token_ids: vec![] }), i + 1)),
                }
            }
        });
        Ok(s.boxed())
    }

    async fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        Ok(vec![1; text.split_whitespace().count()])
    }

    async fn detokenize(&self, ids: &[u32]) -> Result<String> {
        Ok("x ".repeat(ids.len()))
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(vec![vec![0.1, 0.2, 0.3]; inputs.len()])
    }

    fn memory_rss_bytes(&self) -> Option<u64> {
        None
    }

    async fn unload(&self) -> Result<()> {
        Ok(())
    }
}

pub fn chat_body(model: &str, max_tokens: u32, extra: Value) -> Value {
    let mut v = serde_json::json!({"model": model, "messages": [{"role": "user", "content": "Tell me a story."}], "max_tokens": max_tokens, "temperature": 0});
    if let Value::Object(m) = extra {
        v.as_object_mut().unwrap().extend(m);
    }
    v
}

/// `promtool check metrics` on a `/metrics` body when promtool is on PATH (F-09-03); prints SKIP otherwise.
pub fn promtool_check(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new("promtool")
        .args(["check", "metrics"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    else {
        eprintln!("SKIP: promtool not on PATH; /metrics checked by the built-in lexer only");
        return;
    };
    child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "promtool check metrics failed ({}):\n{}{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("promtool check metrics: ok");
}

/// Resident set size of this test process in KiB (`ps`, works on macOS and Linux).
pub fn rss_kib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
}
