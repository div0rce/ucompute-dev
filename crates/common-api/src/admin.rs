//! Health, readiness, metrics, OpenAPI document and the Common `/api/*` extensions used by the
//! CLI and the desktop app.

use crate::{ApiError, AppState, Json};
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::sse::{Event, Sse};
use axum::response::{IntoResponse, Response};
use common_core::{CommonError, ErrorCode, Result};
use common_models::feasibility::calibration;
use common_models::{KvType, MemoryEstimate};
use common_runtime::manager::LoadOptions;
use common_runtime::DevicePreference;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

/// Hand-written OpenAPI 3.1 document covering every entry of `crate::ROUTES`.
pub const OPENAPI_JSON: &str = include_str!("../openapi.json");

pub async fn health() -> axum::Json<Value> {
    axum::Json(json!({"status": "ok", "version": common_core::VERSION}))
}

/// 200 when a request could be served now: no model loaded, or the loaded engine answers a
/// tokenize round-trip within 2 s. 503 while a load holds the slot or the engine is unresponsive.
pub async fn ready(State(st): State<Arc<AppState>>) -> Response {
    let Ok(slot) = st.manager.loaded.try_lock() else {
        return not_ready("a model is loading");
    };
    let Some(model) = slot.as_ref().map(|s| s.model.clone()) else {
        return axum::Json(json!({"status": "ready", "model": Value::Null})).into_response();
    };
    drop(slot);
    match tokio::time::timeout(Duration::from_secs(2), model.tokenize("ready")).await {
        Ok(Ok(_)) => axum::Json(json!({"status": "ready", "model": model.info().model_id})).into_response(),
        Ok(Err(e)) => not_ready(&format!("engine error: {}", e.message)),
        Err(_) => not_ready("engine did not answer within 2 s"),
    }
}

fn not_ready(reason: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        axum::Json(json!({"status": "not_ready", "reason": reason})),
    )
        .into_response()
}

pub async fn metrics(State(st): State<Arc<AppState>>) -> Response {
    let body = st.metrics.render(st.admission.queued(), st.admission.inflight());
    (
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
        )],
        body,
    )
        .into_response()
}

pub async fn openapi() -> Response {
    (
        [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        OPENAPI_JSON,
    )
        .into_response()
}

pub async fn hardware() -> std::result::Result<axum::Json<common_hardware::HardwareReport>, ApiError> {
    let report = tokio::task::spawn_blocking(common_hardware::probe)
        .await
        .map_err(|e| CommonError::internal(format!("probe task failed: {e}")))??;
    Ok(axum::Json(report))
}

pub async fn status(State(st): State<Arc<AppState>>) -> axum::Json<Value> {
    axum::Json(json!({
        "version": common_core::VERSION,
        "uptime_seconds": st.started.elapsed().as_secs(),
        "loaded": st.manager.status().await,
        "queue": {
            "inflight": st.admission.inflight(),
            "queued": st.admission.queued(),
            "max_concurrent": st.admission.max_concurrent,
            "max_queue": st.admission.max_queue,
        },
    }))
}

/// Store manifests plus catalog entries not downloaded yet, each with a fit estimate for this
/// device (GGUF-based when the blob is present, file-size-only otherwise).
pub async fn models(State(st): State<Arc<AppState>>) -> std::result::Result<axum::Json<Value>, ApiError> {
    let loaded = st.manager.status().await.map(|i| i.model_id);
    let list = tokio::task::spawn_blocking(move || model_list(&st, loaded.as_deref()))
        .await
        .map_err(|e| CommonError::internal(format!("listing task failed: {e}")))??;
    Ok(axum::Json(json!({"models": list})))
}

fn model_list(st: &AppState, loaded: Option<&str>) -> Result<Vec<Value>> {
    let available = st.manager.hardware.memory.available_bytes;
    let mut out = Vec::new();
    let manifests = st.store.list()?;
    for m in &manifests {
        let entry = st.catalog.get(&m.id);
        let blob = st.store.blob_path(&m.sha256);
        let (estimate, basis) = match st.metadata(&m.sha256, &blob) {
            Ok(meta) => {
                let n_ctx = meta.context_length.clamp(1, 4096);
                (common_models::estimate(&meta, m.bytes, n_ctx, KvType::F16), "gguf")
            }
            Err(_) => (file_size_estimate(m.bytes), "file_size"),
        };
        out.push(json!({
            "id": m.id,
            "name": m.name,
            "bytes": m.bytes,
            "license": m.license,
            "license_url": m.license_url,
            "source": entry.map_or(m.source_url.as_str(), |e| e.source.as_str()),
            "family": entry.and_then(|e| e.family.clone()),
            "parameters": entry.and_then(|e| e.parameters.clone()),
            "quantization": entry.and_then(|e| e.quantization.clone()),
            "chat": m.chat || entry.is_some_and(|e| e.chat),
            "redistribution": m.redistribution,
            "tags": m.tags,
            "downloaded": true,
            "loaded": loaded == Some(m.id.as_str()),
            "fit": common_models::check_fit(&estimate, available, 0.10),
            "estimate": estimate,
            "estimate_basis": basis,
        }));
    }
    for e in st
        .catalog
        .models
        .iter()
        .filter(|e| !manifests.iter().any(|m| m.id == e.id))
    {
        let estimate = file_size_estimate(e.bytes);
        out.push(json!({
            "id": e.id,
            "name": e.name,
            "bytes": e.bytes,
            "license": e.license,
            "license_url": e.license_url,
            "source": e.source,
            "family": e.family,
            "parameters": e.parameters,
            "quantization": e.quantization,
            "chat": e.chat,
            "redistribution": e.redistribution,
            "tags": e.tags,
            "downloaded": false,
            "loaded": false,
            "fit": common_models::check_fit(&estimate, available, 0.10),
            "estimate": estimate,
            "estimate_basis": "file_size",
        }));
    }
    Ok(out)
}

/// Lower bound when the GGUF header is not available: weights (= file size) plus the fixed
/// baseline, no KV cache or compute buffers. "fits" here means "weights alone fit".
fn file_size_estimate(bytes: u64) -> MemoryEstimate {
    let overhead = if cfg!(target_os = "macos") {
        calibration::OVERHEAD_GPU_BYTES
    } else {
        calibration::OVERHEAD_CPU_BYTES
    };
    MemoryEstimate {
        weights_bytes: bytes,
        kv_cache_bytes: 0,
        compute_buffer_bytes: 0,
        overhead_bytes: overhead,
        total_bytes: bytes.saturating_add(overhead),
        n_ctx: 0,
    }
}

fn check_id(id: &str) -> Result<()> {
    if common_core::paths::safe_name(id) {
        Ok(())
    } else {
        Err(CommonError::invalid(
            "model id may only contain letters, digits, '-', '_' and '.'",
        ))
    }
}

/// SSE stream of `common_artifacts::Progress` events; the last one has `done: true`. A failure is
/// sent as `{"error": {...}}` and ends the stream. At most `MAX_PULLS` run at once (429 beyond);
/// the stream ends at server shutdown so an open download cannot hold the server up.
pub async fn pull(State(st): State<Arc<AppState>>, Path(id): Path<String>) -> std::result::Result<Response, ApiError> {
    check_id(&id)?;
    let entry = st.catalog.get(&id).cloned().ok_or_else(|| {
        CommonError::new(ErrorCode::ModelNotFound, format!("no model named {id} in the catalog"))
            .with_hint("common models list")
    })?;
    let permit = st.pull_permit()?;
    let store = st.store.clone();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    // `pull` takes a `&mut dyn FnMut` without a Send bound, so its future cannot be spawned on the
    // multi-thread runtime; drive it on a blocking thread with the runtime handle instead.
    // ponytail: no de-duplication of concurrent pulls of the same id (the verified downloader
    // tolerates it); add a per-id guard if the desktop ever fires duplicate pulls.
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let progress_tx = tx.clone();
        let r = handle.block_on(common_artifacts::pull(&store, &entry, &mut |p| {
            let _ = progress_tx.send(serde_json::to_value(p).unwrap_or(Value::Null));
        }));
        if let Err(e) = r {
            let _ = tx.send(e.to_openai_json());
        }
    });
    let events = tokio_stream::wrappers::UnboundedReceiverStream::new(rx)
        .map(|v| Ok::<_, Infallible>(Event::default().data(v.to_string())))
        .take_until(st.shutdown.clone().cancelled_owned());
    Ok(Sse::new(events).into_response())
}

#[derive(Deserialize, Default)]
pub struct LoadBody {
    #[serde(default)]
    pub device: Option<DevicePreference>,
    #[serde(default)]
    pub n_ctx: Option<u32>,
}

pub async fn load(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<LoadBody>,
) -> std::result::Result<axum::Json<common_runtime::LoadedModelInfo>, ApiError> {
    check_id(&id)?;
    let opts = LoadOptions {
        device: body.device,
        n_ctx: body.n_ctx,
        ..st.load_options()
    };
    let model = st.manager.ensure_loaded(&id, opts).await?;
    Ok(axum::Json(model.info().clone()))
}

/// Stop the daemon the way SIGTERM does (the desktop app asks this before it would kill the
/// process, which on Windows cannot be signalled): `serve_on` drains HTTP, closes the peer Node
/// (mDNS goodbye, QUIC close) and unloads the model, then returns. Same auth, CORS and Host guards
/// as every route.
pub async fn shutdown(State(st): State<Arc<AppState>>) -> (StatusCode, axum::Json<Value>) {
    tracing::info!("shutdown requested over the API");
    st.shutdown.cancel();
    (StatusCode::ACCEPTED, axum::Json(json!({"status": "shutting_down"})))
}

pub async fn unload(State(st): State<Arc<AppState>>) -> std::result::Result<StatusCode, ApiError> {
    st.manager.unload().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Unloads the model first when it is the one being removed, then deletes manifest (+ unshared blob).
pub async fn remove(
    State(st): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    check_id(&id)?;
    if st.manager.status().await.is_some_and(|i| i.model_id == id) {
        st.manager.unload().await?;
    }
    st.store.remove(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct ImportBody {
    pub path: String,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
}

/// Hash + link a local GGUF into the store. The path must be absolute; nothing is moved.
/// Trust boundary (same as the CLI): a file without a valid GGUF header is refused
/// (`model_file_invalid`) before it is hashed or linked.
pub async fn import(
    State(st): State<Arc<AppState>>,
    Json(body): Json<ImportBody>,
) -> std::result::Result<axum::Json<common_artifacts::ModelManifest>, ApiError> {
    let path = std::path::PathBuf::from(&body.path);
    if !path.is_absolute() {
        return Err(CommonError::invalid("path must be absolute").into());
    }
    if !path.is_file() {
        return Err(CommonError::invalid(format!("no such file: {}", body.path)).into());
    }
    if let Some(id) = &body.id {
        check_id(id)?;
    }
    let store = st.store.clone();
    let m = tokio::task::spawn_blocking(move || {
        common_models::read_metadata(&path)?;
        store.import(&path, body.id.as_deref(), body.license.as_deref())
    })
    .await
    .map_err(|e| CommonError::internal(format!("import task failed: {e}")))??;
    Ok(axum::Json(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_models::FitVerdict;

    #[test]
    fn file_size_estimate_is_weights_plus_baseline() {
        let e = file_size_estimate(1_000);
        assert_eq!(e.weights_bytes, 1_000);
        assert_eq!(e.kv_cache_bytes + e.compute_buffer_bytes, 0);
        assert_eq!(e.total_bytes, 1_000 + e.overhead_bytes);
        assert!(matches!(
            common_models::check_fit(&e, 10 * e.total_bytes, 0.10),
            FitVerdict::Fits { .. }
        ));
        assert!(matches!(
            common_models::check_fit(&e, 100, 0.10),
            FitVerdict::TooLarge { .. }
        ));
        assert_eq!(file_size_estimate(u64::MAX).total_bytes, u64::MAX);
    }

    #[test]
    fn openapi_document_parses() {
        let v: Value = serde_json::from_str(OPENAPI_JSON).unwrap();
        assert_eq!(v["openapi"], "3.1.0");
        assert_eq!(v["info"]["version"], "1.0.0");
        assert!(v["paths"].is_object());
    }

    #[test]
    fn ids_are_validated() {
        assert!(check_id("qwen2.5-0.5b-instruct-q4_k_m").is_ok());
        assert_eq!(check_id("../x").unwrap_err().code, ErrorCode::InvalidRequest);
        assert_eq!(check_id("").unwrap_err().code, ErrorCode::InvalidRequest);
    }
}
