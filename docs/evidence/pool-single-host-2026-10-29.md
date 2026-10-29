# WP-04 S1-M: configuration C on ggml-cpu at both stages, bytes per token, H-side P1 preflight (SINGLE-HOST, 2026-10-29)

**SINGLE-HOST.** Everything here ran on one machine, this Mac. Both daemons ran in one test process on loopback. Nothing here satisfies a two-machine criterion (F-03-01, F-03-02, F-03-04's Gate D numerical check, Gate D). No ledger row is promoted by this document. Plan: `docs/design/wp04-implementation-plan-2026-10-23.md`, "Stage order" item 2 (S1-M) and "P1 procedure" P1.0; ADR-0005.

## Setup

| | |
|---|---|
| Host | MacBook Air, Apple M2, 8 GB, macOS 26.2 (25C5048a); the machine was busy (load average 3.3 to 6.8: other sessions, Spotlight, Time Machine) |
| Tree | branch `wp04/fix-pass` at `b97527e`, plus the uncommitted test-only change below (not committed, not pushed) |
| Toolchain | rustc 1.98.0 (88d9e12ae 2026-08-18), debug test profile for the test binaries, release profile for the preflight daemon |
| Engine | llama.cpp b11609 (`llama-server --version`: build 11609, commit fcac19079), archive `llama-b11609-bin-macos-arm64.tar.gz` sha256 `d03f28b5a5baa9cdfbb01db5ed22c695126c11c193326f14cdc51d001be10f4e` (equal to `catalog/backends.json`); `ggml-rpc-server` banner "RPC server v8.0.0", transport TCP |
| Model (R1a) | `qwen2.5-0.5b-instruct-q8_0`, sha256 `ca59ca7f13d0e15a8cfa77bd17e65d24f6844b554a7b6c12e07a5f89ff76844e`, Apache-2.0; 24 blocks, hidden 896 (3,584 B per f32 activation), vocabulary 151,936 |
| Prompts | the three pre-registered P1 prompts, `tests/distributed/two_machine/split_prereg.json` (44, 50 and 47 tokens); raw completion, temperature 0, seed 42, 65 tokens; `cache_prompt: false` (UCompute's adapter) |
| Split | `split_at` 12, `split_mode` `same_backend` (configuration C): H's CPU holds blocks [0,12), R holds [12,24) and the output layer, so one logits row crosses per token |

## Code change (test-only)

- `crates/ucompute-runtime/src/llamacpp/rpc_server.rs`: `TEST_STAGE_CPU_ENV` (`UCOMPUTE_TEST_STAGE_CPU`). When it is `1` and `UCOMPUTE_TEST_SINGLE_HOST_POOL` is also `1`, `start_at` adds `-d CPU` to the rpc-server line, so R runs ggml-cpu, not this Mac's Metal. The server still exposes exactly one device (`CPU (Apple M2)`). Production sets neither flag, so `args()` is unchanged. A Mac stage needs the pool flag and a loopback peer anyway (`rpc::may_host`). Unit test: `rpc_server_cpu_stage_only_under_both_test_flags`. `TEST_POOL_ENV` now lives in this file, and `ucompute_network::rpc::TEST_POOL_ENV` re-exports it, so there is one spelling.
- `crates/ucompute-api/tests/split_single_host.rs`: `config_c_cpu_both_stages_r1a_prereg_single_host`, `#[ignore]` (a measurement, run on demand). It does the following:
  - loads R1a on H's CPU only (`device: cpu`, so `-ngl 0 --device none --no-kv-offload --no-op-offload`) and records 65 greedy token ids per prompt;
  - opens the split through `POST /v1/completions` with the pre-registered load request ("Hello", 1 token);
  - asserts the placement (CPU [0,12), RPC0 [12,24)) and R's stage device (`CPU (Apple M2)`);
  - generates on the split engine (`split::ensure`'s model, `return_token_ids`) with checkpoints of `/api/status` `split` and of H's QUIC connection to R (`/api/peers` `tx_bytes`/`rx_bytes`, quinn UDP payload bytes);
  - prints one `S1M {json}` line per prompt, then asserts that the ids are equal.

## Commands

```sh
caffeinate -i env UCOMPUTE_MDNS=0 UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 \
  cargo test -p ucompute-api --test split_single_host config_c_cpu -- --ignored --nocapture
```

The command ran three times:

- run 1 (21:28 UTC): the lab's checkpoint method exactly as P1's driver applies it;
- runs 2 (21:38 UTC) and 3 (21:55 UTC): with the same-prompt warm-up described under "Finding".

The engine-direct diagnostics and the plaintext twin run the pinned binaries directly. Their scripts are in `pool-single-host-2026-10-29/scripts/` (run from the repository root); the command lines are recorded as `cmd` (llama-server) and `rpc_server_cmd` (ggml-rpc-server: `-t 6 -d CPU`, or `-t 4 -d CPU` for `split_direct_norepack_t4`) in `engine-direct-ids.json`, and as `rpc_server_cmd` in `plaintext-twin.json` (added after the verifier found the rpc-server lines missing).

## 1. Tokens: split against the local CPU-only run

Placement from the engine log, via the API: `{"stages":[{"device":"CPU","first_block":0,"end_block":12},{"device":"RPC0","first_block":12,"end_block":24}]}`. Stage device: `CPU (Apple M2)`.

| Prompt | Prompt tokens | Split ids = local CPU ids (65) | First differing position | Top-2 log-probabilities at that position, local run |
|---|---|---|---|---|
| 1 | 44 | no | 30 | 825: −1.3067, 1030: −1.3661 (gap 0.059) |
| 2 | 50 | **yes, 65 of 65** | — | — |
| 3 | 47 | no | 10 | 11: −1.16775, 13: −1.16807 (gap 0.0003) |

The ids were identical in all three runs, for the split and for the local run. Both sides are deterministic, so the test's final assertion failed on prompts 1 and 3 in every run. **Result: the split's greedy tokens do not equal the local CPU-only run's for 2 of 3 prompts. The same-backend pass rule ("identical greedy tokens for 64 positions") is not met on this configuration.**

### Cause investigated: the engine's kernel choice, not the carriage

The engine was run directly (no UCompute daemon, plaintext loopback RPC, no QUIC), with UCompute's own split line (`--rpc … --device RPC0 --tensor-split 1 --split-mode layer -ngl 13 --no-op-offload`) and its own CPU-only line. Each cell below is the first differing position of the 65 ids, for prompts 1/2/3; "=" means equal.

| | UCompute local (CPU) | UCompute split (tunnel) |
|---|---|---|
| direct local, repack on (the default) | = / = / = | 30 / = / 10 |
| direct local, `--no-repack` | 30 / 29 / 10 | 33 / 29 / 38 |
| direct split, repack on, plaintext RPC | 30 / = / 10 | **= / = / =** |
| plaintext twin (direct split behind the lab's counting proxy) | — | **= / = / =** |
| direct split, `--no-repack`, rpc-server `-t 6` or `-t 4` | = / 14 / 10 | 30 / 14 / 38 |

- **Carriage exactness.** The split through UCompute's QUIC tunnel gave the same 65 ids on every prompt as the same engine line over plaintext loopback RPC, with and without the counting proxy. On this evidence the tunnel changes no token.
- **Where the difference comes from.** The local run repacks every q8_0 weight for the CPU (`CPU_REPACK model buffer size = 500.52 MiB`). The split repacks only H's half (`181.29 MiB`). R's half is uploaded into a plain RPC buffer and runs ggml-cpu's unrepacked kernels.
- **The tie-flip mechanism.** The same engine on the same CPU flips the same near-ties with repack on and with repack off (local repack against `--no-repack`: 30 / 29 / 10). The differences appear only where the top two candidates nearly tie, at gaps of 0.0003 and 0.059 in log-probability.
- **Not isolated further.** With repack off at both ends, split and local still differ (30 / 14 / 55 against direct local `--no-repack`). rpc-server's thread count is ruled out: `-t 4` and `-t 6` gave identical ids. A candidate, not verified: llama-server registers the Accelerate BLAS device (`- BLAS : Accelerate` in its log) and may use it for batched prefill matmuls on CPU weights, while `ggml-rpc-server -d CPU` exposes only the CPU device.

**For OD-3 and F-03-04.** On this Mac, "ggml-cpu on both stages" is not one numeric path: the local whole-model engine and the RPC stage use different CPU kernels for the same weights. A token-identity rule therefore fails on near-ties for reasons that have nothing to do with the carriage. The carriage-exactness candidate that held here is a different comparison: tunnel against plaintext RPC, with the same engine line, 3 of 3 prompts and 65 of 65 ids. No max-abs logits comparison was made: the API exposes no logits, and the engine-direct runs record top-3 probabilities only.

## 2. Bytes per token (configuration C, R1a, runs 2 and 3)

The counters are H's session counters (`/api/status` `split`): plaintext RPC payload counted at H's pump, before QUIC. They were read at the lab's checkpoints, 0.2 s after each generation. "Decode per token" is `((after_n65 − after_n1) − (after_n1 − loaded)) / 64`. The plaintext counters were byte-identical in runs 2 and 3, and the decode figures were identical on all three prompts. They also equal the plaintext twin's proxy counts exactly (`plaintext-twin.json`).

| Per generated (decode) token | Bytes | × hidden·4 |
|---|---|---|
| H → R (`to_r`) | **13,029.25** | 3.64 |
| R → H (`to_h`) | **607,752** (151,936 × 4 + 8: one f32 logits row) | 169.6 |
| Direction switches | 2.00 (1.00 synchronous turn per token) | |

Breakdown of H → R per decode token, from the plaintext twin:

| Item | Bytes per token |
|---|---|
| SET_TENSOR (7 requests) | 9,902 |
| … data: `ffn_inp-11` and `ffn_out-11`, two f32 rows of 896 | 7,168 |
| … data: `attn_inp_kq_mask`, 256 f16 | 512 |
| … data: `inp_pos`, `out_ids`, `attn_inp_k_idxs`, `attn_inp_v_idxs` | 24 |
| … headers | 2,198 |
| GRAPH_COMPUTE (one upload of 178,277 B in the 64 steps, at the first decode step) | 2,785.6 |
| GET_TENSOR (the logits request) | 321 |
| GRAPH_RECOMPUTE | 20.7 |

The steady state, without the one graph upload, is **10,244 B per token** towards R. Two activations cross, not one: the scheduler places block 11's last residual add (`ffn_inp-11 + ffn_out-11`) on R, so both operands cross instead of their sum.

| Per prompt token (prefill: the max_tokens 1 request after the warm-up) | Prompt 1 (44) | Prompt 2 (50) | Prompt 3 (47) |
|---|---|---|---|
| H → R, whole prefill | 341,344 | 387,544 | 364,444 |
| H → R per prompt token | 7,757.8 | 7,750.9 | 7,754.1 |
| R → H, whole prefill (one logits row: only the last position's output is computed) | 607,752 | 607,752 | 607,752 |
| R → H per prompt token | 13,812.5 | 12,155.0 | 12,930.9 |

H → R is 7,700 B per prompt token (two activations, one mask row of 512 B, 20 B of positions and indices) plus 2,544 B per request.

### QUIC and AEAD overhead (same runs)

The overhead is the UDP payload bytes of H's QUIC connection to R (quinn `udp_tx` and `udp_rx`) minus the plaintext counters, per decode token. The UDP counts include ACK frames, keep-alives and any other frame on that connection.

| Prompt (runs 2 / 3) | UDP H → R | Overhead over `to_r` | UDP R → H | Overhead over `to_h` | Both directions |
|---|---|---|---|---|---|
| 1 | 17,096.9 / 17,029.6 | +31.2 % / +30.7 % | 621,685.0 / 621,675.6 | +2.29 % / +2.29 % | +2.90 % / +2.89 % |
| 2 | 17,244.1 / 16,993.5 | +32.3 % / +30.4 % | 621,714.4 / 621,692.7 | +2.30 % / +2.29 % | +2.93 % / +2.88 % |
| 3 | 17,118.2 / 16,787.5 | +31.4 % / +28.8 % | 621,697.8 / 621,722.2 | +2.29 % / +2.30 % | +2.91 % / +2.86 % |

- **The bulk direction.** R → H carries about 13.9 kB per token above its 607.8 kB payload (2.3 %). That is QUIC packet and frame headers plus the 16-byte AEAD tag per packet.
- **The small direction.** H → R's 3.8 to 4.2 kB per token is mostly H's ACKs for that inbound bulk, not encryption of H's own requests. It is therefore large in relative terms (about 30 %) and small in absolute terms.
- **Both directions together.** The QUIC and AEAD carriage adds **about 2.9 %** to the plaintext RPC bytes in this configuration.
- **Prefill.** H → R +3.2 to +3.4 %, R → H +2.5 to +2.6 %.

### Against the lab's R0 (plaintext loopback)

R0 has no R1a figure and no configuration with a CPU head. It ran stories15M (hidden 288, vocabulary 32,000) only. The closest row is R0 configuration C, the stage that holds the last blocks and the output (RPC1). Lab worktree `eloquent-williamson-2da66b` at `8e7ac65`, `rpc-dataflow/results-r0/results.md` line 26 reads:

> `| C | 1 | 3761 | 128007 | 0.96 | … l_out-2#0 1152 [1.00] … |`

That row gives, per decode step, prefill excluded: 3,761 B to the RPC server, 128,007 B to the head, 0.96 turns. Line 12 gives the whole-request figures, 4,867 and 130,008 B per token.

- **Towards the head.** The structure matches: one f32 logits row plus a reply header. R0 has 32,000 × 4 + 7 = 128,007 B; here 151,936 × 4 + 8 = 607,752 B. These are plaintext counts on both sides, and the twin shows UCompute's counters equal the plaintext proxy's to the byte. So the QUIC and AEAD cost is not in the counters at all. It is the UDP figures above: about 2.3 % on this direction.
- **Towards the stage.** The bytes do not scale from R0. R0's stage boundary fell between two RPC servers, and one activation crossed (`l_out-2`). Here the head is H's own CPU, and the scheduler sends two (`ffn_inp-11`, `ffn_out-11`). R0's 3,761 B scaled to hidden 896 would be about 6,193 B per step; measured here: 13,029 B, or 10,244 B in steady state.
- **No P comparison.** The lab's P1 predictions (`p1-predictions/PREDICTIONS.md` lines 46–52: 14,150 B towards R and 3,592 B towards H for R1a) are for configuration P, not C. Nothing here tests them.

## Finding: llama-server's prompt cache confounds the lab's checkpoint subtraction

The pinned llama-server keeps a host-memory prompt cache by default. Its log reads "prompt cache is enabled, size limit: 8192 MiB" and "idle slots will be saved to prompt cache upon starting a new task". When a request with a different prompt starts, it saves the slot's previous state, and that save reads R's KV cache back to H over the tunnel. The engine-direct log shows "saving prompt with length 108" and then "114", in the tunnelled run as well. The n65 request on the same prompt does not trigger a save.

The save was measured per prompt as (warm-up window − n1 window), runs 2 and 3, identical:

- R → H: 6,400 B after "Hello" (1 token), 663,808 B after prompt 1's 108 tokens, 700,672 B after prompt 2's 114 tokens. That is 6,144 B per cached token (12 layers × K and V × 2 KV heads × 64 × f16) plus 256.
- H → R: 199,036 B. This is probably mostly a full graph re-upload, because the previous request ended on a decode-shaped graph; the decode graph's upload measured 178,277 B. The save's own requests and the re-upload are not separated.
- 56 direction switches.

Run 1 used P1's driver order: `loaded`, then max_tokens 1, then max_tokens 65, with no warm-up (`tests/distributed/two_machine/split_two_machine.py` line 466). It read the following per decode token:

| Counter | Prompt 1 | Prompt 2 | Prompt 3 | Correct value (runs 2 and 3) |
|---|---|---|---|---|
| `to_r` | 9,919.3 | 9,919.3 | 9,919.3 | 13,029.25 (−24 %) |
| `to_h` | 607,652 | 597,380 | 596,804 | 607,752 (about −1.7 % on prompts 2 and 3) |
| Switches | 1.125 | 1.125 | 1.125 | 2.0 (0.56 turns per token instead of 1.00) |

The previous prompt's save lands in the `after_n1 − loaded` term, which the method subtracts.

**Effect on P1 (not measured here).** The same mechanism applies in configuration P whenever a request follows a request with a different prompt. Under P, R holds blocks [0,12), so the save reads R's KV back to H. Here that is 663,808 B over 108 tokens. Spread over 64 decode tokens it is about 10.4 kB, against a predicted 3,592 B per token towards H. As written, the driver's per-token bytes and turns would fail the lab's ±10 % and 1.00 ± 0.05 rules for this reason, not because of the carriage. Two remedies exist:

- the warm-up this test uses: a max_tokens 1 request on the same prompt before `loaded`;
- `--cache-ram 0` on the split engine, which also stops H reading R's KV after each request.

**Resolution (same day, before any P1 run).** Both are applied, for different reasons:

- **`--cache-ram 0` on every engine (product).** The adapter sends `cache_prompt: false` on every request, so a saved prompt is never reused: the save only held host RAM (up to 8 GiB by default) and, on a split, read R's KV back over the network. `crates/ucompute-runtime/src/llamacpp/process.rs` adds it to the engine line and reserves `--cache-ram`/`-cram` from extra arguments (`cpu_args` asserts it). Rerun of this test with it (prompts 1-3 identical): the save towards H is gone (warm-up window minus n1 window 64 B, from 663,808-700,672 B); decode per token unchanged (H to R 13,029.25 B, R to H 607,752 B, 2.0 switches).
- **A same-prompt max_tokens 1 warm-up before `loaded` in the P1 driver (measurement).** With the cache off, the first request on a new prompt still carries a one-time graph upload: 191,332 B and 8 switches to R more than the next request on the same prompt. In the driver's old order that lands in the subtracted term: H to R would read about 10,040 B per token (−23 %) and 1.875 switches (0.94 turns). `split_two_machine.py` now sends the warm-up and requires it to succeed; `split_prereg.json` `checkpoints` is amended (allowed: no P1 run exists).

## 3. Timing (as measured; not a claim)

llama-server's own timings, 65-token request, runs 2 / 3:

| Prompt | Split decode, ms per token | Local CPU decode, ms per token |
|---|---|---|
| 1 | 101.3 / 84.6 | 11.6 / 27.7 |
| 2 | 130.4 / 90.7 | 12.0 / 25.4 |
| 3 | 83.6 / 86.9 | 11.6 / 31.5 |

Both daemons ran in one debug-build process and pumped about 0.6 MB per token through loopback QUIC. The machine was busy. These are SINGLE-HOST observations with no bearing on P1's predictions or on any speedup, and none is claimed.

## 4. H-side P1.0 preflight dry run

The steps:

1. Stashed the uncommitted change (`git stash push`), leaving the tracked tree clean at `b97527e`. Created one untracked file, this document as a placeholder.
2. Built with `caffeinate -i cargo build --release -p ucompute-cli` (2 min 14 s).
3. Started `UCOMPUTE_MDNS=0 UCOMPUTE_HOME=<scratch home> target/release/ucompute serve --host 127.0.0.1 --port 64627`. The scratch home holds symlinks to `.ucompute/models` and `.ucompute/backends`; ports 8192 and 8193 were not touched.

Results:

- `GET /health` → `{"commit":"b97527e18488ab157b0e110242ca99292d98e7d7","status":"ok","version":"0.1.0"}`, equal to `git rev-parse HEAD`. The untracked file did not add `-dirty`.
- The driver's own H commands, run through its `shell("h", …)`:
  - `H_CMDS["git"]` (`git rev-parse HEAD && git status --porcelain --untracked-files=no`) gave HEAD and no tracked change;
  - `H_CMDS["tree"]` (`git ls-tree -r HEAD`) listed 467 entries.
- The H half of `same_commit` passes: HEAD non-empty, no tracked change, H's build commit equal to HEAD. Recorded in `pool-single-host-2026-10-29/preflight-h-half.json`.
- The daemon was stopped (SIGTERM; port closed, no engine left) and the change restored (`git stash pop`).
- Negative control: the same build with the change in the tree served `"commit":"b97527e…-dirty"`. That daemon was stopped too.
- R's half (`R_CMDS["tree"]` on sentinel, R's `/health`) was not run: sentinel is out of scope until its soak ends.

## 5. Suites re-run after the change

The change touches `rpc_server::start_at`, which C5 and C6 exercise, so they were re-run, one binary at a time, with `caffeinate -i env UCOMPUTE_MDNS=0 UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1`:

| Suite | Result |
|---|---|
| `cargo test -p ucompute-runtime --lib rpc_server` | 4 passed |
| `cargo test -p ucompute-runtime --test rpc_server_single_host` | 4 passed |
| `cargo test -p ucompute-network --lib` | 83 passed |
| `cargo test -p ucompute-network --test rpc_single_host` | 28 passed |
| `cargo test -p ucompute-api --test split_single_host` | 9 passed, 1 ignored (the S1-M measurement) |

`cargo clippy -p ucompute-runtime -p ucompute-network -p ucompute-api --all-targets -- -D warnings` is clean, and `cargo fmt --all -- --check` is clean. The C5 and C6 results recorded before this change are in `docs/VERIFICATION.md`, entry "WP-04 fix pass" (rpc_single_host 28/28, split_single_host 9/9).

No canary string appears in the S1-M records, the engine-direct outputs or `.ucompute/logs`. R's rpc-server logs lived in the test's temporary state directories and were not scanned.

## What this shows and does not show

**Shows (SINGLE-HOST, this Mac):**

- configuration C with ggml-cpu on both stages runs through the real QUIC tunnel, with the placement as planned;
- the tunnel's greedy tokens equal plaintext loopback RPC's for the same engine line (3 of 3 prompts, 65 of 65 ids);
- UCompute's split counters equal a plaintext proxy's to the byte;
- the QUIC and AEAD carriage costs about 2.9 % of the plaintext bytes in this configuration;
- llama-server's prompt cache distorts the lab's checkpoint method as P1's driver applies it;
- the H half of P1's `same_commit` check passes on a clean tracked tree with an untracked file present.

**Does not show:**

- anything across two machines, or about Wi-Fi or Ethernet;
- configuration P on this tree;
- cross-ISA (arm64 against x86) behaviour;
- the R half of the preflight;
- a numerical (max-abs) logits comparison;
- that split and local tokens match: they do not, on 2 of 3 prompts;
- a speedup.

F-03-04 is not promoted. OD-3's question stays open: the token-identity rule fails here on near-ties for kernel reasons, while carriage exactness held.

## Files

- `pool-single-host-2026-10-29/s1m-run1-lab-checkpoints.jsonl`, `s1m-run2.jsonl`, `s1m-run3.jsonl`: one record per prompt with checkpoints, per-token figures, ids, texts and engine timings.
- `pool-single-host-2026-10-29/engine-direct-ids.json`: command lines, ids and top-3 probabilities of the engine-direct configurations.
- `pool-single-host-2026-10-29/plaintext-twin.json`: the proxy's per-command and per-tensor counts, and the tensor shapes.
- `pool-single-host-2026-10-29/preflight-h-half.json`: the H-half preflight, raw shell output and `/health`, plus the dirty-tree control.
