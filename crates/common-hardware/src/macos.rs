//! macOS probe: one `sysctl` call, `vm_stat`, `ioreg`, `sw_vers`. All parsers are pure.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::GpuInfo;
use std::collections::HashMap;

/// `sysctl` keys that mark an inference-relevant ISA feature when their value is `1`.
const FEATURES: &[(&str, &str)] = &[
    ("hw.optional.neon", "neon"),
    ("hw.optional.arm.FEAT_FP16", "fp16"),
    ("hw.optional.arm.FEAT_DotProd", "dotprod"),
    ("hw.optional.arm.FEAT_I8MM", "i8mm"),
    ("hw.optional.arm.FEAT_BF16", "bf16"),
    ("hw.optional.arm.FEAT_SME", "sme"),
    ("hw.optional.arm.FEAT_SME2", "sme2"),
    ("hw.optional.avx1_0", "avx"),
    ("hw.optional.avx2_0", "avx2"),
    ("hw.optional.avx512f", "avx512f"),
    ("hw.optional.fma", "fma"),
    ("hw.optional.f16c", "f16c"),
];

/// `key: value` lines from `sysctl <keys>` (without `-n`).
fn parse_sysctl(out: &str) -> HashMap<&str, &str> {
    out.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim(), v.trim())).collect()
}

fn features(m: &HashMap<&str, &str>) -> Vec<String> {
    FEATURES.iter().filter(|(k, _)| m.get(k) == Some(&"1")).map(|(_, n)| n.to_string()).collect()
}

/// Available memory from `vm_stat`: (free + inactive + speculative pages) x page size.
/// Inactive pages hold cached data the kernel reclaims first and speculative pages are
/// read-ahead, so both count as usable without swapping. Wired, active and compressor
/// pages do not.
fn parse_vm_stat(out: &str) -> Option<u64> {
    let page: u64 = out.lines().next()?.split("page size of ").nth(1)?.split_whitespace().next()?.parse().ok()?;
    let pages = |label: &str| -> Option<u64> {
        let line = out.lines().find(|l| l.starts_with(label))?;
        line.split_once(':')?.1.trim().trim_end_matches('.').parse().ok()
    };
    let free = pages("Pages free:")? + pages("Pages inactive:")? + pages("Pages speculative:")?;
    free.checked_mul(page)
}

/// GPU core count from `ioreg -rc AGXAccelerator -d 1` (`"gpu-core-count" = 10`).
fn parse_gpu_cores(ioreg: &str) -> Option<u32> {
    ioreg.lines().find_map(|l| l.trim().strip_prefix("\"gpu-core-count\" = ")?.trim().parse().ok())
}

#[cfg(target_os = "macos")]
pub(crate) fn probe() -> common_core::Result<crate::HardwareReport> {
    use crate::{cmd, CpuInfo, HardwareReport, MemoryInfo};
    use common_core::{CommonError, HostSummary};

    // Absolute paths: system tools, never whatever PATH points at.
    let out = cmd("/usr/sbin/sysctl", &["machdep.cpu.brand_string", "hw.physicalcpu", "hw.logicalcpu", "hw.memsize", "hw.optional"])
        .ok_or_else(|| CommonError::io("sysctl failed"))?;
    let m = parse_sysctl(&out);
    fn num<T: std::str::FromStr>(m: &HashMap<&str, &str>, k: &str) -> common_core::Result<T> {
        m.get(k).and_then(|v| v.parse().ok()).ok_or_else(|| CommonError::io(format!("sysctl {k}: missing or not a number")))
    }
    let chip = m.get("machdep.cpu.brand_string").unwrap_or(&"").to_string();
    let total: u64 = num(&m, "hw.memsize")?;
    let apple_silicon = cfg!(target_arch = "aarch64");

    let gpus = if apple_silicon {
        vec![GpuInfo {
            name: chip.clone(),
            vendor: "Apple".into(),
            backend: "metal".into(),
            memory_bytes: Some(total),
            cores: cmd("/usr/sbin/ioreg", &["-rc", "AGXAccelerator", "-d", "1"]).and_then(|o| parse_gpu_cores(&o)),
        }]
    } else {
        // ponytail: Intel Macs with AMD/Intel GPUs are not a supported target; list nothing.
        Vec::new()
    };

    Ok(HardwareReport {
        host: HostSummary {
            os_version: cmd("/usr/bin/sw_vers", &["-productVersion"]).unwrap_or_default(),
            chip: chip.clone(),
            ..Default::default()
        },
        cpu: CpuInfo {
            model: chip,
            physical_cores: num(&m, "hw.physicalcpu")?,
            logical_cores: num(&m, "hw.logicalcpu")?,
            features: features(&m),
            ..Default::default()
        },
        memory: MemoryInfo {
            total_bytes: total,
            available_bytes: cmd("/usr/bin/vm_stat", &[]).and_then(|o| parse_vm_stat(&o)).unwrap_or(0),
            unified: apple_silicon,
        },
        gpus,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSCTL: &str = "machdep.cpu.brand_string: Apple M2\nhw.physicalcpu: 8\nhw.logicalcpu: 8\nhw.memsize: 8589934592\nhw.optional.neon: 1\nhw.optional.arm.FEAT_FP16: 1\nhw.optional.arm.FEAT_DotProd: 1\nhw.optional.arm.FEAT_I8MM: 1\nhw.optional.arm.FEAT_BF16: 1\nhw.optional.arm.FEAT_SME: 0\nhw.optional.arm.caps: 868632327146696703\n";

    #[test]
    fn sysctl_map_and_features() {
        let m = parse_sysctl(SYSCTL);
        assert_eq!(m["machdep.cpu.brand_string"], "Apple M2");
        assert_eq!(m["hw.memsize"], "8589934592");
        assert_eq!(features(&m), ["neon", "fp16", "dotprod", "i8mm", "bf16"]);
        let intel = parse_sysctl("machdep.cpu.brand_string: Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz\nhw.optional.avx2_0: 1\nhw.optional.avx512f: 0\nhw.optional.fma: 1\n");
        assert_eq!(intel["machdep.cpu.brand_string"], "Intel(R) Core(TM) i7-9750H CPU @ 2.60GHz");
        assert_eq!(features(&intel), ["avx2", "fma"]);
    }

    #[test]
    fn vm_stat_formula() {
        let out = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                                3964.\nPages active:                             99041.\nPages inactive:                           97570.\nPages speculative:                          475.\nPages throttled:                              0.\nPages wired down:                        155166.\n";
        assert_eq!(parse_vm_stat(out), Some((3964 + 97570 + 475) * 16384));
        assert_eq!(parse_vm_stat("garbage"), None);
        assert_eq!(parse_vm_stat("Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\n"), None);
    }

    #[test]
    fn ioreg_gpu_cores() {
        let out = "+-o AGXAcceleratorG14G  <class AGXAcceleratorG14G, id 0x100000a0b>\n    {\n      \"model\" = \"Apple M2\"\n      \"gpu-core-count\" = 10\n      \"IOClass\" = \"AGXAcceleratorG14G\"\n    }\n";
        assert_eq!(parse_gpu_cores(out), Some(10));
        assert_eq!(parse_gpu_cores(""), None);
        assert_eq!(parse_gpu_cores("\"gpu-core-count\" = many"), None);
    }
}
