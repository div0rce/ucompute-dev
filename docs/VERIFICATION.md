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

Current summary reconciled 2026-10-18, gates E and G updated 2026-10-19 for af3e37c, against the committed evidence below and docs/evidence/desktop-two-machine-2026-10-19.md; this documentation review did not re-run runtime qualification. Historical entries retain their dates and limitations. Release cohort targets are in SPEC_HANDOFF_V2 §§31–33.

| Gate | Scope | State | Evidence |
|---|---|---|---|
| A — Build | build, lint, fmt, dependency/license and secrets checks | Historical builds/tests exist; no current cross-platform release certification | M1/M3 entries; Windows checks; CI definitions are not an executed pipeline |
| B — Inference | real load, generation, streaming, cancellation, cleanup | Bounded real CPU/Metal and Windows Arc/Vulkan evidence; current artifact qualification still required | M1, packaged macOS, physical M3 reports |
| C — Network | discovery, auth, revocation, disconnects, malformed messages | SINGLE-HOST tests plus physical Mac/Windows Mode B and hotspot discovery; full default-quota/platform security qualification open | [Physical run](evidence/m3-two-machine-2026-10-18.md); B-014/015/017 |
| D — Pooling | partitioned execution, correctness, memory, failure semantics | Not implemented or verified; whole-request remote execution does not satisfy it | ADR-0003 D6; B-016 |
| E — Product | install, select, run, pair, pool, diagnose unaided | Partial: packaged Mac evidence; installed macOS and Windows apps paired through their UIs; the Windows app ran a prompt on the Mac; a Mac daemon-API request ran on the Windows GPU (af3e37c, defects open); fresh-account UI, Windows accessibility, Linux and independent-user study incomplete | Desktop evidence; [desktop two-machine](evidence/desktop-two-machine-2026-10-19.md); F-10-01/05/11 remain open |
| F — Public swarm | adversarial review, accounting, privacy, abuse, churn | Not verified; public contribution remains disabled | Threat model and frontier research; no deployment approval |
| G — Release | signed artifacts, provenance, compatibility, CI, docs, benchmarks | Checksummed ad-hoc-signed Mac artifact and a checksummed native Windows NSIS development build from caf1497 (sha256 df13fe79…; evidence af3e37c) without a code-signing certificate (B-008); stable/public gate incomplete | B-007/008/018; no stable cross-platform release claim |

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

## M3 evidence, interim (2026-10-14, commits 5034fb6 + 107b34b)

Everything in this section is **SINGLE-HOST**: two or more Common nodes running on the owner's one Mac over loopback. It proves protocol and policy behaviour, not two-machine operation (B-002). Suites: `cargo test -p common-identity -p common-policy -p common-observability -p common-network` with `COMMON_HOME=.common COMMON_REQUIRE_FIXTURES=1` → 35 + 57 tests pass; clippy clean.

### F-06-01 — device identity
- `keygen_persist_load`, `file_permissions_0600` (dir 0700, key 0600), `private_key_never_logged` (Debug/Display of every type scanned for the secret), compile_fail doctest proves `DeviceIdentity` has no `Serialize`.

### F-02-03 / F-06-03 — encrypted, mutually authenticated transport
- rustls 0.23 raw public keys (RFC 7250) over quinn 0.11, TLS 1.3 only: `handshake_pinned_keys_succeeds_single_host`, `unpinned_key_rejected` (alert inside the handshake, no stream ever opens), `revoked_key_rejected`, `wrong_alpn_fails`, `tls12_downgrade_attempt_fails`, `no_plaintext_mode_exists` (source scan: the quinn Endpoint is the only socket), `packet_capture_contains_no_plaintext_prompt_single_host` (test-only UDP relay records every datagram of a remote generation; canary prompt and golden completion absent in UTF-8 and UTF-16; positive control finds the canary in the plaintext frame).
- Limitation: reqwest's feature unification compiles rustls `tls12` into the binary; no Common config enables it.

### F-02-04 / F-02-05 / F-08-10 — capabilities, liveness, versioning
- `capabilities_exchanged_after_handshake` (received `hardware_json` equals the sender's probe), `oversized_capability_message_rejected` (64 KiB + 1 → `frame_too_large`), `protocol_major_mismatch_is_typed`, `minor_negotiation_is_min`, prost unknown-field tolerance; `kill_peer_detected_within_bound_single_host`: 6.50 s observed (bound 7 s; keep-alive 2 s, idle 6 s).

### F-02-01 — LAN discovery
- Signed TXT record (id, pair flag, proto, ts, Ed25519 sig; no hostnames/models), tampered and expired records ignored, pinned peers flagged trusted; `two_instances_discover_within_5s_single_host` passes on this Mac (mdns-sd two instances on one host).

### F-02-02 — pairing
- `correct_code_pairs_and_pins_both_sides_single_host` (QUIC/TLS RPK + SPAKE2 + HMAC confirmation over `export_keying_material` and both peer ids): 20–44 ms end to end on loopback; `wrong_code_fails_and_pins_nothing`; `brute_force_rate_limited` (6 wrong codes → `pairing_locked`, 7th connection refused); `mitm_relay_detected` (relay terminating TLS on both legs fails confirmation on both sides); QR payload parse.

### F-06-02 / F-06-07 / F-02-09 — policy and groups (unit + property)
- `default_deny` and `local_only_never_leaves_device_fuzz`: 2 × 2,000 random (config, store, request) combinations, zero violations; `workload_only_to_group_members`, `non_member_rejected`; remote path: `local_only_privacy_never_leaves_device` (policy denies before any stream opens), `unauthorized_peer_is_refused` (no load happens on the server).

### Mode B — remote execution (spec section 7 B; not pooled inference)
- `remote_golden_text_single_host` (greedy 32 tokens on the peer equal the golden text and a local run), `remote_streaming_delivers_before_done`, `remote_cancel_stops_server_generation_within_500ms`, `peer_lost_mid_stream_is_typed`; server verifies the requested model hash against its manifest and the loaded model (no substitution); audit entries carry ids, sizes and durations only.

## M3 review fixes and SINGLE-HOST daemon run (2026-10-15, commit 5e5f10d)

- Three adversarial reviews (security, correctness, acceptance) raised 45 findings; fixes landed in 5e5f10d. Notable: TLS session resumption bypassed the pinned/revoked verifier (now tested by `revoked_client_cannot_resume_with_cached_session`); PeerStore lost cross-process revocations; capabilities were verified over a re-encoding.
- Suites after fixes, run by the integrator: `COMMON_HOME=.common COMMON_REQUIRE_FIXTURES=1 cargo test --workspace` → 334 passed, 0 failed; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all --check` clean; `apps/desktop: pnpm test` → 68 passed; `pnpm build` ok.
- Two-daemon run on this Mac, driven by `tests/distributed/single_host/m3_evidence.py`: report in `docs/evidence/m3-single-host-2026-10-15.md`. All 8 steps passed: discovery 1.0 s after both ready; pair join 25.7 ms; 6 wrong codes → `pairing_locked`; remote completion golden (warm 86 ms total, 4.8 ms first token, ~420–468 tok/s); local_only → `policy_denied` with zero server entries; revoke → `peer_revoked`, peer connection closed ≤0.01 s, reconnects refused in the handshake; SIGKILL detected in 4.855 s; relay capture 0 plaintext hits over 11 needles; decode TPOT overhead −0.5 %; local p95 +11.6 % under a peer flood (crate-level test).
- **All of the above is SINGLE-HOST.** It is not two-machine evidence (B-002).

## M3 follow-ups and final suite (2026-10-15, commits 2434bde, 68e73f0)

- Follow-up workflow: peer admission through the daemon (one engine slot always local), QR identity in the handshake, one connection per peer pair, audit rotation with security events kept, sender audit before send in API and CLI, shared schema bounds, `control.proto` drift test. Two reviews raised 16 findings, every one checked by a skeptic; all fixed.
- New tests with numbers (SINGLE-HOST): `local_only_never_leaves_device_10k_single_host` 10,000 requests, 0 violations, policy p99 114 µs; `privacy_mode_enforced_end_to_end_single_host` 400 requests in local_only, 0 request frames; `peer_flood_throttled_through_daemon_single_host` local p95 ×1.065–1.127 (2 slots, 1 for peers); `private_key_absent_from_real_logs_and_bundle_single_host` 0 hits in 10 encodings with a synthetic positive control.
- Flaky test fixed: the capture test generated 32 tokens and sometimes had fewer than 3 searchable words (the random canary changes the completion); it now generates 96 (8/8 runs, 15–21 words, 0 hits).
- Defects found by running the product, fixed in 68e73f0: (1) a second `PUT /api/network` while enabled failed with EADDRINUSE and turned networking off (regression test on a fixed port); (2) the CLI mesh test leaked 4 daemons and 4 engines after failed runs (`--exit-with-parent` on every spawned daemon).
- Default-settings two-daemon rerun and desktop UI pairing: `docs/evidence/m3-ui-pairing-and-defaults-2026-10-15.md`.
- Final suite at 68e73f0: `COMMON_HOME=.common COMMON_REQUIRE_FIXTURES=1 cargo test --workspace --no-fail-fast` → **364 passed, 0 failed** (41 test binaries); `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all --check` clean; desktop `pnpm test` 77 passed, `pnpm build` ok; no daemon or engine left running afterwards.
- Open: one `500 backend_failed` among ~130,000 flood requests in an earlier full run (B-015), not reproduced.

## Packaged desktop app: stale daemon fix (2026-10-15, commit 5f0c580)

- Report from another Apple Silicon Mac: the bundled daemon rejected `--exit-with-parent` (exit 2) and the app never got a daemon. Root cause: `tauri build` bundled a daemon built before the flag existed; the build never rebuilt it. Full diagnosis, fix and results: `docs/evidence/desktop-sidecar-fix-2026-10-15.md`.
- Also fixed: quit SIGKILLed the daemon and orphaned its engine; a crashed app made the daemon die on SIGPIPE and orphan its engine; the bundle was not signed.
- `COMMON_HOME=.common COMMON_REQUIRE_FIXTURES=1 cargo test -p common-cli --test desktop_sidecar` → 3 passed (the lifecycle test failed before the stdio fix with "engine stops with the daemon not within 5s").
- `cd apps/desktop && pnpm tauri build --bundles app,dmg` → the build step reports `build-sidecar: staged daemon accepts: serve --host 127.0.0.1 --port 8192 --exit-with-parent`; signed with identity `-`.
- `python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/Common_0.1.0_aarch64.dmg --built target/release --work <empty>` → ALL PASS (15 checks): dmg sha256 `7cc481d1c821625eadf00e0dcc3fbca50ab9e59041114b819efed2250ef934c2`; daemon `3168e521…8286d`, shell `c66c8c35…ea01`; fresh-HOME install healthy in 2.4 s with the engine downloaded; golden completion on Metal at 418–438 tok/s; quit, restart, crash (daemon and engine gone in 0.52 s) and restart leave nothing running.
- Workspace at 5f0c580: `cargo test --workspace --no-fail-fast` → **367 passed, 0 failed**; clippy `-D warnings` and fmt clean; desktop 77 passed, build ok; no daemon or engine left running.
- Limits: one Mac (fresh HOME, not a fresh user account or second Mac); ad-hoc signed, not notarized, so `spctl` rejects it and a downloaded copy needs "Open Anyway" once (B-008).

## Packaged desktop app: clean rebuild and re-verification (2026-10-17, commit 08e267e)

- `target/` had been cleaned, so the `.app` and `.dmg` were rebuilt from the committed source: `cd apps/desktop && pnpm tauri build --bundles app,dmg` → `build-sidecar: staged daemon accepts: serve --host 127.0.0.1 --port 8192 --exit-with-parent`; bundle and both executables signed with identity `-`.
- The rebuilt executables are byte-identical to the 2026-10-15 verified build and to the copy installed in `/Applications`: `common` `3168e521de50cc514d926a5f4839112c1fa1f04a122103592a1f4b160558286d`, `common-desktop` `c66c8c3570d7b855a577c09ca429a2b3314829c6565d211f1ef4bde6c965ea01`. The new `Common_0.1.0_aarch64.dmg` is `de28776842120d97b197d7e99666b4b3fee34bf36de05fe0ace72429f5af1c31` (6,002,232 bytes); the image differs from the earlier one only in its own metadata.
- `cargo test -p common-cli --test desktop_sidecar` (fixtures required) → 3 passed: daemon healthy 5.0 s, exited 0.86 s after its parent was SIGKILLed with its engine stopped, restart healthy 3.9 s, SIGTERM stop 55 ms, no orphaned engine.
- `python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/Common_0.1.0_aarch64.dmg --built target/release --work <empty>` → **ALL PASS** (15 checks): fresh-HOME install healthy in 4.0 s with the engine downloaded; golden completion on `MTL0 (Apple M2)` at 401 and 323 tok/s (first launch, restart); quit, restart, crash (daemon and engine gone in 1.0 s) and restart leave nothing running.
- `cargo test --workspace --no-fail-fast` (fixtures required) → **367 passed, 0 failed**; clippy `-D warnings` and fmt clean; desktop 77 passed, build ok.
- Observed, not changed: the installed app paired with a second Mac on the LAN on 2026-10-15 (pairing succeeded twice; that Mac's connections to this one worked; this Mac's 441 dials to it failed with `peer_lost`; no group was created, so no remote request ran). See docs/PROGRESS.md.

## Windows daemon and first two-machine M3 run (2026-10-18)

- Windows: `cargo build --release --target x86_64-pc-windows-gnu -p common-cli` (MinGW-w64, no MSVC license needed); `common.exe` imports only Windows system DLLs. On the HP laptop (Windows 11 Home 26200, Core Ultra 7 258V, Arc 140V): `common hardware` reports the right CPU, memory, GPU, Vulkan and disk; `common setup` downloads and verifies the win-vulkan-x64 engine (first real use of the .zip path); `common run stories15M-q4_0 ... --temp 0` gives the golden continuation on `Vulkan0 (Intel(R) Arc(TM) 140V GPU (16GB))`, 565 tok/s decode warm.
- Windows defects fixed: disk free read 0; GPU names with nested parentheses cut short; a killed daemon left `llama-server.exe` running (now a kill-on-close job object; verified on the laptop: engine gone 1.5 s after `taskkill /F` of the daemon alone).
- Two-machine run (Mac + laptop, eduroam), `tests/distributed/two_machine/mesh_two_machine.py`, twice: pairing, remote execution in both directions (golden, JSON and SSE), local_only refusal, prompt-free audit logs, revocation (0.012 s) with re-pairing, and loss detection (4.19 / 4.23 s) with reconnection all PASS. Discovery FAILS because eduroam drops mDNS between clients (confirmed with the OS mDNS stacks). Details: `docs/evidence/m3-two-machine-2026-10-18.md`.
- Mac: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy --target x86_64-pc-windows-gnu -p common-cli -p common-runtime -p common-hardware -p common-artifacts -- -D warnings` clean; hardware 17, artifacts 33 (incl. the real Windows zip), runtime log parser tests pass.

## Two machines: hotspot discovery and installed desktop apps (2026-10-19)

- F-02-01 — LAN discovery, TWO-MACHINE (commits 88183a9, 1a36b3c): Mac (Apple M2, macOS 26.2) and the HP laptop (Windows 11, Arc 140V) on one iPhone personal hotspot, daemons built from bf40e20, `tests/distributed/two_machine/mesh_two_machine.py`. Each daemon listed the other within about 1 s, unpaired entries `trusted: false`, a request to the unpaired peer refused with 403 `peer_unauthorized`; pairing, remote execution both ways, local_only refusal, prompt-free audit, revocation and reconnection PASS. On eduroam discovery fails because the network drops multicast between clients. Details: `docs/evidence/m3-two-machine-2026-10-18.md` (run 3).
- DESKTOP-TWO-MACHINE — installed apps (commit af3e37c): `Common_0.1.0_x64-setup.exe` built natively on the laptop from caf1497 (`pnpm tauri build --bundles nsis`, sha256 `df13fe79…`), silent per-user install, the bundled `common.exe` accepts the app's exact daemon arguments, app crash (`Stop-Process -Force`) ends its daemon in 0.11 s. The owner paired the macOS app (`/Applications/Common.app`, 5f0c580) and the Windows app through their UIs; the wrong-code message was shown in the Windows app (entered through its DevTools port). A prompt from the Windows app ran on the Mac (Metal, 508 tok/s, stories15M fixture); a Mac daemon-API request ran on the Arc GPU (910 tok/s). Fixture throughput is a path check, not performance. Five desktop defects found, open at that commit. Details: `docs/evidence/desktop-two-machine-2026-10-19.md`.
- Pairing initiated on Windows (2026-10-19, installed apps from the run above, not yet fixed): the Windows app showed a code at `172.20.10.6:62448`; the Mac's join timed out after 4 s (`pairing_failed`, "idle timeout"). `pktmon` on the laptop logged the Mac's QUIC Initial packets to that port as `Drop … DropReason INET: receive inspection`, so Windows Firewall discards them (inbound rules exist only for UDP 8193 and 5353 on the Private profile; default inbound action block; no program rule, and no Windows notification events for `common.exe`). Pairing initiated on the Mac and joined from Windows works (run above). Firewall policy, not an application defect; no firewall setting was changed.

## UCompute program: ledger and document integrity (2026-10-19)

### PROGRAM-UCOMPUTE — additive infrastructure program: ledger and document integrity
- Date / commit: 2026-10-19 / working tree on 915562a (baseline ddc39f6; committed as 915562a and the completion commit that follows 915562a)
- Host: owner's Mac (Apple M2, 8 GB, macOS 26.2) · label: REAL (documentation and registry tooling only; no product code)
- Command: `python3 scripts/validate_registry.py` → exit 0: `registry valid: 148 P0 rows (original); 4 added; 152 total; NOT_STARTED=105, IN_PROGRESS=3, BLOCKED=0, IMPLEMENTED_UNVERIFIED=8, VERIFIED=36, INFEASIBLE_WITH_EVIDENCE=0`
- Command: `python3 -m unittest discover -s tests/registry -q` → `Ran 19 tests … OK`
- Command: `python3 scripts/gen_registry.py --check` → `registry outputs up to date`
- Integrity: `git show ddc39f6:docs/REQUIREMENTS.csv` (80,476 bytes, 148 rows) is a byte prefix of the working ledger (84,446 bytes; only U-01..U-04 appended). `git show ddc39f6:docs/SPEC_HANDOFF_V2.md` (25,523 bytes, §§1–24) is a byte prefix of the working spec (85,525 bytes). The frozen digest of the 148 original rows matches ddc39f6 (f1e23801… for id/priority/description/acceptance; c22f9929… with milestone and dependencies). `git diff --check ddc39f6 -- <program paths>` clean. No commit between ddc39f6 and af3e37c touches the program files.
- Mutation probe (scratch copies of docs/ and scripts/, re-run with the completion edits): rejected: deleted, duplicated or added original rows; changed original description, priority, acceptance, milestone or dependencies; unknown or later-milestone dependencies; U row VERIFIED without evidence or commit; vague U acceptance; BLOCKED without blocker. Accepted (limits): U-row priority, milestone, dependency or acceptance edits (additive rows evolve by reviewed decision); an original row's owner or section label; a well-formed but nonexistent commit hash or evidence path; a deleted ledger, or a deleted leaf row, regenerated to default statuses (D-002 relies on the existing CSV; history is in Git); a VERIFIED row downgraded with evidence erased.
- Review: Codex adversarial review 2026-10-18 found 4 material findings, all fixed: WP-11/Gate F circularity; WP-12 prerequisites; cohort test rejecting U-row progress; U-01 acceptance weaker than §28.2. A 2026-10-19 review of spec coverage, website/distribution and staleness against af3e37c produced the completion edits.
- Scope: proves ledger structure, preservation of the original scope and document consistency only. No runtime, platform or security re-verification; no requirement status changed.

### CREDITS-D019 — unified compute credits with contributor consent: ledger and document integrity
- Date / commit: 2026-10-19 / working tree on e76e7fc (replaces, in the same uncommitted working tree, the superseded "Free Compute Commons" version of D-019/U-05)
- Host: owner's Mac (Apple M2, 8 GB, macOS 26.2) · label: REAL (documentation and registry only; no product code)
- Command: `python3 scripts/gen_registry.py` → `wrote 153 rows -> docs/REQUIREMENTS.csv, docs/requirements/REGISTRY.md`
- Command: `python3 scripts/validate_registry.py` → exit 0: `registry valid: 148 P0 rows (original); 5 added; 153 total; NOT_STARTED=106, IN_PROGRESS=3, BLOCKED=0, IMPLEMENTED_UNVERIFIED=8, VERIFIED=36, INFEASIBLE_WITH_EVIDENCE=0`
- Command: `python3 -m unittest discover -s tests/registry -q` → `Ran 19 tests … OK` (cohort assertion 5 added / 153 total)
- Command: `python3 scripts/gen_registry.py --check` → `registry outputs up to date`
- Integrity: `git show ddc39f6:docs/SPEC_HANDOFF_V2.md` (25,523 bytes) is a byte prefix of the working spec (113,475 bytes). `git show e76e7fc:docs/REQUIREMENTS.csv` (85,008 bytes) is a byte prefix of the working ledger (92,211 bytes; one line, U-05, appended). The validator's frozen digest of the 148 original rows' id/priority/milestone/dependencies/description/acceptance matches ddc39f6. `git diff --check` clean on the changed documentation and scripts. A `grep` for the superseded wording (commons, commercial-provider enrollment, community terms, newcomer allowance) in AGENTS.md, README.md, CONTRIBUTING.md, docs/ (outside docs/evidence/) and scripts/ finds it only where D-019's rationale, the PROGRESS entry and this entry name the replaced draft.
- Mutation probe (scratch copy of docs/ and scripts/): adding F-09-11 (M13) to U-05's dependencies → `REGISTRY INVALID: … U-05 (M10): depends on F-09-11 in later milestone M13`.
- Review: the owner-requested independent review (2026-10-19; commercial feasibility, contributor consent, credit accounting, security, open-source consistency, fidelity to the owner's text) produced 36 findings, each confirmed by a skeptic pass and applied: verification priced at the rate F-07-09's acceptance implies (r ≈ 29.5%, illustrative margin ≈ −$1.71, fee covers it only if a check costs ≤ ~32% of a run) and funded as a platform or requester spend, never minted; stage 1 gated on H9 demand evidence, a capped pilot, counsel review (B-006) and processor reconciliation, stage 2 on pre-registered repeat paid use; usage-billed credits counted when collected, contributor shares pending until then, chargebacks debiting the purchaser and never honest contributors, losses restored to the reserve (payment loss L_t), verification spend V_t, holder-balance reconciliation with system accounts netting to zero, reserve backing requirement Q*_t with typed refusal, source lots with a published consumption order, recorded issuer key and owner-signed grant budgets; unit contribution π_t split from fixed cost K_t (break-even S*), fair-access platform cost, contributor κ_t separate from Cloud, grant-to-paid conversion, credits-only contributor value (1 − f)·p·u·ρ; sharing modes with terms shown and confirmed (re-confirmed on change), loosening only by interactive owner confirmation on the device, hard limits required before donate or share for credits, enforcement on the device against a requester-signed funding class (provider_refuses_mode_mismatch), offline restriction with drain-or-stop, per-job F-07-05 history; charges from requester-verifiable metered units within a published ceiling and only by owner-granted clients; Sybil-resistant grant weighting with a per-period budget cap; self-hosted accounting group-scoped and rejected on the shared ledger; the engineering guardrails attributed as such in D-019 and §26.5; F-09-11 removed from the economy list (its scope is tool integrations); cryptocurrency and blockchain worded as never required and not implemented at this stage (F-07-12 unaffected); README, AGENTS and CONTRIBUTING wording aligned. U-05 dependencies gained F-06-01/02/05/10, F-07-05, X-05 and X-06 (all at or before M10). A documentation review, not product verification.
- Scope: records the revised D-019 (spec §§26.5, 29.4) and U-05 as NOT_STARTED. No runtime, platform, security or economic verification; no credits, payments, Swarm or Cloud behavior exists; every economic figure is a hypothesis; no requirement status changed.
