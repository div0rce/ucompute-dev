//! WP-04 C6: the `split:<peer>` target (ADR-0005 D2, D5, D7, D8). One model's blocks split between
//! this device (the head) and one paired device (the stage host), with llama.cpp RPC carried only
//! inside the authenticated connection (`common_network::rpc`).
//!
//! Order of checks (`openai::run`, then [`ensure`]): the policy for pooled inference with that peer
//! (before admission, any load or any network activity), `split_at`, the acknowledgement of
//! [`disclosure`], the licence gate, the model's KV shape and the stage estimate, the connection,
//! the session, and only then the split engine's load, which uploads the stage's weights.
//!
//! One session at a time ([`Active`] in `AppState::split`). It ends on unload, on any Deny or loss
//! (`rpc`'s 1 s tick: privacy `local_only`, networking off, the group's pooling off, a hold or a
//! revocation) or 10 s after the split engine stopped using it (a local load replaced it). Its end
//! stops the split engine and fails its requests with the session's reason; nothing falls back to
//! a local run (D7).

use crate::AppState;
use common_core::{CommonError, ErrorCode, Result};
use common_identity::PeerId;
use common_models::{KvType, StageRange};
use common_network::rpc::{self, SplitSession, SplitSpec};
use common_policy::PrivacyMode;
use common_runtime::manager::LoadOptions;
use common_runtime::{EnginePid, LoadedModel, Split, SplitStage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;

/// Model licences whose weights may be uploaded to another device (X-08).
pub const LICENCES: [&str; 2] = ["Apache-2.0", "MIT"];

/// ADR-0005 D8, shown before the first session (`acknowledged` accepts it).
pub fn disclosure(peer: &str) -> String {
    format!(
        "Pooling splits one model between this device and {peer}. {peer} receives the model weights for its part and, \
for every token, intermediate data from which your prompt and the reply can be reconstructed. The connection is \
encrypted; the other device is not. Pooling does not make a request more private than running it on {peer}, and it \
is usually slower than running a model that fits here."
    )
}

/// How the model is split (ADR-0005 D2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitMode {
    /// Configuration P: the stage holds blocks `[0, split_at)`, this device's GPU the rest and the
    /// output layer, so no logits row crosses the link.
    #[default]
    Pooled,
    /// Configuration C: this device's CPU holds `[0, split_at)`, the stage the rest and the output
    /// layer (one logits row per token crosses the link). Same backend when the stage runs on its
    /// CPU, as the Linux stage host does.
    SameBackend,
}

/// The request fields of a split target (Common extension).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SplitParams {
    /// First block of the second stage (`0 < split_at < block_count`), chosen by the caller.
    pub split_at: Option<u32>,
    pub split_mode: Option<SplitMode>,
    /// The caller accepted [`disclosure`].
    #[serde(default)]
    pub acknowledged: bool,
}

/// The open session and the split it serves.
pub struct Active {
    pub peer: PeerId,
    pub model_id: String,
    pub split_at: u32,
    pub mode: SplitMode,
    pub session: Arc<SplitSession>,
    pub split: Split,
}

impl Active {
    /// `/api/status` `split`: ids, numbers and the end reason, never content.
    pub fn status(&self) -> Value {
        let c = self.session.counters();
        json!({
            "peer": self.peer.to_hex(),
            "session": self.session.session_id(),
            "model_id": self.model_id,
            "split_at": self.split_at,
            "split_mode": self.mode,
            "stages": self.split.stages,
            "stage_device": self.session.accepted().device,
            "to_r": c.to_r,
            "to_h": c.to_h,
            "switches": c.switches,
            "ended": self.session.failure().map(|e| e.code.as_str()),
        })
    }
}

/// Refused unless the licence allows uploading the weights, or `peer_addr` is a SINGLE-HOST test
/// peer (`COMMON_TEST_SINGLE_HOST_POOL=1` and loopback: `rpc::single_host_pool_test`).
pub fn licence_gate(license: &str, peer_addr: Option<&SocketAddr>) -> Result<()> {
    if LICENCES.contains(&license) || peer_addr.is_some_and(rpc::single_host_pool_test) {
        return Ok(());
    }
    Err(CommonError::new(
        ErrorCode::PolicyDenied,
        format!(
            "pooling uploads this model's weights to the other device, and its licence ({license}) is not one that allows that ({})",
            LICENCES.join(" or ")
        ),
    ))
}

/// The llama.cpp device of this host's GPU in the pinned build, if it has one.
fn local_gpu(hw: &common_hardware::HardwareReport) -> Option<&'static str> {
    let has = |n: &str| hw.backends.iter().any(|b| b.name == n && b.available);
    if cfg!(target_os = "macos") && has("metal") {
        Some("MTL0")
    } else if cfg!(windows) && has("vulkan") {
        Some("Vulkan0")
    } else {
        None
    }
}

fn stage(device: &str, first_block: u32, end_block: u32) -> SplitStage {
    SplitStage {
        device: device.into(),
        first_block,
        end_block,
    }
}

/// The stages for `split_at` of `n_layer` blocks and the remote stage's share. `gpu` is this
/// device's GPU, which configuration P needs.
pub fn plan(n_layer: u32, split_at: u32, mode: SplitMode, gpu: Option<&str>) -> Result<(Vec<SplitStage>, StageRange)> {
    if split_at == 0 || split_at >= n_layer {
        return Err(CommonError::invalid(format!(
            "split_at must be between 1 and {} for this model ({n_layer} blocks)",
            n_layer.saturating_sub(1)
        )));
    }
    let remote = |first, end, output| StageRange {
        first,
        end,
        input: false,
        output,
    };
    Ok(match mode {
        SplitMode::Pooled => {
            let gpu = gpu.ok_or_else(|| {
                CommonError::new(
                    ErrorCode::GpuUnavailable,
                    "split_mode pooled keeps this device's part on its GPU, and this device has none",
                )
                .with_hint("use \"split_mode\": \"same_backend\"")
            })?;
            (
                vec![stage("RPC0", 0, split_at), stage(gpu, split_at, n_layer)],
                remote(0, split_at, false),
            )
        }
        SplitMode::SameBackend => (
            vec![
                stage(common_runtime::llamacpp::split::CPU_STAGE, 0, split_at),
                stage("RPC0", split_at, n_layer),
            ],
            remote(split_at, n_layer, true),
        ),
    })
}

/// The pinned llama.cpp release on this platform; a stage host refuses any other.
pub fn engine_release() -> Result<String> {
    common_artifacts::backend::BackendPins::builtin()?
        .for_current_platform("llama.cpp")
        .map(|p| p.release.clone())
        .ok_or_else(|| CommonError::new(ErrorCode::BackendUnavailable, "no pinned llama.cpp for this platform"))
}

/// End the open session, if any; its watcher stops the split engine.
pub fn close(st: &AppState) {
    if let Some(a) = st.split_slot().take() {
        a.session.close();
    }
}

/// The split engine for `model_id` with `peer`: the open session when it serves the same split,
/// else a new one. Called after `evaluate` allowed pooled inference with `peer` and after
/// admission. Returns the loaded model and its session.
pub async fn ensure(
    st: &Arc<AppState>,
    model_id: &str,
    peer: PeerId,
    params: &SplitParams,
) -> Result<(Arc<dyn LoadedModel>, Arc<SplitSession>)> {
    let split_at = params.split_at.ok_or_else(|| {
        CommonError::invalid("a split target needs \"split_at\": the first block of the second stage")
    })?;
    if !params.acknowledged {
        return Err(
            CommonError::new(ErrorCode::PolicyDenied, disclosure(&peer.short_label()))
                .with_hint("resend with \"acknowledged\": true to accept this"),
        );
    }
    let mode = params.split_mode.unwrap_or_default();
    if !common_core::paths::safe_name(model_id) {
        return Err(CommonError::invalid(
            "model id may only contain letters, digits, '-', '_' and '.'",
        ));
    }
    // the head loads the file itself: it must be here, and the stage gets only its part
    st.manager.resolve(model_id)?; // the typed reason: not in the catalog, or not downloaded
    let (manifest, blob) = st.store.resolve(model_id)?;
    // before any connection, unless only the SINGLE-HOST test flag could lift it (checked below)
    let licence_after_connect = match licence_gate(&manifest.license, None) {
        Ok(()) => false,
        Err(_) if std::env::var_os(rpc::TEST_POOL_ENV).is_some_and(|v| v == "1") => true,
        Err(e) => return Err(e),
    };
    let meta = st.metadata(&manifest.sha256, &blob)?;
    meta.uniform_kv().map_err(CommonError::invalid)?;
    let (stages, remote) = plan(meta.block_count, split_at, mode, local_gpu(&st.manager.hardware))?;
    let size = std::fs::metadata(&blob)?.len();
    let n_ctx = meta.context_length.min(4096);
    let est_bytes = common_models::estimate_stage(&meta, size, n_ctx, KvType::F16, remote).total_bytes;

    let reuse = st.split_slot().as_ref().and_then(|a| {
        (a.peer == peer
            && a.model_id == model_id
            && a.split_at == split_at
            && a.mode == mode
            && a.session.failure().is_none())
        .then(|| (a.session.clone(), a.split.clone()))
    });
    let (session, split) = match reuse {
        Some(r) => r,
        None => {
            let busy = st
                .manager
                .snapshot()
                .loaded
                .is_some_and(|v| v.in_flight > 0 && v.info.split.is_some());
            if busy {
                return Err(CommonError::new(
                    ErrorCode::Overloaded,
                    "another split of this device is still serving requests; retry when they finish",
                ));
            }
            let replaced = st.split_slot().is_some();
            close(st);
            let node = st.node().await?;
            let conn = st.peer_connection(&node, peer).await?;
            if licence_after_connect {
                licence_gate(&manifest.license, Some(&conn.remote_addr))?;
            }
            let engine = EnginePid::default();
            let spec = SplitSpec {
                model_sha256: manifest.sha256.clone(),
                engine_release: engine_release()?,
                layer_start: remote.first,
                layer_end: remote.end,
                n_layer: meta.block_count,
                est_bytes,
                // read on every tick: `local_only` set mid-session ends it (D5); gone means deny
                privacy: {
                    let st = Arc::downgrade(st);
                    rpc::PrivacySource::new(move || {
                        st.upgrade().map_or(PrivacyMode::LocalOnly, |s| s.settings().privacy)
                    })
                },
                engine: engine.clone(),
                byte_cap: None,
                limits: Default::default(),
            };
            // the stage frees its one session slot a moment after the session it replaces ends
            let mut tries = if replaced { 20 } else { 1 };
            let session = loop {
                tries -= 1;
                match SplitSession::open(&node, conn.clone(), spec.clone()).await {
                    Err(e) if e.code == ErrorCode::Overloaded && tries > 0 => {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await
                    }
                    r => break Arc::new(r?),
                }
            };
            let split = Split {
                rpc_ports: vec![session.rpc_port()],
                stages,
                n_layer: meta.block_count,
                no_mmap: false,
                engine_pid: engine,
            };
            // D7: the session's end stops its split engine (and only it); requests fail with its reason
            let (manager, watched, ports) = (st.manager.clone(), session.clone(), split.rpc_ports.clone());
            tokio::spawn(async move {
                let reason = watched.ended().await;
                tracing::info!(code = %reason.code, "pooled session ended; stopping its split engine");
                if let Err(e) = manager.unload_split(&ports).await {
                    tracing::warn!(error = %e, "split engine not stopped");
                }
            });
            *st.split_slot() = Some(Active {
                peer,
                model_id: model_id.to_string(),
                split_at,
                mode,
                session: session.clone(),
                split: split.clone(),
            });
            (session, split)
        }
    };
    let opts = LoadOptions {
        split: Some(split),
        ..st.load_options()
    };
    match st.manager.ensure_loaded(model_id, opts).await {
        Ok(m) => Ok((m, session)),
        Err(e) => {
            // the session's own reason (peer lost, policy, cap) says more than the engine's exit
            let e = session.failure().unwrap_or(e);
            // no engine uses it: free the stage now rather than after the idle close
            session.close();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR-0005 D5 and X-08: weights go to another device only under an allowing licence; the
    /// test flag lifts that only for a loopback peer, never for a LAN address.
    #[test]
    fn licence_gate_refuses_unknown_licence_cross_device() {
        let lan: SocketAddr = "192.168.1.167:8193".parse().unwrap();
        assert!(licence_gate("Apache-2.0", Some(&lan)).is_ok());
        assert!(licence_gate("MIT", None).is_ok());
        let e = licence_gate("unknown", Some(&lan)).unwrap_err();
        assert_eq!(e.code, ErrorCode::PolicyDenied);
        assert!(e.message.contains("licence (unknown)"), "{e}");
        assert!(licence_gate("Llama-3.2", None).is_err());
    }

    /// The flag is process-wide, so this test sets it; the lib tests never rely on its absence.
    #[test]
    fn test_flag_ignored_for_non_loopback_peer() {
        std::env::set_var(rpc::TEST_POOL_ENV, "1");
        let lan: SocketAddr = "192.168.1.167:8193".parse().unwrap();
        let lo: SocketAddr = "127.0.0.1:8193".parse().unwrap();
        assert!(licence_gate("unknown", Some(&lo)).is_ok());
        assert_eq!(
            licence_gate("unknown", Some(&lan)).unwrap_err().code,
            ErrorCode::PolicyDenied
        );
        assert!(licence_gate("unknown", None).is_err());
    }

    #[test]
    fn plans_follow_adr_0005_d2() {
        let (p, r) = plan(6, 2, SplitMode::Pooled, Some("MTL0")).unwrap();
        assert_eq!(p, vec![stage("RPC0", 0, 2), stage("MTL0", 2, 6)]);
        assert_eq!((r.first, r.end, r.input, r.output), (0, 2, false, false));
        let (c, r) = plan(6, 2, SplitMode::SameBackend, None).unwrap();
        assert_eq!(c, vec![stage("CPU", 0, 2), stage("RPC0", 2, 6)]);
        assert_eq!((r.first, r.end, r.input, r.output), (2, 6, false, true));
        assert_eq!(
            plan(6, 2, SplitMode::Pooled, None).unwrap_err().code,
            ErrorCode::GpuUnavailable,
            "configuration P never falls back to the CPU"
        );
        for k in [0, 6, 7] {
            assert_eq!(
                plan(6, k, SplitMode::Pooled, Some("MTL0")).unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
    }

    #[test]
    fn disclosure_names_the_peer_and_what_it_sees() {
        let d = disclosure("ab12-cd34-ef56");
        assert!(d.contains("between this device and ab12-cd34-ef56"));
        assert!(d.contains("your prompt and the reply can be reconstructed"));
    }
}
