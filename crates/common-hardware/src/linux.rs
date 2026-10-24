//! Linux probe: `/proc/cpuinfo`, `/proc/meminfo`, `/etc/os-release`, `lspci`. All parsers are pure.
//! UNVERIFIED on real Linux hardware: only the fixture tests below have run (no Linux host, B-004).
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use crate::{CpuInfo, GpuInfo};

/// `/proc/cpuinfo` flag -> reported feature name (x86 `flags` and arm64 `Features` lines).
const FEATURES: &[(&str, &str)] = &[
    ("asimd", "neon"),
    ("asimdhp", "fp16"),
    ("asimddp", "dotprod"),
    ("i8mm", "i8mm"),
    ("bf16", "bf16"),
    ("sve", "sve"),
    ("sme", "sme"),
    ("avx", "avx"),
    ("avx2", "avx2"),
    ("avx512f", "avx512f"),
    ("avx512_vnni", "avx512_vnni"),
    ("fma", "fma"),
    ("f16c", "f16c"),
    ("amx_tile", "amx"),
];

/// Value of the first `key<tab>: value` line in `/proc/cpuinfo`.
fn field<'a>(cpuinfo: &'a str, key: &str) -> Option<&'a str> {
    cpuinfo.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        (k.trim() == key).then(|| v.trim())
    })
}

/// Physical cores = distinct (physical id, core id) pairs; arm64 lacks both, so logical count is used.
fn parse_cpuinfo(cpuinfo: &str) -> CpuInfo {
    let logical = cpuinfo.lines().filter(|l| l.starts_with("processor")).count() as u32;
    let mut cores = std::collections::HashSet::new();
    let (mut pkg, mut core) = (None, None);
    for l in cpuinfo.lines() {
        match l.split_once(':').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("physical id", v)) => pkg = Some(v),
            Some(("core id", v)) => core = Some(v),
            _ => {}
        }
        if let (Some(p), Some(c)) = (pkg, core) {
            cores.insert((p, c));
            (pkg, core) = (None, None);
        }
    }
    let flags = field(cpuinfo, "flags")
        .or_else(|| field(cpuinfo, "Features"))
        .unwrap_or("");
    let have: std::collections::HashSet<&str> = flags.split_whitespace().collect();
    CpuInfo {
        model: field(cpuinfo, "model name")
            .or_else(|| field(cpuinfo, "Model"))
            .or_else(|| field(cpuinfo, "Hardware"))
            .unwrap_or("")
            .to_string(),
        physical_cores: if cores.is_empty() { logical } else { cores.len() as u32 },
        logical_cores: logical,
        features: FEATURES
            .iter()
            .filter(|(f, _)| have.contains(f))
            .map(|(_, n)| n.to_string())
            .collect(),
        ..Default::default()
    }
}

/// `(MemTotal, MemAvailable)` in bytes from `/proc/meminfo` (values are in kB).
fn parse_meminfo(meminfo: &str) -> Option<(u64, u64)> {
    let kb = |key: &str| -> Option<u64> {
        field(meminfo, key)?
            .split_whitespace()
            .next()?
            .parse::<u64>()
            .ok()?
            .checked_mul(1024)
    };
    Some((kb("MemTotal")?, kb("MemAvailable")?))
}

/// `PRETTY_NAME` (or `VERSION_ID`) from `/etc/os-release`, quotes stripped.
fn parse_os_release(s: &str) -> Option<String> {
    let get = |key: &str| {
        s.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().trim_matches('"').to_string())
    };
    get("PRETTY_NAME")
        .or_else(|| get("VERSION_ID"))
        .filter(|v| !v.is_empty())
}

/// GPU lines from `lspci` (VGA, 3D and Display controllers). Vendor from the description.
fn parse_lspci(out: &str) -> Vec<GpuInfo> {
    out.lines()
        .filter_map(|l| {
            let (addr_class, desc) = l.split_once(": ")?;
            let class = addr_class.split_once(' ')?.1;
            if !(class.contains("VGA") || class.contains("3D controller") || class.contains("Display controller")) {
                return None;
            }
            let (vendor, backend) = if desc.contains("NVIDIA") {
                ("NVIDIA", "cuda")
            } else if desc.contains("AMD") || desc.contains("Advanced Micro Devices") {
                ("AMD", "rocm")
            } else if desc.contains("Intel") {
                ("Intel", "vulkan")
            } else {
                (desc.split_whitespace().next().unwrap_or(""), "vulkan")
            };
            Some(GpuInfo {
                name: desc.trim().to_string(),
                vendor: vendor.into(),
                backend: backend.into(),
                memory_bytes: None,
                cores: None,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
pub(crate) fn available_memory() -> Option<u64> {
    parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?).map(|(_, a)| a)
}

#[cfg(target_os = "linux")]
pub(crate) fn probe() -> common_core::Result<crate::HardwareReport> {
    use crate::{cmd, HardwareReport, MemoryInfo};
    use common_core::{CommonError, HostSummary};

    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| CommonError::io(format!("{p}: {e}")));
    let cpu = parse_cpuinfo(&read("/proc/cpuinfo")?);
    let (total, available) = parse_meminfo(&read("/proc/meminfo")?)
        .ok_or_else(|| CommonError::io("/proc/meminfo: MemTotal/MemAvailable missing"))?;
    Ok(HardwareReport {
        host: HostSummary {
            os_version: std::fs::read_to_string("/etc/os-release")
                .ok()
                .and_then(|s| parse_os_release(&s))
                .unwrap_or_default(),
            chip: cpu.model.clone(),
            ..Default::default()
        },
        cpu,
        memory: MemoryInfo {
            total_bytes: total,
            available_bytes: available,
            unified: false,
        },
        gpus: cmd("lspci", &[]).map(|o| parse_lspci(&o)).unwrap_or_default(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x86_cpuinfo() -> String {
        // 2 sockets x 2 cores x 2 threads = 8 logical, 4 physical.
        let mut s = String::new();
        for p in 0..8 {
            s += &format!(
                "processor\t: {p}\nvendor_id\t: GenuineIntel\nmodel name\t: Intel(R) Xeon(R) Gold 6338 CPU @ 2.00GHz\nphysical id\t: {}\ncore id\t: {}\nflags\t\t: fpu sse4_2 avx avx2 fma f16c avx512f avx512_vnni amx_tile amx_bf16\n\n",
                p / 4,
                (p / 2) % 2
            );
        }
        s
    }

    #[test]
    fn cpuinfo_x86_counts_physical_cores_and_features() {
        let c = parse_cpuinfo(&x86_cpuinfo());
        assert_eq!(c.model, "Intel(R) Xeon(R) Gold 6338 CPU @ 2.00GHz");
        assert_eq!((c.physical_cores, c.logical_cores), (4, 8));
        assert_eq!(
            c.features,
            ["avx", "avx2", "avx512f", "avx512_vnni", "fma", "f16c", "amx"]
        );
    }

    #[test]
    fn cpuinfo_arm64_falls_back_to_logical() {
        let s = "processor\t: 0\nBogoMIPS\t: 50.00\nFeatures\t: fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp cpuid asimdrdm asimddp i8mm bf16\nCPU implementer\t: 0x41\n\nprocessor\t: 1\nBogoMIPS\t: 50.00\nFeatures\t: fp asimd asimdhp asimddp i8mm bf16\nCPU implementer\t: 0x41\nModel\t\t: Raspberry Pi 5 Model B Rev 1.0\n";
        let c = parse_cpuinfo(s);
        assert_eq!((c.physical_cores, c.logical_cores), (2, 2));
        assert_eq!(c.features, ["neon", "fp16", "dotprod", "i8mm", "bf16"]);
        assert_eq!(c.model, "Raspberry Pi 5 Model B Rev 1.0");
        assert_eq!(parse_cpuinfo("").logical_cores, 0);
    }

    #[test]
    fn meminfo_kb_to_bytes() {
        let s = "MemTotal:       65536000 kB\nMemFree:         1000000 kB\nMemAvailable:   32000000 kB\nBuffers:          500000 kB\n";
        assert_eq!(parse_meminfo(s), Some((65536000 * 1024, 32000000 * 1024)));
        assert_eq!(parse_meminfo("MemTotal: 1 kB\n"), None);
    }

    #[test]
    fn os_release_pretty_name() {
        assert_eq!(
            parse_os_release("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n").as_deref(),
            Some("Ubuntu 24.04.1 LTS")
        );
        assert_eq!(parse_os_release("VERSION_ID=12\n").as_deref(), Some("12"));
        assert_eq!(parse_os_release(""), None);
    }

    #[test]
    fn lspci_gpu_lines() {
        let out = "00:00.0 Host bridge: Intel Corporation Device 4660 (rev 02)\n00:02.0 VGA compatible controller: Intel Corporation Alder Lake-S GT1 [UHD Graphics 770] (rev 0c)\n01:00.0 VGA compatible controller: NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)\n01:00.1 Audio device: NVIDIA Corporation GA102 High Definition Audio Controller (rev a1)\n03:00.0 Display controller: Advanced Micro Devices, Inc. [AMD/ATI] Navi 31 [Radeon RX 7900 XTX]\n04:00.0 3D controller: NVIDIA Corporation GH100 [H100 SXM5 80GB] (rev a1)\n";
        let g = parse_lspci(out);
        let v: Vec<(&str, &str)> = g.iter().map(|g| (g.vendor.as_str(), g.backend.as_str())).collect();
        assert_eq!(
            v,
            [
                ("Intel", "vulkan"),
                ("NVIDIA", "cuda"),
                ("AMD", "rocm"),
                ("NVIDIA", "cuda")
            ]
        );
        assert_eq!(g[1].name, "NVIDIA Corporation GA102 [GeForce RTX 3090] (rev a1)");
        assert!(parse_lspci("").is_empty());
    }
}
