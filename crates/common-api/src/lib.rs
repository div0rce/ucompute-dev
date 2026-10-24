//! Local HTTP API (F-09-01..04, F-08-03, F-08-08, F-06-06).
//!
//! Secure defaults: binds `127.0.0.1` unless `config.host` says otherwise; a non-loopback bind
//! without an API key is refused (PolicyDenied) before any socket is opened. The engine's own port
//! is never exposed: every request enters here, passes admission control, and leaves with a typed
//! error (OpenAI shape) or a counted response.
//!
//! Readiness semantics (`GET /ready`): 200 when no model is loaded (the server can accept load
//! requests) or when the loaded engine answers a tokenize round-trip; 503 while a load holds the
//! slot or when the engine does not answer within 2 s. `GET /health` is pure liveness.
//!
//! Chat on a GGUF without `tokenizer.chat_template` (e.g. the stories15M fixture): the API builds a
//! plain prompt of `role: content` lines followed by `assistant:` and generates raw text, so the
//! OpenAI chat endpoint still works; tool calls cannot be parsed from such a model.
//!
//! Common Mesh (M3, ADR-0003) lives in [`mesh`]: privacy mode and peer networking are off by
//! default and persisted in `COMMON_HOME/config.json`; a request leaves this device only through
//! `target: "peer:<id>"` after `PolicyEngine::evaluate` allowed it, and the response says so.

mod admin;
pub mod admission;
mod jobs;
pub mod mesh;
pub mod metrics;
mod openai;

use admission::Admission;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, FromRequest, MatchedPath, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::Router;
use common_core::{CommonError, ErrorCode, Paths, Result};
use common_identity::{DeviceIdentity, PeerStore};
use common_models::GgufMetadata;
use common_network::NetworkConfig;
use common_observability::Audit;
use common_runtime::manager::ModelManager;
use metrics::Metrics;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Request bodies above this are rejected (F-08-08: bounded memory per request).
pub const BODY_LIMIT: usize = 2 * 1024 * 1024;
/// Concurrent `/api/models/{id}/pull` downloads (each holds a blocking thread).
pub const MAX_PULLS: usize = 2;
/// After shutdown is signalled, open connections get this long to drain before they are closed.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>,
    /// Max in-flight generations; excess waits up to `queue_timeout_ms` then gets 503 (429 when
    /// the queue itself is full). Also the engine's parallel slots (`-np`): a request admitted
    /// here never queues invisibly inside the engine. llama-server splits the planned context
    /// evenly across slots, so values above 1 shrink the context each request gets by that
    /// factor (`/api/status` reports the per-slot value).
    pub max_concurrent: usize,
    pub max_queue: usize,
    pub queue_timeout_ms: u64,
    pub default_device: common_runtime::DevicePreference,
    /// Browser origins allowed to call the API (the desktop webview). Never `*`: a wildcard would
    /// let any website the user visits drive the local daemon.
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,
    /// Peer transport parameters (port, bind address, mDNS, timeouts). Whether the endpoint is
    /// up is the persisted setting behind `PUT /api/network`; `network.enabled` only seeds that
    /// setting when `COMMON_HOME/config.json` does not exist yet.
    #[serde(default)]
    pub network: NetworkConfig,
}

fn default_cors_origins() -> Vec<String> {
    [
        "tauri://localhost",
        "http://tauri.localhost",
        "https://tauri.localhost",
        "http://localhost:1420",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".into(),
            port: 8192,
            api_key: None,
            max_concurrent: 1,
            max_queue: 32,
            queue_timeout_ms: 30_000,
            default_device: common_runtime::DevicePreference::Auto,
            cors_origins: default_cors_origins(),
            network: NetworkConfig::default(),
        }
    }
}

pub struct AppState {
    pub config: ApiConfig,
    pub paths: Paths,
    pub store: common_artifacts::Store,
    pub catalog: common_artifacts::Catalog,
    pub manager: Arc<ModelManager>,
    pub metrics: Metrics,
    pub admission: Admission,
    pub started: Instant,
    /// Cancelled when `serve` starts shutting down; every generation runs on a child of it, so
    /// in-flight streams end (with a `cancelled` error + `[DONE]`) instead of holding the server open.
    pub shutdown: CancellationToken,
    /// This device's Ed25519 identity (`COMMON_HOME/identity/device.key`, 0600; Debug redacts).
    pub identity: Arc<DeviceIdentity>,
    /// Pinned peers, groups, revocations (`identity/peers.json`).
    pub peers: Arc<PeerStore>,
    /// Append-only audit of every off-device action (`logs/audit.jsonl`); ids and counts only.
    pub audit: Arc<Audit>,
    /// Privacy mode and network switches (default all off), persisted to `config.json` (0600) and
    /// re-read when that file changes (`common privacy` from another process applies to the next
    /// request). `mesh::AppState::policy` builds the `PolicyEngine` from this on every call.
    settings: Mutex<mesh::SettingsCell>,
    /// Serialises peer-store writes made here that must not race each other (an address refresh
    /// must never re-pin a device that was just unpaired).
    peer_writes: Mutex<()>,
    /// The peer Node, discovery handles and inbound accept loop; `Some` only while enabled.
    pub net: tokio::sync::Mutex<mesh::Net>,
    /// The one pairing session at a time (ADR-0003 D2: single-use code). Lock order: `pairing`
    /// before `net`, so a network stop can close the pairing window first.
    pairing: tokio::sync::Mutex<Option<mesh::Pairing>>,
    /// Revocations issued or applied here, offered by every connection's control loop (ADR-0003 D7).
    revocations: tokio::sync::broadcast::Sender<common_identity::Revocation>,
    /// Who should get each revocation and who already has it (`mesh::RevocationFanout`).
    fanout: Mutex<mesh::RevocationFanout>,
    /// `quinn::Connection::stable_id`s that already have a serving task and a control loop: a
    /// connection `Node::connect` returns may be one `Node::accept` handed out too.
    attached: Mutex<std::collections::HashSet<usize>>,
    /// `mesh::sync_loop`'s redial backoff per pinned device: (failed dials in a row, next dial).
    /// `POST /api/peers/{id}/connect` clears an entry, so a user's retry is never held back.
    pub(crate) redial: Mutex<HashMap<common_identity::PeerId, (u32, Instant)>>,
    /// The `peer_quota` in force (from `config.json` at start; a change applies at the next start).
    pub peer_quota: mesh::PeerQuota,
    /// How peers share this device (F-06-04): those quotas, `load_options()` and the admission
    /// semaphore, one set for every peer connection.
    peer_serve: common_network::remote::ServeOptions,
    /// `GET /api/jobs`: requests run here, sent to a paired device or served for one (in memory,
    /// ids and numbers only). The same registry as `peer_serve.jobs`.
    pub jobs: Arc<common_observability::jobs::Jobs>,
    /// GGUF headers by sha256 (chat-template lookup per request, fit estimates per listing).
    meta_cache: Mutex<HashMap<String, Arc<GgufMetadata>>>,
    /// Bounds concurrent downloads (`MAX_PULLS`).
    pulls: Arc<Semaphore>,
}

impl AppState {
    /// Loads (or creates) the device identity, the peer store, the audit log and the persisted
    /// settings under `paths`; `Io` when `COMMON_HOME` is not writable. Nothing listens yet:
    /// `serve_on` (or `mesh::AppState::apply_network`) starts the peer Node when enabled.
    pub fn new(
        config: ApiConfig,
        paths: Paths,
        catalog: common_artifacts::Catalog,
        manager: Arc<ModelManager>,
    ) -> Result<Self> {
        let admission = Admission::new(
            config.max_concurrent,
            config.max_queue,
            Duration::from_millis(config.queue_timeout_ms),
        );
        let identity = Arc::new(DeviceIdentity::load_or_create(&paths)?);
        let peers = Arc::new(PeerStore::open(&paths)?);
        let audit = Arc::new(Audit::open(&paths)?);
        let settings = mesh::SettingsCell::load(&paths, &config)?;
        let peer_quota = settings.peer_quota();
        let peer_serve = peer_quota.serve_options(load_options(&config), admission.slots(), admission.max_concurrent);
        Ok(Self {
            jobs: peer_serve.jobs.clone(),
            store: manager.store.clone(),
            config,
            paths,
            catalog,
            manager,
            metrics: Metrics::new(),
            admission,
            started: Instant::now(),
            shutdown: CancellationToken::new(),
            identity,
            peers,
            audit,
            settings: Mutex::new(settings),
            peer_writes: Mutex::new(()),
            net: tokio::sync::Mutex::new(mesh::Net::default()),
            pairing: tokio::sync::Mutex::new(None),
            revocations: tokio::sync::broadcast::channel(64).0,
            fanout: Mutex::new(mesh::RevocationFanout::default()),
            attached: Mutex::new(std::collections::HashSet::new()),
            redial: Mutex::new(HashMap::new()),
            peer_quota,
            peer_serve,
            meta_cache: Mutex::new(HashMap::new()),
            pulls: Arc::new(Semaphore::new(MAX_PULLS)),
        })
    }

    /// Load options every API load uses: the engine gets exactly `max_concurrent` slots, so
    /// admission and engine capacity agree (an admitted request never waits inside the engine).
    pub fn load_options(&self) -> common_runtime::manager::LoadOptions {
        load_options(&self.config)
    }

    /// A download permit, or 429 when `MAX_PULLS` downloads are already running.
    pub(crate) fn pull_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.pulls.clone().try_acquire_owned().map_err(|_| {
            CommonError::new(
                ErrorCode::Overloaded,
                format!("{MAX_PULLS} downloads are already in progress"),
            )
        })
    }

    /// Parsed GGUF header for a blob, cached by hash (blobs are immutable).
    pub fn metadata(&self, sha256: &str, path: &Path) -> Result<Arc<GgufMetadata>> {
        if let Some(m) = self.meta_cache.lock().unwrap_or_else(|p| p.into_inner()).get(sha256) {
            return Ok(m.clone());
        }
        let m = Arc::new(common_models::read_metadata(path)?);
        self.meta_cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(sha256.to_string(), m.clone());
        Ok(m)
    }
}

fn load_options(config: &ApiConfig) -> common_runtime::manager::LoadOptions {
    common_runtime::manager::LoadOptions {
        parallel_slots: Some(config.max_concurrent.max(1) as u32),
        ..Default::default()
    }
}

/// Every served route. `router()` is built from this table and `tests/api.rs` checks it against
/// `openapi.json`, so a route cannot exist in one place without the other (F-09-04).
pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("GET", "/ready"),
    ("GET", "/metrics"),
    ("GET", "/openapi.json"),
    ("GET", "/v1/models"),
    ("GET", "/v1/models/{id}"),
    ("POST", "/v1/chat/completions"),
    ("POST", "/v1/completions"),
    ("POST", "/v1/embeddings"),
    ("GET", "/api/hardware"),
    ("GET", "/api/status"),
    ("GET", "/api/models"),
    ("POST", "/api/models/{id}/pull"),
    ("POST", "/api/models/{id}/load"),
    ("POST", "/api/unload"),
    ("DELETE", "/api/models/{id}"),
    ("POST", "/api/models/import"),
    ("GET", "/api/network"),
    ("PUT", "/api/network"),
    ("GET", "/api/peers"),
    ("DELETE", "/api/peers/{id}"),
    ("POST", "/api/peers/{id}/revoke"),
    ("POST", "/api/pair/start"),
    ("GET", "/api/pair/status"),
    ("POST", "/api/pair/join"),
    ("GET", "/api/groups"),
    ("PUT", "/api/groups"),
    ("GET", "/api/privacy"),
    ("PUT", "/api/privacy"),
    ("GET", "/api/audit"),
    ("POST", "/api/peers/{id}/connect"),
    ("POST", "/api/shutdown"),
    ("GET", "/api/jobs"),
    ("POST", "/api/peers/{id}/disconnect"),
];

/// Build the axum router (exposed for in-process tests).
pub fn router(state: Arc<AppState>) -> Router {
    let mut r = Router::new();
    for (method, path) in ROUTES {
        let handler = match (*method, *path) {
            ("GET", "/health") => get(admin::health),
            ("GET", "/ready") => get(admin::ready),
            ("GET", "/metrics") => get(admin::metrics),
            ("GET", "/openapi.json") => get(admin::openapi),
            ("GET", "/v1/models") => get(openai::list_models),
            ("GET", "/v1/models/{id}") => get(openai::get_model),
            ("POST", "/v1/chat/completions") => post(openai::chat),
            ("POST", "/v1/completions") => post(openai::completions),
            ("POST", "/v1/embeddings") => post(openai::embeddings),
            ("GET", "/api/hardware") => get(admin::hardware),
            ("GET", "/api/status") => get(admin::status),
            ("GET", "/api/models") => get(admin::models),
            ("POST", "/api/models/{id}/pull") => post(admin::pull),
            ("POST", "/api/models/{id}/load") => post(admin::load),
            ("POST", "/api/unload") => post(admin::unload),
            ("DELETE", "/api/models/{id}") => delete(admin::remove),
            ("POST", "/api/models/import") => post(admin::import),
            ("GET", "/api/network") => get(mesh::network_get),
            ("PUT", "/api/network") => put(mesh::network_put),
            ("GET", "/api/peers") => get(mesh::peers_get),
            ("DELETE", "/api/peers/{id}") => delete(mesh::peer_delete),
            ("POST", "/api/peers/{id}/revoke") => post(mesh::peer_revoke),
            ("POST", "/api/pair/start") => post(mesh::pair_start),
            ("GET", "/api/pair/status") => get(mesh::pair_status),
            ("POST", "/api/pair/join") => post(mesh::pair_join),
            ("GET", "/api/groups") => get(mesh::groups_get),
            ("PUT", "/api/groups") => put(mesh::groups_put),
            ("GET", "/api/privacy") => get(mesh::privacy_get),
            ("PUT", "/api/privacy") => put(mesh::privacy_put),
            ("GET", "/api/audit") => get(mesh::audit_get),
            ("POST", "/api/peers/{id}/connect") => post(mesh::peer_connect),
            ("POST", "/api/shutdown") => post(admin::shutdown),
            ("GET", "/api/jobs") => get(jobs::jobs_get),
            ("POST", "/api/peers/{id}/disconnect") => post(mesh::peer_disconnect),
            other => unreachable!("route {other:?} is in ROUTES but has no handler"),
        };
        r = r.route(path, handler);
    }
    r.fallback(not_found)
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(state.clone(), auth))
        .layer(middleware::from_fn_with_state(state.clone(), cors))
        .layer(middleware::from_fn_with_state(state.clone(), track))
        .with_state(state)
}

/// Where `serve` will listen, or PolicyDenied for a non-loopback host without an API key, or
/// InvalidRequest for a peer bind address the endpoint cannot use (IPv6, see
/// [`NetworkConfig::bind_ip`]): refused at startup, not when networking is next turned on.
pub fn bind_addr(config: &ApiConfig) -> Result<SocketAddr> {
    config.network.bind_ip()?;
    let ip: IpAddr = config
        .host
        .parse()
        .map_err(|_| CommonError::invalid(format!("host must be an IP address, got {:?}", config.host)))?;
    if !ip.is_loopback() && config.api_key.is_none() {
        return Err(CommonError::new(
            ErrorCode::PolicyDenied,
            format!("refusing to listen on {} without --api-key", config.host),
        ));
    }
    Ok(SocketAddr::new(ip, config.port))
}

/// Bind and serve until `shutdown` resolves.
pub async fn serve(
    state: Arc<AppState>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let addr = bind_addr(&state.config)?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| CommonError::io(format!("cannot bind {addr}: {e}")))?;
    serve_on(state, listener, shutdown).await
}

/// Serve on an already-bound listener (tests bind `127.0.0.1:0` themselves). The peer Node is
/// started first when the persisted setting says so (a failure is logged, the API still serves,
/// `GET /api/network` shows `enabled: false`). When `shutdown` resolves or `state.shutdown` is
/// cancelled (`POST /api/shutdown`): in-flight generations are cancelled, connections get
/// `SHUTDOWN_GRACE` to drain, then the Node is closed (mDNS goodbye, QUIC close) and the loaded
/// model is unloaded (engine process stopped) and this returns.
pub async fn serve_on(
    state: Arc<AppState>,
    listener: tokio::net::TcpListener,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    if let Ok(a) = listener.local_addr() {
        tracing::info!(addr = %a, "common api listening");
    }
    if let Err(e) = state.apply_network().await {
        tracing::warn!(error = %e, "peer networking not started");
        state.set_settings(mesh::Settings {
            network_enabled: false,
            ..state.settings()
        });
    }
    let root = state.shutdown.clone();
    // Either way in: the caller's future (signals) or the token (`POST /api/shutdown`); without
    // the token arm a cancelled token would end serving only after the grace sleep below.
    let signal = async move {
        tokio::select! {
            () = shutdown => {}
            () = root.cancelled() => {}
        }
        root.cancel();
    };
    let server = axum::serve(listener, router(state.clone())).with_graceful_shutdown(signal);
    let grace = async {
        state.shutdown.cancelled().await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };
    let r = tokio::select! {
        r = server => r.map_err(|e| CommonError::io(e.to_string())),
        () = grace => {
            tracing::warn!("connections still open {SHUTDOWN_GRACE:?} after shutdown; closing them");
            Ok(())
        }
    };
    state.stop_network().await;
    if let Err(e) = state.manager.unload().await {
        tracing::warn!(error = %e, "unload at shutdown failed");
    }
    r
}

/// Error response: `CommonError` in the OpenAI shape with the code's HTTP status (F-08-03).
#[derive(Debug)]
pub struct ApiError {
    pub error: CommonError,
    pub status: StatusCode,
}

impl From<CommonError> for ApiError {
    fn from(error: CommonError) -> Self {
        let status = StatusCode::from_u16(error.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self { error, status }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut resp = (self.status, axum::Json(self.error.to_openai_json())).into_response();
        if self.error.code == ErrorCode::Overloaded {
            resp.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("1"));
        }
        resp
    }
}

/// `axum::Json` with rejections in the OpenAI error shape (400 invalid_request).
pub struct Json<T>(pub T);

impl<S, T> FromRequest<S> for Json<T>
where
    axum::Json<T>: FromRequest<S, Rejection = JsonRejection>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> std::result::Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(v)) => Ok(Json(v)),
            Err(rej) => {
                let status = if matches!(rej, JsonRejection::BytesRejection(_)) {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    StatusCode::BAD_REQUEST
                };
                Err(ApiError {
                    error: CommonError::invalid(json_rejection_message(&rej)),
                    status,
                })
            }
        }
    }
}

fn json_rejection_message(rej: &JsonRejection) -> String {
    match rej {
        JsonRejection::MissingJsonContentType(_) => {
            "request body must be JSON with Content-Type: application/json".into()
        }
        JsonRejection::BytesRejection(_) => format!("could not read the request body (limit {} bytes)", BODY_LIMIT),
        other => format!("invalid JSON body: {}", other.body_text()),
    }
}

async fn not_found() -> Response {
    ApiError {
        error: CommonError::invalid("no such route"),
        status: StatusCode::NOT_FOUND,
    }
    .into_response()
}

/// Bearer auth on every route except `/health` when `config.api_key` is set.
async fn auth(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    if let Some(key) = &st.config.api_key {
        if req.uri().path() != "/health" && req.method() != Method::OPTIONS {
            let presented = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "));
            if !presented.is_some_and(|p| ct_eq(p.as_bytes(), key.as_bytes())) {
                return ApiError::from(CommonError::new(ErrorCode::Unauthorized, "missing or invalid API key"))
                    .into_response();
            }
        }
    }
    next.run(req).await
}

/// Constant-time byte comparison (no early exit on the first differing byte).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// CORS for the allow-listed browser origins only (the desktop webview), enforced at request
/// time: a browser request from any other origin is refused with 403 before a handler runs (CORS
/// headers alone only hide the response; a "simple" POST would still execute). Without an API
/// key the `Host` header must be a loopback name too, so a DNS-rebound page cannot talk to the
/// daemon as if it were same-origin. Non-browser clients send neither header and are unaffected.
async fn cors(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    // Header reads stay in this block: a `&Request` held across the await below would make the
    // future !Send (the body is !Sync).
    let origin = {
        let hdr = |name: header::HeaderName| req.headers().get(name).and_then(|v| v.to_str().ok());
        let origin = hdr(header::ORIGIN);
        let allowed = origin.is_some_and(|o| st.config.cors_origins.iter().any(|a| a == o));
        let cross_site =
            origin.is_none() && hdr(header::HeaderName::from_static("sec-fetch-site")) == Some("cross-site");
        if (origin.is_some() && !allowed) || cross_site {
            return ApiError::from(CommonError::new(
                ErrorCode::PolicyDenied,
                "cross-origin browser requests are not allowed",
            ))
            .into_response();
        }
        if st.config.api_key.is_none() && !hdr(header::HOST).is_some_and(host_is_loopback) {
            return ApiError::from(CommonError::new(
                ErrorCode::PolicyDenied,
                "Host must be a loopback address",
            ))
            .into_response();
        }
        origin.filter(|_| allowed).map(str::to_owned)
    };
    let Some(origin) = origin else {
        return next.run(req).await;
    };
    let mut resp = if req.method() == Method::OPTIONS {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(req).await
    };
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&origin) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    h.insert(header::VARY, HeaderValue::from_static("Origin"));
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        // `x-common-client-ref`: the desktop app's id for its own requests (`GET /api/jobs`).
        HeaderValue::from_static("authorization, content-type, x-common-client-ref"),
    );
    resp
}

/// `Host` value (with optional port) that names this machine: `localhost` or a loopback IP.
fn host_is_loopback(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(h, _)| h),
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// `common_requests_total{route,status}`; the route label is the matched pattern, so cardinality
/// is bounded by `ROUTES` plus "unmatched".
async fn track(State(st): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_string(), |m| m.as_str().to_string());
    let resp = next.run(req).await;
    st.metrics
        .requests
        .with_label_values(&[&route, resp.status().as_str()])
        .inc();
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bind_is_loopback() {
        let c = ApiConfig::default();
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(c.port, 8192);
        let addr = bind_addr(&c).unwrap();
        assert!(addr.ip().is_loopback());
        assert_eq!(addr.port(), 8192);
        assert!(bind_addr(&ApiConfig {
            host: "::1".into(),
            ..c.clone()
        })
        .unwrap()
        .ip()
        .is_loopback());
    }

    /// An IPv6 peer bind (`::`, `[::]`) is refused when the daemon starts, with a clear error.
    #[test]
    fn ipv6_peer_bind_is_refused_at_startup() {
        for bind in ["::", "[::]"] {
            let c = ApiConfig {
                network: NetworkConfig {
                    bind_addr: bind.into(),
                    ..NetworkConfig::default()
                },
                ..Default::default()
            };
            let e = bind_addr(&c).unwrap_err();
            assert_eq!(e.code, ErrorCode::InvalidRequest, "{bind}");
            assert!(e.message.contains("IPv6 is not supported"), "{bind}: {e}");
        }
    }

    #[test]
    fn non_loopback_without_key_is_policy_denied() {
        let c = ApiConfig {
            host: "0.0.0.0".into(),
            ..Default::default()
        };
        let e = bind_addr(&c).unwrap_err();
        assert_eq!(e.code, ErrorCode::PolicyDenied);
        assert!(e.message.contains("0.0.0.0") && e.message.contains("--api-key"), "{e}");
        assert!(bind_addr(&ApiConfig {
            api_key: Some("k".into()),
            ..c.clone()
        })
        .is_ok());
        assert_eq!(
            bind_addr(&ApiConfig {
                host: "localhost".into(),
                ..c
            })
            .unwrap_err()
            .code,
            ErrorCode::InvalidRequest,
            "hostnames are not resolved"
        );
    }

    #[test]
    fn host_header_loopback_forms() {
        for ok in [
            "127.0.0.1:8192",
            "127.0.0.1",
            "localhost:8192",
            "LOCALHOST",
            "[::1]:8192",
            "[::1]",
            "127.0.0.2:1",
        ] {
            assert!(host_is_loopback(ok), "{ok}");
        }
        for bad in [
            "evil.example",
            "evil.example:8192",
            "192.168.1.5:8192",
            "[2001:db8::1]:8192",
            "",
            "localhost.evil.example",
            "::1",
        ] {
            assert!(!host_is_loopback(bad), "{bad}");
        }
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab"));
        assert!(ct_eq(b"", b""));
    }

    #[test]
    fn error_response_shape_and_retry_after() {
        let r = ApiError::from(CommonError::new(ErrorCode::Overloaded, "busy")).into_response();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(r.headers().get(header::RETRY_AFTER).unwrap(), "1");
        let r = ApiError::from(CommonError::new(ErrorCode::ModelNotFound, "x")).into_response();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert!(r.headers().get(header::RETRY_AFTER).is_none());
    }

    /// F-08-03: every stable code, including any an engine-crash path adds to `ErrorCode::ALL`
    /// (the core snapshot test makes every new code join it), reaches an API caller with its own
    /// HTTP status and code string, never a silent fallback.
    #[test]
    fn every_error_code_maps_to_its_stable_status_and_code() {
        for code in ErrorCode::ALL {
            let e = ApiError::from(CommonError::new(*code, "x"));
            assert_eq!(e.status.as_u16(), code.http_status(), "{code:?}");
            assert_eq!(e.error.to_openai_json()["error"]["code"], code.as_str(), "{code:?}");
        }
    }

    #[test]
    fn routes_are_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for (m, p) in ROUTES {
            assert!(seen.insert((m, p)), "duplicate route {m} {p}");
            assert!(p.starts_with('/') && !p.ends_with('/'), "{p}");
            assert!(matches!(*m, "GET" | "POST" | "PUT" | "DELETE"), "{m}");
        }
    }
}
