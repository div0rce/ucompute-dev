# ADR-0005: Split inference over the authenticated transport (M4, WP-04)

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

- **Status:** Proposed
- **Date:** 2026-10-23
- **Inputs:** `docs/SPEC_HANDOFF_V2.md` §§9, 28.4 (Modes C–E), WP-04, Gate D; ledger rows F-03-01/02/03/04/10, F-06-02, F-06-04, F-06-08, X-08, I-05; ADR-0002 (supervised engine, D-007); ADR-0003 D6 (carriage topology) and §9 (as built); `docs/security/THREAT_MODEL.md` (SD-07, R5, R6); B-011, B-016; Core Lab `REPORT.md` Appendix F and Addenda 1 and 5, `rpc-dataflow/PREREGISTRATION.md`, `INTEGRATION.md` step 5; pinned llama.cpp b11609 (macOS arm64 and Ubuntu x64 CPU, D-024).
- **Markers:** ✅ verified from a primary source · ⚠️ secondary/partial · `unverified` not checked.
- **Implementation:** C1–C6 and the C8 driver are built, uncommitted at the time of writing and tested only by unit, synthetic and SINGLE-HOST tests (section 9). No two-machine run exists. Where section 9 and sections 2–8 disagree, section 9 is what runs.

## 1. Context

WP-04 asks for static physical pooling: one model's transformer blocks split between two physical hosts, with traffic only over UCompute's authenticated, encrypted transport. Two-host Mode B (whole requests) is not pooling. llama.cpp's RPC layer offload is the only real stage executor available now (Appendix F). Its server is unauthenticated plaintext TCP with no memory cap, and its README says never to expose it (D-007). ADR-0003 D6 fixed the topology: loopback-only engines at both ends, one QUIC stream per TCP connection. This ADR fixes what D6 left open: who may open a session, how both loopback ends are bound to the right processes, limits, failure semantics, the disclosure, and code placement. It decides head-mode (star) dataflow only. Every byte goes between the origin and one peer, so the existing per-peer `PooledInference` policy is sufficient (lab Addendum 1).

## 2. Verified facts (2026-10-23)

| Fact | Marker |
|---|---|
| Both pinned b11609 archives ship `ggml-rpc-server` and `libggml-rpc` | ✅ |
| `ggml-rpc-server` defaults to `-H 127.0.0.1` and warns on any other host; it has `-p`, `-t`, `-d`, `-c` and no memory cap | ✅ |
| It reads `LLAMA_CACHE` | ✅ (strings) |
| libggml-rpc prints `TCP (RDMA auto-negotiate enabled)`, contains `GGML_RPC_NO_RDMA`, logs `handshake ok: … transport: %s`, and refuses on `RPC server version mismatch` | ✅ (strings) |
| Protocol v8.0.0; the server serves each accepted connection to completion, one at a time (accept loop, `ggml-rpc.cpp` 2782-2790 at b11609) | ✅ (source, lab INTEGRATION §8, d741ef8) |
| Per-connection buffer isolation: each accepted connection gets a fresh `rpc_server` (2399-2401); `deserialize_tensor` nulls any buffer outside that connection's set (1641); the destructor frees all of the connection's buffers on close (2393), so a tunnel drop frees R's weights | ✅ (source, lab) |
| llama-server opens about 11-12 sequential TCP connections per RPC server per load-and-generate (plus 2 for `--list-devices`); only one is open at a time | ✅ (loopback measurement, Metal both stages, lab Addendum 9) |
| Each `SET_TENSOR` arrives whole in a `std::vector` (2565-2573), so R transiently holds its largest tensor twice (macOS: the freed buffer stayed resident, MALLOC_LARGE +48 MB for a 47.81 MiB output tensor); R's idle baseline is about 37 MiB | ✅ macOS / `unverified` Linux glibc |
| Config P per decode token: head to R three activations (embd, inp_mixed_embd, node_1) plus a 2·n_kv B f16 mask plus about 2.9 KB of descriptors and commands; R to head one activation plus 8 B; exactly one synchronous round trip per token; prefill: 3·p activations plus a 2·n_kv·p B mask up | ✅ (loopback, lab) |
| Same backend (Metal on both stages, loopback): greedy tokens equal local for 64/64 in every split configuration; the handshake logs `transport : TCP (RDMA auto-negotiate enabled)` | ✅ SINGLE-HOST (lab) |
| With `--rpc`, `--list-devices` lists `MTL0`, `BLAS`, then `RPC0`, `RPC1`, ... in `--rpc` order | ✅ (R0, lab b252188, SINGLE-HOST) |
| Per RPC server, llama-server opens 12-13 short connections (each open under 5 ms, 42-59 B up and 48-68 B down; three carry 3,302 B up), then one long-lived session connection that carries all weights and per-token traffic; peak concurrency is 1 | ✅ (R0) |
| The per-layer `load_tensors: layer N assigned to device X` lines are llama DEBUG lines: shown at `-lv 5`, none at `-lv 4` (UCompute's level). The head's `[negotiate_hello] handshake ok: server version 8.0.0, transport: TCP` and R's `[rpc_serve_client] client connected, transport: TCP` are ggml-rpc debug lines gated by `GGML_RPC_DEBUG` (`ggml/src/ggml-rpc/log.h`); R's startup banner `transport : TCP (RDMA auto-negotiate enabled)` prints unconditionally | ✅ (R0; log fixture `rpc-dataflow/results-r0/b11609-rpc-split-A.log` in the lab) |
| Longest silence on the link while a request is in flight: 13-21 ms in the first request (setup plus a 10-token prefill), about 5 ms after (stories15M, loopback); idle `ggml-rpc-server` (Metal) before any client: 40-41 MiB | ✅ (R0; a tiny model on loopback: scale by R's prefill time) |
| A 64-byte pattern from a block a server holds is found in that server's plaintext stream; one from a block it does not hold is not (positive and negative control for the capture scan; `tools/rpc_dataflow.py` `weight_pattern()` reads it from the GGUF tensor table) | ✅ (R0) |
| `--tensor-split` follows `--device` order; UCompute passes `--fit off -ngl 999` | ✅ (lab Addendum 5) |
| The input embedding stays on the head CPU; the output layer goes with layer `n_layer` | ⚠️ (llama.cpp source via Context7, lab) |
| `validate_extra_args` accepts `--rpc <addr>` as two tokens; `RESERVED_FLAGS` has 7 entries | ✅ (`process.rs:22,37`) |
| `groups_put` writes `group_updated` and closes no connection | ✅ (`mesh.rs:1733`) |
| `free_port` binds and drops `127.0.0.1:0` | ✅ (`process.rs:134`) |
| `PR_SET_PDEATHSIG` fires when the parent *thread* exits | ✅ (prctl(2)) |
| rpc-server keeps buffers per client connection, so one client cannot address another's buffers | ✅ from b11609 source (lab INTEGRATION.md §9): `rpc_serve_client` builds a fresh `rpc_server` per connection (`ggml-rpc.cpp:2399-2401`), `deserialize_tensor` nulls any buffer that is not that connection's (`:1641`), the destructor frees all of a connection's buffers (`:2393-2397`), one client at a time (`:2782-2790`) |
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
- Configuration **C** (same backend, ggml-cpu on both stages): `--device RPC0 -ngl (n+1−k)`. R holds [k,n) plus output, and one logits row crosses per token. *(C6: the head's blocks are the plan's `CPU` stage, proved by the `CPU KV buffer` share, and the engine also gets `--no-op-offload`: op offload would copy the CPU blocks' weights to the first device, R, for every large batch.)*
- `GGML_RPC_NO_RDMA=1` is set on both engines (the environment scrub sets it explicitly), so RPC uses TCP. The engines stay at UCompute's `-lv 4`: at `-lv 5` llama-server's `launch_slot_` debug line logs each request's prompt and the log carries reply text (lab rpc-dataflow, b11609, 2026-10-24; 0 hits at `-lv 4`), so the debug-level proof is never used in production. *(Amended 2026-10-24: the first text set `GGML_RPC_DEBUG=1` and `-lv 5` on H for the layer and transport lines of R0.)*
- The load fails unless the info-level log proves the plan: `llamacpp::split::check_buffers` (1fb589e, from the lab's d4f9516) requires the devices of the `using device` lines in `--device` order equal to the plan, and each device's KV share equal to its planned block count (RPC buffers attributed by endpoint, because b11609 labels every RPC buffer `RPC0[…]`); the output layer follows the last stage by llama.cpp's rule, shown by the mapping's 20,000-partition test. `check_proof` (the `-lv 5` + `GGML_RPC_DEBUG=1` per-layer and transport proof) is kept for SINGLE-HOST diagnosis only.
- A split never falls back to CPU or to local. The response reports the placement as parsed from the log.

### D3 Stage host (R)
- Linux only in this build.
- One `ggml-rpc-server` per session: `-H 127.0.0.1 -p <ephemeral> -t <cores−2>`, no `-c`, no `-d`.
- Environment cleared and scrubbed, with `LLAMA_CACHE` removed and `GGML_RPC_NO_RDMA=1`, `MALLOC_ARENA_MAX=2` set.
- `pre_exec`: `PR_SET_PDEATHSIG(SIGKILL)` plus a `getppid` re-check, `RLIMIT_AS` = granted + the largest tensor of R's range (each `SET_TENSOR` is held twice transiently; in config C that is the output layer) + measured baseline (about 37 MiB on macOS) + thread stacks and arenas, nice 10.
- It is spawned from a runtime worker task, never from a short-lived thread.
- No byte is forwarded until the `127.0.0.1:pR` listener is proved to belong to the child pid (`/proc/<pid>/fd` against `/proc/net/tcp`).
- At every tunnel open and every second, every established client of pR must be one of the daemon's own sockets; otherwise the session is torn down.
- At most one session per R.
- `engine_release` must equal R's pinned release. The platform may differ, and ggml's HELLO version check is the second gate.

### D4 Carriage
- Protocol minor 2 adds `ControlMessage` bodies `RpcSessionOpen` (10), `RpcSessionAccept` (11) and `RpcTunnelOpen` (12). Each is the first frame of a new bidirectional stream on the existing pinned, mutually authenticated connection (ALPN `ucompute/1`, unchanged).
- A minor-1 peer refuses with `expected RemoteRequest`, so the feature fails closed. H refuses first with `protocol_incompatible`.
- The session stream stays open for the session's life. Its FIN or reset, or the connection's loss, ends the session.
- Each TCP connection on H's bridge becomes one tunnel stream carrying raw ggml RPC bytes after the first frame. TCP FIN maps to stream FIN, and TCP RST to stream reset.
- H's bridge binds `127.0.0.1:0`, keeps the listener, and gives a stream only to connections owned by its supervised `llama-server` pid. On macOS the check uses `lsof`; on Linux, `/proc`.
- `rpc.rs` is the only `ucompute-network` module allowed to touch TCP. A test asserts every bind and connect in it is loopback.
- **Input filter (C5, `rpc_meter.rs`, from the Core Lab's `RPC-SURFACE.md`).** At b11609, `COMM_INIT` (20) makes rpc-server listen on `0.0.0.0:<client's port>` or dial a host the client names, and a `GRAPH_COMPUTE` node with op 92-95 makes R's CPU backend call a function pointer from `op_params`; `-H 127.0.0.1` covers neither (read from source, not exercised). Every chunk from the head passes the filter before any byte of it is written to rpc-server: a command outside the allowlist is refused at its first byte. The allowlist is the 14 commands of a real session (lab R0 configuration A) plus `FREE_BUFFER` (4), sent at unload, and `SET_TENSOR_HASH` (7), which a real load of the R1b model sends for its tensors over 10 MiB (nine Q6_K `ffn_down` weights; without `-c`, which D3 never passes, R reads no file); a graph node with op 92-95 or ≥ 101, or a top-level node id of 0 (`refused_graph_node`), is refused before the graph's last byte (`docs/security/THREAT_MODEL.md`, RPC CVE disposition). **R enforces it** (R does not trust H); H runs it too. A refusal resets the tunnel, ends the session, stops rpc-server, and R's `rpc_tunnel_close` line names the check (`refused_command`, `refused_graph_op`, `refused_graph_size`), never the bytes. The filter narrows two known capability paths; it does not make rpc-server safe against a hostile head (OD-8).

### D5 Authorization and consent
**On H, before any socket or process:**
- `evaluate(privacy, Peer(R), PooledInference)` must allow. That requires privacy `trusted_group`, networking on, R pinned and not revoked, and a shared group with `allow_pooled_inference`. A group with only `allow_remote_inference` does not grant pooling.
- R is not held, and its connection is live and proven.
- The request carries `acknowledged: true` against the disclosure (D8). Without it: 403 plus the text.
- The model's catalog licence is in {Apache-2.0, MIT}, because the weights are uploaded to R (X-08).

**On R:** `may_serve(H, PooledInference)` must allow, and H is not held.

**During the session:** both sides re-run their checks every second. Any Deny ends the session within 5 s.

**Test flag:** `UCOMPUTE_TEST_SINGLE_HOST_POOL=1` lifts the licence gate and the Linux-only rule, and only for loopback peers.

### D6 Limits

| Limit | Default |
|---|---|
| Sessions | 1 per R |
| Tunnels | at most 8 open at once per session (a concurrent cap: llama-server opens about 12 sequential connections per load, so a per-session count would fail every load); the socket-owner check (lsof, 50-100 ms on macOS) adds about 0.5-1.2 s per load there |
| Bytes | `2 × est_bytes + 1 GiB` per direction per load, on H, recalibrated after P1. Per load, not per session: the runtime restarts a split engine once after a lost port race, and that start uploads R's weights again (a new engine pid starts a new load) |
| Memory | `est_bytes × 1.1 ≤ available now − 2 GiB` at admission; `RLIMIT_AS` thereafter |
| Time | TTL 30 min; idle close 10 s after the last tunnel opened, and 300 s before the first one (the head checks the model's hash, plans and starts its engine after the session opens; C6 measured 34-56 s for qwen2.5-0.5b's hash in a debug build on the 8 GB Mac, SINGLE-HOST; llama-server's short connections last about 25 ms, so a tunnel that opened and closed between two ticks counts as activity); silence watchdog `T_rpc` while a request is in flight: H's only wait in a step is one `GET_TENSOR` reply per remote stage and the longest silence is R's prefill of its range, so `T_rpc` must exceed R's prefill time for the longest admitted prompt (bounded from the calibrated prefill rate; pre-registered from R0/S1: 10 s for P1, lab `C5-NUMBERS.md`). It is armed by `GRAPH_COMPUTE`, `GRAPH_RECOMPUTE` and `GET_TENSOR`, never by the weight upload, which the split load timeout bounds |
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
- `crates/ucompute-network/src/rpc.rs` (carriage);
- `crates/ucompute-runtime/src/llamacpp/{split,rpc_server,sockets}.rs` (engines);
- `crates/ucompute-api` (target and consent).

This deviates from the ledger's `crates/ucompute-distributed`. `ucompute-network` already depends on `ucompute-runtime` and `ucompute-policy`, so a new crate would add a crate and an indirection and nothing else. Revisit when native frames (F-03-03) or a second stage executor arrive.

### D10 Evidence and claims
- Two daemons on one host are SINGLE-HOST and never satisfy F-03-01, F-03-02 or Gate D.
- F-03-03 stays open (B-016). RPC carriage bytes per token are reported as a ratio to `hidden × 4` (config P measures about 3.9 up and 1 down for qwen2.5-0.5B, lab; config C also returns vocab × 4 B per token, 608 KB for qwen2.5).
- F-03-10 is capped at `IMPLEMENTED_UNVERIFIED` while F-08-04 is `NOT_STARTED`.
- F-03-04 needs the same backend. A Metal/CPU or arm64/x86 run is reported as measured and never relabelled.
- Expected on the first pair (this Mac's Metal head, sentinel's x86 CPU stage): greedy tokens diverge from a Metal-only reference within a few tokens (lab REPORT Addendum 11, SINGLE-HOST: a CPU stage diverged from Metal within 1-10 tokens, and so did whole-model CPU execution, while Metal+Metal splits matched local 64/64). That pair's correctness evidence is therefore closeness (top-k agreement and per-position logprob differences, through C7's `n_probs` passthrough: llama-server's `completion_probabilities` / OpenAI `logprobs.content`, b11609 `server-task.cpp` 256-295, 1060), not token identity; identity is claimed only for a same-backend, same-ISA pair.
- Evidence says "one model split across two devices", not "larger model", unless the model does not fit on either host. There is no speedup claim.

## 4. Alternatives considered

| Alternative | Decision |
|---|---|
| New `ucompute-distributed` crate | Rejected for now (D9) |
| Separate pooled engine outside `ModelManager` | Rejected. The single slot plus reload-on-different-options already keeps local work local, and a test pins it. |
| Readiness by connect probe on R | Rejected. A squatter answers it (D3). |
| Expose rpc-server on the LAN behind a firewall | Rejected (D-007, hard rule) |
| SSH `-L` port-forward for an early cross-architecture check | Rejected. Cross-machine traffic goes only through UCompute's transport. |
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

## 9. Implementation notes (2026-10-29)
Built by WP-04 steps C1–C6 and C8 (plan `docs/design/wp04-implementation-plan-2026-10-23.md`). C7 (logprobs passthrough, OD-2) is not built. Every test below is a unit, synthetic or **SINGLE-HOST** test; none is two-machine evidence (D10). Linux-only tests ran on sentinel only.

### 9.1 Where the code is
| Part | Code |
|---|---|
| Reserved flags (C1) | `crates/ucompute-runtime/src/llamacpp/process.rs` `RESERVED_FLAGS` |
| Split launch and placement proof (C2) | `crates/ucompute-runtime/src/llamacpp/split.rs` (`check_buffers`), `process.rs`, `llamacpp.rs`, `manager.rs` |
| Stage host engine and socket owners (C3) | `crates/ucompute-runtime/src/llamacpp/rpc_server.rs`, `sockets.rs` (Core Lab drop-ins, reviewed); the reaper in `crates/ucompute-cli/src/main.rs` |
| Protocol minor 2 (C4) | `proto/ucompute/v1/control.proto`, `crates/ucompute-network/src/proto.rs`, `lib.rs` (`PROTOCOL_MINOR = 2`, `RPC_SESSION_MINOR`, `MAX_RPC_FRAME` 1 KiB) |
| Carriage, filter and meter (C5) | `crates/ucompute-network/src/rpc.rs`, `rpc_meter.rs` (Core Lab drop-in) |
| Split target, consent and licence (C6) | `crates/ucompute-api/src/split.rs`, `openai.rs` (`split:<peer>`), `admin.rs` (`/api/status` `split`) |
| Two-machine driver (C8) | `tests/distributed/two_machine/split_two_machine.py`, `split_prereg.json`, `run_mac_linux.sh SPLIT=1` |

### 9.2 Differences from sections 2–8
- **Names (D-028, 2026-10-29).** Sections 2–8 keep the names they were written with. The code is in `crates/ucompute-*`, the test flag is `UCOMPUTE_TEST_SINGLE_HOST_POOL=1` and the protobuf mirror is `proto/ucompute/v1/control.proto`; ADR-0003 §9.16 lists the renamed protocol identifiers.
- **D3 spawn.** "Spawned from a runtime worker task" is not enough: tokio's workers and blocking-pool threads both retire, and on sentinel a server forked from a `tokio::spawn` task was killed by `PR_SET_PDEATHSIG` 10 s later when its forking thread retired (strace). On Linux, `rpc_server::spawn` forks every server on one dedicated thread that never exits (`rpc-server-forker`). Other platforms call `spawn` directly (they set no PDEATHSIG).
- **D3 owner check on macOS.** `lsof` is queried by port (`-iTCP:<port>`), not by pid, so a listener held by another pid is seen.
- **D4 filter.** At b11609 `COMM_INIT` (20), `COMM_ALLREDUCE` (21) and `COMM_FREE` (22) are outside the allowlist; `COMM_INIT` with rank 0 would make rpc-server listen on `0.0.0.0` (read from source, never run, because running it opens a listener beyond loopback).
- **D6 bytes.** The cap is per load on H, as amended in D6.
- **D3 `RLIMIT_AS`.** As built it is granted × 1.5 + 1 GiB (`rpc::mem_cap`), looser than D3's granted + the range's largest tensor + baseline + stacks, which is not built.

### 9.3 Enforcing tests
| Decision | Tests |
|---|---|
| D2 flags, launch, no fallback | `process.rs`: `extra_args_cannot_set_split_flags`, `split_args_follow_the_lab_mapping`, `rpc_endpoints_render_loopback_only`, `split_skips_cpu_branch_flags`, `split_sets_no_rdma_and_keeps_lv4`, `environment_is_scrubbed`; `llamacpp.rs`: `split_never_falls_back_to_cpu`, `split_publishes_each_engine_start_pid`, `split_load_fails_on_a_placement_mismatch` |
| D2 placement proof | `split.rs`: `rpc_args_put_devices_in_stage_order`, `unrealizable_orders_are_refused`, `round_trip_property_over_random_partitions`, `info_level_lines_prove_each_configuration`, `info_level_check_refuses_other_plans_and_silent_logs`, `cpu_stage_is_proved_by_its_kv_share_synthetic`, `non_tcp_transport_gaps_and_duplicates_are_refused`; `tests/split_single_host.rs`: `split_load_places_layers_as_planned_single_host` |
| D3 stage engine | `rpc_server.rs`: `rpc_server_args_are_fixed`, `rpc_server_env_has_no_rdma_no_cache`, `banner_names_the_one_device`; `tests/rpc_server_single_host.rs`: `rpc_server_binds_loopback_only_single_host`, `rpc_server_stops_on_drop_single_host`, `squatted_rpc_port_is_detected_and_sent_nothing_single_host`, `foreign_client_on_rpc_port_is_detected_single_host`, and on Linux only `rpc_server_memory_cap_refuses_alloc_synthetic_head_single_host`, `rpc_server_survives_blocking_pool_idle_single_host`, `rpc_server_dies_with_daemon_single_host`, `rpc_server_runs_at_nice_10_single_host`; `sockets.rs`: `linux_listener_clients_and_squatters`, `macos_real_lsof_listener_clients_and_squatters`, `real_sentinel_capture_gives_the_expected_facts`; `ucompute-cli`: `reaper_stops_orphaned_rpc_server` |
| D4 messages and minor | `proto.rs`: `every_message_round_trips`, `control_proto_mirrors_proto_rs`, `tunnel_open_frame_over_1kib_is_frame_too_large`, `minor_1_peer_sees_rpc_frames_as_unknown`; `tests/peer_transport.rs`: `negotiated_minor_is_min_of_both_single_host`, `rpc_session_refused_by_minor_1_peer_single_host` |
| D4 carriage, owners, filter | `tests/invariants.rs`: `no_plaintext_mode_exists` (TCP only in `rpc.rs`, loopback only); `tests/rpc_single_host.rs`: `bridge_refuses_a_local_process_that_is_not_our_engine_single_host`, `foreign_client_on_the_stage_port_tears_down_single_host`, `foreign_client_refuses_the_next_tunnel_before_the_tick_single_host`, `comm_init_never_reaches_the_stage_single_host`, `stage_refuses_a_function_pointer_graph_from_a_hostile_head_single_host`, `stage_refuses_a_null_graph_node_from_a_hostile_head_single_host`, `a_real_sessions_commands_all_reach_the_stage_synthetic_head_single_host` (with commands 4 and 7); `rpc_meter.rs`: `commands_outside_the_allowlist_are_refused_at_their_first_byte`, `a_function_pointer_op_is_refused_before_the_graphs_last_byte`, `an_oversized_graph_is_refused_at_its_header`, `a_real_session_replays_to_the_recording_proxys_totals` (the lab's 7de354b recapture: its three real `GRAPH_COMPUTE` payloads carry 13 distinct ops, all in 0-48 or 74 or 100, none refused) |
| D5 authorization and consent | `tests/rpc_single_host.rs`: `rpc_session_refused_without_pooled_policy_single_host`, `pooled_needs_policy_on_both_ends_single_host`, `rpc_session_refused_for_engine_release_mismatch_single_host`, `group_policy_off_mid_session_tears_down_within_5s_single_host`, `revoke_and_hold_mid_session_tear_down_single_host`; `rpc.rs`: `test_flag_needs_a_loopback_peer`; `ucompute-api` `split.rs`: `licence_gate_refuses_unknown_licence_cross_device`, `test_flag_ignored_for_non_loopback_peer`, `disclosure_names_the_peer_and_what_it_sees`, `plans_follow_adr_0005_d2`; `tests/mesh.rs`: `split_target_denied_before_any_connection_without_pooled_policy`, `split_requires_split_at_and_acknowledgement`; `tests/split_single_host.rs`: `group_pooling_off_mid_session_stops_the_split_engine_single_host` |
| D6 limits | `rpc.rs`: `byte_cap_is_two_estimates_plus_a_gib`, `tunnel_cap_and_idle_and_ttl`; `rpc_meter.rs`: `byte_cap_counts_per_load`, `byte_cap_trips_above_the_cap_per_direction`, `the_upload_never_arms_the_watchdog`, `a_decode_step_is_one_turn_and_its_bytes`, `switches_are_per_tunnel_and_totals_per_session`; `tests/rpc_single_host.rs`: `rpc_session_refused_when_memory_short_single_host`, `second_session_refused_single_host`, `tunnel_cap_is_8_single_host`, `session_byte_cap_resets_tunnels_per_load_single_host`, `session_ttl_and_idle_close_single_host`, `silence_watchdog_resets_hung_stage_single_host`; `ucompute-models`: `stage_estimate_below_full_model`, `non_uniform_kv_models_are_refused_for_a_split`; `manager.rs`: `a_split_head_plans_with_its_stage_estimate` |
| D7 failure, no fallback | `tests/rpc_single_host.rs`: `connection_loss_stops_remote_rpc_server_within_6s_single_host`, `tunnel_for_unknown_or_foreign_session_refused_single_host`; `ucompute-api` `tests/split_single_host.rs`: `split_peer_killed_mid_generation_is_peer_lost_within_10s_single_host`, `split_cancel_mid_generation_single_host`, `unload_stops_remote_rpc_server_single_host`, `remote_memory_released_when_head_engine_stops_single_host`; `manager.rs`: `mode_b_refused_while_split_loaded` |
| I-05 local stays local | `ucompute-api` `tests/split_single_host.rs`: `local_request_never_uses_the_split_engine_single_host` |
| D8 exposure, no content in logs | `tests/rpc_single_host.rs`: `audit_has_session_and_tunnel_lines_without_content_single_host`, `packet_capture_contains_no_plaintext_canary_single_host`, `tunnel_bytes_and_switches_counted_both_directions_synthetic_head_single_host` |
| D10 claims and the golden check | `ucompute-api` `tests/split_single_host.rs`: `split_completion_golden_64_tokens_single_host` (configurations P and C equal the local run, SINGLE-HOST) |
| C8 driver (offline, synthetic) | `tests/distributed/two_machine/test_split_driver.py`: `test_synthetic_run_passes_every_step`, `test_each_broken_behaviour_fails_its_step`, `test_a_canary_in_a_head_log_fails_the_scan`, `test_parsers`, `test_synthetic_runs_lab_input_is_accepted_by_the_lab_scorer` and `test_lab_helper_input_through_this_drivers_writer_is_accepted_and_scored_alike` (both run the Core Lab's `score_p1.py`, read only; skipped without that checkout); `test_runners.py`: `test_every_script_a_runner_names_exists` |

### 9.4 The P1 driver
`run_mac_linux.sh SPLIT=1` starts this Mac's dev daemon (H, `.ucompute-split` hardlinked from `.ucompute`) and the Linux daemon (R), quits the installed app, saves and restores both daemons' state, and runs `split_two_machine.py` under `caffeinate -dimsu`. It follows the Core Lab's P1 contract at d569c5b (`p1-predictions/SCORING-INPUT.md`, `PREFLIGHT.md`), which supersedes plan §3 where they differ:
- **Preflight** (P1.0 plus every `PREFLIGHT.md` item; a failure sends no request): a clean tree at HEAD, R's `~/src/ucompute` holding exactly HEAD's files (the commit `scripts/mac-host.sh sync` recorded, then every file's blob hash) and both daemons' `/health` commit equal to HEAD; on R no `llama-server`, `ggml-rpc-server` or soak process, `ss -ltnupH` shows no listener of the run user's other than UCompute's (its TCP on loopback), no active or online `loginctl` session but the run's own SSH sessions and the user manager, and `who` empty apart from the run's own SSH session (OD-10); an R command that fails (ssh exits 255) FAILs its step; R's 1-minute load below 1 and H's below 2 for 5 minutes; AC power, `caffeinate`, at least 3 GB free and no other engine or build on H; the busiest processes on both hosts and a `ping` RTT are recorded.
- **Pre-registration.** `split_prereg.json` (prompts and canaries from the lab's 13fc793 `prompts.json`; R1a `split_at` 12, R1b 14; `T_rpc` 10 s; the caps; the pass rules) is written into the run as `prereg.json` before P1.1.
- **Requests.** H's `/v1/completions`, streamed, temperature 0, seed 42, `stream_options.include_usage`, the raw prompt text, `split_mode` pooled (configuration P), `acknowledged`. UCompute's adapter always sends `cache_prompt: false`, so both requests prefill.
- **Counters and timing.** Per prompt, in one session with nothing in between: `loaded`, after a `max_tokens` 1 request and after a `max_tokens` 65 request, each `GET /api/status` `split` 0.2 s after `[DONE]`. Per decode token = ((after_n65 − after_n1) − (after_n1 − loaded)) / 64; turns = that difference for switches / 2. TTFT and TPOT are client-side times of the 65-token stream. `<run>/lab-input.json` is the lab's input format, with `link` `wifi` or `wired`; token ids are left out (UCompute's `/v1/completions` returns none).
- **Also checked.** The four default-deny cases (P1.1), the session (P1.2: placement, rpc-server on `127.0.0.1` only and refused from H's LAN probe at R's IPv4 address while R's sshd answers there, the `transport` banner), two loads (P1.3), a local request with the split loaded (P1.9), local references (tokens compared as measured), the audit join (`/api/audit?n=1000` on both sides; a full page must reach back past the run's start, and the sessions opened equal those the driver saw) and the canary scan of every log, audit file and run record on both hosts (P1.10), teardown (P1.11).
- **Not automated:** memory (P1.5), the capture (P1.7), the nine failure cases (P1.8), configuration C (P1.4) and the R-range weight pattern; each needs its own evidence.
