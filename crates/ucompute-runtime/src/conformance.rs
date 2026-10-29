//! Backend conformance suite (F-01-06): the same checks run against every adapter. Each check is
//! named so a report can say exactly what an adapter gets wrong. The suite loads `req` itself and
//! always unloads at the end.

use crate::types::*;
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use ucompute_core::Result;

pub struct ConformanceReport {
    pub passed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

impl ConformanceReport {
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

const PROMPT: &str = "Once upon a time, a little rabbit";

/// Run every conformance check against `backend` using `req` (a small real GGUF).
pub async fn run(backend: Arc<dyn InferenceBackend>, req: LoadRequest) -> Result<ConformanceReport> {
    let model = backend.load(req).await?;
    let mut report = ConformanceReport {
        passed: vec![],
        failed: vec![],
    };
    let mut record = |name: &str, r: std::result::Result<(), String>| match r {
        Ok(()) => report.passed.push(name.into()),
        Err(e) => report.failed.push((name.into(), e)),
    };
    record("load_reports_proof_fields", load_reports_proof_fields(model.info()));
    record("greedy_is_deterministic", greedy_is_deterministic(&model).await);
    record(
        "streaming_delivers_before_done",
        streaming_delivers_before_done(&model).await,
    );
    record("cancellation_within_200ms", cancellation_within_200ms(&model).await);
    record(
        "tokenize_detokenize_roundtrip",
        tokenize_detokenize_roundtrip(&model).await,
    );
    record("usage_counts_match", usage_counts_match(&model).await);
    record("unload_terminates_process", unload_terminates_process(&model).await);
    Ok(report)
}

fn greedy(max_tokens: u32) -> GenerateRequest {
    GenerateRequest {
        input: PromptInput::Raw(PROMPT.into()),
        max_tokens,
        temperature: 0.0,
        seed: Some(42),
        return_token_ids: true,
        ..Default::default()
    }
}

async fn run_greedy(
    model: &Arc<dyn LoadedModel>,
    max_tokens: u32,
) -> std::result::Result<(String, Vec<u32>, Usage, Timings, String), String> {
    let s = model
        .generate(greedy(max_tokens), CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    collect(s).await.map_err(|e| e.to_string())
}

fn load_reports_proof_fields(info: &LoadedModelInfo) -> std::result::Result<(), String> {
    if info.backend_version.is_empty() {
        return Err("backend_version is empty".into());
    }
    if info.device_actual.is_empty() {
        return Err("device_actual is empty".into());
    }
    if info.total_layers == 0 {
        return Err("total_layers is 0".into());
    }
    Ok(())
}

async fn greedy_is_deterministic(model: &Arc<dyn LoadedModel>) -> std::result::Result<(), String> {
    let (t1, i1, ..) = run_greedy(model, 32).await?;
    let (t2, i2, ..) = run_greedy(model, 32).await?;
    if t1.is_empty() || i1.is_empty() {
        return Err("empty generation".into());
    }
    if t1 != t2 || i1 != i2 {
        return Err(format!("two greedy runs differ: {:?} vs {:?}", t1, t2));
    }
    Ok(())
}

async fn streaming_delivers_before_done(model: &Arc<dyn LoadedModel>) -> std::result::Result<(), String> {
    let mut s = model
        .generate(greedy(32), CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    let mut tokens_before_done = 0;
    while let Some(ev) = s.next().await {
        match ev.map_err(|e| e.to_string())? {
            GenerateEvent::Token { .. } => tokens_before_done += 1,
            GenerateEvent::Done { .. } => break,
            GenerateEvent::ToolCallDelta { .. } => {}
        }
    }
    if tokens_before_done < 2 {
        return Err(format!(
            "only {tokens_before_done} Token events before Done; expected streaming"
        ));
    }
    Ok(())
}

/// Cancel after the first token; the stream must end within 200 ms and the engine must be free
/// again right away (a 1-token follow-up delivers its first token within 500 ms, which with one
/// engine slot proves the cancelled request released it).
async fn cancellation_within_200ms(model: &Arc<dyn LoadedModel>) -> std::result::Result<(), String> {
    let cancel = CancellationToken::new();
    let mut s = model
        .generate(greedy(128), cancel.clone())
        .await
        .map_err(|e| e.to_string())?;
    match s.next().await {
        Some(Ok(GenerateEvent::Token { .. })) => {}
        other => return Err(format!("expected a first token, got {other:?}")),
    }
    cancel.cancel();
    let t = Instant::now();
    let mut saw_cancelled = false;
    while let Some(ev) = s.next().await {
        if matches!(&ev, Err(e) if e.code == ucompute_core::ErrorCode::Cancelled) {
            saw_cancelled = true;
        }
    }
    let elapsed = t.elapsed();
    if elapsed > Duration::from_millis(200) {
        return Err(format!("stream took {elapsed:?} to end after cancel"));
    }
    if !saw_cancelled {
        return Err("stream ended without a Cancelled error item".into());
    }
    let mut s2 = model
        .generate(greedy(1), CancellationToken::new())
        .await
        .map_err(|e| e.to_string())?;
    let t2 = Instant::now();
    match tokio::time::timeout(Duration::from_millis(500), s2.next()).await {
        Ok(Some(Ok(GenerateEvent::Token { .. }))) => {}
        other => {
            return Err(format!(
                "engine not free after cancel: first token of follow-up after {:?}: {other:?}",
                t2.elapsed()
            ))
        }
    }
    while s2.next().await.is_some() {}
    Ok(())
}

async fn tokenize_detokenize_roundtrip(model: &Arc<dyn LoadedModel>) -> std::result::Result<(), String> {
    let ids = model.tokenize(PROMPT).await.map_err(|e| e.to_string())?;
    if ids.is_empty() {
        return Err("tokenize returned no ids".into());
    }
    let text = model.detokenize(&ids).await.map_err(|e| e.to_string())?;
    if !text.contains(PROMPT) {
        return Err(format!("detokenize({ids:?}) = {text:?} does not contain the prompt"));
    }
    Ok(())
}

async fn usage_counts_match(model: &Arc<dyn LoadedModel>) -> std::result::Result<(), String> {
    let (_, ids, usage, _, finish) = run_greedy(model, 16).await?;
    let n = ids.len() as u32;
    // An EOS stop is counted by the engine but carries no text/id.
    let ok = usage.completion_tokens == n || (finish == "stop" && usage.completion_tokens == n + 1);
    if !ok {
        return Err(format!(
            "usage.completion_tokens = {} but {} token ids received (finish {finish})",
            usage.completion_tokens, n
        ));
    }
    if usage.prompt_tokens == 0 {
        return Err("usage.prompt_tokens is 0".into());
    }
    Ok(())
}

async fn unload_terminates_process(model: &Arc<dyn LoadedModel>) -> std::result::Result<(), String> {
    let pid = model.info().pid.ok_or("no pid reported")?;
    model.unload().await.map_err(|e| e.to_string())?;
    let t = Instant::now();
    while crate::llamacpp::process::alive(pid) {
        if t.elapsed() > Duration::from_secs(3) {
            return Err(format!("pid {pid} still alive 3 s after unload"));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    model.unload().await.map_err(|e| format!("second unload failed: {e}"))?;
    Ok(())
}
