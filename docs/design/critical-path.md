# UCompute critical path and dependency map (engineering-owned)

Updated at each checkpoint. One current priority per workstream. Status words follow the ledger:
nothing here is complete unless its evidence exists. The lab's WP-04 view is ucompute-core-lab
`WP04-DEPENDENCIES.md`; the website's plan is in its own repository.

## Current priority per workstream

| Workstream | Current priority | Next action |
|---|---|---|
| Engineering | WP-04 C5–C8 with the stage-side RPC filter (round 8, resumed 2026-10-29 after a weekly usage limit stopped it after C3 and C4) | Commit round 8 once its review and whole-tree check pass; then S1 SINGLE-HOST runs and P1 |
| Core Lab | Help WP-04: drop-ins, pre-registered numbers, scoring | Review the C2 and C5 diffs read-only; score P1 when its evidence exists |
| Website | Candidate 1 published and received | The browser and Lighthouse rerun on sentinel when it is reachable |

## Milestones and what each waits on

| Milestone | Proof required | Status | Blocked on | Owner |
|---|---|---|---|---|
| **A. macOS private-alpha artifact** | `verify_packaged_app.py` ALL PASS on a DMG built from a commit, including stage 6 (daemon SIGKILL: engine stopped, one restart, inference after) and the notices; promotion record | **Engineering part done:** Gates A and B at 3379fe9, packaged check ALL PASS 19/19, promotion record (0b0c1a4); artifact approved by the owner (D-025, SHA-256 a3239588…) | — | Engineering |
| A1. Publish A to named testers | The owner's written go-ahead in `docs/DECISIONS.md`; the website's preview page | **Done, 2026-10-29:** published to its cohort (the owner alone, stage-snapshot only, website a9c4166, hosted hash verified) and received: the owner reported "Yes, it works" after the download, checksum and launch steps | — (a wider cohort needs its own D-025 amendment) | Owner, website |
| **B. WP-04 P1: one model split across this Mac (Metal, H) and sentinel (CPU, R) over Common's authenticated transport** | Plan section 3 evidence: placement proved at `-lv 4`, valid tokens, no plaintext on the wire, typed failures; scored by the lab against `ae88d0b`/`1722119` | C1 done (5ef22fd); C2 pure part done (1fb589e); C2 launch committed (55b880a), proved SINGLE-HOST (64/64 tokens equal to local Metal, re-run 3/3 by the lab) | C3 (lab drop-ins `rpc_server.rs`, `sockets.rs` at 5dddc88), C4, C5 (lab `rpc_meter.rs` at aa6d5f2, numbers in C5-NUMBERS.md), C6, C8 (lab scorer 3500023), then **the R-side RPC filter** (lab 217f6bb: a 16-command allowlist from a real session, and refusing GRAPH_COMPUTE nodes with custom ops 92–95 or ops ≥ 101, because those make R call a function pointer the head chose; COMM_INIT/ALLREDUCE make rpc-server listen on 0.0.0.0 or reach any host), enforced on R before bytes reach rpc-server; then S1-M/S1-L SINGLE-HOST runs, then P1. A pooled session with a device the owner does not fully control also needs a sandbox for rpc-server (OD-8) | Engineering (lab supports) |
| **C. Linux headless install** | `verify_linux_install.py` ALL PASS from a tarball built at a commit with the round-6 runtime; 24 h soak PASS | Tarball path ALL PASS 16 checks on the pre-round-6 build (1e9ae23); soak driver ready (c122812); Linux suite 506/3 at round 7 (2 flood tests determined, owner decision; the memory estimate fixed in 4907256) | ALL PASS at 0b0c1a4 with the round 6+7 runtime (36761b4); the 24 h soak ran from 2026-10-24 04:40 (first sample: engine kill recovered in 0.69 s) but is not collected: sentinel is off the network | Sentinel back on the network; `run_soak_linux.sh collect` | Engineering |
| **D. Windows requalification** | NSIS build from HEAD on the laptop, `verify_windows_app.ps1`, `run_mac_windows.sh` | Not started this round | The laptop is offline | Engineering (laptop availability: owner) |
| **E. Mac-to-Mac run** | `run_mac_mac.sh` with both Macs awake, fixed builds | Not started this round | Kyle's Mac is offline | Engineering |

## Hardware availability (2026-10-29)

- This Mac: available; the owner's app runs on 8192 (quit and relaunched only by the packaged check).
- Sentinel: on Wi-Fi only (`eno2` has no cable). It dropped off the LAN between about 22:23 and 02:19
  local on 2026-10-23/24 without rebooting, and its SSH timed out again around 04:05. P1 needs a
  stable link for uploads of hundreds of MB. It was off the network for most of 2026-10-24 and rebooted on
  2026-10-28 (cause not visible without root). Likely cause of the drops: Ubuntu's default Wi-Fi power saving;
  the owner turned it off on 2026-10-29 (`wifi.powersave = 2` in NetworkManager, `iw ... power_save off`; the lab
  confirmed `Power save: off`, signal -58 dBm, link 351/390 Mbit/s). RTT from the Mac 6/23/103 ms
  (min/avg/max, sentinel under load); P1's preflight re-measures it with both hosts quiet, and an Ethernet cable
  remains the fix if the jitter stays.
- Windows laptop and Kyle's Mac: offline.

## Owner actions that unblock something

1. If sentinel's link still drops or jitters with power saving off: an Ethernet cable on its `eno2`.
2. When convenient: the laptop and Kyle's Mac awake and on the network (milestones D and E).
3. Mode B on CPU-only devices: a peer generation slows local tokens by 35–50% on a CPU engine (sentinel,
   determined in round 7). Keeping local priority needs preempting peer generations, which breaks
   "admitted requests complete" (F-08-08) for peers. Choose: preempt, refuse new peer work near local
   work (partial), or keep the behaviour and document the limit (default: keep and document).
