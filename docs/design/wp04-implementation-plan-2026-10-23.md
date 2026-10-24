# WP-04 implementation plan (2026-10-23)

Working plan produced by an engineering-session workflow on 2026-10-23 (read-only audit and synthesis; not a decision record). Decisions live in docs/DECISIONS.md and docs/decisions/; status lives in docs/REQUIREMENTS.csv and docs/VERIFICATION.md.

# WP-04 synthesis: split inference over the authenticated transport

This is a read-only synthesis. Nothing was edited, built, run or committed. Code facts were re-checked at HEAD `177f7ed`. The working tree also holds uncommitted fix-round changes in `common-runtime`, `common-network`, `common-api` and `common-cli`, so line numbers will move. The plan starts only after that in-flight round is committed.

## 0. Decisions taken across the three designs and two judges

| Question | Chosen | Source | Why |
|---|---|---|---|
| Where the bridge lives | `crates/common-network/src/rpc.rs`, admitted through `tests/invariants.rs` `ALLOWED_MODULES` (empty today). The ledger names `crates/common-distributed`; ADR-0005 records the deviation. | D2 | `common-network` already depends on `common-runtime` (`ModelManager`) and `common-policy` (see `serve_peer_with`, `remote.rs:687`). No new crate and no injection hook are needed. |
| Split engine | A typed `LoadRequest.split` inside the existing `ModelManager` slot. There is no parallel `PoolSession`. | D2 | I-05 still holds: a `local` or `local_only` request has different load options, so the manager reloads locally and the split session closes. A test pins this. |
| Flag hole | Add `--rpc`, `--device/-dev`, `--tensor-split/-ts`, `--split-mode/-sm`, `-mg/--main-gpu`, `-ngl/--n-gpu-layers`, `--fit`, `--list-devices` and `--no-mmap` to `RESERVED_FLAGS` (`process.rs:22`, 7 entries today). Rewrite the test at `process.rs:433`, which relies on `--rpc` passing through `extra_args`. | D1 + D2 | `validate_extra_args` accepts the two tokens `"--rpc" "1.2.3.4:1"`. In-tree code never reaches it, but it is a latent route to raw RPC on a LAN address. |
| RDMA | Set `GGML_RPC_NO_RDMA=1` on both engines. Fail the load unless the log says `transport: TCP`. | D2 (strings confirmed by both judges) | `clean_environment` removes `GGML_*` but sets nothing, and libggml-rpc says "RDMA auto-negotiate enabled". |
| Weights on disk on R | No `-c`, and `LLAMA_CACHE` is removed from the environment. | D2 | X-08: a cached copy would be a redistribution event. |
| H-side owner check | Required. The bridge gives a QUIC stream only to TCP connections owned by the supervised `llama-server` pid. | D1 | Without it, any local user on H reaches R's unauthenticated rpc-server through Common. |
| R-side owner check | Required. The `127.0.0.1:pR` listener must belong to the child pid before the first byte. At each tunnel open and on the 1 s tick, every established client of pR must be one of the daemon's own sockets. | D1 | `free_port` (`process.rs:134`) binds and drops its socket, so a local process can squat pR. A connect-probe readiness check would then send weights and prompt-invertible activations to the squatter. |
| Mid-session policy | A 1 s tick on both sides re-runs `evaluate` / `may_serve` and the held check. Any Deny tears the session down within 5 s. | D1 | `groups_put` (`mesh.rs:1733`) closes no connection, so turning `allow_pooled_inference` off mid-session would otherwise never take effect. `// ponytail:` polling; switch to events if 1 s is too coarse. |
| Memory cap on R | Linux `RLIMIT_AS`, `MALLOC_ARENA_MAX=2`, `PR_SET_PDEATHSIG(SIGKILL)` and nice 10. Spawn from a tokio worker task, never inside `spawn_blocking`, because PDEATHSIG fires when the parent *thread* exits. After `prctl`, `getppid()` is re-checked. | D1 + judge 2 | A hostile head can send `ALLOC_BUFFER` with no bytes. PDEATHSIG is tied to the thread, which none of the three designs caught. |
| R role platforms | Linux only in this build. A Mac or Windows R refuses with `policy_denied` ("hosting a pooled stage is not available on this platform yet"). | ponytail | It removes the macOS footprint watchdog and Windows `GetExtendedTcpTable` work from the first run. Add them when a Mac-to-Mac matched pair is wanted (OD-3). |
| Failure semantics | Do not depend on llama.cpp aborting. On session loss H calls `manager.unload()` and maps in-flight requests to `peer_lost`, or to `peer_revoked` / `peer_held` / `policy_denied` when the local reason is known. There is no automatic local fallback, which amends ADR-0003 D6. | D2 | It avoids silent substitution. The ⚠️ abort behaviour does not matter. |
| Hung stage | A silence watchdog: a request is in flight on H and no R→H byte arrives for `T_rpc`, so the tunnel is reset. `T_rpc` is pre-registered from R0/S1. | D3 | QUIC keep-alive cannot see a hung `GRAPH_COMPUTE`. |
| Licence | Cross-device split loads need the catalog licence in {Apache-2.0, MIT}. Unknown-licence fixtures (stories15M, stories260K; B-011) are refused. | D1 | RPC uploads R's layer weights (X-08). |
| Test escape hatch | One flag, `COMMON_TEST_SINGLE_HOST_POOL=1`. It lifts the licence gate and the Linux-only R rule **only when the peer's remote address is loopback**. | new | SINGLE-HOST tests can use stories15M and a Mac R, and the flag cannot weaken a real two-machine run. |
| Consent | The split request must carry `"acknowledged": true` against the disclosure text, as `privacy_put` does. Without it: 403 plus the text. Consent on `groups_put` loosening is deferred to Gate E (OD-9). | D1, trimmed | WP-04 needs an explicit disclosure. The group-loosening prompt is a desktop and Gate E concern. |
| Memory evidence | In split mode, drop `--no-kv-offload --no-op-offload` (CPU branch, `process.rs:98`). Run the split head and its local reference both with `--no-mmap`. Measure the Mac with `footprint`. | D3 | F-03-02 needs memory figures that compare like with like. |
| Correctness configurations | **P** (performance and memory): `--device RPC0,MTL0`; R holds blocks [0,k), the head holds [k,n) plus output. **C** (same backend): ggml-cpu on both stages, `--device RPC0 -ngl (n+1-k)`. R then holds [k,n) plus output, so a logits row crosses per token. | D2 + D3 | The F-03-04 wording requires the same backend. Arm64 against x86 CPU kernels is reported as measured and never relabelled (OD-3). |
| Ledger ceilings | F-03-03 stays open (B-016; the ratio is reported). F-03-10 is capped at `IMPLEMENTED_UNVERIFIED` while F-08-04 is `NOT_STARTED`. F-03-01's dependencies F-02-03 and F-01-02 are VERIFIED, so it can reach VERIFIED. | D3 + ledger | It stops anyone mass-marking rows. |

**Not built in this package:** the planner (F-03-05; lab INTEGRATION step 1), pipeline overlap (F-03-06), native frames (F-03-03), the `-c` cache, a Landlock or seccomp sandbox (F-06-08), the macOS and Windows R role, ring dataflow and peer-to-peer policy (not needed for head mode, lab Addendum 1), desktop UI, and a separate `pool_max_bytes` setting (group opt-in plus the memory headroom cover it).

---

## 1. Implementation plan (ordered commits)

**Ground rules for every commit.**
- Every test that runs two daemons on one machine has `single_host` in its name and in its evidence.
- At each commit: `cargo test`, `python3 scripts/validate_registry.py`, and a `docs/VERIFICATION.md` entry.
- Never push.

### C0. Run R0 first (lab-owned, no Common code)
- **What:** run `ucompute-core-lab/rpc-dataflow/PREREGISTRATION.md` as written, with Amendment 1 (`footprint`). It is plaintext on loopback, SINGLE-HOST, with stories15M and configurations L, A, B and C.
- **Additions needed by Common (record only, no pass rule):**
  - (a) the `-lv 4` log text for `layer N assigned to device X` and the handshake `transport:` line, saved as `crates/common-runtime/tests/fixtures/b11609-rpc-split.log`;
  - (b) the RPC device names from `--list-devices` with `--rpc` set (design 2 saw `RPC0`; confirm here);
  - (c) TCP connections per endpoint and whether they are long-lived;
  - (d) the longest silence on the socket during a request, which seeds `T_rpc`;
  - (e) the idle `ggml-rpc-server` footprint;
  - (f) a 64-byte weight pattern from the remote stage's GGUF range, seen in the proxy's plaintext stream. This is the positive control for the P1 capture scan.
- **Estimate:** 1–2 h.
- **Does not claim:** anything two-machine, or the QUIC bridge's overhead.

### C1. `runtime: reserve the split flags` (security fix, lands alone)
- **Files:** `crates/common-runtime/src/llamacpp/process.rs` (`RESERVED_FLAGS` grows to 18 entries; the test at :433 is rewritten to use a harmless flag).
- **Tests:**
  - `extra_args_cannot_set_split_flags`: both the two-token and the `--flag=value` forms of every flag above are refused with `invalid_request`.
  - The existing extra-args test still passes, using a non-reserved flag.
- **Estimate:** 0.5 h.
- **Does not claim:** any split capability.

### C2. `runtime: typed split launch, placement proved from the log`
- **Placement-log parser candidate (lab dd0b7e1 or later, `src/placement_log.rs`, Apache-2.0, std only, no regex):** `parse(log)` reads `load_tensors: layer N assigned to device X`, `[negotiate_hello] handshake ok: … transport: T` and `using device RPCk (endpoint)`; `check(proof, stages, n_layer)` requires every layer 0..=n_layer exactly once on the planned device (the output layer with the last stage) and every handshake TCP, with actionable errors ("start the engine with -lv 5", "set GGML_RPC_DEBUG=1", "expected TCP (set GGML_RPC_NO_RDMA=1)", wrong/duplicate/missing layer). 5 tests on real b11609 lines, one over the committed R0 fixture. Its stage tuples match `rpc_split_args`. Port it with the mapping in C2 after review; it fails every load without -lv 5 and GGML_RPC_DEBUG=1, by design.
- **Source to port (lab, 2026-10-23):** the `--device` / `--tensor-split` mapping comes from ucompute-core-lab `src/llamacpp.rs` at 1914372 or later, not the earlier 4-test version: it formats split weights with Rust's shortest round-trip decimal (`{w}`; `{:.9}` did not round-trip f32 values below about 0.0156: `split_text_round_trips_small_weights`) and refuses all-zero weights instead of treating them as an even split (b11609 then splits by free memory, `llama-model.cpp:1619`: `all_zero_weights_are_not_an_even_split`); 7 unit tests including the 20,000-partition round trip; Apache-2.0 like this repository. The engine's own `-lv 5` per-layer log confirmed the mapping on real runs (lab rpc-dataflow run 4).
- **Files:**
  - `crates/common-runtime/src/types.rs`:
    - `LoadRequest.split: Option<Split { rpc_ports: Vec<u16>, devices: Vec<String>, tensor_split: Vec<f32>, n_gpu_layers: u32, no_mmap: bool }>`;
    - `LoadedModelInfo.split` (stages as placed);
  - new `crates/common-runtime/src/llamacpp/split.rs`: port `llama_assign`, `tensor_split`, `tensor_split_arg` and `rpc_split_args` with their 4 tests from `ucompute-core-lab/src/llamacpp.rs` (lab-owned Apache-2.0 code from the same project);
  - `process.rs` `build_args`, split branch:
    - `--rpc` rendered only as `127.0.0.1:<port>`;
    - `--device … --tensor-split … --split-mode layer -ngl <n> --fit off [--no-mmap]`;
    - the GPU/CPU branch, `--device none`, `--no-kv-offload`/`--no-op-offload` and force-gpu-fail are skipped;
    - the environment adds `GGML_RPC_NO_RDMA=1`;
  - `logparse.rs`: the per-layer device lines and `transport: TCP`;
  - `llamacpp.rs`: a split never falls back to CPU, and the load fails on a placement mismatch or a transport other than TCP.
- **Tests:**
  - `split_args_follow_the_lab_mapping`;
  - `rpc_endpoints_render_loopback_only`;
  - `split_skips_cpu_branch_flags`;
  - `split_never_falls_back_to_cpu`;
  - `layer_assignment_parsed_from_b11609_log` (fixture from C0);
  - `split_load_fails_without_tcp_transport_line`;
  - the 4 ported lab tests, including the 20,000-partition round trip;
  - real: `split_load_places_layers_as_planned_single_host`. The test starts a plaintext loopback `ggml-rpc-server` as a control (spec §28.4) and runs stories15M through configurations P and C, with 64 greedy tokens equal to local and a non-empty, non-error check.
- **Estimate:** 1 day.
- **Does not claim:** carriage, or anything cross-host.

### C3. `runtime/cli: supervised ggml-rpc-server and socket-owner checks`
- **Files:**
  - new `crates/common-runtime/src/llamacpp/rpc_server.rs`. `start(engine_dir, logs, threads, mem_cap) -> RpcServer { child, port, device }` with:
    - args `-H 127.0.0.1 -p <free_port> -t <physical cores − 2>`, and never `-c` or `-d`;
    - `env_clear` + `clean_environment` − `LLAMA_CACHE` + `GGML_RPC_NO_RDMA=1` + `MALLOC_ARENA_MAX=2`;
    - `kill_on_drop`;
    - Linux `pre_exec`: `PR_SET_PDEATHSIG(SIGKILL)`, `getppid` re-check, `setrlimit(RLIMIT_AS, mem_cap)`, `setpriority(10)`;
    - spawned from an async task, with a doc comment explaining why;
    - readiness requires `listener_owned_by(child_pid, port)` before anything connects. If a squatter holds the port, retry once on a new port, then fail with `backend_failed`;
  - new `crates/common-runtime/src/llamacpp/sockets.rs`:
    - `listener_owned_by(pid, port)` and `conn_peer_owned_by(pid, local_port, remote_port)`;
    - `established_clients(port) -> Vec<(port, owned_by_self)>`;
    - Linux implementation: `/proc/<pid>/fd` socket inodes against `/proc/net/tcp`;
    - macOS implementation: `/usr/sbin/lsof -nP -a -p <pid> -iTCP -Fn`;
    - no new crate (`libc` is already a dependency);
  - `crates/common-cli/src/main.rs:221` `reap_orphaned_engines`: also reaps `ggml-rpc-server`.
- **Tests:**
  - `rpc_server_binds_loopback_only` (and a connect to the host's LAN address is refused);
  - `rpc_server_args_are_fixed`;
  - `rpc_server_env_has_no_rdma_no_cache`;
  - `rpc_server_stops_on_drop`;
  - `squatted_rpc_port_is_detected_and_sent_nothing` (the squatter records 0 bytes);
  - `foreign_client_on_rpc_port_is_detected`;
  - `reaper_stops_orphaned_rpc_server`;
  - Linux only, run on sentinel:
    - `rpc_server_memory_cap_kills_server_not_daemon` (a huge `ALLOC_BUFFER`);
    - `rpc_server_survives_blocking_pool_idle` (alive 15 s after the blocking pool's idle reap);
    - `rpc_server_dies_with_daemon` (PDEATHSIG).
- **Estimate:** 1 day.
- **Does not claim:** a sandbox (F-06-08). Same-user attackers stay R6.

### C4. `proto: minor 2, RPC session and tunnel messages`
- **Files:**
  - `proto/common/v1/control.proto` and `crates/common-network/src/proto.rs`, new `ControlMessage` oneof bodies:
    - `RpcSessionOpen = 10 {session_id(16 B), model_sha256, engine_release, layer_start, layer_end, n_layer, est_bytes, ttl_s}`;
    - `RpcSessionAccept = 11 {session_id, granted_bytes, threads, device}`;
    - `RpcTunnelOpen = 12 {session_id}`;
  - `lib.rs`: `PROTOCOL_MINOR = 2`.
  - No close message: the session stream's FIN or reset closes the session.
- **Tests:**
  - `every_message_round_trips`, and `control_proto_mirrors_proto_rs` extended;
  - `unknown_oneof_variant_tolerated`;
  - `negotiated_minor_is_min_of_both`;
  - `tunnel_open_frame_over_1KiB_is_frame_too_large`.
- **Estimate:** 0.5 day.
- **Does not claim:** native activation frames (F-03-03 and B-016 stay open).

### C5. `network: RPC carriage (bridge and stage)`
- **Files:**
  - New `crates/common-network/src/rpc.rs`, H side, `SplitSession::open(conn, spec)`:
    - requires `negotiated_minor ≥ 2`, otherwise `protocol_incompatible` with an update hint;
    - opens the session stream, sends Open and waits up to 30 s for Accept;
    - binds `127.0.0.1:0` and keeps the listener;
    - holds `conn.in_flight()` for the session's life;
    - an accept loop with `conn_peer_owned_by(engine_pid)`. Otherwise it closes the connection and writes a local `rpc_bridge_refused` audit line;
    - per accepted connection: `open_bi`, `RpcTunnelOpen`, then a 64 KiB pump each way with `AtomicU64` byte and direction-switch counters.
  - `rpc.rs`, R side, `serve_session` / `serve_tunnel`, in this order:
    1. `may_serve(H, PooledInference)` and not held;
    2. platform is Linux (or the loopback-only test flag);
    3. `engine_release == b11609`;
    4. at most 1 session per R;
    5. `est_bytes × 1.1 ≤ available_now − 2 GiB`;
    6. spawn (C3) and write the `session_open` audit line;
    7. Accept.
    - Tunnel validation: session found on this connection and peer, live, fewer than 8 tunnels, `established_clients(pR)` all ours, then connect.
  - Limits and teardown:
    - session byte cap `2 × est_bytes + 1 GiB` per direction (`// ponytail:` recalibrate from R0 and P1);
    - TTL 30 min; idle close 10 s after the last tunnel closes;
    - `T_rpc` silence watchdog;
    - 1 s policy and ownership tick on both sides.
    - Teardown: H resets its TCP connections, calls `manager.unload()` and fails in-flight requests with the typed error; R kills rpc-server and checks that no listener is left.
    - Tunnel resets use one pinned code. The reason goes into the audit line.
  - `remote.rs` `handle_stream`:
    - dispatches on the first frame;
    - the Mode B cap of 8 (`MAX_STREAMS_PER_CONNECTION`) applies only to `RemoteRequest`;
    - RPC frames have their own budget: 1 session stream plus 8 tunnels.
  - `tests/invariants.rs`: `ALLOWED_MODULES = ["rpc.rs"]`, plus a grep that every bind or connect in `rpc.rs` uses `Ipv4Addr::LOCALHOST`.
  - Audit events `session_open`, `session_close`, `rpc_tunnel_open`, `rpc_tunnel_close`: ids, counts, bytes and durations only, never content.
  - Prometheus metric `common_rpc_tunnel_bytes_total{dir}`.
- **Tests** (two Nodes, one process, loopback QUIC):
  - `rpc_session_refused_without_pooled_policy_single_host` (a remote-only group; nothing spawned);
  - `pooled_needs_policy_on_both_ends_single_host` (a matrix over `allow_pooled` on H and R, `serve_peers`, held, revoked, networking off);
  - `rpc_session_refused_for_engine_release_mismatch`;
  - `rpc_session_refused_when_memory_short`;
  - `rpc_session_refused_by_minor_1_peer_single_host`;
  - `second_session_refused`;
  - `tunnel_for_unknown_or_foreign_session_refused` (no connect to pR);
  - `tunnel_cap_is_8`;
  - `bridge_refuses_a_local_process_that_is_not_our_engine_single_host`;
  - `tunnel_bytes_and_switches_counted_both_directions` (synthetic echo, so labelled `synthetic`);
  - `session_byte_cap_resets_tunnels`;
  - `session_ttl_and_idle_close`;
  - `silence_watchdog_resets_hung_stage_single_host`;
  - `group_policy_off_mid_session_tears_down_within_5s_single_host`;
  - `revoke_and_hold_mid_session_tear_down_single_host`;
  - `connection_loss_stops_remote_rpc_server_within_6s_single_host`;
  - `audit_has_session_and_tunnel_lines_without_content`;
  - `packet_capture_contains_no_plaintext_canary_or_weight_pattern_single_host` (existing UDP relay capture; positive control from C0);
  - `no_plaintext_mode_exists` updated.
- **Estimate:** 2 days.
- **Does not claim:** two machines, or protection against same-user processes (R6). The owner-check window is at most 1 s (new R11).

### C6. `models/api/cli: split target`
- **Files:**
  - `crates/common-models/src/gguf.rs`: `layer_bytes()` plus input and output bytes, using the gap rule of `feasibility::tensor_bytes`;
  - `feasibility.rs`: `estimate_stage`;
  - `manager.rs`: `LoadOptions.split`; the plan uses the stage estimate; split load timeout = 300 s + R bytes ÷ 5 MB/s (`// ponytail:` tune from P1);
  - `crates/common-api/src/openai.rs`:
    - `parse_target` accepts `split:<peer>`, a required `split_at`, optional `split_mode: pooled|same_backend` and a required `acknowledged`;
    - order inside `run()`: `evaluate(privacy, Peer, PooledInference)` → licence gate → resolve the hash → stage estimate → open or reuse `AppState.split` → `ensure_loaded`;
    - the envelope carries `placement` parsed from the log, not the plan;
    - `peer_lost` mapping while the session has failed;
  - `lib.rs` and `mesh.rs`:
    - the `AppState.split` lifecycle (unload, networking off, `local_only` and Sharing off close it);
    - Mode B requests get `overloaded` while a split is loaded;
    - `/api/status` gains `loaded.info.split`;
  - `openapi.json`;
  - `crates/common-cli`: `common run --split <peer> --split-at k [--same-backend]`, with an interactive disclosure prompt.
- **Tests:**
  - `layer_bytes_sum_to_tensor_data` (stories15M and qwen0.5b);
  - `stage_estimate_below_full_model`;
  - `split_target_denied_before_any_connection_without_pooled_policy`;
  - `split_requires_split_at_and_acknowledgement` (403 includes the disclosure);
  - `licence_gate_refuses_unknown_licence_cross_device`, and `test_flag_ignored_for_non_loopback_peer`;
  - `local_only_never_leaves_device_10k_single_host`, extended to split targets;
  - `local_request_never_uses_the_split_engine_single_host` (R's tunnel counters unchanged);
  - `mode_b_refused_while_split_loaded`;
  - real, two daemons on the Mac:
    - `split_completion_golden_64_tokens_single_host` (P and C);
    - `split_peer_killed_mid_generation_is_peer_lost_within_10s_single_host`;
    - `split_cancel_mid_generation_single_host`;
    - `unload_stops_remote_rpc_server_single_host`;
    - `remote_memory_released_when_head_engine_stops_single_host`.
- **Estimate:** 1.5 days.
- **Does not claim:** a placement planner (`split_at` is chosen by hand), speedups, or "larger model".

### C7. `api: logprobs passthrough` (only if OD-2 is approved; not needed for P1 to start)
- **Files:**
  - `types.rs` (`GenerateRequest.n_probs`);
  - `llamacpp/sse.rs` (the completion probabilities field; its b11609 name is ⚠️ and gets confirmed in R0);
  - `openai.rs`: `logprobs`/`top_logprobs`, which return `null` today at `:352`, `:392` and `:396`.
- **Tests:** `logprobs_pass_through_real_engine` (stories15M, greedy, top-20); `logprobs_absent_when_not_requested`.
- **Estimate:** 0.5 day.
- **Does not claim:** raw logits. Top-k log-probabilities are a labelled proxy (D-005).

### C8. `tests+docs: two-machine split driver, ADR-0005, threat model`
- **Files:**
  - new `tests/distributed/two_machine/split_two_machine.py`;
  - `run_mac_linux.sh SPLIT=1` (sentinel is R; a Mac dev daemon from the same commit is H; see §3);
  - offline tests against `fake_mesh.py`, labelled `synthetic`;
  - `docs/decisions/ADR-0005-…md`, Proposed (§2), and ADR-0003 §9.7 pointing to it;
  - `docs/security/THREAT_MODEL.md`:
    - a §2.13 row for pooled sessions;
    - R5 extended to both RPC parsers;
    - new **R11**: "other local users on loopback during a pooled session, polled owner checks". R10 is already spake2;
    - SD-07 note;
  - `docs/REQUIREMENTS.csv`:
    - F-03-01, F-03-02 and F-03-04 move to `IMPLEMENTED_UNVERIFIED`, and their code paths become `crates/common-network/src/rpc.rs; crates/common-runtime/src/llamacpp/{split,rpc_server,sockets}.rs`;
    - F-03-03 is untouched;
    - F-03-10 moves to `IMPLEMENTED_UNVERIFIED` with its cap noted;
  - `docs/VERIFICATION.md`, `docs/PROGRESS.md`.
- **Estimate:** 1 day.
- **Does not claim:** physical evidence.

**Total:** about 8 engineering days plus R0 (1–2 h), S1 (0.5 day) and P1 (0.5 day plus write-up).

---

## 2. Draft ADR-0005

```markdown
# ADR-0005: Split inference over the authenticated transport (M4, WP-04)

- **Status:** Proposed
- **Date:** 2026-10-23
- **Inputs:** `docs/SPEC_HANDOFF_V2.md` §§9, 28.4 (Modes C–E), WP-04, Gate D; ledger rows F-03-01/02/03/04/10, F-06-02, F-06-04, F-06-08, X-08, I-05; ADR-0002 (supervised engine, D-007); ADR-0003 D6 (carriage topology) and §9 (as built); `docs/security/THREAT_MODEL.md` (SD-07, R5, R6); B-011, B-016; Core Lab `REPORT.md` Appendix F and Addenda 1 and 5, `rpc-dataflow/PREREGISTRATION.md`, `INTEGRATION.md` step 5; pinned llama.cpp b11609 (macOS arm64 and Ubuntu x64 CPU, D-024).
- **Markers:** ✅ verified from a primary source · ⚠️ secondary/partial · `unverified` not checked.
- **Implementation:** not built. Section 9 will describe the code once it exists. Where section 9 and sections 2–8 disagree, section 9 is what runs.

## 1. Context

WP-04 asks for static physical pooling: one model's transformer blocks split between two physical hosts, with traffic only over Common's authenticated, encrypted transport. Two-host Mode B (whole requests) is not pooling. llama.cpp's RPC layer offload is the only real stage executor available now (Appendix F). Its server is unauthenticated plaintext TCP with no memory cap, and its README says never to expose it (D-007). ADR-0003 D6 fixed the topology: loopback-only engines at both ends, one QUIC stream per TCP connection. This ADR fixes what D6 left open: who may open a session, how both loopback ends are bound to the right processes, limits, failure semantics, the disclosure, and code placement. It decides head-mode (star) dataflow only. Every byte goes between the origin and one peer, so the existing per-peer `PooledInference` policy is sufficient (lab Addendum 1).

## 2. Verified facts (2026-10-23)

| Fact | Marker |
|---|---|
| Both pinned b11609 archives ship `ggml-rpc-server` and `libggml-rpc` | ✅ |
| `ggml-rpc-server` defaults to `-H 127.0.0.1` and warns on any other host; it has `-p`, `-t`, `-d`, `-c` and no memory cap | ✅ |
| It reads `LLAMA_CACHE` | ✅ (strings) |
| libggml-rpc prints `TCP (RDMA auto-negotiate enabled)`, contains `GGML_RPC_NO_RDMA`, logs `handshake ok: … transport: %s`, and refuses on `RPC server version mismatch` | ✅ (strings) |
| Protocol v8.0.0; the server takes one client at a time; `--list-devices` opened two sequential connections | ⚠️ (one session, `--list-devices` only) |
| An endpoint appears as device `RPC0`, in `--rpc` order | ⚠️ (one session); re-confirmed in R0 |
| `--tensor-split` follows `--device` order; Common passes `--fit off -ngl 999` | ✅ (lab Addendum 5) |
| The input embedding stays on the head CPU; the output layer goes with layer `n_layer` | ⚠️ (llama.cpp source via Context7, lab) |
| `validate_extra_args` accepts `--rpc <addr>` as two tokens; `RESERVED_FLAGS` has 7 entries | ✅ (`process.rs:22,37`) |
| `groups_put` writes `group_updated` and closes no connection | ✅ (`mesh.rs:1733`) |
| `free_port` binds and drops `127.0.0.1:0` | ✅ (`process.rs:134`) |
| `PR_SET_PDEATHSIG` fires when the parent *thread* exits | ✅ (prctl(2)) |
| rpc-server keeps buffers per client connection, so one client cannot address another's buffers | `unverified`; read b11609 `ggml-rpc.cpp` before C5 |
| Metal arm64 head against x86 CPU rpc-server interoperates | `unverified`; P1 step 2 |
| llama.cpp aborts on RPC errors | ⚠️; not relied on (D7) |

## 3. Decision

### D1 Scope
Mode C, one workload partitioned, between the origin H and exactly one peer R:
- static two-stage layer split, `split_at` chosen by the caller;
- head dataflow;
- no planner (F-03-05), no overlap (F-03-06), no native frames (F-03-03, B-016), no tensor cache.

### D2 Launch and placement (H)
- `LoadRequest.split` is typed. `--rpc` is rendered only from loopback ports the bridge owns.
- Every split and RPC flag is reserved, so `extra_args` cannot set any of them.
- Configuration **P**: `--device RPC0,MTL0 --tensor-split <exact midpoints> --split-mode layer -ngl 999 --fit off`. R holds blocks [0,k); H holds [k,n) plus the output layer, so no logits row crosses the link.
- Configuration **C** (same backend, ggml-cpu on both stages): `--device RPC0 -ngl (n+1−k)`. R holds [k,n) plus output, and one logits row crosses per token.
- `GGML_RPC_NO_RDMA=1` is set.
- The load fails unless the log shows `transport: TCP` and a layer-to-device assignment equal to the plan.
- A split never falls back to CPU or to local. The response reports the placement as parsed from the log.

### D3 Stage host (R)
- Linux only in this build.
- One `ggml-rpc-server` per session: `-H 127.0.0.1 -p <ephemeral> -t <cores−2>`, no `-c`, no `-d`.
- Environment cleared and scrubbed, with `LLAMA_CACHE` removed and `GGML_RPC_NO_RDMA=1`, `MALLOC_ARENA_MAX=2` set.
- `pre_exec`: `PR_SET_PDEATHSIG(SIGKILL)` plus a `getppid` re-check, `RLIMIT_AS` = granted + measured baseline, nice 10.
- It is spawned from a runtime worker task, never from a short-lived thread.
- No byte is forwarded until the `127.0.0.1:pR` listener is proved to belong to the child pid (`/proc/<pid>/fd` against `/proc/net/tcp`).
- At every tunnel open and every second, every established client of pR must be one of the daemon's own sockets; otherwise the session is torn down.
- At most one session per R.
- `engine_release` must equal R's pinned release. The platform may differ, and ggml's HELLO version check is the second gate.

### D4 Carriage
- Protocol minor 2 adds `ControlMessage` bodies `RpcSessionOpen` (10), `RpcSessionAccept` (11) and `RpcTunnelOpen` (12). Each is the first frame of a new bidirectional stream on the existing pinned, mutually authenticated connection (ALPN `common/1`, unchanged).
- A minor-1 peer refuses with `expected RemoteRequest`, so the feature fails closed. H refuses first with `protocol_incompatible`.
- The session stream stays open for the session's life. Its FIN or reset, or the connection's loss, ends the session.
- Each TCP connection on H's bridge becomes one tunnel stream carrying raw ggml RPC bytes after the first frame. TCP FIN maps to stream FIN, and TCP RST to stream reset.
- H's bridge binds `127.0.0.1:0`, keeps the listener, and gives a stream only to connections owned by its supervised `llama-server` pid. On macOS the check uses `lsof`; on Linux, `/proc`.
- `rpc.rs` is the only `common-network` module allowed to touch TCP. A test asserts every bind and connect in it is loopback.

### D5 Authorization and consent
**On H, before any socket or process:**
- `evaluate(privacy, Peer(R), PooledInference)` must allow. That requires privacy `trusted_group`, networking on, R pinned and not revoked, and a shared group with `allow_pooled_inference`. A group with only `allow_remote_inference` does not grant pooling.
- R is not held, and its connection is live and proven.
- The request carries `acknowledged: true` against the disclosure (D8). Without it: 403 plus the text.
- The model's catalog licence is in {Apache-2.0, MIT}, because the weights are uploaded to R (X-08).

**On R:** `may_serve(H, PooledInference)` must allow, and H is not held.

**During the session:** both sides re-run their checks every second. Any Deny ends the session within 5 s.

**Test flag:** `COMMON_TEST_SINGLE_HOST_POOL=1` lifts the licence gate and the Linux-only rule, and only for loopback peers.

### D6 Limits

| Limit | Default |
|---|---|
| Sessions | 1 per R |
| Tunnels | 8 per session |
| Bytes | `2 × est_bytes + 1 GiB` per direction per session, recalibrated after R0 and P1 |
| Memory | `est_bytes × 1.1 ≤ available now − 2 GiB` at admission; `RLIMIT_AS` thereafter |
| Time | TTL 30 min; idle close 10 s after the last tunnel; silence watchdog `T_rpc` (pre-registered from R0/S1) while a request is in flight |
| Rate | the existing per-peer `bytes_per_second` (unlimited by default) |

### D7 Failure semantics
On session loss, a policy Deny, a cap breach or a watchdog reset:
- H resets its tunnels, unloads the split engine itself, and fails in-flight requests. The error is `peer_revoked`, `peer_held` or `policy_denied` when this side knows the reason, `peer_lost` otherwise, within 10 s.
- R kills rpc-server and asserts no listener remains.
- There is no automatic local fallback. This amends ADR-0003 D6, which allowed one; spec rule: no silent substitution.
- If the H daemon dies, R sees idle within 4 s.
- If the R daemon dies, PDEATHSIG kills rpc-server.
- Orphans are reaped at the next start.

### D8 Exposure and disclosure
- R sees, in plaintext inside rpc-server: the weights of its range, every boundary activation, its KV cache, and per-token timing.
- With public weights, R can rebuild the prompt and the reply. Pooling is a capacity feature with no privacy gain over Mode B.
- Integrity is not verified; correctness is shown only in tests.
- Both C++ RPC parsers face a peer-chosen byte stream (R5 extended).
- Other local users can reach the loopback ports for at most 1 s between owner checks (new R11). Same-user processes remain R6.

Disclosure text, shown before the first session:

> Pooling splits one model between this device and <R>. <R> receives the model weights for its part and, for every token, intermediate data from which your prompt and the reply can be reconstructed. The connection is encrypted; the other device is not. Pooling does not make a request more private than running it on <R>, and it is usually slower than running a model that fits here.

### D9 Code placement
- `crates/common-network/src/rpc.rs` (carriage);
- `crates/common-runtime/src/llamacpp/{split,rpc_server,sockets}.rs` (engines);
- `crates/common-api` (target and consent).

This deviates from the ledger's `crates/common-distributed`. `common-network` already depends on `common-runtime` and `common-policy`, so a new crate would add a crate and an indirection and nothing else. Revisit when native frames (F-03-03) or a second stage executor arrive.

### D10 Evidence and claims
- Two daemons on one host are SINGLE-HOST and never satisfy F-03-01, F-03-02 or Gate D.
- F-03-03 stays open (B-016). RPC carriage bytes per token are reported as a ratio to `hidden × 4`.
- F-03-10 is capped at `IMPLEMENTED_UNVERIFIED` while F-08-04 is `NOT_STARTED`.
- F-03-04 needs the same backend. A Metal/CPU or arm64/x86 run is reported as measured and never relabelled.
- Evidence says "one model split across two devices", not "larger model", unless the model does not fit on either host. There is no speedup claim.

## 4. Alternatives considered

| Alternative | Decision |
|---|---|
| New `common-distributed` crate | Rejected for now (D9) |
| Separate pooled engine outside `ModelManager` | Rejected. The single slot plus reload-on-different-options already keeps local work local, and a test pins it. |
| Readiness by connect probe on R | Rejected. A squatter answers it (D3). |
| Expose rpc-server on the LAN behind a firewall | Rejected (D-007, hard rule) |
| SSH `-L` port-forward for an early cross-architecture check | Rejected. Cross-machine traffic goes only through Common's transport. |
| One multiplexed QUIC stream for all TCP connections | Rejected. It needs extra framing and adds head-of-line blocking. |
| `-c` tensor cache on R | Deferred (X-08 redistribution; owner decision) |
| Event-driven policy teardown | Deferred. The 1 s tick covers every source, including `peers.json` written by another process. |
| macOS or Windows R | Deferred. Needs a footprint watchdog or `GetExtendedTcpTable` owner checks. |
| Landlock or seccomp on R | Deferred (F-06-08) |
| Automatic local fallback | Deferred (D7) |
| Ring dataflow, peer-to-peer policy | Deferred to native frames |

## 5. Test plan
Unit, SINGLE-HOST and physical tests as listed in the WP-04 plan (commits C1–C8; runs R0, S1, P1). Every two-daemon test name contains `single_host`, and every synthetic fixture is named `synthetic`.

## 6. Consequences
- Pooling exists only by explicit opt-in on both devices plus per-request consent.
- An 8 GB head unloads its local engine to load a split.
- Every peer loss costs a head reload.
- First loads upload R's share of the weights over Wi-Fi, and repeat loads upload again, because there is no cache.
- Linux is the only stage host in this build.

## 7. Risks and open questions
- Cross-architecture RPC interoperation (`unverified`). The fallback pair is Mac to Mac, which needs a macOS R.
- Wi-Fi jitter against the 4 s idle timeout during uploads of hundreds of MB.
- Mixed-kernel numerics against the F-03-04 tolerances.
- `T_rpc` and the cap calibration.
- Per-client buffer isolation in rpc-server (`unverified`).
- The `lsof` dependency on macOS: present on every macOS release, at about 50–100 ms per accepted connection.

## 8. Verification record
Empty until C1–C8, R0, S1 and P1 exist.
```

---

## 3. First genuine two-machine run: this Mac (H) and sentinel (R)

**Hosts:**
- **H:** MacBook Air M2, 8 GB, macOS 26.2, b11609 Metal, 192.168.1.176.
- **R:** sentinel, i9-9900K, 14 GiB, Ubuntu 26.04, b11609 CPU (D-024), 192.168.1.167.

Both are on the owner's home Wi-Fi. Both run a daemon API from the same commit; no app UI is driven.

**Models:** catalog, Apache-2.0, already in this Mac's `.common`:
- R1a: `qwen2.5-0.5b-instruct-q8_0`, 24 blocks, hidden 896, so 3,584 B per f32 activation; `split_at=12`.
- R1b, acceptance: `qwen2.5-1.5b-instruct-q4_k_m`, 28 blocks, hidden 1536, so 6,144 B; `split_at=14`.

R needs no model copy for the split. The local CPU references on sentinel need OD-4.

### Stage order

1. **R0 (lab, SINGLE-HOST, plaintext loopback):** C0. It produces the log fixture, `T_rpc` seed, footprint baseline and weight-pattern positive control.
2. **S1-M (SINGLE-HOST, this Mac):** H = dev daemon, R = a second daemon under `COMMON_TEST_SINGLE_HOST_POOL=1`. Run the full C5 and C6 suites. Also run configuration C with ggml-cpu arm64 on both stages through the real tunnel: the same-backend carriage-exactness candidate for F-03-04 (OD-3). Compare bytes per token with R0 to isolate the QUIC and AEAD cost.
3. **S1-L (SINGLE-HOST, sentinel):** `scripts/mac-host.sh run 'cd ~/src/common && cargo test -p common-runtime -p common-network -- --include-ignored'` for the Linux-only tests (RLIMIT_AS, PDEATHSIG and thread survival, `/proc` owner checks, squatting). Record the idle rpc-server `VmPeak` as the RLIMIT baseline.
4. **P1 (physical), as below.**

### P1 procedure
- **Split point and expectations (lab, SINGLE-HOST loopback, 2026-10-23):** the lab planner's own split point beat hand-picked splits on every emulated link; with R holding block 0 plus 20 blocks and H the output layer it ran 47% faster than a 16/16 split with the output on R at 40 ms / 20 Mbit/s, with identical tokens: prefer more blocks on R and the output on H (config P). Same-backend split timing was predictable within ±25% on a quiet machine (TTFT 92%, TPOT 88%). Expect per-token time above (local stage times + RTT): an idling Metal stage ran about 2.5x slower per step (3.0 to about 8.0 ms from 4 ms RTT up), a device power-state effect, not the carriage's (lab split-exec Amendments 1 and 6).
Command: `LINUX_HOST=192.168.1.167 LINUX_USER=div0rce SPLIT=1 OUT=docs/evidence/pool-two-machine-<date>/run-<HHMM> tests/distributed/two_machine/run_mac_linux.sh`

**P1.0 Preflight**
- Same commit on both hosts:
  - sentinel: `scripts/mac-host.sh sync`, then `cargo build --release -p common-cli`;
  - Mac: `cargo build --release -p common-cli`.
- Quit the installed app; `state.py` saves its state.
- Start the H dev daemon with `COMMON_HOME=$PWD/.common-split`. Fill that home with **hardlinks** (`cp -al`) of `.common/models` and `backends`: no extra disk, since this Mac has about 4.2 GB free (B-010).
- `caffeinate -dimsu` for the run (a sleeping Mac invalidated an earlier run).
- Record both host manifests, the engine archive hashes and the model sha256.
- Confirm:
  - `who` on sentinel shows no other interactive user (OD-10);
  - no other acceptance run is using sentinel ports;
  - the Mac has at least 3 GB free memory.
- Pre-register `T_rpc`, the byte caps, `RLIMIT_AS`, the prompts with canaries (3), and `split_at`, in `prereg.json` **before** step P1.1.

**P1.1 Default deny on hardware**
- (a) Unpaired: refused with `peer_unauthorized` before any socket.
- (b) Paired, group with `allow_remote_inference` only: `policy_denied`.
- (c) `allow_pooled` on H only: R refuses and spawns nothing.
- (d) Policy present, `acknowledged` missing: 403 with the disclosure.

After each case, on sentinel: `pgrep -a ggml-rpc-server` is empty, and `ss -ltnup` shows only `127.0.0.1:8192` plus UDP 8193.

**P1.2 Session up (R1a, configuration P)**
- Setup: group `pool` with `allow_pooled_inference` on both sides; `serve_peers` on R; privacy `trusted_group` on H; acknowledged request.
- Checks:
  - `ss -ltnp` on sentinel shows rpc-server only on `127.0.0.1:<pR>`, owned by the child of the daemon pid;
  - `nc -z -w2 192.168.1.167 <pR>` from the Mac is refused;
  - H log shows the handshake ok, version v8.0.0 and `transport: TCP`. This is the cross-architecture verdict: on failure, stop and record (OD-3 fallback);
  - the layer-to-device log matches the plan, and the envelope `placement` equals it.

**P1.3 R1a generation**
- 3 prompts × 64 greedy tokens (`cache_prompt:false`), against a local Metal run on the Mac with the same flags.
- Record load 1 and load 2 times and upload bytes. Both loads upload, because there is no cache, so they are not called cold and warm.

**P1.4 R1b acceptance**
- Configuration P: 3 × 64 greedy tokens, against local Mac Metal (`--no-mmap`) and, under OD-4, local sentinel CPU.
- Configuration C: 3 × 64, against local Mac CPU and sentinel CPU.
- Log-probabilities per position if C7 has landed, labelled as a proxy.
- Pass rule for F-03-04: identical greedy tokens for 64 positions and max-abs ≤ 1e-3 (quantized). Cross-ISA results are reported as measured (OD-3).

**P1.5 Memory (F-03-02)**
- Mac: `footprint -p <llama-server pid>` peak, split against local full model with identical flags.
- sentinel: `VmHWM` from `/proc/<rpc-server pid>/status`, against the `VmHWM` of a full-model local `llama-server --no-mmap` on sentinel.
- Criterion: each host's peak is below the full-model peak by at least the bytes of the layers it does not host (from `gguf::layer_bytes`), ±15%.

**P1.6 Bytes per token and timing**
- Tunnel counters at three checkpoints (after load, after `n_predict=1`, after `n_predict=65`), using the lab method: Δ/64 per direction, and the ratio to `hidden × 4`.
- Direction switches per token (`round_trips`).
- `quinn` stats: RTT and loss.
- TTFT and TPOT against local.
- These are F-03-03 ratio data only. No speedup claim.

**P1.7 Capture**
- H dials R through the existing in-process UDP relay on the Mac. The relay is on the real Wi-Fi path, needs no root, and records every datagram.
- Scan for the 3 canaries and the R-range 64-byte weight pattern: zero hits is the pass.
- The positive control for the same scanner and pattern is R0's plaintext proxy stream.
- A kernel `tcpdump` is used only if the owner runs it (OD-5).

**P1.8 Failure and cancel**
Each case runs on a fresh session with generation in flight:
1. client cancel (the next request on the same session succeeds);
2. `kill -9` of rpc-server;
3. revoke on R;
4. hold on H;
5. `allow_pooled_inference` turned off on R through `PUT /api/groups`;
6. privacy set to `local_only` on H;
7. networking off on H;
8. SIGKILL of the R daemon (PDEATHSIG: `pgrep` is empty within 1 s);
9. SIGKILL of the H daemon (R tears down within 5 s; the next H start reaps `llama-server`).

Pass for each case:
- a typed error within 10 s, or within 5 s for cases 3–7;
- no request left hanging;
- no `ggml-rpc-server` process or listener on R;
- `session_close` with an outcome on both sides.

**P1.9 Local stays local**
- With a split loaded, send `local` and `local_only` requests.
- R's tunnel counters must stay unchanged. The split session closes through the reload, which is expected and recorded.

**P1.10 Audit join and leak scan**
- `session_open` and `session_close` on both sides share `session_id`.
- Scan the audit logs, daemon logs, engine logs and `common diagnostics` bundles for the canaries and the weight pattern: none may appear.

**P1.11 Teardown**
- No rpc-server or listener left.
- The sentinel daemon is stopped.
- `state.py` restore passes on both hosts, and the installed app is restarted.

### Evidence layout
- **Pre-registered predictions (lab ae88d0b, `p1-predictions/PREDICTIONS.md`, committed before any P1 run):** config P, 64 greedy tokens: R1a (qwen 0.5B q8, k=12) TPOT ≈ 37 ms with the measured Metal ramp-down (planner 27.6 ms at 100 Mbit/s; local 8.8 ms), R1b (qwen 1.5B q4_k_m, k=14) ≈ 55 ms (planner 40.2 ms; local 15.2 ms); tunnel payload per decode token R1a 14,150 B up / 3,592 B down, R1b 21,830 B / 6,152 B, exactly one synchronous turn per token. Scoring: bytes ±10%, turns 1.00 ± 0.05, TPOT within [0.75 × planner, 1.25 × ramp], TTFT ±25% once P1's prompt token counts are fixed (send them to the lab first). P1's evidence must therefore export per-token tunnel byte counters, synchronous turns, TPOT and TTFT. The planner rejects both splits as slower than local: the run demonstrates genuine split execution, not a speedup, and claims none.
```
docs/evidence/pool-single-host-<date>.md            # S1-M and S1-L, labelled SINGLE-HOST
docs/evidence/pool-two-machine-<date>.md             # P1 narrative: hosts table, commit, commands, step results, limits, claim boundary
docs/evidence/pool-two-machine-<date>/run-<HHMM>/
  prereg.json                 # pre-registered caps, T_rpc, split_at, prompts and canaries, pass rules (written before P1.1)
  manifest.json               # commit, both host manifests, engine archive sha256, model sha256, full llama-server/rpc-server argv
  steps.json                  # per-step PASS/FAIL/N/A with timings
  h/  daemon.log  llama-server.log  audit.jsonl  status-*.json  footprint-{split,local}-*.txt  quinn-stats.json
  r/  daemon.log  rpc-server.log    audit.jsonl  ss-ltnup-*.txt  proc-status-{rpc,local}-*.txt  pgrep-*.txt
  tokens/  {P,C}-split-<prompt>.json  local-{mac-metal,mac-cpu,sentinel-cpu}-<prompt>.json  compare.json
  bytes/   checkpoints.json  per-token.json
  capture/ relay-summary.json  scan.json             # raw datagrams kept only if small; scan results always
  failure/ <case>.json                                # nine cases
benchmarks/results/<UTC>-pool-qwen2.5-1.5b-instruct-q4_k_m-mac-m2+sentinel.json   # hardware manifest, command line, versions, TTFT/TPOT, bytes/token
```

**Ledger updates, only as far as the evidence goes:**

| Row | Can reach | Condition |
|---|---|---|
| F-03-01 | VERIFIED | P1.2, P1.4 and P1.5 pass; dependencies are already VERIFIED |
| F-03-02 | VERIFIED | P1.5 passes on both hosts; needs OD-4 |
| F-03-04 | per OD-3 | |
| F-03-03 | unchanged | B-016 open, ratio recorded |
| F-03-10 | IMPLEMENTED_UNVERIFIED | capped while F-08-04 is NOT_STARTED |

Gate D is not declared. Updates also go to ADR-0005 §8 and §9, ADR-0003 §9.7, THREAT_MODEL, `VERIFICATION.md` and `PROGRESS.md`.

**R2 (optional):** a 7B run needs OD-7 and is not part of the first run.

---

## 4. Owner decisions

| # | Decision | Blocks | Default if unanswered |
|---|---|---|---|
| OD-1 | Accept ADR-0005 (Proposed → Accepted) | Nothing; ADR-0003 was built while Proposed | Build under Proposed |
| OD-2 | Approve logprobs/top_logprobs passthrough (C7) as a labelled proxy for raw logits (D-005) | Only the per-position logit half of F-03-04 | Skip C7; F-03-04 stays IMPLEMENTED_UNVERIFIED |
| OD-3 | F-03-04 acceptance: does a SINGLE-HOST same-backend run through the real tunnel satisfy the row (Gate D still needs its two-machine numerical check)? Is the cross-ISA CPU configuration C reported only? Is a matched Mac-to-Mac pair wanted (needs the macOS R role and the M2 Pro host)? | F-03-04 status only | Report everything as measured; row not promoted |
| OD-4 | Pull `qwen2.5-0.5b-q8_0` and `qwen2.5-1.5b-q4_k_m` (about 1.8 GB, Apache-2.0, catalog-verified) into sentinel's `~/.common-acceptance` for local CPU references. It is within the host's recorded engineering use, but the standing authorization names only the Mac and the HP laptop. | F-03-02 on the R side, cross-ISA comparison | Do not pull; F-03-02 R side stays open |
| OD-5 | Accept the in-process UDP relay on the real path as the capture method, or run `sudo tcpdump` on the Mac yourself (sentinel has no sudo) | Nothing | Relay |
| OD-6 | Licence allowlist for pooled weight upload {Apache-2.0, MIT}; unknown-licence fixtures refused across devices (B-011, X-08) | Nothing | As proposed |
| OD-7 | R2 capacity run: Qwen2.5-7B Q4_K_M (about 4.7 GB) does not fit on this Mac's disk (4.2 GB free). It needs sentinel as H and a Mac R role (not built) | R2 only | Deferred |
| OD-8 | Landlock or seccomp sandbox for rpc-server (new dependency) | Nothing | Deferred to F-06-08 |
| OD-9 | Require acknowledged consent when `PUT /api/groups` turns `allow_pooled_inference` on | Nothing | Deferred to Gate E; per-request acknowledgement is the gate for now |
| OD-10 | Confirm sentinel has no other interactive local users during P1 (the R11 assumption) | P1 start | Check `who` in preflight; abort if others are present |
| OD-11 | Linux-only stage host in this build | Nothing | Yes |
| OD-12 | The F-03-03 split proposed in ADR-0003 D6: native frame format vs measured RPC ratio | Nothing; B-016 stays open | No criterion change |

None of these blocks implementation. OD-10 is a preflight check. OD-3 and OD-4 limit only which rows can be promoted.

## 5. Open items to settle before P1
- ⚠️ Per-client buffer isolation in rpc-server: read the b11609 `ggml-rpc.cpp` (MIT) before C5. If it does not hold, foreign-client teardown becomes the only control, and R11 gets worse.
- ⚠️ b11609 interoperation between macOS arm64 and Ubuntu x64 over RPC. It is settled only at P1.2. If it fails, the next step is OD-3's Mac-to-Mac route.
- ⚠️ Exact `-lv 4` placement text, connection count and lifetime, the probability field name, and `T_rpc`: all come from R0.
- The `RLIMIT_AS` baseline with glibc on sentinel comes from S1-L. Too tight a limit refuses legitimate sessions.
- Wi-Fi jitter against the 4 s idle timeout during uploads of hundreds of MB. A false `peer_lost` means a head reload, so record every one.

This was read-only: nothing was edited, built, run or committed.
