//! Windows probe: one PowerShell call reading CIM (WMI) classes as JSON. The parser is pure, so it
//! is tested on every host against captured output.
#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

use crate::{CpuInfo, GpuInfo, MemoryInfo};
use serde::Deserialize;

/// The query the probe runs. `@(...)` keeps single results as arrays; sizes are bytes except
/// `FreePhysicalMemory` (KiB, as Win32_OperatingSystem reports it).
pub(crate) const QUERY: &str = "$ErrorActionPreference='Stop';\
$c=@(Get-CimInstance Win32_Processor);\
$o=Get-CimInstance Win32_OperatingSystem;\
$s=Get-CimInstance Win32_ComputerSystem;\
$g=@(Get-CimInstance Win32_VideoController);\
[pscustomobject]@{\
cpu=@($c|Select-Object Name,NumberOfCores,NumberOfLogicalProcessors);\
total=$s.TotalPhysicalMemory;free_kib=$o.FreePhysicalMemory;\
os=$o.Caption;version=$o.Version;\
gpus=@($g|Select-Object Name,AdapterCompatibility,AdapterRAM)}|ConvertTo-Json -Depth 3 -Compress";

#[derive(Deserialize)]
struct Raw {
    cpu: Vec<RawCpu>,
    total: u64,
    free_kib: u64,
    os: Option<String>,
    version: Option<String>,
    #[serde(default)]
    gpus: Vec<RawGpu>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawCpu {
    name: Option<String>,
    number_of_cores: Option<u32>,
    number_of_logical_processors: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RawGpu {
    name: Option<String>,
    adapter_compatibility: Option<String>,
    #[serde(rename = "AdapterRAM")]
    adapter_ram: Option<u64>,
}

pub(crate) struct Parsed {
    pub os_version: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpus: Vec<GpuInfo>,
}

fn vendor(compat: &str, name: &str) -> &'static str {
    let s = format!("{compat} {name}").to_ascii_lowercase();
    if s.contains("nvidia") {
        "NVIDIA"
    } else if s.contains("advanced micro devices") || s.contains("amd") || s.contains("radeon") {
        "AMD"
    } else if s.contains("intel") {
        "Intel"
    } else if s.contains("qualcomm") || s.contains("adreno") {
        "Qualcomm"
    } else if s.contains("microsoft") {
        "Microsoft"
    } else {
        "unknown"
    }
}

/// Parse the JSON `QUERY` prints. Physical/logical cores are summed over sockets. The software
/// "Microsoft Basic Display Adapter" is not a GPU and is dropped. `AdapterRAM` is a 32-bit field
/// that tops out at 4 GiB, so values at the cap are reported as unknown rather than wrong.
pub(crate) fn parse(json: &str) -> Option<Parsed> {
    let r: Raw = serde_json::from_str(json).ok()?;
    let sum = |f: fn(&RawCpu) -> Option<u32>| r.cpu.iter().filter_map(f).sum::<u32>();
    let model = r
        .cpu
        .iter()
        .find_map(|c| c.name.clone())
        .unwrap_or_default()
        .trim()
        .to_string();
    let gpus = r
        .gpus
        .iter()
        .filter_map(|g| {
            let name = g.name.clone()?.trim().to_string();
            let v = vendor(g.adapter_compatibility.as_deref().unwrap_or(""), &name);
            (v != "Microsoft").then(|| GpuInfo {
                vendor: v.into(),
                backend: "vulkan".into(),
                memory_bytes: g.adapter_ram.filter(|&b| b > 0 && b < 0xFFF0_0000), // drivers report ~4 GiB (0xFFF00000+) when the true size does not fit
                cores: None,
                name,
            })
        })
        .collect();
    Some(Parsed {
        os_version: [r.os.unwrap_or_default(), r.version.unwrap_or_default()]
            .join(" ")
            .trim()
            .to_string(),
        cpu: CpuInfo {
            model,
            physical_cores: sum(|c| c.number_of_cores),
            logical_cores: sum(|c| c.number_of_logical_processors),
            ..Default::default()
        },
        memory: MemoryInfo {
            total_bytes: r.total,
            available_bytes: r.free_kib.saturating_mul(1024),
            unified: false,
        },
        gpus,
    })
}

/// `%SystemRoot%\System32\<name>`: system tools and DLLs by absolute path, never via PATH.
pub(crate) fn system32(name: &str) -> std::path::PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    std::path::Path::new(&root).join("System32").join(name)
}

/// `ullAvailPhys` from `GlobalMemoryStatusEx`: free plus standby pages, which Windows hands out
/// without paging. `FreePhysicalMemory` (the CIM figure) leaves the standby list out.
#[cfg(target_os = "windows")]
pub(crate) fn available_memory() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: a zeroed POD struct with dwLength set, as the API requires.
    unsafe {
        let mut m: MEMORYSTATUSEX = std::mem::zeroed();
        m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        (GlobalMemoryStatusEx(&mut m) != 0).then_some(m.ullAvailPhys)
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn probe() -> common_core::Result<crate::HardwareReport> {
    use crate::{cmd, HardwareReport};
    use common_core::{CommonError, HostSummary};

    let ps = system32("WindowsPowerShell\\v1.0\\powershell.exe");
    let out = cmd(
        ps.to_str().unwrap_or("powershell.exe"),
        &["-NoProfile", "-NonInteractive", "-Command", QUERY],
    )
    .ok_or_else(|| CommonError::io("PowerShell hardware query failed"))?;
    let p = parse(&out).ok_or_else(|| CommonError::io("PowerShell hardware query returned unexpected JSON"))?;
    Ok(HardwareReport {
        host: HostSummary {
            os_version: p.os_version,
            chip: p.cpu.model.clone(),
            ..Default::default()
        },
        cpu: p.cpu,
        // The load-time figure, so what the app shows and what a load is judged against agree.
        memory: MemoryInfo {
            available_bytes: available_memory().unwrap_or(p.memory.available_bytes),
            ..p.memory
        },
        gpus: p.gpus,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of `QUERY` output on a Windows 11 laptop with an Intel iGPU (values illustrative).
    const LAPTOP: &str = r#"{"cpu":[{"Name":"13th Gen Intel(R) Core(TM) i7-1355U","NumberOfCores":10,"NumberOfLogicalProcessors":12}],"total":16849584128,"free_kib":7340032,"os":"Microsoft Windows 11 Home","version":"10.0.26100","gpus":[{"Name":"Intel(R) Iris(R) Xe Graphics","AdapterCompatibility":"Intel Corporation","AdapterRAM":2147483648},{"Name":"Microsoft Basic Display Adapter","AdapterCompatibility":"(Standard display types)","AdapterRAM":0}]}"#;

    #[test]
    fn parses_a_windows_laptop() {
        let p = parse(LAPTOP).unwrap();
        assert_eq!(p.cpu.model, "13th Gen Intel(R) Core(TM) i7-1355U");
        assert_eq!((p.cpu.physical_cores, p.cpu.logical_cores), (10, 12));
        assert_eq!(p.memory.total_bytes, 16849584128);
        assert_eq!(p.memory.available_bytes, 7340032 * 1024);
        assert_eq!(p.os_version, "Microsoft Windows 11 Home 10.0.26100");
        assert_eq!(p.gpus.len(), 1, "the basic display adapter is not a GPU");
        assert_eq!(
            (p.gpus[0].vendor.as_str(), p.gpus[0].backend.as_str()),
            ("Intel", "vulkan")
        );
        assert_eq!(p.gpus[0].memory_bytes, Some(2147483648));
    }

    #[test]
    fn sums_sockets_and_treats_capped_vram_as_unknown() {
        let j = r#"{"cpu":[{"Name":"Xeon","NumberOfCores":8,"NumberOfLogicalProcessors":16},{"Name":"Xeon","NumberOfCores":8,"NumberOfLogicalProcessors":16}],"total":1,"free_kib":1,"os":null,"version":null,"gpus":[{"Name":"NVIDIA GeForce RTX 4070 Laptop GPU","AdapterCompatibility":"NVIDIA","AdapterRAM":4293918720}]}"#;
        let p = parse(j).unwrap();
        assert_eq!((p.cpu.physical_cores, p.cpu.logical_cores), (16, 32));
        assert_eq!(p.gpus[0].vendor, "NVIDIA");
        assert_eq!(p.gpus[0].memory_bytes, None);
        assert_eq!(p.os_version, "");
        assert!(parse("not json").is_none());
        assert!(parse(r#"{"cpu":[],"total":0}"#).is_none(), "free memory is required");
    }

    #[test]
    fn query_is_one_line_for_powershell() {
        assert!(!QUERY.contains('\n') && QUERY.contains("ConvertTo-Json"));
    }
}
