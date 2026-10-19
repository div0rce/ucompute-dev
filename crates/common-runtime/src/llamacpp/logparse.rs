//! Proof-of-execution parsing from the engine log. Hand-rolled matchers (no regex crate in the
//! workspace) for the patterns the Codex slice used, re-verified on b11609 at `-lv 4`:
//!
//! ```text
//! llama_prepare_model_devices: using device MTL0 (Apple M2) (unknown id) - 5460 MiB free
//! load_tensors: offloaded 7/7 layers to GPU
//! llama_context: n_ctx                 = 256
//! slot   load_model: id  0 | task -1 | new slot, n_ctx = 128
//! ```

use common_core::ErrorCode;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Proof {
    /// `(offloaded, total)` from `offloaded N/M layers to GPU`. M counts the output layer, so it is
    /// block_count + 1 for most architectures.
    pub offloaded: Option<(u32, u32)>,
    /// `MTL0 (Apple M2)` from `using device MTL0 (Apple M2) ...`; CUDA/Vulkan/ROCm/SYCL ids too.
    pub device: Option<String>,
    /// Last `n_ctx = N` in the log: the per-slot context the server actually allocated.
    pub n_ctx: Option<u32>,
}

impl Proof {
    pub fn gpu_proven(&self) -> bool {
        self.offloaded.is_some_and(|(n, _)| n > 0) && self.device.is_some()
    }

    pub fn why_not_gpu(&self) -> String {
        match (self.offloaded, &self.device) {
            (Some((0, total)), _) => format!("engine offloaded 0/{total} layers to the GPU"),
            (None, _) => "engine log has no 'offloaded N/M layers to GPU' line".into(),
            (Some(_), None) => "engine log has no 'using device' line for a GPU".into(),
            _ => "GPU execution not proven".into(),
        }
    }
}

pub fn parse(log: &str) -> Proof {
    let mut p = Proof::default();
    for line in log.lines() {
        if let Some(v) = offloaded(line) {
            p.offloaded = Some(v);
        }
        if let Some(d) = device(line) {
            p.device = Some(d);
        }
        if let Some(n) = n_ctx(line) {
            p.n_ctx = Some(n);
        }
    }
    p
}

fn take_digits(s: &str) -> Option<(u32, &str)> {
    let n = s.bytes().take_while(u8::is_ascii_digit).count();
    if n == 0 {
        return None;
    }
    Some((s[..n].parse().ok()?, &s[n..]))
}

fn offloaded(line: &str) -> Option<(u32, u32)> {
    let rest = &line[line.find("offloaded ")? + "offloaded ".len()..];
    let (a, rest) = take_digits(rest)?;
    let (b, rest) = take_digits(rest.strip_prefix('/')?)?;
    rest.starts_with(" layers to GPU").then_some((a, b))
}

const GPU_DEVICE_PREFIXES: [&str; 9] = [
    "MTL", "Metal", "CUDA", "Vulkan", "ROCm", "HIP", "SYCL", "OpenCL", "CANN",
];

fn device(line: &str) -> Option<String> {
    let rest = &line[line.find("using device ")? + "using device ".len()..];
    let dev = &rest[..rest.find(' ')?];
    let is_gpu_id = GPU_DEVICE_PREFIXES.iter().any(|p| {
        dev.strip_prefix(p)
            .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
    });
    if !is_gpu_id {
        return None;
    }
    let rest = rest[dev.len()..].strip_prefix(" (")?;
    // Up to the matching `)`: names nest parentheses, e.g. `Vulkan0 (Intel(R) Arc(TM) 140V GPU (16GB))`.
    let mut depth = 1usize;
    let end = rest.char_indices().find_map(|(i, c)| {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        (depth == 0).then_some(i)
    })?;
    let name = &rest[..end];
    Some(format!("{dev} ({name})"))
}

/// `n_ctx` followed by optional spaces, `=`, spaces, digits. `n_ctx_train = 128` does not match.
fn n_ctx(line: &str) -> Option<u32> {
    let mut search = line;
    while let Some(i) = search.find("n_ctx") {
        let after = &search[i + "n_ctx".len()..];
        if let Some(r) = after.trim_start().strip_prefix('=') {
            if let Some((n, _)) = take_digits(r.trim_start()) {
                return Some(n);
            }
        }
        search = after;
    }
    None
}

/// Last `n` lines of `text`.
pub fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Replace every occurrence of `secret` (the per-process API key) before text leaves the adapter.
pub fn scrub(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        return text.to_string();
    }
    text.replace(secret, "[redacted]")
}

/// Classify an early exit from its log tail. Allocation failures are backend problems even though
/// the loader wraps them in "failed to load model"; the GGUF header was already validated, so other
/// model-load failures mean the file itself is bad.
pub fn exit_error_code(tail: &str) -> ErrorCode {
    // `ggml_metal_init: error` precedes the loader's "failed to load model" when the Metal library
    // or device is unusable; that is a backend problem, not a bad file.
    const BACKEND_MARKERS: [&str; 5] = [
        "failed to allocate",
        "unable to allocate",
        "out of memory",
        "couldn't bind",
        "ggml_metal_init: error",
    ];
    const MODEL_MARKERS: [&str; 4] = [
        "failed to load model",
        "unknown architecture",
        "invalid magic",
        "error loading model",
    ];
    if BACKEND_MARKERS.iter().any(|m| tail.contains(m)) {
        ErrorCode::BackendFailed
    } else if MODEL_MARKERS.iter().any(|m| tail.contains(m)) {
        ErrorCode::ModelFileInvalid
    } else {
        ErrorCode::BackendFailed
    }
}

/// The engine lost the port race (another process bound it between our probe and its bind).
pub fn bind_failed(tail: &str) -> bool {
    tail.contains("couldn't bind")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real lines recorded from llama-server b11609 on this Mac (-lv 4).
    const METAL_LOG: &str = "\
0.00.255.005 I cmn  common_param:   - MTL0    : Apple M2 (5461 MiB, 5460 MiB free)
0.00.276.338 I llama_prepare_model_devices: using device MTL0 (Apple M2) (unknown id) - 5460 MiB free
0.00.285.801 I print_info: n_ctx_train           = 128
0.00.304.811 I load_tensors: offloaded 7/7 layers to GPU
0.00.305.385 I llama_context: n_ctx                 = 256
0.00.305.385 I llama_context: n_ctx_seq             = 256
0.00.305.439 W llama_context: n_ctx_seq (256) > n_ctx_train (128) -- possible training context overflow
0.00.352.012 I srv    load_model: initializing, n_slots = 1, n_ctx_slot = 128, kv_unified = 'false'
0.00.352.018 I slot   load_model: id  0 | task -1 | new slot, n_ctx = 128
";
    const CPU_LOG: &str = "\
0.00.066.383 I cmn  common_param:   - MTL0    : Apple M2 (5461 MiB, 5460 MiB free)
0.00.066.390 I cmn  common_param:   - CPU     : Apple M2 (8192 MiB, 8192 MiB free)
0.00.091.790 I load_tensors: offloaded 0/7 layers to GPU
0.00.091.791 I load_tensors:   CPU_Mapped model buffer size =     4.94 MiB
0.00.352.018 I slot   load_model: id  0 | task -1 | new slot, n_ctx = 128
";

    #[test]
    fn metal_proof() {
        let p = parse(METAL_LOG);
        assert_eq!(p.offloaded, Some((7, 7)));
        assert_eq!(p.device.as_deref(), Some("MTL0 (Apple M2)"));
        assert_eq!(
            p.n_ctx,
            Some(128),
            "last n_ctx wins: the slot context, not the padded llama_context one"
        );
        assert!(p.gpu_proven());
    }

    #[test]
    fn cpu_proof() {
        let p = parse(CPU_LOG);
        assert_eq!(p.offloaded, Some((0, 7)));
        assert_eq!(p.device, None, "the device-list line is not a 'using device' line");
        assert!(!p.gpu_proven());
        assert!(p.why_not_gpu().contains("0/7"));
        assert!(parse("").why_not_gpu().contains("no 'offloaded"));
    }

    #[test]
    fn other_gpu_vendors() {
        assert_eq!(
            device("ggml_cuda_init: using device CUDA0 (NVIDIA RTX 4090) - 24000 MiB free").as_deref(),
            Some("CUDA0 (NVIDIA RTX 4090)")
        );
        assert_eq!(
            device("using device Vulkan1 (AMD Radeon) x").as_deref(),
            Some("Vulkan1 (AMD Radeon)")
        );
        assert_eq!(
            device("using device Metal0 (Apple M2)").as_deref(),
            Some("Metal0 (Apple M2)")
        );
        assert_eq!(device("using device CPU (Apple M2)"), None);
        assert_eq!(
            device("llama_prepare_model_devices: using device Vulkan0 (Intel(R) Arc(TM) 140V GPU (16GB)) (0000:00:02.0) - 15872 MiB free").as_deref(),
            Some("Vulkan0 (Intel(R) Arc(TM) 140V GPU (16GB))"),
            "nested parentheses (Windows, Intel Arc)"
        );
        assert_eq!(device("using device Vulkan0 (Intel(R) Arc"), None, "unterminated name");
        assert_eq!(device("using device MTL (x)"), None, "needs an index");
        assert_eq!(device("using device MTL0"), None, "needs a name");
    }

    #[test]
    fn n_ctx_variants() {
        assert_eq!(n_ctx("print_info: n_ctx_train           = 128"), None);
        assert_eq!(n_ctx("n_ctx_seq (256) > n_ctx_train (128)"), None);
        assert_eq!(n_ctx("llama_context: n_ctx = 4096"), Some(4096));
        assert_eq!(n_ctx("new slot, n_ctx=64"), Some(64));
        assert_eq!(offloaded("offloaded 3/x layers to GPU"), None);
        assert_eq!(offloaded("offloaded 3/7 layers to CPU"), None);
    }

    #[test]
    fn tail_scrub_and_classify() {
        assert_eq!(tail("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(tail("a\nb", 5), "a\nb");
        assert_eq!(
            scrub("--api-key s3cret and s3cret again", "s3cret"),
            "--api-key [redacted] and [redacted] again"
        );
        assert_eq!(scrub("x", ""), "x");
        let junk = "E gguf_init_from_reader: invalid magic characters: '?B?(', expected 'GGUF'\nE srv load_model: failed to load model, 'x'\nE srv llama_server: exiting due to model loading error";
        assert_eq!(exit_error_code(junk), ErrorCode::ModelFileInvalid);
        assert_eq!(
            exit_error_code(
                "llama_model_load: error loading model: unable to allocate Metal buffer\nfailed to load model"
            ),
            ErrorCode::BackendFailed
        );
        assert_eq!(
            exit_error_code("error: invalid argument: --device COMMON_TEST_NO_SUCH_DEVICE"),
            ErrorCode::BackendFailed
        );
        let metal = "E ggml_metal_init: error: failed to load library 'default.metallib'\nE srv load_model: failed to load model, '/x.gguf'\nE srv llama_server: exiting due to model loading error";
        assert_eq!(
            exit_error_code(metal),
            ErrorCode::BackendFailed,
            "a Metal init failure is not a damaged file"
        );
        let bind = "E srv         start: couldn't bind HTTP server socket, hostname: 127.0.0.1, port: 41099";
        assert!(bind_failed(bind));
        assert_eq!(exit_error_code(bind), ErrorCode::BackendFailed);
    }
}
