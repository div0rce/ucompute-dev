# WP-04 P1: one model split across two physical machines, 2026-11-06 — TWO-MACHINE, partial

This Mac (H: MacBook Air, Apple M2, 8 GB, Metal) and sentinel (R: Ubuntu 26.04 x86_64, i9-9900K, CPU), home Wi-Fi (ICMP
6.0/7.0/9.0 ms before the run, `manifest.json`; the QUIC connection's smoothed RTT 8.2/12.0/55.0 ms across the 18 checkpoints
during it, the Core Lab's reading), commit 1038a7f on both hosts (`/health` and R's tree equal HEAD). Run folder
`pool-two-machine-2026-11-06/run-0820`, 08:20:51–08:27:12 UTC, `tests/distributed/two_machine/split_two_machine.py`:
**23 steps, 0 FAIL.** First genuine execution of one model across two independent machines in this project: each
machine ran only its own layers, activations crossed UCompute's authenticated, encrypted transport, and tokens came back.

This is P1's automated part. It is not the whole of P1 and does not satisfy Gate D: see "Still owed".

## The run started on the owner's instruction, with two gates lifted

Registered preflight (`split_prereg.json`): this Mac on AC, at least 3.0 GB free, 1-minute load below 2.0 for 300 s. At
start the Mac was on AC with 1.34 GB free at the preflight (1,336,344,576 B) and a load of 12.7–20.6 (a release build had just finished). The owner said in the
engineering session on 2026-11-06, "no just start it". Amendment, written before any generation and kept in the run
(`prereg.json`, `amendment_owner_start`): `h_free_min_bytes` 3.0 GB → 0, `h_load_max` 2.0 → 99, `quiet_s` 300 → 60.
R's load limit, AC power, same commit, no other sessions and everything after preflight stayed as registered. The
tracked rules did not change (HEAD stayed 1038a7f; the runner copy with `--prereg` is `run-0820/run_mac_linux_owner_start.sh`).
The Core Lab's reading, offered beforehand: bytes, turns, placement, tokens, leak checks and failure typing count;
**TPOT and TTFT are measured and not scored**; a later quiet run under the registered rules owes the timing.
Two earlier starts ran no generation (`docs/VERIFICATION.md`, 2026-10-30/31).

## Results

| Check | R1a: qwen2.5-0.5b q8_0, split at 12 | R1b: qwen2.5-1.5b q4_k_m, split at 14 |
|---|---|---|
| Placement at `-lv 4` (`session_*` steps) | RPC0 [0,12), MTL0 [12,24), as planned | RPC0 [0,14), MTL0 [14,28), as planned |
| Load 1, load 2 (both upload) | 41.7 s, 36.0 s | 86.3 s, 82.2 s |
| Upload to R per load | 190,806,685 B | 402,276,324 B |
| Bytes per decode token, H to R (lab prediction ±10 %) | 17,021.6 (16,921; +0.6 %) | 25,173.1 (25,058; +0.5 %) |
| Bytes per decode token, R to H (prediction) | 3,592 (3,592), hidden × 4 + 8 | 6,152 (6,152), hidden × 4 + 8 |
| Turns per token (prediction 1.00) | 1.000 | 1.000 |
| Stage loss seen by H | 0.345 s | 0.243 s |
| Greedy text equal to local Metal, 3 prompts | 1 of 3 (prefixes 135 and 265 chars on the others) | 2 of 3 (prefix 164 chars on the third) |
| TPOT, measured, not scored (band) | 34.0–38.6 ms (20.0–46.2) | 49.0–51.8 ms (29.0–68.8) |
| TTFT, measured, not scored (band) | 153.6, 349.4, 140.6 ms; two outside 150.8–318.8 | 305.1–345.1 ms, inside |

The scorer (`lab-score.json`, unchanged) reads `bytes_ok` and `turns_ok` true on every run of both models.

Passed as well: default deny on hardware before any session (`deny_unpaired`, `deny_remote_only`, `deny_stage_refuses`,
`deny_unacknowledged`, typed 403 and no rpc-server on R); `ggml-rpc-server` bound to 127.0.0.1 only, the LAN probe of its
port refused while the control succeeded; local requests stayed local with a split loaded (R's counters moved by 64 and
24 B of control only); audit sessions joined and closed on both hosts; the three canaries appear in no log on either
host; nothing left on R at teardown; both daemons' state restored. The owner's app was not running before the run, so
the runner did not start it.

**Tokens.** The prefix differences are the expected Metal head with an x86 CPU stage (ADR-0005 D10, OD-3; the lab's
near-ties in S1-M): the text is valid, and not identical to a Metal-only run. F-03-04's criterion is same-backend, so this
run does not meet it; configuration C (P1.4) is what speaks to it.

## Independent score (Core Lab, 2026-11-06)

`p1-predictions/RESULT-run-0820.md`, lab commit ce00165: **bytes, turns and placement PASS on all six runs; 4 of 4 denials
typed; 3 of 3 canaries sent and 0 found in the 22 committed files; timing has no verdict under D-033.** The lab recomputed
the byte figures from the raw counters and reproduced `lab-score.json` exactly. Its reading of that file's
`ramp_down_on_real_hardware: supported`: not a result of this run. The field only compares TPOT with registered values, and
with local Metal in the same run ×1.19–×1.20 slower than registered, the model allows ρ 1.65–2.86 (R1a) and 1.28–2.33 (R1b):
compatible with 2.5, not a measurement of it.

Facts of this loaded Mac, not typical values (lab): split TPOT ×3.54 and ×2.84 local Metal; tunnel upload goodput
36.6–42.4 Mbit/s against the 100 and 300 the planner was shown; a split load takes 36–86 s because every load uploads
weights R already holds; tunnel overhead +3.5 % on real Wi-Fi.

**Defect found by the lab, fixed after this run:** every session closed `cancelled` on H and `peer_lost` on R, so R's audit
read each clean close as a failure. R now records `cancelled` when H ends a session with the connection up.

## Still owed before milestone B and Gate D

| Plan step | What is missing |
|---|---|
| P1.4 | Configuration C and R1b against local sentinel CPU |
| P1.5 | Per-process peak memory on both hosts (`footprint`, `VmHWM`): F-03-01 and F-03-02 require it, and this run recorded none |
| P1.7 | The relay capture and its positive control |
| P1.8 | The nine failure and cancel cases with generation in flight, on hardware |
| P1.6 timing | A quiet run under the registered preflight (AC, 3.0 GB free, load below 2.0) |

Run with memory unprotected on an 8 GB Mac: this says nothing about performance under the registered conditions.
