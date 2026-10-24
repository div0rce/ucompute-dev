# UCompute critical path and dependency map (engineering-owned)

Updated at each checkpoint. One current priority per workstream. Status words follow the ledger:
nothing here is complete unless its evidence exists. The lab's WP-04 view is ucompute-core-lab
`WP04-DEPENDENCIES.md`; the website's plan is in its own repository.

## Current priority per workstream

| Workstream | Current priority | Next action |
|---|---|---|
| Engineering | Close the release blockers (fix round 7) and land WP-04 C2–C6 | Commit round 7 once its whole-tree check and verifiers pass; then round 8 (C3–C6, C8 driver) |
| Core Lab | Help WP-04: drop-ins, pre-registered numbers, scoring | Review the C2 and C5 diffs read-only; score P1 when its evidence exists |
| Website | Ship an approved build to invited testers the same day | Wait for engineering's private-alpha promotion record; then the owner's written go-ahead |

## Milestones and what each waits on

| Milestone | Proof required | Status | Blocked on | Owner |
|---|---|---|---|---|
| **A. macOS private-alpha artifact** | `verify_packaged_app.py` ALL PASS on a DMG built from a commit, including stage 6 (daemon SIGKILL: engine stopped, one restart, inference after) and the notices; promotion record | Stage 6 failed on the round-6 DMG (reaper missed paths with spaces; fixed in abe19c1); re-check is in round 7 | Round 7 packaged run; then the record | Engineering |
| A1. Publish A to named testers | The owner's written go-ahead in `docs/DECISIONS.md`; the website's preview page | Not started | Milestone A; owner decision | Owner, then website |
| **B. WP-04 P1: one model split across this Mac (Metal, H) and sentinel (CPU, R) over Common's authenticated transport** | Plan section 3 evidence: placement proved at `-lv 4`, valid tokens, no plaintext on the wire, typed failures; scored by the lab against `ae88d0b`/`1722119` | C1 done (5ef22fd); C2 pure part done (1fb589e); C2 launch implemented and proved SINGLE-HOST in round 7 (uncommitted: 64/64 tokens equal to local Metal, Metal+Metal pair) | C3 (lab `sockets.rs` drop-in ready), C4, C5 (lab numbers ready: `T_rpc` 10 s during generation only), C6, C8, then S1-M/S1-L SINGLE-HOST runs, then P1 | Engineering (lab supports) |
| **C. Linux headless install** | `verify_linux_install.py` ALL PASS from a tarball built at a commit with the round-6 runtime; 24 h soak PASS | Tarball path ALL PASS 16 checks on the pre-round-6 build (1e9ae23); soak driver ready (c122812) | Round 7 Linux suite run; sync HEAD to sentinel; 24 h soak | Engineering |
| **D. Windows requalification** | NSIS build from HEAD on the laptop, `verify_windows_app.ps1`, `run_mac_windows.sh` | Not started this round | The laptop is offline | Engineering (laptop availability: owner) |
| **E. Mac-to-Mac run** | `run_mac_mac.sh` with both Macs awake, fixed builds | Not started this round | Kyle's Mac is offline | Engineering |

## Hardware availability (2026-10-24)

- This Mac: available; the owner's app runs on 8192 (quit and relaunched only by the packaged check).
- Sentinel: available; on Wi-Fi only (`eno2` has no cable). It dropped off the LAN between about
  22:23 and 02:19 local on 2026-10-23/24 without rebooting. For P1, a wired link would remove that risk
  (owner action).
- Windows laptop and Kyle's Mac: offline.

## Owner actions that unblock something

1. A written go-ahead to publish milestone A to named testers (after A passes).
2. Optional: an Ethernet cable for sentinel before P1.
3. When convenient: the laptop and Kyle's Mac awake and on the network (milestones D and E).
