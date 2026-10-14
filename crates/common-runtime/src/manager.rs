//! Model manager: resolves a catalog id to a verified blob, checks feasibility, plans context,
//! loads through the backend (with GPU→CPU fallback) and keeps the loaded model in a single slot.
//! Shared by the CLI (`common run`) and the API server.

use crate::types::*;
use common_core::{CommonError, ErrorCode, Paths, Result, Timeouts};
use common_hardware::HardwareReport;
use common_models::FitVerdict;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

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
    pub fn new(paths: Paths, store: common_artifacts::Store, backend: Arc<dyn InferenceBackend>, hardware: HardwareReport) -> Self {
        Self { paths, store, backend, hardware, loaded: Mutex::new(None) }
    }

    /// Return the loaded model for `model_id`, loading it first if needed. Errors:
    /// ModelNotFound (not in catalog or store), ModelNotDownloaded, ModelFileInvalid,
    /// InsufficientMemory (with plain-language reason), ContextTooLong, BackendFailed.
    pub async fn ensure_loaded(&self, model_id: &str, opts: LoadOptions) -> Result<Arc<dyn LoadedModel>> {
        if !common_core::paths::safe_name(model_id) {
            return Err(CommonError::invalid("model id may only contain letters, digits, '-', '_' and '.'"));
        }
        // Held across the load: one slot, so loads are serialized and status() waits for the result.
        let mut slot = self.loaded.lock().await;
        if let Some(s) = slot.as_ref() {
            if s.model_id == model_id && compatible(&s.opts, &opts) {
                return Ok(s.model.clone());
            }
        }
        let (path, sha256) = self.resolve(model_id)?;
        let req = self.plan(model_id, path, sha256, &opts)?;
        if let Some(prev) = slot.take() {
            prev.model.unload().await?;
        }
        let model = self.backend.load(req).await?;
        *slot = Some(Slot { model_id: model_id.into(), opts, model: model.clone() });
        Ok(model)
    }

    pub async fn unload(&self) -> Result<()> {
        if let Some(prev) = self.loaded.lock().await.take() {
            prev.model.unload().await?;
        }
        Ok(())
    }

    /// Currently loaded model info, if any.
    pub async fn status(&self) -> Option<LoadedModelInfo> {
        self.loaded.lock().await.as_ref().map(|s| s.model.info().clone())
    }

    /// Blob path + sha256 for `id`, or the typed reason it cannot be used.
    /// ponytail: trusts the manifest hash written by the verified downloader; `common models verify` re-hashes.
    fn resolve(&self, id: &str) -> Result<(PathBuf, String)> {
        match self.store.resolve(id) {
            Ok((m, path)) => Ok((path, m.sha256)),
            Err(e) if e.code == ErrorCode::ModelNotFound => match common_artifacts::Catalog::builtin()?.get(id) {
                Some(_) => Err(CommonError::new(ErrorCode::ModelNotDownloaded, format!("{id} is in the catalog but has not been downloaded")).with_hint(format!("common models pull {id}"))),
                None => Err(CommonError::new(ErrorCode::ModelNotFound, format!("no model named {id} in the catalog or the local store")).with_hint("common models list")),
            },
            Err(e) => Err(e),
        }
    }

    fn plan(&self, id: &str, path: PathBuf, sha256: String, opts: &LoadOptions) -> Result<LoadRequest> {
        let meta = common_models::gguf::read_metadata(&path)?;
        // ponytail: uses the stored probe; re-probe before loading once probe() is known to be cheap.
        let available = self.hardware.memory.available_bytes;
        let requested = opts.n_ctx.unwrap_or(meta.context_length.min(4096));
        let budget = (available as f64 * 0.9) as u64;
        let size = file_size(&path)?;
        let plan = common_models::planning::plan_context(&meta, size, Some(requested), budget, true)?;
        let est = common_models::feasibility::estimate(&meta, size, plan.n_ctx, plan.kv_type);
        if let FitVerdict::TooLarge { reason, .. } = common_models::feasibility::check_fit(&est, available, 0.10) {
            return Err(CommonError::new(ErrorCode::InsufficientMemory, reason));
        }
        Ok(LoadRequest {
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
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use common_hardware::BackendInfo;

    #[test]
    fn compatibility_rules() {
        let loaded = LoadOptions { device: Some(DevicePreference::Cpu), n_ctx: Some(2048), threads: None, parallel_slots: None, embeddings: false };
        assert!(compatible(&loaded, &LoadOptions::default()), "no pins = compatible");
        assert!(compatible(&loaded, &loaded.clone()));
        assert!(compatible(&loaded, &LoadOptions { n_ctx: Some(2048), device: Some(DevicePreference::Cpu), ..Default::default() }));
        assert!(!compatible(&loaded, &LoadOptions { n_ctx: Some(4096), ..Default::default() }));
        assert!(!compatible(&loaded, &LoadOptions { device: Some(DevicePreference::Gpu), ..Default::default() }));
        assert!(!compatible(&loaded, &LoadOptions { embeddings: true, ..Default::default() }));
        assert!(!compatible(&loaded, &LoadOptions { threads: Some(8), ..Default::default() }), "a pin never matches an unpinned default");
        assert!(!compatible(&LoadOptions::default(), &LoadOptions { embeddings: true, ..Default::default() }));
    }

    fn manager(hardware: HardwareReport) -> ModelManager {
        let paths = Paths::at(std::env::temp_dir().join(format!("common-mgr-{}", uuid::Uuid::new_v4())));
        let backend = crate::backend_by_name("llama.cpp", paths.backends(), paths.clone()).unwrap();
        ModelManager::new(paths.clone(), common_artifacts::Store::new(paths), backend, hardware)
    }

    #[test]
    fn default_device_follows_probe() {
        assert_eq!(manager(HardwareReport::default()).default_device(), DevicePreference::Auto, "unknown probe: let the adapter find out");
        let cpu_only = HardwareReport { backends: vec![BackendInfo { name: "cpu".into(), available: true, reason: None }], ..Default::default() };
        assert_eq!(manager(cpu_only).default_device(), DevicePreference::Cpu);
        let metal = HardwareReport {
            backends: vec![BackendInfo { name: "cpu".into(), available: true, reason: None }, BackendInfo { name: "metal".into(), available: true, reason: None }],
            ..Default::default()
        };
        assert_eq!(manager(metal).default_device(), DevicePreference::Auto);
    }

    #[tokio::test]
    async fn bad_ids_and_empty_state() {
        let m = manager(HardwareReport::default());
        assert_eq!(m.ensure_loaded("../etc/passwd", LoadOptions::default()).await.err().unwrap().code, ErrorCode::InvalidRequest);
        assert!(m.status().await.is_none());
        m.unload().await.unwrap();
    }

    #[tokio::test]
    async fn unknown_and_undownloaded_ids_are_typed() {
        let m = manager(HardwareReport::default());
        let e = m.ensure_loaded("no-such-model-xyz", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::ModelNotFound, "{e}");
        let e = m.ensure_loaded("stories15M-q4_0", LoadOptions::default()).await.err().unwrap();
        assert_eq!(e.code, ErrorCode::ModelNotDownloaded, "{e}");
        assert_eq!(e.hint.as_deref(), Some("common models pull stories15M-q4_0"));
    }
}
