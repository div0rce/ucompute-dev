# Verification

How evidence is recorded, and the current state of each quality gate (spec section 11).

## Evidence entry format
```
### <REQ-ID or GATE> — <short title>
- Date / commit: YYYY-MM-DD / <git sha>
- Host: <hostname> (<chip>, <memory>, <OS>)   label: REAL | SINGLE-HOST | SIMULATED
- Command: <exact command>
- Result: PASS | FAIL (+ numbers)
- Artifacts: <paths under benchmarks/results/ or tests/ output>
```
A row in `docs/REQUIREMENTS.csv` may be marked `VERIFIED` only when its `observed_evidence` points to an entry here covering every acceptance criterion.

## Quality gates
| Gate | Scope | State | Evidence |
|---|---|---|---|
| A — Build | clean build, lint, fmt, dependency/license scan, secrets check | Not assessed | — |
| B — Inference | real load, generation, streaming, cancellation, cleanup | Not assessed in this ledger (existing Mac repo reports partial; B-009) | — |
| C — Network | discovery, auth, revocation, disconnects, malformed messages | Not started | — |
| D — Pooling | genuine two-node partitioned execution, correctness, memory, failure semantics | Not started; real verification blocked (B-002) | — |
| E — Product | install, select, run, pair, pool, diagnose without developer | Not started | — |
| F — Public swarm | adversarial review, accounting, privacy, abuse, churn | Not started; disabled by default | — |
| G — Release | signed/checksummed, reproducible, compat matrix, CI, docs, benchmarks | Not started (B-007, B-008) | — |

## Evidence log

### REGISTRY — requirements ledger integrity
- Date: 2026-10-14 · commit: n/a (import kit, not yet in repo)
- Host: Claude cloud workspace (Linux x86_64) · label: REAL (tooling only; no product code)
- Command: `python3 scripts/validate_registry.py && python3 -m unittest discover -s tests/registry -v`
- Result: PASS — 148 P0 rows; NOT_STARTED=143, IN_PROGRESS=3, IMPLEMENTED_UNVERIFIED=2, others 0. Negative tests confirm the validator rejects unevidenced VERIFIED, missing rows, unknown statuses, cycles and stale generated files.
- Scope: proves ledger structure only. Proves nothing about the product.

### PHASE-1 — environment inspector
- Date: 2026-10-14 · label: REAL (macOS branch executed on the owner's Mac this session; Linux branch earlier on a cloud workspace)
- Host: owner's MacBook Air (Apple M2, 8 GB unified, 10-core GPU, Metal 4, macOS 26.2 build 25C5048a, arm64)
- Command: `scripts/inspect-env.sh`
- Result: PASS — report in `docs/environment/ENVIRONMENT-*.md`. Toolchain: rustc/cargo 1.98.0, Python 3.14.8, Node 26.10, uv, Homebrew 7.0.8, cmake, ninja, Xcode clang. No preinstalled llama.cpp, Ollama, MLX. Disk 9.5 GiB free at start (→17 GiB after owner-authorized cache cleanup).

### RECOVERY-CODEX-M1 — recovered Python inference slice re-verified
- Date / commit: 2026-10-14 / Codex history `efdec54` (imported as `refs/recovered/codex-m1/feat/m1-macos-local-inference`)
- Host: owner's MacBook Air (above) · label: REAL
- Command: in `.recovery/codex-m1/` with the pinned runtime + model copied from the original checkout: `python3 -m unittest discover -s tests -v` then `COMMON_REAL_INFERENCE=1 python3 -m unittest discover -s tests -v`
- Result: PASS — 17/17 (13 fixture tests + 4 real: CPU generation, one-token generation, context-overflow rejection, Metal generation with 7/7 layers offloaded). Greedy continuation of "Once upon a time, a little rabbit" = " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big" on both CPU and Metal. Metal decode 455 tok/s on this tiny model (CPU is faster for it; not a benchmark).
- Scope: proves the recovered slice works on one physical Mac; it does not by itself satisfy any registry row (acceptance criteria require golden-token tests, streaming, fuzzing, fallback tests — being implemented in Rust, see M1 entries below).

### ENGINE-SMOKE — pinned `llama-server` b11609 on loopback (basis for ADR-0002)
- Date: 2026-10-14 · label: REAL
- Command: `llama-server -m stories15M-q4_0.gguf --host 127.0.0.1 --port 18081 -c 128 -ngl 99 --no-webui --metrics --offline -t 4`; `curl /health`, `/v1/models`, `/v1/completions` (temperature 0, seed 42, max_tokens 32), streaming variant, `/metrics`, `/props`
- Result: PASS — `/health` = `{"status":"ok"}`; completion text identical to the Codex golden output; usage prompt_tokens 10 / completion_tokens 32; timings prompt 868 tok/s, decode 442 tok/s; 18 SSE `data:` chunks for a 16-token stream; `/metrics` serves Prometheus text (`llamacpp:*`).

### F-09-01 (JS half) — official OpenAI JS client against `common serve`
- Date / commit: 2026-10-14 / working tree after wave 2 (API `common-api`, CLI `common-cli`)
- Host: owner's MacBook Air (Apple M2, 8 GB, macOS 26.2) · label: REAL
- Command: `common serve --port 18192` then `cd examples/clients/openai-js && COMMON_BASE_URL=http://127.0.0.1:18192/v1 COMMON_MODEL=stories15M-q4_0 node smoke.mjs` (openai npm 6.49.0, unmodified; only `baseURL` set)
- Result: PASS — `models.list` 8 models; `completions.create` (temperature 0, seed 42) returns the golden prefix " named Benny was hopping in a field. He "; `chat.completions.create` usage {prompt 16, completion 16}; streaming delivered 16 content chunks and a finish chunk; unknown model → HTTP 404 with code `model_not_found`.
- Python half: `crates/common-api/tests/openai_client_smoke.py` (openai 3.31.0) runs inside `cargo test -p common-api` (real_engine.rs) and printed "ALL CHECKS PASSED".

### F-09-03 — Prometheus exposition validated with promtool
- Date: 2026-10-14 · label: REAL
- Command: `curl -s http://127.0.0.1:18192/metrics > metrics.txt && promtool check metrics < metrics.txt` (promtool from Homebrew `prometheus`)
- Result: PASS — exit 0, no lint output; 42 lines incl. `common_requests_total{route,status}`, `common_tokens_generated_total`, `common_ttft_seconds` histogram, `common_queue_depth`, `common_inflight`; the scrape taken right after chat requests contains no prompt text (grep for the prompt string: 0 matches).

### DESKTOP-E2E — desktop UI against the real daemon (dev loop, browser)
- Date: 2026-10-14 · label: REAL (Vite dev server + `common serve`; not the packaged Tauri app)
- Host: owner's MacBook Air (Apple M2, 8 GB, macOS 26.2)
- Command: `COMMON_HOME=.common common serve` then `pnpm --dir apps/desktop dev`; opened `http://localhost:1420` in the Claude desktop built-in browser
- Result: PASS — Home shows "Common is running… Apple M2, 8.0 GB memory, metal" and the recommended model with its memory fit bar; Models lists all 8 catalog models with size, license (never blank) and fits / "Needs X, you have Y" from `/api/models`; Hardware view values equal `common hardware --json`; Chat: selected `smollm2-360m-instruct-q8_0`, Load model → "Talking to smollm2-360m-instruct-q8_0 on MTL0 (Apple M2), 4,096 token context", a question streamed a reply from the real engine (Metal). Observed defect: the reply footer showed "tokens unknown" (usage/timings not surfaced in the streaming path) — fixed in the follow-up commit referenced below.
- Note: `available` memory during this run was ~1.0 GB because several build agents and test engines were running; the 0.5B models were therefore reported as not fitting. Calibration of the estimate and the available-memory formula is tracked under F-01-03.

## M1 evidence (2026-10-14, commit ca298bd unless noted)

All entries below: Host = owner's MacBook Air (Apple M2, 8 GB unified, 10-core GPU, macOS 26.2 arm64) · engine = pinned llama.cpp b11609 (macos-arm64) · label: REAL unless stated. Full suite: `COMMON_HOME=$PWD/.common COMMON_REQUIRE_FIXTURES=1 cargo test --workspace --exclude common-desktop` → **186 passed, 0 failed** (no silent skips: `COMMON_REQUIRE_FIXTURES=1` turns every fixture skip into a failure); `cargo fmt --check` and `cargo clippy --all-targets -D warnings` clean.

### F-01-01 — hardware probe
- Tests: `common-hardware` `matches_sysctl_on_this_mac`, `report_json_roundtrip`, fixture parsers (14); `common-cli` `hardware_json_matches_sysctl`, `hardware_text_lists_every_backend`.
- Result: PASS — chip "Apple M2", 8 physical / 8 logical cores, 8,589,934,592 B total, Metal GPU 10 cores, backends cpu+metal available, cuda/vulkan unavailable with reasons; probe 41–56 ms (< 2 s bound); unprivileged. Limitation: Linux/Windows paths compiled and fixture-tested only (B-004).

### F-01-02 — GGUF load, golden tokens, streaming, fuzz
- Tests: `common-runtime` `cpu_load_golden_text_and_conformance`, `metal_load_golden_text_and_conformance` (greedy 32-token continuation identical on both devices: " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big"), conformance `greedy_is_deterministic`, `streaming_delivers_before_done`, `usage_counts_match`; `corrupt_file_is_rejected_before_any_spawn`; `common-models` `fuzz_1000_mutated_headers_never_panic_fixture_prefix` and `_synthetic` (2 × 1,000 deterministic mutations under catch_unwind: zero panics, every failure `model_file_invalid`); `common-cli` `replay_manifest_reproduces_tokens` (token ids identical on replay).
- Result: PASS.

### F-01-03 / F-01-08 — memory estimate and KV plan vs measured engine footprint
- Test: `common-models` `tests/estimate_vs_measured.rs` (`estimate_matches_engine_log_and_peak_footprint`, `real_fixture_refusal_reasons`); engine started with `--load-mode none`, context filled, peak `phys_footprint` sampled.
- Result: PASS — 14 model × context × KV × device combinations; weights and KV bytes match the engine log exactly (KV delta −2.3…0.0 %); total estimate vs measured peak footprint within −4.6…+8.9 % (bound ±15 %). Examples (MiB, estimate/measured): qwen2.5-0.5b q4_k_m ctx 4096 Metal 767/769; smollm2-360m q8_0 ctx 8192 Metal 871/876; qwen2.5-1.5b q4_k_m ctx 4096 Metal 1464/1492; stories260K ctx 128 CPU 48/44 (+8.9 %). Requested context above the trained context is refused (`ContextTooLong`) or capped with an explicit `context_note`; a prompt longer than n_ctx returns `context_too_long` (`prompt_longer_than_context_is_context_too_long`). Refusal before load with a plain-language reason: `real_fixture_refusal_reasons`, API `status_ready_models_and_admin_validation`.
- Limitation: calibrated for llama-server b11609 on Apple Silicon; re-run on every pin bump. `available_bytes` (vm_stat free+inactive+speculative) is conservative under memory pressure.

### F-01-04 — Metal by default, proof of offload, CPU fallback, CPU vs Metal numbers
- Tests: `default_device_follows_probe`, `metal_load_golden_text_and_conformance` (log-proven `MTL0 (Apple M2)`, 7/7 layers offloaded), `auto_falls_back_to_cpu_with_a_reason` + `gpu_preference_is_a_typed_error` (forced GPU start failure via `COMMON_TEST_FORCE_GPU_FAIL=1`: Auto reloads on CPU with `fallback_reason` set and a warning; Gpu preference returns `gpu_unavailable`).
- Benchmarks: `common bench <model> --device cpu|gpu --runs 3 -n 64` → `benchmarks/results/20261014T16*.json` (hardware manifest, command line, versions, per-run samples, medians, peak RSS):

| model | device | layers offloaded | TTFT ms | prefill tok/s | decode tok/s | peak RSS MB |
|---|---|---|---|---|---|---|
| stories15M-q4_0 | cpu | 0/6 | 4.1 | 21,570 | 2,229 | 132 |
| stories15M-q4_0 | Metal | 7/6 (+output) | 4.4 | 14,819 | 403 | 113 |
| qwen2.5-0.5b-instruct-q4_k_m | cpu | 0/24 | 158.6 | 299 | 84.0 | 690 |
| qwen2.5-0.5b-instruct-q4_k_m | Metal | 25/24 | 36.8 | 1,312 | 122.7 | 546 |
| smollm2-360m-instruct-q8_0 | cpu | 0/32 | 68.8 | 719 | 80.5 | 716 |
| smollm2-360m-instruct-q8_0 | Metal | 33/32 | 30.2 | 1,718 | 117.7 | 545 |

- Result: PASS. Metal is faster on real models (decode +46 %, prefill 4.4× on the 0.5B); the 15M fixture is faster on CPU (kernel launch overhead dominates). Three runs each on an otherwise quiet machine; not a controlled study.

### F-01-05 / F-05-01 / F-05-02 / F-05-04 — downloads, integrity, cache, resume, license
- Tests (`common-artifacts`, in-process HTTP server): `full_download_then_cache_hit` (second pull: 0 network bytes), `corrupted_byte_is_rejected_and_partial_deleted`, `cut_at_half_then_resume` (network bytes ≤ remaining + 1 MiB), `server_ignoring_range_restarts_from_zero`, `http_url_refused_before_any_network`, `malicious_archives_extract_nothing`, `safe_archive_extracts_verifies_and_detects_tampering`, `verifies_real_llama_cpp_backend_in_common_home`; `common-runtime` `tampered_blob_is_integrity_mismatch_before_any_spawn` (blob re-hashed against the manifest before every load).
- Real network (this session): `common models pull stories260K --yes` after deleting the blob → downloaded from Hugging Face and verified sha256 `270cba1b…`; `common models pull qwen2.5-0.5b-instruct-q2_k --yes` killed (SIGKILL) at 156,140,470 bytes → second pull resumed at 156.1 MB, completed 415.2 MB, `common models verify` OK, partial directory empty afterwards.
- License: `common models list` shows a license for all 8 catalog models ("unknown" for the TinyStories fixtures); `common models pull` prints the license before any byte is fetched; the desktop download dialog quotes it (DESKTOP-E2E). `common models import` records `general.license` from the GGUF when present.
- Result: PASS.

### F-08-01 / F-08-02 / F-08-03 / F-08-08 — timeouts, cancellation, errors, admission
- `stalled_engine_is_a_typed_timeout_within_bound` (FAKE SSE engine that stalls; typed `timeout` within bound + 1 s); generation is never retried.
- Cancellation: conformance `cancellation_within_200ms` and `cancel_mid_generation_releases_the_engine_slot` (stream ends ~10 µs after cancel; engine slot idle in ~1.2 ms); API `client_disconnect_cancels_engine_on_fixture` (engine idle 13 ms after the client dropped the stream); CLI `sigint_mid_generation_stops_and_exits_130` / `sigterm_…` (63 ms from signal to exit, no orphaned engine), `sigterm_to_serve_stops_the_loaded_engine`.
- Errors: `error_codes_are_stable` pins (code, HTTP status, OpenAI type) for all 19 codes; `errors_have_openai_shape_and_stable_codes`.
- Admission: `overload_at_10x_capacity_is_bounded` (max_concurrent 4, max_queue 8, 40 concurrent → exactly 12 × 200 and 28 × 429, `common_requests_rejected_total{reason="queue_full"}` = 28); `admission_control_on_fixture` (real engine); `oversize body` → 413. Live flood (this session): `common serve --max-concurrent 2`, 20 concurrent 96-token completions → all admitted via the queue, daemon RSS 9 → 11 MB peak (engine memory is fixed by its slots).
- Result: PASS. Limitation: KV memory "returns to pre-request level" is shown by slot release, not by an RSS delta.

### F-09-01 / F-09-02 / F-09-03 / F-09-04 — OpenAI-compatible API
- `openai_python_client_smoke` (openai 3.31.0) and `openai_js_client_smoke` (openai npm 6.49.0): models, completions (golden text), chat, streaming (> 1 chunk, finish chunk, `[DONE]`), typed 404/400 — unmodified clients, only `base_url`. CLI `run_cpu_prints_golden_text_and_writes_manifest`.
- Streaming: `sse_stream_framing_on_fixture`, `chat_endpoint_streams_with_usage`; disconnect cancels (above).
- Metrics: `metrics_are_prometheus_text_and_never_contain_prompt`; `promtool check metrics` exit 0 on a live scrape (this session).
- OpenAPI: `route_inventory_matches_openapi_document` (both directions, 17 paths); `openapi-spec-validator` 3.1 validation VALID (this session, `.venv`).
- Result: PASS.

### F-06-06 — secure defaults (SD-01..SD-12)
- `default_bind_is_loopback_and_public_bind_without_key_is_refused`, `auth_required_on_every_route_except_health`, `cross_origin_and_foreign_host_requests_are_refused`, `environment_is_scrubbed`, `key_file_is_owner_only_and_never_overwritten` (engine key via 0600 file, not argv), engine log asserted free of prompt text and key (`cpu_load_golden_text_and_conformance`), `malicious_archives_extract_nothing`, oversize-body 413, `safe_names`.
- Result: PASS. Threat model: `docs/security/THREAT_MODEL.md`.

### X-15 — execution manifests and replay
- `manifest_omits_prompt_unless_saved_and_writes_to_runs`, `replay_args_pin_what_ran_and_need_the_prompt`, `replay_manifest_reproduces_tokens` (`common replay <manifest.json>` regenerates the identical token ids on the same host/backend).
- Result: PASS.

### F-10-02 / F-10-03 / F-10-06 — desktop (vitest + jsdom, plus DESKTOP-E2E above)
- `apps/desktop`: 43 tests — catalog search/filter and fit text, download confirmation shows the license before `/pull`, chat renders streamed deltas incrementally and Stop aborts in < 300 ms (mock daemon), error table equals `ErrorCode::ALL` with `user_message` text (parses `error.rs`), axe 0 violations on 9 views, keyboard-only primary flow.
- Result: PASS (UI-level). Real-daemon streaming and Stop observed in DESKTOP-E2E; the packaged app is covered under F-10-01 (pending).

### F-10-01 — packaged desktop app (macOS arm64, ad-hoc signed)
- Date: 2026-10-14 · label: REAL (developer machine, not a fresh user account)
- Build: `cargo build --release -p common-cli && cd apps/desktop && bash scripts/prepare-sidecar.sh && pnpm tauri build --bundles app,dmg` → `target/release/bundle/macos/Common.app` (9.4 MiB; `Contents/MacOS/common-desktop` + sidecar `Contents/MacOS/common`) and `Common_0.1.0_aarch64.dmg` (4.6 MiB), sha256 `88c50b632f85390e127342a87fe3deb6b1e198230481dfa441d0200931cc3843`. `codesign -dv`: adhoc, linker-signed (no Developer ID, B-008).
- Run: launched `Common.app` with no daemon running and an empty default data dir (`~/Library/Application Support/Common`). The app spawned the sidecar (`common serve --host 127.0.0.1 --port 8192`), which auto-downloaded and verified the pinned llama.cpp b11609 archive (12.1 MB) and reported healthy. Through that sidecar: `POST /api/models/stories15M-q4_0/pull` (19,077,344 bytes, verified), `POST /api/models/stories15M-q4_0/load` → `MTL0 (Apple M2)`, 7/6 layers offloaded, and `/v1/completions` returned the golden continuation " named Benny was hopping in a field. He saw a big, red" at 376 tok/s decode. The same steps are what the Models/Chat views call (DESKTOP-E2E).
- Defects found: (1) the sidecar name passed to Tauri had to be the basename (fixed in 18d936b); (2) a fresh data dir refused to serve without `common setup` (fixed in 8dac23e: first run self-provisions); (3) killing the app with SIGTERM left the sidecar daemon running (fixed: `common serve --exit-with-parent`, see next commit).
- Status: IMPLEMENTED_UNVERIFIED until the install → select → first-token flow is repeated from a fresh macOS user account through the UI alone, and until artifacts are checksummed in a release process (Gate G).
