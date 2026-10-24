# UCompute critical path and dependency map (engineering-owned)

Updated at each checkpoint. One current priority per workstream. Status words follow the ledger:
nothing here is complete unless its evidence exists. The lab's WP-04 view is ucompute-core-lab
`WP04-DEPENDENCIES.md`; the website's plan is in its own repository.

## Current priority per workstream

| Workstream | Current priority | Next action |
|---|---|---|
| Engineering | Milestone A's promotion record, then WP-04 C3–C6 and C8 (round 8) | Release-candidate gate at 3379fe9 (running); then round 8 with the lab's C3/C5 drop-ins |
| Core Lab | Help WP-04: drop-ins, pre-registered numbers, scoring | Review the C2 and C5 diffs read-only; score P1 when its evidence exists |
| Website | Ship an approved build to invited testers the same day | Wait for engineering's private-alpha promotion record; then the owner's written go-ahead |

## Milestones and what each waits on

| Milestone | Proof required | Status | Blocked on | Owner |
|---|---|---|---|---|
| **A. macOS private-alpha artifact** | `verify_packaged_app.py` ALL PASS on a DMG built from a commit, including stage 6 (daemon SIGKILL: engine stopped, one restart, inference after) and the notices; promotion record | ALL PASS 19/19 on the DMG built from 45af6ee (stage 6 included); TLS advisories fixed (b555109); gate re-run at 3379fe9 for the record | The release-candidate gate; then the record | Engineering |
| A1. Publish A to named testers | The owner's written go-ahead in `docs/DECISIONS.md`; the website's preview page | Not started | Milestone A; owner decision | Owner, then website |
| **B. WP-04 P1: one model split across this Mac (Metal, H) and sentinel (CPU, R) over Common's authenticated transport** | Plan section 3 evidence: placement proved at `-lv 4`, valid tokens, no plaintext on the wire, typed failures; scored by the lab against `ae88d0b`/`1722119` | C1 done (5ef22fd); C2 pure part done (1fb589e); C2 launch committed (55b880a), proved SINGLE-HOST (64/64 tokens equal to local Metal, re-run 3/3 by the lab) | C3 (lab drop-ins `rpc_server.rs`, `sockets.rs` at 5dddc88), C4, C5 (lab `rpc_meter.rs` at aa6d5f2, numbers in C5-NUMBERS.md), C6, C8 (lab scorer 3500023), then S1-M/S1-L SINGLE-HOST runs, then P1 | Engineering (lab supports) |
| **C. Linux headless install** | `verify_linux_install.py` ALL PASS from a tarball built at a commit with the round-6 runtime; 24 h soak PASS | Tarball path ALL PASS 16 checks on the pre-round-6 build (1e9ae23); soak driver ready (c122812); Linux suite 506/3 at round 7 (2 flood tests determined, owner decision; the memory estimate fixed in 4907256) | Sync HEAD to sentinel, rebuild the tarball, re-verify; 24 h soak | Engineering |
| **D. Windows requalification** | NSIS build from HEAD on the laptop, `verify_windows_app.ps1`, `run_mac_windows.sh` | Not started this round | The laptop is offline | Engineering (laptop availability: owner) |
| **E. Mac-to-Mac run** | `run_mac_mac.sh` with both Macs awake, fixed builds | Not started this round | Kyle's Mac is offline | Engineering |

## Hardware availability (2026-10-24)

- This Mac: available; the owner's app runs on 8192 (quit and relaunched only by the packaged check).
- Sentinel: on Wi-Fi only (`eno2` has no cable). It dropped off the LAN between about 22:23 and 02:19
  local on 2026-10-23/24 without rebooting, and its SSH timed out again around 04:05. P1 needs a
  stable link for uploads of hundreds of MB: a wired link is now the most useful owner action.
- Windows laptop and Kyle's Mac: offline.

## Owner actions that unblock something

1. A written go-ahead to publish milestone A to named testers (after A passes).
2. An Ethernet cable for sentinel before P1 (its Wi-Fi link has dropped twice in six hours).
4. Mode B on CPU-only devices: a peer generation slows local tokens by 35–50% on a CPU engine (sentinel,
   determined in round 7). Keeping local priority needs preempting peer generations, which breaks
   "admitted requests complete" (F-08-08) for peers. Choose: preempt, refuse new peer work near local
   work (partial), or keep the behaviour and document the limit (default: keep and document).
3. When convenient: the laptop and Kyle's Mac awake and on the network (milestones D and E).
