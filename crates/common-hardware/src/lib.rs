//! Hardware and backend detection (F-01-01).
//!
//! Contract: `probe()` must complete in < 2 s, run unprivileged, and report values that match the
//! OS tools (`sysctl` / `system_profiler` on macOS, `/proc` + `lspci` on Linux).
//! STUB — implementation pending.

use common_core::{CommonError, ErrorCode, HostSummary, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CpuInfo {
    pub model: String,
    pub physical_cores: u32,
    pub logical_cores: u32,
    pub arch: String,
    /// ISA features relevant to inference (e.g. `neon`, `avx2`, `avx512`, `amx`).
    pub features: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    /// Memory that can be used right now without swapping (best effort).
    pub available_bytes: u64,
    /// True on Apple Silicon and other unified-memory systems: GPU shares `total_bytes`.
    pub unified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    /// Which llama.cpp backend drives it: `metal`, `cuda`, `vulkan`, `rocm`.
    pub backend: String,
    pub memory_bytes: Option<u64>,
    pub cores: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BackendInfo {
    /// `cpu`, `metal`, `cuda`, `vulkan`.
    pub name: String,
    pub available: bool,
    /// Plain-language reason when not available.
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct HardwareReport {
    pub host: HostSummary,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpus: Vec<GpuInfo>,
    pub backends: Vec<BackendInfo>,
    pub disk_free_bytes: Option<u64>,
    pub probe_ms: f64,
}

/// Probe the local machine. Never requires privileges.
pub fn probe() -> Result<HardwareReport> {
    Err(CommonError::new(ErrorCode::NotImplemented, "hardware probe not implemented"))
}
