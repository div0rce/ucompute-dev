# ADR-0002: llama.cpp binding strategy — supervised pinned `llama-server` process

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

- **Status:** Accepted 2026-10-14
- **Supersedes:** "ADR-0002 not written" entry in `docs/DECISIONS.md`
- **Inputs:** inspection of `.recovery/codex-m1/ucompute.py` and its evidence; smoke test of the pinned
  `llama-server` (b11609) on 127.0.0.1 (this session); ADR-0001 stack table.

## Context
Three ways to drive llama.cpp from Rust were on the table:

| Option | Pros | Cons |
|---|---|---|
| A. `llama-cpp-2` crate (in-process FFI, builds llama.cpp via cmake) | Full in-process control of tokens/KV | Builds llama.cpp from source inside `cargo build` (slow, multi-GB target dir on a 9.5 GiB-free disk); tightly coupled to llama.cpp internals per its own README; a backend crash takes the daemon down |
| B. Thin in-repo `bindgen` wrapper over prebuilt `libllama` | Pinned binaries, in-process | Same crash-domain problem; large unsafe surface we must maintain across llama.cpp API churn |
| C. **Supervise the pinned prebuilt `llama-server` binary on loopback** | Pinned, hash-verified release archive (already proven by the Codex slice); process isolation (backend crash is a typed error, not a daemon crash); OpenAI-compatible HTTP + SSE streaming, `/health`, Prometheus `/metrics`, embeddings, grammar/JSON-schema constrained output, LoRA, multimodal (`mtmd`), speculative decoding and `--rpc` layer offload all come from the engine; cancellation = client disconnect | One HTTP hop on loopback (~sub-ms); engine process lifecycle must be supervised; metrics parsed from server JSON rather than native structs |

The Codex slice (`.recovery/codex-m1/ucompute.py`) already proved option C's provisioning half: download the official release archive, verify SHA-256, extract safely, verify every file against the archive, run with a scrubbed environment, and parse `offloaded N/M layers` / `using device Metal0` from the logs to prove GPU execution. This ADR generalizes that to `llama-server`.

## Decision
1. `crates/ucompute-runtime` implements `InferenceBackend` for llama.cpp by **spawning the pinned `llama-server`** bound to `127.0.0.1:<ephemeral>` with a per-process random `--api-key`, `--metrics`, `--no-webui`, `--offline`, and a scrubbed environment (no `DYLD_*`, `LD_*`, `GGML_*`, `LLAMA_ARG_*`).
2. Backend archives are pinned per platform in `catalog/backends.json` (release tag, URL, SHA-256, bytes). Only `macos-arm64` is pinned and verified today; other platforms are added when a host exists to verify them (B-004).
3. GPU proof is not inferred from flags: the adapter parses the server log for actual layer offload and device lines, exactly as the Codex slice did. A GPU load that fails falls back to CPU **with a logged, surfaced reason**, never silently (F-01-04).
4. UCompute never exposes the engine port. All client traffic enters through `ucompute-api` (default bind `127.0.0.1`), which applies admission control, typed errors and metrics, then proxies to the engine over loopback.
5. Distributed partitioning (M4) will use the engine's `--rpc` layer offload to `ggml-rpc-server` instances that are **bound to loopback on the remote host and reached only through UCompute's authenticated, encrypted transport** (D-007). This ADR does not decide the transport (ADR-0003).
6. The Python slice stays in `.recovery/codex-m1/` as a correctness oracle and provisioning reference; it is not shipped.

## Consequences
- `cargo build` stays light (no C++ build), which matters on the 8 GB / 9.5 GiB-free development Mac.
- An in-process backend (option A/B) can still be added later behind the same `InferenceBackend` trait (F-01-06) if a requirement needs it (e.g. activation-level access for F-06-12 spot checks).
- The llama.cpp release is a runtime dependency that must be re-pinned deliberately; its MIT license text ships with the binaries.
- Re-verification rule: every pin bump re-runs the golden-token test (F-01-02) on CPU and Metal.
