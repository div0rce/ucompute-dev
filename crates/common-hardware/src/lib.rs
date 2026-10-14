//! Hardware and backend detection (F-01-01).
//!
//! Contract: `probe()` must complete in < 2 s, run unprivileged, and report values that match the
//! OS tools (`sysctl` / `system_profiler` on macOS, `/proc` + `lspci` on Linux).
//!
//! Platform work lives in `macos.rs` and `linux.rs`; every parser there is a pure `&str` function
//! so it is unit-tested with fixture text on any host. Windows gets only what `std` provides.

use common_core::{HostSummary, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

mod linux;
mod macos;

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
    /// Detected on this host *and* shipped by the pinned engine build (ADR-0002). This is the only
    /// value `ModelManager::default_device` and the CLI act on, so a loader on PATH is not enough.
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

impl HardwareReport {
    /// The host fields an `ExecutionManifest` records.
    pub fn host_summary(&self) -> HostSummary {
        self.host.clone()
    }
}

impl From<HardwareReport> for HostSummary {
    fn from(r: HardwareReport) -> Self {
        r.host
    }
}

impl From<&HardwareReport> for HostSummary {
    fn from(r: &HardwareReport) -> Self {
        r.host.clone()
    }
}

#[cfg(target_os = "linux")]
use linux::probe as platform_probe;
#[cfg(target_os = "macos")]
use macos::probe as platform_probe;

/// Windows and anything else: only what `std` knows.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn platform_probe() -> Result<HardwareReport> {
    // ponytail: std cannot see physical cores, memory or GPUs here; add a WMI/sysinfo path when
    // Windows becomes a supported target. Unknown strings stay empty, unknown numbers stay 0.
    let n = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    Ok(HardwareReport {
        cpu: CpuInfo {
            physical_cores: n,
            logical_cores: n,
            ..Default::default()
        },
        ..Default::default()
    })
}

/// Probe the local machine. Never requires privileges.
pub fn probe() -> Result<HardwareReport> {
    let start = Instant::now();
    let mut r = platform_probe()?;
    r.host.os = std::env::consts::OS.to_string();
    r.host.arch = std::env::consts::ARCH.to_string();
    r.host.memory_bytes = r.memory.total_bytes;
    r.cpu.arch = r.host.arch.clone();

    // nvidia-smi is authoritative for NVIDIA cards (it only answers when a driver is loaded);
    // lspci-only NVIDIA entries stay listed when it is absent so the user sees the card.
    let nvidia = nvidia_gpus();
    if !nvidia.is_empty() {
        r.gpus.retain(|g| g.vendor != "NVIDIA");
    }
    r.gpus.extend(nvidia.iter().cloned());

    let metal_reason = if cfg!(target_os = "macos") {
        "no Apple Silicon GPU detected"
    } else {
        "Metal is only available on macOS"
    };
    r.backends = vec![
        BackendInfo {
            name: "cpu".into(),
            available: true,
            reason: None,
        },
        backend(
            "metal",
            r.gpus.iter().any(|g| g.backend == "metal"),
            ENGINE_BACKENDS,
            metal_reason,
        ),
        backend("cuda", !nvidia.is_empty(), ENGINE_BACKENDS, "no NVIDIA GPU detected"),
        backend(
            "vulkan",
            on_path("vulkaninfo"),
            ENGINE_BACKENDS,
            "vulkaninfo not found on PATH (no Vulkan loader installed)",
        ),
    ];
    r.disk_free_bytes = disk_free(&common_core::Paths::from_env().root);
    r.probe_ms = start.elapsed().as_secs_f64() * 1000.0;
    Ok(r)
}

/// Backends the pinned llama.cpp build ships (ADR-0002 §2: only macos-arm64 is pinned, CPU+Metal).
/// Mirrors `capabilities().devices` in `common_runtime::llamacpp`, which cannot be called from here
/// because common-runtime depends on this crate.
// ponytail: one engine, one pinned platform. When a second pin lands, put the device list in
// catalog/backends.json (or common-core) and read it from both crates.
const ENGINE_BACKENDS: &[&str] = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    &["cpu", "metal"]
} else {
    &["cpu"]
};

/// `available` only when the backend is detected on the host and `engine` ships it. The engine gap
/// is reported first because no host change (installing a loader, adding a card) can close it.
fn backend(name: &str, detected: bool, engine: &[&str], reason: &str) -> BackendInfo {
    let reason = if !engine.contains(&name) {
        Some(format!("the pinned llama.cpp build ships only: {}", engine.join(", ")))
    } else if !detected {
        Some(reason.into())
    } else {
        None
    };
    BackendInfo {
        name: name.into(),
        available: reason.is_none(),
        reason,
    }
}

/// Run a tool and return its trimmed stdout, or None if it is missing or exits non-zero.
fn cmd(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path)
        .any(|d| d.join(name).is_file() || (cfg!(windows) && d.join(format!("{name}.exe")).is_file()))
}

/// NVIDIA GPUs as reported by the driver. Empty when nvidia-smi is missing or reports none.
fn nvidia_gpus() -> Vec<GpuInfo> {
    // ponytail: nvidia-smi can take ~0.5 s while the driver wakes up; acceptable inside the 2 s budget.
    cmd(
        "nvidia-smi",
        &["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"],
    )
    .map(|o| parse_nvidia_smi(&o))
    .unwrap_or_default()
}

/// Parses `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`
/// (one `<name>, <MiB>` line per GPU). Lines that do not fit are skipped.
fn parse_nvidia_smi(out: &str) -> Vec<GpuInfo> {
    out.lines()
        .filter_map(|l| {
            let (name, mib) = l.rsplit_once(',')?;
            let mib: u64 = mib.trim().parse().ok()?;
            Some(GpuInfo {
                name: name.trim().to_string(),
                vendor: "NVIDIA".into(),
                backend: "cuda".into(),
                memory_bytes: mib.checked_mul(1024 * 1024),
                cores: None,
            })
        })
        .collect()
}

/// Free bytes on the volume holding `root` (or its nearest existing ancestor), via `df -Pk`.
fn disk_free(root: &Path) -> Option<u64> {
    // ponytail: shelling out to df avoids a libc dep and unsafe statvfs; swap in when df parsing bites.
    root.ancestors()
        .filter(|p| p.exists())
        .find_map(|p| cmd("/bin/df", &["-Pk", &p.to_string_lossy()]).and_then(|o| parse_df(&o)))
}

/// POSIX `df -P` output: header line, then one line whose 4th column is available 1K-blocks.
fn parse_df(out: &str) -> Option<u64> {
    out.lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse::<u64>()
        .ok()?
        .checked_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_json_roundtrip() {
        let r = HardwareReport {
            host: HostSummary {
                os: "macos".into(),
                os_version: "26.2".into(),
                arch: "aarch64".into(),
                chip: "Apple M2".into(),
                memory_bytes: 8 << 30,
            },
            cpu: CpuInfo {
                model: "Apple M2".into(),
                physical_cores: 8,
                logical_cores: 8,
                arch: "aarch64".into(),
                features: vec!["neon".into()],
            },
            memory: MemoryInfo {
                total_bytes: 8 << 30,
                available_bytes: 1 << 30,
                unified: true,
            },
            gpus: vec![GpuInfo {
                name: "Apple M2".into(),
                vendor: "Apple".into(),
                backend: "metal".into(),
                memory_bytes: Some(8 << 30),
                cores: Some(10),
            }],
            backends: vec![
                backend("cpu", true, &["cpu"], ""),
                backend("cuda", false, &["cpu", "cuda"], "no NVIDIA GPU detected"),
            ],
            disk_free_bytes: Some(14 << 30),
            probe_ms: 12.5,
        };
        let s = serde_json::to_string(&r).unwrap();
        let back: HardwareReport = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
        assert_eq!(HostSummary::from(&r), r.host);
        assert_eq!(r.host_summary(), r.host);
        assert_eq!(HostSummary::from(r.clone()), r.host);
    }

    #[test]
    fn backend_needs_detection_and_engine_support() {
        let engine = &["cpu", "metal"];
        assert_eq!(
            backend("metal", true, engine, "x"),
            BackendInfo {
                name: "metal".into(),
                available: true,
                reason: None
            }
        );
        assert_eq!(backend("metal", false, engine, "x").reason.as_deref(), Some("x"));
        // F-01-01: a Vulkan loader on PATH (vulkaninfo) does not make Vulkan usable by a CPU+Metal build.
        let vk = backend("vulkan", true, engine, "x");
        assert!(!vk.available);
        assert_eq!(
            vk.reason.as_deref(),
            Some("the pinned llama.cpp build ships only: cpu, metal")
        );
        assert_eq!(
            backend("vulkan", false, engine, "x").reason,
            vk.reason,
            "engine gap outranks detection"
        );
    }

    #[test]
    fn parses_nvidia_smi_lines() {
        let g =
            parse_nvidia_smi("NVIDIA GeForce RTX 3090, 24576\nNVIDIA A100-SXM4-40GB, 40960\nNo devices were found\n");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "NVIDIA GeForce RTX 3090");
        assert_eq!(g[0].memory_bytes, Some(24576 << 20));
        assert_eq!((g[1].vendor.as_str(), g[1].backend.as_str()), ("NVIDIA", "cuda"));
        assert!(parse_nvidia_smi("").is_empty());
    }

    #[test]
    fn parses_df_posix_format() {
        let mac = "Filesystem   1024-blocks      Used Available Capacity  Mounted on\n/dev/disk3s5   239362496 193215528  14520408    94%    /System/Volumes/Data\n";
        assert_eq!(parse_df(mac), Some(14520408 * 1024));
        let linux = "Filesystem     1024-blocks     Used Available Capacity Mounted on\n/dev/mapper/ubuntu--vg-ubuntu--lv 100000000 40000000  60000000      40% /\n";
        assert_eq!(parse_df(linux), Some(60000000 * 1024));
        assert_eq!(parse_df("df: /x: No such file or directory"), None);
        assert_eq!(parse_df(""), None);
    }

    #[test]
    fn disk_free_walks_up_to_existing_ancestor() {
        let cwd = std::env::current_dir().unwrap();
        assert!(disk_free(&cwd.join("does").join("not").join("exist")).unwrap_or(0) > 0 || cfg!(windows));
    }

    /// Acceptance for F-01-01 on the development Mac: values match sysctl, probe is fast, unprivileged.
    #[cfg(target_os = "macos")]
    #[test]
    fn matches_sysctl_on_this_mac() {
        let wall = Instant::now();
        let r = probe().unwrap();
        assert!(wall.elapsed().as_millis() < 2000, "probe took {:?}", wall.elapsed());
        eprintln!("{}", serde_json::to_string_pretty(&r).unwrap()); // evidence; shown with --nocapture
        assert!(r.probe_ms < 2000.0, "probe_ms {}", r.probe_ms);

        let sysctl = |k: &str| cmd("/usr/sbin/sysctl", &["-n", k]).unwrap();
        assert_eq!(r.cpu.physical_cores.to_string(), sysctl("hw.physicalcpu"));
        assert_eq!(r.cpu.logical_cores.to_string(), sysctl("hw.logicalcpu"));
        assert_eq!(r.memory.total_bytes.to_string(), sysctl("hw.memsize"));
        assert_eq!(r.host.memory_bytes, r.memory.total_bytes);
        assert_eq!(r.host.chip, sysctl("machdep.cpu.brand_string"));
        assert_eq!(r.cpu.model, r.host.chip);
        assert_eq!(r.host.os, "macos");
        assert_eq!(r.host.arch, std::env::consts::ARCH);
        assert_eq!(
            r.host.os_version,
            cmd("/usr/bin/sw_vers", &["-productVersion"]).unwrap()
        );
        assert!(r.memory.available_bytes > 0 && r.memory.available_bytes < r.memory.total_bytes);
        assert!(r.disk_free_bytes.unwrap() > 0);

        let avail = |n: &str| r.backends.iter().find(|b| b.name == n).unwrap().available;
        assert!(avail("cpu"));
        assert_eq!(
            r.backends.iter().map(|b| b.name.as_str()).collect::<Vec<_>>(),
            ["cpu", "metal", "cuda", "vulkan"]
        );
        assert!(
            !avail("cuda") && !avail("vulkan"),
            "the macos pin ships CPU+Metal only, whatever is on PATH"
        );
        if cfg!(target_arch = "aarch64") {
            assert!(avail("metal"));
            assert!(r.memory.unified);
            assert!(r.cpu.features.iter().any(|f| f == "neon"));
            assert_eq!(r.gpus.len(), 1);
            assert_eq!(
                (r.gpus[0].vendor.as_str(), r.gpus[0].backend.as_str()),
                ("Apple", "metal")
            );
            assert_eq!(r.gpus[0].name, r.host.chip);
            assert_eq!(r.gpus[0].memory_bytes, Some(r.memory.total_bytes));
            assert!(r.gpus[0].cores.unwrap() > 0, "ioreg gpu-core-count");
        }
    }
}
