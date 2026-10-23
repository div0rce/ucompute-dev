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
| Protocol v8.0.0; the server serves each accepted connection to completion, one at a time (accept loop, `ggml-rpc.cpp` 2782-2790 at b11609) | ✅ (source, lab INTEGRATION §8, d741ef8) |
| Per-connection buffer isolation: each accepted connection gets a fresh `rpc_server` (2399-2401); `deserialize_tensor` nulls any buffer outside that connection's set (1641); the destructor frees all of the connection's buffers on close (2393), so a tunnel drop frees R's weights | ✅ (source, lab) |
| llama-server opens about 11-12 sequential TCP connections per RPC server per load-and-generate (plus 2 for `--list-devices`); only one is open at a time | ✅ (loopback measurement, Metal both stages, lab Addendum 9) |
| Each `SET_TENSOR` arrives whole in a `std::vector` (2565-2573), so R transiently holds its largest tensor twice (macOS: the freed buffer stayed resident, MALLOC_LARGE +48 MB for a 47.81 MiB output tensor); R's idle baseline is about 37 MiB | ✅ macOS / `unverified` Linux glibc |
| Config P per decode token: head to R three activations (embd, inp_mixed_embd, node_1) plus a 2·n_kv B f16 mask plus about 2.9 KB of descriptors and commands; R to head one activation plus 8 B; exactly one synchronous round trip per token; prefill: 3·p activations plus a 2·n_kv·p B mask up | ✅ (loopback, lab) |
| Same backend (Metal on both stages, loopback): greedy tokens equal local for 64/64 in every split configuration; the handshake logs `transport : TCP (RDMA auto-negotiate enabled)` | ✅ SINGLE-HOST (lab) |
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
- `pre_exec`: `PR_SET_PDEATHSIG(SIGKILL)` plus a `getppid` re-check, `RLIMIT_AS` = granted + the largest tensor of R's range (each `SET_TENSOR` is held twice transiently; in config C that is the output layer) + measured baseline (about 37 MiB on macOS) + thread stacks and arenas, nice 10.
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
| Tunnels | at most 8 open at once per session (a concurrent cap: llama-server opens about 12 sequential connections per load, so a per-session count would fail every load); the socket-owner check (lsof, 50-100 ms on macOS) adds about 0.5-1.2 s per load there |
| Bytes | `2 × est_bytes + 1 GiB` per direction per session, recalibrated after R0 and P1 |
| Memory | `est_bytes × 1.1 ≤ available now − 2 GiB` at admission; `RLIMIT_AS` thereafter |
| Time | TTL 30 min; idle close 10 s after the last tunnel; silence watchdog `T_rpc` while a request is in flight: H's only wait in a step is one `GET_TENSOR` reply per remote stage and the longest silence is R's prefill of its range, so `T_rpc` must exceed R's prefill time for the longest admitted prompt (bounded from the calibrated prefill rate; pre-registered from R0/S1) |
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
- F-03-03 stays open (B-016). RPC carriage bytes per token are reported as a ratio to `hidden × 4` (config P measures about 3.9 up and 1 down for qwen2.5-0.5B, lab; config C also returns vocab × 4 B per token, 608 KB for qwen2.5).
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
