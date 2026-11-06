# UCompute Platform Engineering: charter, backlog, next milestone

Adopted 2026-11-06 (D-034). This workstream builds, integrates, secures, validates, distributes and maintains the platform.
`docs/design/critical-path.md` holds the current priorities and the handoff; `docs/REQUIREMENTS.csv` stays the ledger; this
file holds what they do not: who owns what, and the order of work. Status words follow the ledger, and nothing here
is done without its evidence.

**Mission.** Reliable infrastructure that independent developers can install, integrate and trust: general-purpose
computing, with AI inference as the first workload. Working inference is not rewritten to get there.

## Ownership

| Area | Lives in | Open items that matter now |
|---|---|---|
| Portable runtime, hardware discovery, backends | `crates/ucompute-runtime`, `-hardware`, `-models` | Second backend (F-01-06); estimate +13 % for `smollm2-360m` weights on Linux CPU |
| macOS, Windows and Linux native behaviour | `apps/desktop`, `packaging/` | Windows requalification on renamed builds; F-10-01 clean-account first token |
| Identity, authenticated Mesh, pairing, discovery | `crates/ucompute-identity`, `-network`, `-policy` | D-029 acceptance on renamed builds; Windows key ACLs (B-017) |
| Local, remote and distributed computation | `crates/ucompute-network`, `-runtime`, `-api` | WP-04 P1's remaining steps; F-03-04 |
| Scheduling, placement, resources | `crates/ucompute-models`, future scheduler | WP-05 waits on WP-04's measurements |
| Security, isolation, resource limits | `crates/ucompute-policy`, `docs/security/` | ADR-0006 awaits the owner; SG-2 before any untrusted workload |
| Protocol, API, CLI, SDKs, compatibility | `proto/`, `crates/ucompute-api`, `-cli`, `sdks/` | F-09-05; U-01 independent peer |
| Packaging, installers, CI, updates, release engineering | `packaging/`, `.github/workflows/`, `apps/desktop/scripts/` | Signing (B-008); update path (F-10-07); CI's first Linux real-engine job |
| Production integration of Core Lab work | wherever it lands | Scored P1; planner proposals after WP-04 |
| Requirements, evidence, release readiness | `docs/` | This file, the ledger, `VERIFICATION.md` |

**Interfaces.** Core Lab supplies independently tested algorithms, measurements and reviews; Engineering owns production
adoption. Website & DevEx consumes verified interfaces, packages and release documentation; Engineering owns the runtime and
SDK implementations. Growth consumes approved claims and evidence and does not decide whether a capability ships.
Program Control tracks dependencies and blocked handoffs and does not direct code; its repository is empty today, so
this file and `critical-path.md` are what it reads.

**Operating rules.** Owner decisions and security gates stand. Reviewable branches and PRs, no rewritten history. Private
pushes, public source publication, installer distribution and release approval are separate authorizations. No claim without
reproducible evidence. No work to look busy. Close integrated capabilities before starting review rounds. Choose the next
task by product impact, security risk, critical-path dependency and verified user value.

## State of the inherited work (2026-11-06)

| Checkpoint | Actual state |
|---|---|
| D-028/D-029 UCompute-only migration | Mac and sentinel migrated. Pairing on renamed builds exercised by P1. Open: the D-029 acceptance run (discovery, Mesh, revocation) on renamed builds, an outdated build failing cleanly, reboot and failed-install recovery; the laptop's migration stopped part-way (offline) |
| WP-04 fix pass and integration | Fix pass committed and scored by the lab; integrated in `wp04/fix-pass`, PRs #1 and #2 open with CI green, merging to `main` now |
| S1 and P1 | S1-M and S1-L done. P1's first generating run passed 2026-11-06 (23 steps, 0 FAIL, D-033). Open: P1.4, P1.5, P1.7, P1.8, quiet timing |
| Linux reliability | Milestone C met at d3b178b: 24 h soak PASS with documented interference, tarball ALL PASS 16/16 |
| Windows acceptance | Blocked: the laptop is off the network. App ALL PASS 13/13 at 77262a5 under the old name; nothing on renamed builds |
| Private-alpha restrictions | Unchanged. Candidate 2 (0.1.1) published to the owner's Access cohort (D-031). It predates 578999f and D-026, so the next candidate carries them and needs its own record and approval |

## Backlog, dependency order

**Finish first**
1. P1's remaining steps: driver for memory, capture, failure cases, configuration C; run on the Mac and sentinel. Closes F-03-01 and F-03-02, and milestone B except timing.
2. Integrate `wp04/fix-pass` and `rename/ucompute` into `main`.
3. D-029 acceptance on the Mac and sentinel: `tests/distributed/two_machine/run_mac_linux.sh`; an old build from sentinel's backup failing cleanly.
4. A quiet P1 timing run (owner: AC, other apps closed, 5 quiet minutes).

**Next product milestone: external developer alpha** (below)

**Then, unblocked**
- WP-02: default peer fairness (F-06-04) with the flood client off the host under test; B-015.
- Runtime: bound `post_json`; bound the wait on the load lock; connection churn at soak rates on macOS and Windows; the Linux estimate error.
- WP-05 first deterministic local-or-peer scheduler, using WP-04's measurements.
- WP-07 independent peer implementation (U-01); WP-09 private job pools.

**Blocked on hardware or the owner**
- Windows: laptop online; then `docs/environment/windows-test-host.md`, finish `laptop-migrate.ps1`, native NSIS build, `verify_windows_app.ps1`, `run_mac_windows.sh`.
- Mac-to-Mac: the second Mac online; `run_mac_mac.sh`.
- Signed updates (F-10-07), signed installers: signing identities (B-008).
- Sentinel keep-awake when headless: the owner's sleep-target mask or polkit rule.
- Mobile, confidential computing, real-money rows: B-003, B-005, B-006.

## Next milestone: external developer alpha

Spec section 31. **Goal:** a developer who is not on this project installs a released build, integrates it through a
versioned SDK and gets a real streamed result, with cancellation, a typed refusal and default-local execution, unaided.

**Exit criteria:** live Python and TypeScript suites on each advertised platform; typed errors and cancellation; a
tested security contact; no critical or high issue affecting the offered scope; the U-04 study; the owner approves
external distribution.

| # | Task | Rows | Depends on | Who |
|---|---|---|---|---|
| 1 | **Python and TypeScript SDKs, live real-engine suite in CI** | F-09-05 | F-09-04 | Engineering (in progress) |
| 2 | An install path that does not need source: name the `ucompute` command shipped in the DMG, measure the Gatekeeper behaviour of that binary, ship the Linux tarball through a promotion record | F-10-01, WP-03 | 1 for the guide's examples | Engineering, Website & DevEx |
| 3 | `ucompute models recommend` and an API field | F-01-07 | — | Engineering |
| 4 | Tool-call round trip on a pinned tool-capable fixture | F-01-12, B-013 | fixture licence check | Engineering |
| 5 | Machine-readable compatibility matrix and its validator | U-03 | evidence from 1 and 2 | Engineering |
| 6 | Candidate 3 with the runtime fixes: Gates A and B, promotion record | WP-08 | 2 | Engineering, then the owner |
| 7 | Security contact tested end to end | spec §31 | — | Owner, then Engineering |
| 8 | U-04 study: three independent developers | U-04 | 1, 2, 6, 7 | Owner recruits; Website & DevEx supports |

Advertised platforms for this cohort are macOS on Apple Silicon and Linux x86_64 (CPU). Windows joins when the laptop
returns. Pooling is not offered in this cohort.
