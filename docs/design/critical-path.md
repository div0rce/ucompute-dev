# UCompute critical path and dependency map (engineering-owned)

Updated at each checkpoint. One current priority per workstream. Status words follow the ledger:
nothing here is complete unless its evidence exists. The lab's WP-04 view is ucompute-core-lab
`WP04-DEPENDENCIES.md`; the website's plan is in its own repository.

## Current priority per workstream

| Workstream | Current priority | Next action |
|---|---|---|
| Engineering | Finish P1: memory, capture, failure cases, configuration C | The extended driver, then a run with this Mac on AC; records in `pool-two-machine-2026-11-06.md` |
| Core Lab | Help WP-04: drop-ins, pre-registered numbers, scoring | Review the C2 and C5 diffs read-only; score P1 when its evidence exists |
| Website | Candidate 2 published and verified (ce71dc1, 4441b78); D-032 recorded in its `docs/DEPLOYMENT.md` | Idle until the next promotion record, the WP-04 verdict or the capability snapshot; coordinate with Growth as D-032 says |
| Growth (D-032) | Its own repository and environment; consumes the handoff below | No engineering dependency and no request pending |

## Cross-workstream handoff (D-032)

Existing files only. What each workstream may take from the others, and what counts as a public claim.

- **Verified capability updates** (Engineering, read by everyone): `docs/VERIFICATION.md` (dated entries with commands and results),
  `docs/PROGRESS.md` (newest first), `docs/REQUIREMENTS.csv` rows at `VERIFIED` with their evidence paths, and `docs/evidence/`.
  Only a `VERIFIED` row is a publicly claimable capability; `IMPLEMENTED_UNVERIFIED`, SINGLE-HOST and synthetic results are not.
  The Core Lab's equivalents are its own repository's results and `WP04-DEPENDENCIES.md`.
- **Approved public claims** (Website-owned): the website repository's `reference/claims.json` and its claims workflow
  (`docs/DEPLOYMENT.md` there, "Claims and the capability snapshot"). Growth takes public wording from it, not from engineering's
  working records; site text is never evidence (spec §29.5).
- **Release changes**: `docs/DECISIONS.md` (the D-025, D-030, D-031 pattern: artifact, hash, channel, cohort) and the WP-08 promotion
  records in `docs/VERIFICATION.md`; the website's `reference/releases.json` is the published state. The private-alpha cohort is the
  owner's Access policy and nothing else; no workstream expands it.
- **Growth requests** go to Website (positioning, content, SEO, onboarding, analytics, distribution). One needing an engineering or
  lab fact is answered from the records above; anything beyond them is an owner decision. Analytics on ucompute.org stay under spec
  §29.5 and Website's deployment authority; Growth's own analytics live in Growth's own channels.
- **Mechanism for now**: cross-session messages between the Claude Code sessions plus these files. A durable read-only feed (GitHub
  issues or a status file) is a later step, taken only if routine coordination needs it.

## Milestones and what each waits on

| Milestone | Proof required | Status | Blocked on | Owner |
|---|---|---|---|---|
| **A. macOS private-alpha artifact** | `verify_packaged_app.py` ALL PASS on a DMG built from a commit; promotion record; owner approval | **Met for candidate 2 (0.1.1): published 2026-10-30** on the website's stage-snapshot link behind Access (website ce71dc1, 4441b78; `verify:release` pass there and from this Mac: 6,425,890 B, SHA-256 `c1166ba9…b937`), after D-031. Candidate 1 withdrawn (D-030) | None for this candidate; the next build needs its own record and approval | Website (page state), Owner (Access policy) |
| A1. Publish A to named testers | The owner's written go-ahead in `docs/DECISIONS.md`; the website's preview page | **Done, 2026-10-29:** published to its cohort (the owner alone, stage-snapshot only, website a9c4166, hosted hash verified) and received: the owner reported "Yes, it works" after the download, checksum and launch steps | — The spec's private-alpha cohort (at least five consented testers) stays open with no date: asked for more testers on 2026-10-29, the owner answered "what i have is all i got for now"; a wider cohort needs its own D-025 amendment | Owner, website |
| **B. WP-04 P1: one model split across this Mac (Metal, H) and sentinel (CPU, R) over UCompute's authenticated transport** | Plan section 3 evidence: placement proved at `-lv 4`, valid tokens, no plaintext on the wire, typed failures; scored by the lab against `ae88d0b`/`1722119` | **First generating run passed 2026-11-06 (23 steps, 0 FAIL, 1038a7f, `pool-two-machine-2026-11-06.md`):** placement, bytes, turns, default deny, loopback rpc-server, local stays local, audit join and canary scan, under D-033 (timing not scored) | P1.4 configuration C, P1.5 memory, P1.7 capture, P1.8 failure cases, a quiet timing run | Engineering; the Core Lab scores |
| **C. Linux headless install** | `verify_linux_install.py` ALL PASS from a tarball built at a commit with the round-6 runtime; 24 h soak PASS | **Met at d3b178b (2026-11-01):** the renamed tarball `ucompute-0.1.1-linux-x86_64.tar.gz` (SHA-256 `3b7cfa37…a4f6`) ALL PASS 16/16 on sentinel (`linux-install-d3b178b/`); the third 24 h soak at the same commit PASS with documented interference (`linux-soak-2026-10-31.md`) | None for the milestone; distributing the tarball is the owner's decision (spec §4) | Engineering (done); Owner (distribution) |
| **D. Windows requalification** | NSIS build from HEAD on the laptop, `verify_windows_app.ps1`, `run_mac_windows.sh` | Installer built natively from 77262a5 and `verify_windows_app.ps1` ALL PASS 13/13 (2026-10-29); the laptop is reachable and set never to sleep on the charger | The Mac–Windows two-machine run with renamed builds (after D-027), UIA/NVDA, clean-account install | Engineering |
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
- Windows laptop: not available for now (owner, 2026-10-29: only sentinel and this MacBook). Its D-029/D-030 migration stopped part-way when it went off the network: the inventory (`C:\Users\jessg\ucompute-migration-2026-10-29\inventory.json`) is written; its app data and app logs are moved into `former-data\`; the old app's uninstaller ran. Not confirmed: the test tasks' re-creation under the new name, the move of the remaining folders (old CLI, test scripts, build tree, verifier folders) and the verification. When it is back, first install the keep-awake task (`scripts\windows-keep-awake.ps1 -Install`, the owner rule of 2026-10-29; not yet run on Windows), then read `C:\Users\jessg\laptop-migrate.log`, rerun the script (`laptop-migrate.ps1`, idempotent for areas already moved) and record the result; its three firewall rules under the former name are the owner's to remove. Milestone D's two-machine run waits for the laptop.
- The second soak runs from the pre-rename tree and home on sentinel: collect it with the runner as it was at 37cd454 (`git show 37cd454:tests/soak/run_soak_linux.sh`, saved outside the repository), not with today's, which would check and remove the renamed home instead (Core Lab, 2026-10-29).
- Sentinel holds a sleep and idle block inhibitor (`ucompute-keep-awake` user service, lingering on, 2026-10-29), which also stops the login screen's default 20-minute idle suspend after the desktop session ends. After a reboot with nobody logged in locally, polkit grants only the idle part until the owner logs in (the service retries each minute); masking sleep system-wide needs the owner (`sudo systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target`).
- Kyle's Mac: offline.
- Sentinel's local desktop: the owner's GNOME session (seat0, Wayland, Firefox open) has been logged in since before
  the second soak; `who` does not list it. P1's preflight requires no other session (OD-10). The owner asked
  engineering on 2026-10-29 to end it itself: right before P1, after the soak has ended, find div0rce's seat0 session
  (`loginctl list-sessions`) and run `loginctl terminate-session <id>` (Firefox restores its tabs on next launch),
  then run the preflight. Not before the soak ends: a change on sentinel during the soak is another interference window.

## Owner actions that unblock something

1. If sentinel's link still drops or jitters with power saving off: an Ethernet cable on its `eno2`.
2. Sentinel, keep-awake headless (its desktop session is gone since the 2026-10-30 kernel crash, `docs/evidence/migration-sentinel-2026-10-30.md`): `sudo systemctl mask sleep.target suspend.target hibernate.target hybrid-sleep.target`, or a polkit rule allowing `div0rce` `org.freedesktop.login1.inhibit-block-sleep` and `-idle`; until then Ubuntu's never-suspend-on-AC default is the only guard. Optionally report the kernel bug (`Bad rss-counter state`, 7.0.0-38-generic) from `/var/crash/linux-image-7.0.0-38-generic-202610301305.crash`.
3. When convenient: the laptop and Kyle's Mac awake and on the network (milestones D and E).
4. Accept or amend ADR-0006 (zero-trust execution, security gates SG-1 to SG-3; Proposed). Today's work stays inside
   SG-1 either way.
