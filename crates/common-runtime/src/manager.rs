//! Model manager: resolves a catalog id to a verified blob, checks feasibility, plans context,
//! loads through the backend (with GPU→CPU fallback) and keeps the loaded model in a single slot.
//! Shared by the CLI (`common run`) and the API server.

use crate::types::*;
use common_core::{CommonError, ErrorCode, Paths, Result, Timeouts};
use common_hardware::HardwareReport;
use common_models::FitVerdict;
use futures_util::StreamExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub struct ModelManager {
    pub paths: Paths,
    pub store: common_artifacts::Store,
    pub backend: Arc<dyn InferenceBackend>,
    pub hardware: HardwareReport,
    /// ponytail: one loaded model at a time (8 GB dev machine). Upgrade: LRU map keyed by id.
    pub loaded: Mutex<Option<Slot>>,
}

/// What is loaded and the options it was loaded with (needed to judge "compatible opts").
pub struct Slot {
    pub model_id: String,
    pub opts: LoadOptions,
    pub model: Arc<dyn LoadedModel>,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    pub device: Option<DevicePreference>,
    pub n_ctx: Option<u32>,
    pub threads: Option<u32>,
    pub parallel_slots: Option<u32>,
    pub embeddings: bool,
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
        }
    }

    /// Return the loaded model for `model_id`, loading it first if needed. Errors:
    /// ModelNotFound (not in catalog or store), ModelNotDownloaded, IntegrityMismatch (blob bytes
    /// differ from the manifest hash), ModelFileInvalid, InsufficientMemory (with plain-language
    /// reason), ContextTooLong, BackendFailed, Overloaded (the loaded model is still in use and
    /// would have to be replaced).
    pub async fn ensure_loaded(&self, model_id: &str, opts: LoadOptions) -> Result<Arc<dyn LoadedModel>> {
        if !common_core::paths::safe_name(model_id) {
            return Err(CommonError::invalid(
                "model id may only contain letters, digits, '-', '_' and '.'",
            ));
        }
        // Held across the load: one slot, so loads are serialized and status() waits for the result.
        let mut slot = self.loaded.lock().await;
        if let Some(s) = slot.as_ref() {
            if s.model_id == model_id && compatible(&s.opts, &opts) {
                return Ok(s.model.clone());
            }
            // F-08-08: admitted requests complete. A caller still holding the model (between
            // ensure_loaded and generate) or a live stream/call never has its engine killed.
            refuse_if_busy(s, Arc::strong_count(&s.model) > 1)?;
        }
        let (path, sha256) = self.resolve(model_id)?;
        // F-05-01: the bytes handed to the engine are the bytes the manifest promised. Hashing a
        // 1 GB blob takes ~1 s on an M2, well under the engine load time, off the async threads.
        // ponytail: re-hashes on every load; cache a verified (len, mtime) per blob if it ever matters.
        let (store, id) = (self.store.clone(), model_id.to_string());
        tokio::task::spawn_blocking(move || store.verify(&id))
            .await
            .map_err(|e| CommonError::internal(format!("hash task failed: {e}")))??;
        let (req, context_note) = self.plan(model_id, path, sha256, &opts)?;
        if let Some(prev) = slot.take() {
            prev.model.unload().await?;
        }
        let model: Arc<dyn LoadedModel> = Arc::new(Managed::new(self.backend.load(req).await?, context_note));
        *slot = Some(Slot {
            model_id: model_id.into(),
            opts,
            model: model.clone(),
        });
        Ok(model)
    }

    /// Stop the loaded engine. `Overloaded` while streams or calls are still using it (F-08-08);
    /// a caller merely holding the handle (the CLI unloads before dropping it) does not block this.
    pub async fn unload(&self) -> Result<()> {
        let mut slot = self.loaded.lock().await;
        if let Some(s) = slot.as_ref() {
            refuse_if_busy(s, false)?;
        }
        if let Some(prev) = slot.take() {
            prev.model.unload().await?;
        }
        Ok(())
    }

    /// Currently loaded model info, if any.
    pub async fn status(&self) -> Option<LoadedModelInfo> {
        self.loaded.lock().await.as_ref().map(|s| s.model.info().clone())
    }

    /// Blob path + sha256 for `id`, or the typed reason it cannot be used.
    fn resolve(&self, id: &str) -> Result<(PathBuf, String)> {
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
        path: PathBuf,
        sha256: String,
        opts: &LoadOptions,
    ) -> Result<(LoadRequest, Option<String>)> {
        let meta = common_models::gguf::read_metadata(&path)?;
        // ponytail: uses the stored probe; re-probe before loading once probe() is known to be cheap.
        let available = self.hardware.memory.available_bytes;
        let requested = opts.n_ctx.unwrap_or(meta.context_length.min(4096));
        let budget = (available as f64 * 0.9) as u64;
        let size = file_size(&path)?;
        let plan = common_models::planning::plan_context(&meta, size, Some(requested), budget, true)?;
        if let Some(reason) = &plan.reason {
            tracing::warn!(
                model = id,
                n_ctx = plan.n_ctx,
                kv = plan.kv_type.as_llama_arg(),
                "{reason}"
            );
        }
        let est = common_models::feasibility::estimate(&meta, size, plan.n_ctx, plan.kv_type);
        if let FitVerdict::TooLarge { reason, .. } = common_models::feasibility::check_fit(&est, available, 0.10) {
            return Err(CommonError::new(ErrorCode::InsufficientMemory, reason));
        }
        let req = LoadRequest {
            model_id: id.into(),
            model_path: path,
            model_sha256: sha256,
            device: opts.device.unwrap_or_else(|| self.default_device()),
            n_ctx: plan.n_ctx,
            n_gpu_layers: None,
            threads: opts.threads,
            parallel_slots: opts.parallel_slots.unwrap_or(1),
            kv_type: plan.kv_type,
            embeddings: opts.embeddings,
            timeouts: Timeouts::default(),
            extra_args: vec![],
        };
        Ok((req, plan.reason))
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
        let stream = self.inner.generate(req, cancel).await?;
        // The guard lives inside the stream: dropping the stream releases the model.
        Ok(stream
            .map(move |ev| {
                let _keep = &guard;
                ev
            })
            .boxed())
    }

    async fn tokenize(&self, text: &str) -> Result<Vec<u32>> {
        let _guard = self.in_use.clone();
        self.inner.tokenize(text).await
    }

    async fn detokenize(&self, ids: &[u32]) -> Result<String> {
        let _guard = self.in_use.clone();
        self.inner.detokenize(ids).await
    }

    async fn embed(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let _guard = self.in_use.clone();
        self.inner.embed(inputs).await
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

    /// FAKE in-test model (no engine): a stream from it never yields, so it stays in flight until dropped.
    struct Idle(LoadedModelInfo);

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
            Ok(())
        }
    }

    fn managed(note: Option<&str>) -> Arc<dyn LoadedModel> {
        Arc::new(Managed::new(
            Arc::new(Idle(LoadedModelInfo {
                model_id: "a".into(),
                ..Default::default()
            })),
            note.map(str::to_string),
        ))
    }

    /// F-08-08 / F-01-08: the planner's note reaches `info()`, and a model with work in flight is
    /// neither replaced nor unloaded.
    #[tokio::test]
    async fn busy_model_is_never_replaced_or_unloaded() {
        let m = manager(HardwareReport::default());
        *m.loaded.lock().await = Some(Slot {
            model_id: "a".into(),
            opts: LoadOptions::default(),
            model: managed(Some("capped to 128")),
        });
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

        *m.loaded.lock().await = Some(Slot {
            model_id: "a".into(),
            opts: LoadOptions::default(),
            model: managed(None),
        });
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
