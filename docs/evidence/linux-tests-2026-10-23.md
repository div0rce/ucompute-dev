# Daemon crates' tests on Linux (sentinel), 2026-10-23 — SINGLE-HOST

First run of the documented test suite on Linux: `COMMON_HOME=$HOME/.common-acceptance COMMON_MDNS=0 cargo test
--no-fail-fast` (the workspace's default members, the daemon crates; the desktop crate is built separately and needs
GTK) on sentinel (Ubuntu 26.04 LTS x86_64, Intel Core i9-9900K, 14 GiB, `docs/environment/linux-test-host.md`) at
c811a90, whose code equals ce54da1 (the Linux CPU engine pin, D-024). The acceptance home held the engine and the
stories15M and Qwen2.5 models; `COMMON_REQUIRE_FIXTURES` was not set, so a missing fixture would skip, not fail.
Load average about 1-6 (the machine was otherwise idle). Log: `linux-tests-2026-10-23/cargo-test-c811a90.log.gz`.

**446 passed, 5 failed, 2 ignored** (the two ignored are the child-process helpers, as on macOS).

| Failure | What it shows |
|---|---|
| `common-cli` `reaper_stops_detached_engine` (0 of 1 orphan found) | The orphan reaper treats an engine as orphaned only when its parent is pid 1 (`main.rs` ~255). Under a subreaper (`systemd --user` on Ubuntu desktop sessions) an orphan's parent is not 1, so a crashed daemon's engines are not reaped on Linux. Product defect |
| `common-api` `engine_state_shows_a_real_load_in_progress_on_fixture` | `GET /api/status` did not answer within 1 s during a model load on Linux (`real_engine.rs:572`). To be diagnosed |
| `common-artifacts` `verifies_real_llama_cpp_backend_in_common_home` | The test hard-codes the macOS arm64 engine archive. Test defect |
| `common-api` `peer_flood_throttled_through_daemon_single_host` (local p95 ×1.830) and `common-network` `peer_flood_throttled_local_p95_within_20pct_single_host` (×1.601) | On a quiet 16-thread host with a CPU-only engine, local latency under a throttled peer flood exceeds the 1.2 bound; on the M2 (Metal) these fail only under load (`docs/VERIFICATION.md`, determination). Indicates compute contention that slot reservation alone does not prevent on CPU engines (F-06-04). To be determined |

Everything else passed on Linux, including the real-engine golden, conformance, cancellation, OpenAI Python and
JavaScript client smoke tests, the mesh SINGLE-HOST suite and the pairing, discovery and peer-transport tests.
