# Progress Log

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

Newest first. Every entry: date, agent, what changed, counts, evidence, next commands.

## 2026-10-29/30 — Claude Code — D-026 built; candidate 2 ready; Mac migrated; WP-04 fix pass and S1-M (unit, SINGLE-HOST)

**Implemented / verified:**
- **Milestone C met at d3b178b:** the renamed Linux tarball (0.1.1, SHA-256 `3b7cfa37…a4f6`) ALL PASS 16/16 on sentinel (`docs/evidence/linux-install-d3b178b/`), and the soak gate below.
- Milestone C soak gate met: the third 24 h Linux soak at d3b178b PASS with documented interference (0 untyped with a 960 s client timeout; 0 `backend_failed` in 4.69 M admitted burst requests; 2,607 engine kills recovered within 1 s; `docs/evidence/linux-soak-2026-10-31.md`). The install gate (renamed tarball) is next.
- Stable runtime (milestone C): the soak's frequent `backend_failed` traced to the engine client reusing connections llama-server had closed; fixed (no idle reuse) and reproduced before/after twice on sentinel, 0.53–0.56 % → 0; the first-token clock now runs from submit (`docs/evidence/burst-failures-linux-2026-10-31.md`). Sentinel: kernel crash during the desktop-session teardown, a short rustc fault burst after it, Linux suite 616 passed / 3 known failures, S1-L closed. P1: two starts, no measurement (a runner bug fixed; then this Mac's battery, memory, load and a near-full disk); a watcher starts it once this Mac is on AC.
- Milestone A1 met for candidate 2: UCompute 0.1.1 (`c1166ba9…b937`) approved by the owner (D-031) and published by the website on the stage-snapshot link behind Access (ce71dc1, 4441b78); engineering re-verified the published file's size and hash through Access from this Mac. Second 24 h Linux soak collected: FAIL as registered (2 untyped no-response in 5.3 M requests, engine alive; `docs/evidence/linux-soak-2026-10-29.md`); milestone C open; a SINGLE-HOST investigation runs on the Mac. D-032: fourth workstream (Growth) and the cross-workstream handoff.
- D-026 local-work priority on CPU engines: `local_priority` setting (default `refuse_new`, opt-in `preempt`, `none`), API, app Settings › Sharing selection; unit and SINGLE-HOST tests on this Mac with real engines (`docs/VERIFICATION.md` 2026-10-30 D-026). F-06-04 stays IMPLEMENTED_UNVERIFIED: the flood bound with D-026 is to be measured on sentinel.
- WP-04 fix pass (4ddec61) and S1-M with the `--cache-ram 0` change (f7fe48e); candidate 2 (0.1.1, SHA-256 `c1166ba9…b937`) built and verified ALL PASS 19/19, promotion record awaiting the owner's approval; this Mac migrated to it (`docs/evidence/migration-mac-2026-10-29.md`); D-030 (no trace of the former name); CI on both PRs green.

**Ledger:** NOT_STARTED 103 · IN_PROGRESS 6 · IMPLEMENTED_UNVERIFIED 8 · VERIFIED 36 · BLOCKED 0 · INFEASIBLE 0 (153 rows).

**Limitations:** D-026 not run on sentinel or two machines; split requests and embeddings are not counted as local work; the restart test is in process only. The second Linux soak is still running (ends about 07:43 UTC 2026-10-30).

**Next:** collect the soak (moving its home into the backup, not deleting it); migrate sentinel (`~/ucompute-migration-2026-10-29`, backup then verified model carry-over); sync and build HEAD there; S1-L and the Linux suite, D-026's flood tests included; R's half of the preflight; end the desktop session; P1 with `run_mac_linux.sh SPLIT=1`.

## 2026-10-28/29 — Claude Code — candidate 1 published; Windows app verified; soaks; WP-04 round 8; GitHub sync; total rename to UCompute (unit, SINGLE-HOST, one-machine packaged checks)

**Implemented / verified:**
- Milestone A1: candidate 1 (`UCompute_0.1.0_aarch64.dmg`, SHA-256 a3239588…dd6d) approved (D-025, cohort the owner alone), published as a stage snapshot and received ("Yes, it works"); Access download check passed.
- Milestone D (part): Windows NSIS app built natively from 77262a5 on the laptop, `verify_windows_app.ps1` ALL PASS 13/13 (b341a74).
- Milestone C: the 24 h soak of 2026-10-24/25 FAIL on one untyped no-response in about 556k burst requests (cause not determined); a 3 h overload-only rerun PASS (0 in 716k); the second 24 h soak runs until about 07:43 UTC 2026-10-30, with documented interference from the rename runs and its reading rule fixed before the verdict (`docs/evidence/linux-soak-2026-10-29.md`).
- WP-04 round 8 (C3–C6, C8) committed with 14 review findings open (`docs/design/wp04-round8-open-findings.md`); the lab's P1 prompt check passed (65 tokens, 12/12).
- Decisions: D-026 (Mode B on CPU engines, user choice, default option B), D-027/D-028 (total rename, no compatibility; GitHub: private `div0rce/ucompute-dev` as origin, PRs with CI, the public `div0rce/ucompute` untouched), D-029 (no automatic data deletion, per-machine acceptance test, frozen protocol identifiers, capability retention); ADR-0006 zero-trust execution (Proposed); ADR-0003 §9.16.
- The rename itself (this commit): see `docs/VERIFICATION.md` 2026-10-29 D-028.

**Ledger:** NOT_STARTED 103 · IN_PROGRESS 6 · IMPLEMENTED_UNVERIFIED 8 · VERIFIED 36 · BLOCKED 0 · INFEASIBLE 0 (153 rows).

**Limitations:** no device runs a renamed build yet; renamed packages (DMG, NSIS, tarball) not built or verified; Linux suite after the rename not clean (environmental, see VERIFICATION); the Mac flood timing test needs an idle machine.

**Next:** push `rename/ucompute` to the private origin and open the PR; WP-04 fix pass (workflow `wp04-fix-pass`, no sentinel until the soak ends); collect the soak with the 37cd454 copy of `run_soak_linux.sh`; after it, the Linux suites, device migration per `docs/release/migration-to-ucompute.md`, S1, then P1 (end sentinel's desktop session first, owner-approved).

## 2026-10-23/24 — Claude Code — Linux host onboarded; fixed builds on two machines; WP-04 designed; fix round 6; Linux install path (TWO-MACHINE daemon API, SINGLE-HOST, unit)

**Implemented / verified:**
- Sentinel (Ubuntu 26.04 x86_64, i9-9900K) is the fourth machine with evidence: llama.cpp b11609 Ubuntu CPU build pinned for linux-x64 (D-024, ce54da1); golden local inference; the Mac–Linux runner (6de3487) and the first fixed-build two-machine run (177f7ed: discovery and hold ALL PASS, mesh all PASS but one nearby-list miss; B → A ran on the Linux CPU); the first Linux test run (e3c2ece: 446 passed, 5 failed).
- Core Lab calibration executed on the Mac + sentinel and handed to the lab (8f34f1b; 24 of 26 points valid; analysis is the lab's).
- WP-04 split inference: ADR-0005 Proposed (6df25ee, with lab facts in c2cd6dc, ce86fbc, 3eea3f7), platform readiness plan M1–M10 and WP-04 implementation plan C0–C8 (c811a90, with lab notes fa19afc..457ad98).
- Capability snapshot draft 3 (f052074) with the record fixes the fact-checkers found (run-1845 figures, missing VERIFICATION entries, B-004).
- M5/M6: `tests/soak/local_soak.py` (13355e6; its smoke run on the pre-round-6 build found that an engine SIGKILL was fatal until the daemon restarted); the Linux user-level tarball, systemd user unit and `verify_linux_install.py`, ALL PASS 16 checks on sentinel (13355e6, 9d1ffd3, 1e9ae23); `scripts/gen_notices.py` writes THIRD_PARTY_NOTICES and refuses a licence outside `deny.toml` (9d1ffd3; it found webpki-roots' CDLA-Permissive-2.0, now allowed).
- Fix round 6 + 6b (abe19c1, 77fa300, 418f5de, fac5556; unit, FAKE, SINGLE-HOST): engine crash recovery with a restart bound and `last_exit`; thread default from performance cores (the round-6 regression made parallel engines 9× slower); swaps planned on a fresh memory sample; disk preflight; the two Mac–Linux discovery findings (per-family mDNS copies; re-query on a missed announcement), IPv4-only binds, `unreachable` peers; the reaper matches engine paths with spaces (it never worked for the packaged macOS app); desktop daemon restart; fixtures provisioned by catalog tag. Final check 480 passed, 1 load-sensitive flood failure that passed alone; four verifiers, no unresolved item; open items carried to round 7 (`docs/VERIFICATION.md`).

**Blocked / open:** the Windows laptop and Kyle's Mac are offline; the reaper misses orphans under a subreaper (Linux) and three failed GPU starts block CPU loads for 60 s — round 7; packaged-app stage 6 not re-run since the space-in-path fix; the status-during-load and CPU flood-fairness determinations; WP-04 C1–C8 not started in code.

**Ledger:** NOT_STARTED 103 · IN_PROGRESS 6 (F-08-04, F-06-09, F-09-10 moved from NOT_STARTED) · BLOCKED 0 · IMPLEMENTED_UNVERIFIED 8 · VERIFIED 36 · INFEASIBLE 0.

**Next commands:**
```sh
# round 7 (reaper, Linux pins, help text, desktop notices, Linux suite run, packaged re-check)
# WP-04 C1: reserve the split flags (crates/ucompute-runtime/src/llamacpp/process.rs)
scripts/mac-host.sh sync   # with MAC_HOST=192.168.1.167 MAC_USER=div0rce MAC_KEY=~/.ssh/id_ed25519, then the 24 h soak
```

## 2026-10-23 — Claude Code — fix round 5: the LOW items left by rounds 2 and 3; flood tests determined; macOS app from ffe1a47 (unit, synthetic, SINGLE-HOST)

- Mesh (780c2b9; verifier 10/10 resolved): a peer-sourced revocation releases the hold on the running Node under the `net` lock; Disconnect checks and holds atomically; a dial cancelled mid held exchange closes its connection at once; the held exchange asks the store once more at its end and at its 10 s bound; the held minor-0 redial is audited once; the transport close codes (0, RE, UA, PV, FT, HD) pinned on the wire (the pairing codes PF and PL are not); the `control.proto` minor-0 comment corrected. Behaviour: a protocol mismatch claimed before the device proves its key reads `peer_unauthorized` with an update hint (426 is now always proven), and so does a frame-size close (`FT`) without the hint; `health::supervise` retries unproven refusals with backoff up to 30 s (only tests call it; the daemon's sync loop keeps its own backoff up to 300 s). 11 mutants killed.
- Desktop (ffe1a47; verifier 3 resolved, 3 deferred): Allow keeps focus after a cancel or failure and is described by its outcome; Settings › Privacy's Cancel is `aria-disabled` while the service is down; the shell counts the daemon as its own only after its own child prints `listening on http://127.0.0.1:8192`, waits up to 20 s while it starts, and on Windows asks only that daemon to stop. 445 desktop tests, 13 shell tests, 12 mutants killed; SINGLE-HOST CLI contract check.
- Drivers (ffe1a47; verifier 5 resolved, 5 deferred): the proven-refusal hold step needs real proof; the discovery driver leaves a restore through a dead tunnel to the runner; the revocation steps need A's signed revocation of B and a refusal from B's exact endpoint. 58 offline tests (synthetic), 9 mutants killed; SINGLE-HOST hold and mesh runs ALL PASS (2 N/A each), with a binary of unknown source, so not evidence for the committed crates.
- Final check (after the fixes, before the verifiers and the commits): fmt and clippy (macOS and windows-gnu) clean; full `cargo test --no-fail-fast` 452 passed, 1 failed, 2 ignored. The failure (`privacy_mode_enforced_end_to_end_single_host`) ran across a Low Power Sleep and hibernate on 1% battery and passed alone. `cargo test -p ucompute-desktop` 13/13 (not in the plain `cargo test`).
- F-06-04 flood tests determined environmental (SINGLE-HOST, this Mac on battery at 61%, both variants alike): `peer_flood_throttled_through_daemon_single_host`, the identical test at 95592b0 (before any crates change of the fix rounds) and at ffe1a47, alternated ABBA over 8 rounds at load average about 4-10: pre-fix median ×1.212, 4 of 8 over the 1.2 bound; fixed median ×1.252, 6 of 8 over; paired differences 4 up, 4 down, Wilcoxon W− = 10 against a critical value of 3: no significant difference. The 1.2 bound sits inside this 8 GB M2's run-to-run spread under load; the failures do not need the fix rounds (the pre-fix tree fails 4 of 8); point estimate +0.08, and 8 pairs cannot exclude a rise of up to about +0.22 (about 18%). `peer_flood_throttled_local_p95_within_20pct_single_host`: 1.39/1.35/1.35/1.24 with round 5 against 1.46/1.24/1.18 on the pre-round-5 tree (7d2cc98; crates unchanged since 3df9438), the same load-sensitive bound, not a regression. Raw data in `docs/VERIFICATION.md` (fix round 5).
- macOS app from ffe1a47 (SINGLE-HOST, one Mac): `pnpm tauri build` with a clean tree, DMG 6,182,535 bytes, sha256 `695c44d1…13064`; `verify_packaged_app.py` ALL PASS, 15 checks (fresh install healthy in 3.98 s with the pinned engine b11609 downloaded, golden completion on `MTL0 (Apple M2)` at 348 and 294 tok/s, quit, restart, crash in 1.03 s, nothing left running); installed in /Applications (the previous app kept as `UCompute.app.prev`), the owner's identity (8983-9a1e-e341) and pairings unchanged. Evidence: `docs/evidence/desktop-packaged-mac-m2-ffe1a47.json`.
- Deferred: needing the laptop, the PowerShell firewall read, NVDA checks, cross-hold on two machines and `UComputeTestWifi`; needing the owner's port 8192, the packaged-app race check; needing a crates change, D1; needing any two machines, the physical `state.py` name restore; not run, `tests/distributed/test_runner_flow.sh` (it can kill a real tunnel). LOW residuals: `apply_peer_revocation` still disconnects only on the connection's Node; no CLI test pins the listening line; no driver test pins `revoked_by == A` or the listen-port substitution; the pairing close codes PF and PL are unpinned. The openapi `connect` description now says 426 is proven only (text, uncommitted).
- Documents: VERIFICATION fix round 5 entry and the round 2 and 3 open lists marked closed or deferred; ADR-0003 §9.3, §9.6, §9.8, §9.9, §9.12, §9.15; THREAT_MODEL M3-03, M3-05, M3-17 and the firewall row; `docs/api/README.md`; the UI 2.0 evidence; `docs/environment/windows-test-host.md`; `apps/desktop/README.md`; `crates/ucompute-api/openapi.json` (the `connect` description); AGENTS. Counts: no ledger status changed (`validate_registry.py`: VERIFIED 36, IMPLEMENTED_UNVERIFIED 8, IN_PROGRESS 3, BLOCKED 0, INFEASIBLE_WITH_EVIDENCE 0, NOT_STARTED 106; 153 rows). Nothing from rounds 2-5, and none of the round 1 product fixes, has run on two machines (95592b0's driver fixes ran in the pre-fix physical baseline, `docs/evidence/mesh-physical-2026-10-22-baseline.md`); the only packaged build with them is the ffe1a47 macOS app on this Mac.

Next:
    the Windows laptop (offline now): build the NSIS installer from HEAD natively on it and run apps/desktop/scripts/verify_windows_app.ps1, then WIN_HOST=<laptop> OUT=<dir> [NO_MDNS=1] [CROSS_HOLD=1] tests/distributed/two_machine/run_mac_windows.sh
    on the laptop: the PowerShell firewall read, scripts\windows-uia-dump.ps1 and scripts\windows-nvda-run.ps1 (D-022)
    the owner's always-on device (192.168.1.167): not yet reachable from this Mac
    caffeinate -i env UCOMPUTE_MDNS=0 UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test --no-fail-fast; cargo test -p ucompute-desktop   # on mains power, no sleep
    repeat the SINGLE-HOST shakedown with a binary built from HEAD (discovery, hold with --c/--d, mesh)
    rebuild the app on the second Mac from HEAD before run_mac_mac.sh, with both Macs awake

## 2026-10-22 (night) — Claude Code — fix rounds 2 and 3: desktop, two-machine drivers, mesh security (unit, synthetic, SINGLE-HOST)

- Round 2 (18 desktop and 44 driver findings, all fixed with tests; checkers: desktop 18/18 resolved, drivers 42 resolved and 2 partly): the firewall read throws non-not-found PowerShell errors, an exited own daemon counts as adopted, failed reads retried within 5 s, "Loading…" before a busy service's first read, privacy not known on a failed read, status bar named groups and other accessibility fixes, "not refreshed" live values, Chat copy; drivers that could pass without proving their claim fixed, N/A for unmet preconditions, exit codes 3/1, `state.py` save and restore in every driver, the runner's restore on exit, the cross-hold step and `CROSS_HOLD=1`, no 19222 forward. Results: desktop `pnpm test` 33 files 443 passed, desktop crate 10 passed, clippy (macOS and windows-gnu) clean, 20 mutants killed; driver tests 55 OK (synthetic), runner flow PASS, 58 mutants killed; SINGLE-HOST shakedown run 2 ALL PASS (discovery 5 N/A, hold with cross-hold 1 N/A, mesh 2 N/A).
- Round 3 (7 security findings, 1 high, all fixed with tests; checkers 7/7 resolved): a refusal is the device's own only after its Hello was read (a forged Initial close can no longer end a dial or read as revoked or held); revocation fan-out state only for stored records; the held exchange follows the store and closes on revoke or unpair; a peer's revocation ends a hold; holds under the `net` lock. Results: fmt and clippy (macOS and windows-gnu) clean; ucompute-network lib 57, peer_transport 25 (+1 ignored), invariants 2, pairing 8, remote_single_host 11; ucompute-api lib 53, mesh 33, api 16, jobs 2, ui_fixtures 5; ucompute-cli mesh_single_host 3, cli 13 (3 tests updated to the new rule after a first run failed on it).
- Documents: VERIFICATION round entries with the exact commands; ADR-0003 §9.3, §9.9, §9.12 and §9.15; THREAT_MODEL M3-03, M3-17 and the firewall row; `docs/api/README.md` (proven and unproven refusals, `peer_held` on connect); the UI 2.0 evidence (round 2 and 3 changes, open items); `docs/environment/windows-test-host.md` (runner flags, tunnels, `state.py`, the hold driver and cross-hold, step-name changes, the UIA dump and the NVDA run with its keystroke caveat); both READMEs and AGENTS. D-021 naming: UCompute is the product's name, replacing the former name; code identifiers keep the former name for now (AGENTS, README, spec §26.1, ADR-0004, the UI 2.0 plan, the desktop README).
- Counts: no ledger status changed (`validate_registry.py`: VERIFIED 36, IMPLEMENTED_UNVERIFIED 8, IN_PROGRESS 3, BLOCKED 0, INFEASIBLE_WITH_EVIDENCE 0, NOT_STARTED 106; 153 rows). No fix of rounds 1-3 has run on two machines or is in a packaged build.
- Open: round 2 (PowerShell read not run on Windows; port-8192 startup race; focus after a cancelled or failed Allow; NVDA or UIA checks of the status bar groups and the Allow outcome; the hold driver's proven-refusal step can pass vacuously; cross-hold only SINGLE-HOST; `UComputeTestWifi` not in the repo and a restore FAIL likely after a DHCP change; D1 freshness N/A; `windows-uia-dump.ps1` and `windows-nvda-run.ps1` never run); round 3 (all LOW: off-lock hold release during a Sharing restart, a strong held-connection handle after a cancelled dial, untested held minor-0 audit and `CLOSE_HELD` wire value, the `control.proto` minor-0 comment, `health::supervise` final on `peer_unauthorized`). Details: `docs/VERIFICATION.md`. Against the laptop's 593d4ea app (minor 0) the hold driver's `peer_held` steps cannot pass: rebuild the installer first.

Next:
    commit rounds 1-3; caffeinate -i env UCOMPUTE_MDNS=0 UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test --no-fail-fast; (cd apps/desktop && caffeinate -i pnpm test --maxWorkers=2)
    repeat the SINGLE-HOST shakedown with the round-3 binary (discovery, hold with --c/--d, mesh)
    rebuild the macOS app and the Windows NSIS installer from that commit, then WIN_HOST=<laptop> OUT=<dir> [NO_MDNS=1] [CROSS_HOLD=1] tests/distributed/two_machine/run_mac_windows.sh
    on the laptop: scripts\windows-uia-dump.ps1 and scripts\windows-nvda-run.ps1 from a scheduled task while it is unattended

## 2026-10-22 (evening) — Claude Code — cross-cutting review of af3e37c..245fd14 and its fixes (unit + SINGLE-HOST)

- Reviewed: everything committed since af3e37c; 37 findings confirmed by skeptics (2 high: a Sharing change silently ended every Disconnect hold). Fixed with tests in the working tree after 95592b0 (some in 95592b0 itself): the hold survives Sharing changes; revocations cross a hold (protocol minor 1: the holder answers the held device's Capabilities with `peer_held`, the two exchange revocations, then `CLOSE_HELD`); a refusal only the pinned key's holder can send ends a dial; no IPv6 candidates from an IPv4 endpoint; peer `finish_reason` capped at 32 characters; desktop inspectors, privacy mode and never-read stores honest about staleness; chat sends a paired device only the messages exchanged with it; the Windows firewall rule read through PowerShell, adopted daemons not checked, domain networks named, Allow replacing only this program's rule; drivers labelled as daemon API runs, N/A never PASS, audit checks by marker; sync keep-list from HEAD's tree.
- Documents corrected: the UI 2.0 evidence (what is done, hold compatibility), the eduroam evidence (raw output not saved; exact commands; daemon API run versus manual UI steps), the 2026-10-21 evidence (labels replaced by plain names; the re-pair PASS is not a pass; audit counts recounted from the raw log: the driver's "0 lines in 164 s" was really 1 redial line plus 1 from its own dead-hint check), `docs/api/README.md` and both READMEs (routes, fields and error codes since af3e37c; the UI 2.0 app), VERIFICATION entries for the packaged app, the browser round, eduroam and the firewall step, AGENTS and B-002 (three machines, next steps), the sync docs (exactly what is deleted), commands that now run as written.
- Verified here: offline driver tests 9 passed, sync keep-list PASS (synthetic). No ledger status changed. No fix has run on two machines; no packaged build contains them yet.

Next:
    commit the fixes; UCOMPUTE_MDNS=0 UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test --workspace --no-fail-fast; (cd apps/desktop && pnpm test --maxWorkers=2)
    rebuild the macOS app and the Windows NSIS installer from that commit; run scripts\windows-sync-clean.ps1 and scripts\windows-keep-awake.ps1 on the laptop
    then the daemon API runs and the manual UI steps listed in the afternoon entry below

## 2026-10-22 (afternoon) — Claude Code — UI 2.0 packaged on macOS, browser checks, signal timing, eduroam connect-at-address

- Verified: Desktop UI 2.0 packaged macOS app ALL PASS (15 checks) and installed (`docs/evidence/desktop-ui2-2026-10-22.md`); browser accessibility with colour contrast 0 violations at 1024×700 and 720×480 after 11 fixes, keyboard flows clean (SINGLE-HOST); jobs report `connecting` for requests to paired devices; the whole Windows desktop crate type-checks and passes clippy for x86_64-pc-windows-gnu.
- Determined: the two signal-exit test failures were memory pressure on the 8 GB Mac (engine shutdown under swap hit the 2 s grace), not a defect; tests now assert the designed bound and a stuck-engine test exercises the kill fallback (`docs/VERIFICATION.md`, d9d8067).
- TWO-MACHINE on eduroam (no mDNS): connect at address restored the Mac-laptop connection in 0.17 s from a stale hotspot address, and a remote run returned the golden output on the laptop (`docs/evidence/mesh-physical-2026-10-22-eduroam.md`).
- Incident: the first guarded-sync attempt had a PowerShell escaping bug that deleted every file in the laptop's build copy of the repo (sources and caches only; the installed app, its data and everything else untouched). Fixed with a tracked cleanup script that refuses to delete when paths look wrong (593d4ea); the copy was restored and a full Windows rebuild started.
- Hypothesis to check on Kyle's Mac: its macOS application firewall prompts for each new ad-hoc-signed build, which would explain the failed pairing on its random port (2026-10-21) and, earlier, this Mac's failed dials to it.
- No ledger status changed.

Next:
    finish the Windows NSIS build (scheduled task UComputeTestBuild), verify it with apps/desktop/scripts/verify_windows_app.ps1, install it, then
    WIN_HOST=10.75.120.9 OUT=<dir> NO_MDNS=1 tests/distributed/two_machine/run_mac_windows.sh   # daemon API run, not an app UI run
    OUT=<dir> tests/distributed/two_machine/run_mac_mac.sh when Kyle's Mac is awake            # daemon API run too
    by hand in the apps, each with its own evidence: the Windows firewall consent step, pairing started in each app,
    a wrong code typed by hand, the Mac-side group and run steps

## 2026-10-22 — Claude Code — Desktop UI 2.0 implemented (unit + SINGLE-HOST); capability snapshot draft; publication readiness

- Implemented: Desktop UI 2.0 per docs/design/desktop-ui-2-plan.md (control-center layout, seven sections, status bar, design system) and backend additions E0-E5 (peer details, engine state, jobs registry without prompt text, peer_lost audit, disconnect hold). Evidence: docs/evidence/desktop-ui2-2026-10-22.md.
- Verified on one Mac: desktop 420/420, Rust 431 passed with 2 load-sensitive signal-timing failures (pass alone), fmt/clippy clean; 29 confirmed review findings and 16 Devices gaps fixed with tests.
- Also: docs/capabilities/snapshot-2026-10-22.md (draft for the website), docs/release/public-source-readiness-2026-10-22.md (not ready to publish: owner authorization and personal-data redaction), partial physical Mac-to-Mac evidence (docs/evidence/mesh-physical-2026-10-21.md).
- No ledger status changed.

Next:
    rebuild the packaged apps from this commit (macOS on Kyle's Mac or here after freeing disk; Windows NSIS on the laptop)
    repeat the invalid Mac-to-Mac checks with Kyle's Mac awake; Windows runs when the laptop is on the same network
    plan §5.5 steps 3-7 (real-browser axe with contrast, keyboard-only, VoiceOver/NVDA, packaged apps, physical checks)

## 2026-10-21 — Claude Code — discovery/reconnection fixes, desktop defects, Windows firewall consent step (unit + SINGLE-HOST)

- Implemented (not yet physically verified): the discovery and reconnection fixes, the six desktop defects from docs/evidence/desktop-two-machine-2026-10-19.md, the Windows Firewall consent step, clearer connection states. Details and commands: docs/VERIFICATION.md (2026-10-21 entry).
- Verified on one Mac: fmt and clippy clean (also for the Windows target), 407 Rust tests passed plus the fixed CLI test, desktop 111/111; one load-sensitive latency test (F-06-04 p95) fails on this loaded machine with and without the change.
- New test host: Kyle's MacBook Pro (M2 Pro, 16 GB, macOS 15.7.9) reachable over SSH with Kyle's consent (UCompute project scope); Rust 1.98.0, Node 26.10.0 and pnpm installed in Kyle's home folder. Mac to Mac remote runs worked both ways with the installed apps (Qwen2.5 0.5B).
- Limits: no status changes; F-10-05 and F-10-01 stay IMPLEMENTED_UNVERIFIED until the physical run.

Next:
    rebuild the macOS app (apps/desktop: pnpm tauri build) and the Windows NSIS installer on the laptop; install on both Macs and the laptop
    python3 -I tests/distributed/two_machine/discovery_two_machine.py --a http://127.0.0.1:8192 --b <tunnelled peer API> --out <dir>

## 2026-10-19 — Claude Code — owner's economics clarification integrated (D-019, D-020, U-05; documentation and registry only)

- Integrated the owner's same-day clarification of D-019 (after bfcf4a7). Model: "Use your own compute for free. Share compute to earn credits. Let trusted people use your hardware for free. Spend credits to access compensated compute. Purchase credits when you need more." Added, as §26.5 rules with U-05 checks: the binding self-compute exclusion (no credits for one's own workloads or resources under the same beneficial ownership or control; funding class own; eligibility check before crediting; signed receipts against replay; circular-flow holds; collusion simulation bound; residual risk recorded) and the binding trusted-access allowlist (authenticated, provider-scoped identities; zero debit, reward and fee; precedence over pricing; no silent paid fallback; workload-only access; no Cloud needed). Added the owner's expected-behaviour table to §26.5. Revised issuance and payment integrity (Purchase → Spend → Execute → Verify → Compensate; compensation is a transfer, never duplicated) and replaced "the platform restores every payment loss" with bounded exposure: exposure limits, funded loss reserve Z_t, risk-based holds and provider terms (§26.5, §29.4, threat model, U-05). Credit value: purchased credits are prepaid access, never valueless; F-07-07 reconciled additively (its statement holds literally in stage 0). Removed self-hosted group accounting (§26.5 Cloud rule and check, U-05 clauses and test name, WP-11 and §31 "self-hosted group run"); the deferral is recorded only with the owner's sentence. D-020: exactly the five relayed items, the owner's "blanket approval for everything" read as approval of those items only, explicit non-authorizations, website-session ownership (also stated in §29.5).
- Edited: spec §§25, 26.1, 26.3, 26.5, 29.3, 29.4, 29.5, 30 (WP-09/11/12/13), 31; D-019, D-020; U-05 via scripts/registry_data.py (regenerated); THREAT_MODEL §6.2 (three rows, bounded-exposure clause, residual-risk paragraph); AGENTS hard rule; README, CONTRIBUTING and ADR-0004 amendment lines.
- Review fixes (same day): 27 confirmed findings of the workflow's documentation review applied (receivables instead of negative balances, reservation lifecycle, self-dealing bound with payment failures and history gain, interactive-only allowlist and own-device changes, account-key scoping, no resale of free capacity, no paid fallback past a grant's limits or under a general spending cap, one credit-value statement, stage 0 credits retired or funded before stage 1, D-019 attribution and regulatory wording); listed in VERIFICATION.md CREDITS-D019-CLARIFICATION.
- Counts unchanged: original 148 P0 (VERIFIED 36, IMPLEMENTED_UNVERIFIED 8, IN_PROGRESS 3, NOT_STARTED 101); added 5, all NOT_STARTED; total 153.
- Verified (tooling only): VERIFICATION.md CREDITS-D019-CLARIFICATION.
- Limits: no code; credits, trusted access, payments, Swarm and Cloud do not exist; no marketplace, billing or ledger work started; every economic figure stays a hypothesis. The entry below's open question on federation between self-hosted and shared ledgers is withdrawn (outside committed scope). Open for the owner: provider terms for payment failures beyond the exposure limits (before stage 1); whether trusted access may be granted on a device in private mode (the text allows it: the allowlist is independent of the mode); how an owner registers devices as its own for the self-compute classification (the text now allows it only by interactive owner confirmation on the device); whether stage 0 credits are retired or funded at p before stage 1. crates/, apps/, Cargo.lock, tests/distributed/ and docs/evidence/ were not touched.

Next:
```
python3 scripts/validate_registry.py && python3 -m unittest discover -s tests/registry -q && python3 scripts/gen_registry.py --check
```
Then continue WP-01/WP-02 as below; U-05 is exercised with WP-09/WP-11 in stage 0.

## 2026-10-19 — Claude Code — unified compute credits with contributor consent recorded (D-019 revised; documentation and registry only)

- Recorded the owner's revised 2026-10-19 decision, "Contribute compute. Earn compute. Buy compute when you need more. Share your compute. Not your computer.", as binding principle D-019, with four guardrails added by the engineering session and labelled as such in D-019 and §26.5. It replaces in place the same-day, uncommitted "free compute" draft (community compute barred from paid work); ids D-019 and U-05 are kept and no trace of the two-economy version remains. Edited: spec §25 amendment note and U-05 declaration, §26.1 product purpose/cost and messages (owner slogan and taglines), §26.3 participant-without-hardware row, §26.5 rewritten as "Compute credits and contributor consent" (stages 0–3 with preconditions; eight rules, each with a falsifiable U-05 check), §29.1/29.2/29.3 alignment (network platform fee and compute-credit rows), §29.4 credit economy model (issuance, redemption, contributor share, platform fee, outstanding liability, reserve, grant budget, refunds/chargebacks, fraud and verification cost, capacity coverage; the owner's $10 → $8.50 + $1.50 example worked through as a hypothesis, showing an illustrative margin of about −$1.71 at the verification rate F-07-09's acceptance implies; falsification conditions), §29.5 product-page rule, WP-09/11/12/13, §31 Public contributor network and Commercial managed service gates; ADR-0004 amendment; threat model §6.2 credit-economy abuse row; README, AGENTS hard rule, CONTRIBUTING.
- U-05 retitled "Unified compute credits with contributor consent" (P1, M10, economics; depends on F-06-01/02/05/10, F-07-01/02/05/07/08/09/11, X-05/06, I-03), NOT_STARTED, acceptance rewritten (one balance with source lots; backed, reconciling append-only ledger with collected funding, recorded issuer key and reserve refusal; per-device sharing modes default off, terms shown and confirmed, loosened only on the device, enforced by the scheduler and again by the device, consent recorded per allocation and receipt; metered, owner-authorized spends with price, source and fee shown before spend; no transfer or cash redemption while the F-07-11 gate is closed; Sybil-bounded fair access; free Run, private Mesh and self-hosted operation). Original rows F-07-02/07/08/10/11, F-09-11 and I-03 unchanged; they read as parts of the one economy. `tests/registry/test_validator.py` keeps the 5 added / 153 total assertion.
- Counts: original 148 P0 unchanged (VERIFIED 36, IMPLEMENTED_UNVERIFIED 8, IN_PROGRESS 3, NOT_STARTED 101, BLOCKED 0, INFEASIBLE_WITH_EVIDENCE 0). Added 5 (U-01..U-05): NOT_STARTED 5. Total 153.
- Verified (tooling only): VERIFICATION.md CREDITS-D019.
- Limits: no code; no credits, purchases, payments, Swarm or Cloud exist, all stay disabled/NOT_STARTED; every economic figure is a labelled hypothesis. Open for the owner: how F-07-07's "no monetary value" UI statement is met truthfully once payments back credits (needed before stage 1); whether donated work should ever earn spendable credits from a funded grant budget; any future backed federation between self-hosted and shared ledgers (self-hosted accounting is group-scoped and never spendable on the network); the published lot-consumption order (stage 2 review). The owner-requested independent review (commercial feasibility, contributor consent, credit accounting, security, open-source consistency, fidelity to the owner's text) ran on 2026-10-19; its 36 confirmed findings are applied in the text (VERIFICATION.md CREDITS-D019). It is a documentation review, not verification.

Next:
```
python3 scripts/validate_registry.py && python3 -m unittest discover -s tests/registry -q && python3 scripts/gen_registry.py --check
```
Then continue WP-01/WP-02 as in the entry below; U-05 is exercised with WP-09/WP-11 in stage 0.

## 2026-10-19 — Claude Code — hotspot discovery; desktop to desktop on two machines; Windows pairing firewall finding

- Implemented / verified: LAN discovery on two physical machines (F-02-01 VERIFIED, 88183a9/1a36b3c); native Windows NSIS build of the desktop app, installed and paired with the macOS app through their UIs, prompts run both ways (af3e37c, `docs/evidence/desktop-two-machine-2026-10-19.md`). Ledger text of F-10-01 and F-10-05 updated; both stay IMPLEMENTED_UNVERIFIED (fresh-account install-to-first-token, UI-only wrong-code and Mac-side steps, Windows-initiated pairing).
- Found: pairing that the Windows app initiates is dropped by Windows Firewall on the random pairing port (pktmon `receive inspection`); the Mac's audit log held 567 `peer_rejected` lines for one unreachable paired Mac (one per ~35 s redial); the Windows app shows macOS wording and names the laptop "This Mac"; Chat needs a local model even for remote runs; Windows quit kills the daemon (no mDNS goodbye).
- In progress: discovery/reconnection and desktop fixes (manual connect-at-address, two dial candidates, re-signed record after interface changes, redial backoff with one audit line per state change, graceful shutdown route, platform-neutral wording, remote chat without a local model), then a consent-gated Windows Firewall rule scoped to the bundled daemon, then rebuilt apps re-tested on both machines with `tests/distributed/two_machine/discovery_two_machine.py`.
- Limits: two machines only; no pooling; fixture throughput is a path check.

Next:
    cargo test --workspace --no-fail-fast; (cd apps/desktop && pnpm test)
    python3 -I tests/distributed/two_machine/discovery_two_machine.py --a http://127.0.0.1:8192 --b http://127.0.0.1:18302 --out <dir>

## 2026-10-19 — Claude Code, completing Codex (GPT-6, 2026-10-18) — Operation UCompute: additive infrastructure program

Codex extended the specification on 2026-10-18 and stopped on a usage limit before recording evidence or committing. Its work is committed unchanged in 915562a and completed in the completion commit that follows 915562a.

- Program: SPEC_HANDOFF_V2 §§25–33 appended (§§1–24 byte-identical to ddc39f6); ADR-0004; U-01..U-04 (P1/P2) appended to the single ledger; generator/validator cohort support with a frozen digest of the original rows; registry tests 8 → 19; research refresh (13-project competitive refresh, H6–H9); entry docs, threat model §6, B-016..B-018, D-015..D-017.
- Completed 2026-10-19: D-018 naming: UCompute is the program's public-facing name in use (owner directive 2026-10-19) and ucompute.org (owner-held) its intended canonical site (§26.1, §29.5, WP-13); naming and trademark clearance remain open and are not claimed. Core/Protocol/Scheduler/SDK definitions and Run/Mesh/Swarm/Cloud cross-references (§26.1); every open ledger row named in a §30 package; af3e37c desktop evidence reflected in §27.1, WP-01, next action, README, AGENTS, API docs, threat model, blockers and gates E/G; broken DECISIONS/BLOCKERS tables fixed; validator digest now also freezes original milestones and dependencies.
- Counts (unchanged by this documentation work): original 148 P0: VERIFIED 36, IMPLEMENTED_UNVERIFIED 8, IN_PROGRESS 3, NOT_STARTED 101, BLOCKED 0, INFEASIBLE_WITH_EVIDENCE 0. Added 4 (U-01..U-04): NOT_STARTED 4. Total 152.
- Verified (tooling only): VERIFICATION.md PROGRAM-UCOMPUTE.
- Review: Codex adversarial review (seven perspectives) found 4 material issues, all fixed before it stopped: WP-11 depended on the Gate F it must establish; WP-12 required its own research/rehearsal outputs; a registry test rejected legitimate U-row progress; U-01 acceptance was weaker than §28.2. Its final read of the entry documents did not finish; a 2026-10-19 review of the same files produced the completion edits.
- Limits: no runtime, platform or security re-verification; no application code; no U row progressed. ucompute.org site work happens outside this repository and has no evidence here. Validator limits are listed in VERIFICATION.

Next:
```
python3 scripts/validate_registry.py && python3 -m unittest discover -s tests/registry -q && python3 scripts/gen_registry.py --check
```
Then the integrator commit for the 2026-10-19 desktop evidence and fixes (F-10-05/F-10-01 ledger text, WP-01 remaining checks); WP-02 in parallel.

## 2026-10-18 — Claude Code — Windows daemon; first two-machine M3 run

The `ucompute` daemon now runs on Windows 11 (x64/arm64 pins). Tested on the owner's HP laptop (Core Ultra 7 258V, Arc 140V), driven from the Mac over SSH: setup, golden inference on the Arc GPU via Vulkan, and the full M3 flow against the Mac on eduroam — pairing, remote execution both ways, local_only refusal, revocation, loss detection and reconnection — PASS twice. Discovery fails on eduroam (the network drops mDNS between clients). Three Windows defects fixed on the way (disk free, GPU name parsing, engine outliving a killed daemon). Evidence: `docs/evidence/m3-two-machine-2026-10-18.md`. Ledger statuses unchanged; two-machine evidence added to F-02-02/03/05, F-06-02/05/10; F-02-01 and F-10-05 still open (B-002, B-004 updated).

Next:
```
# on a network that carries mDNS (home Wi-Fi or a phone hotspot), with both daemons running:
python3 -I tests/distributed/two_machine/mesh_two_machine.py --a http://127.0.0.1:18301 --b http://127.0.0.1:18302 --out <dir>
```
Then the Windows desktop build (Tauri: MSVC toolchain needs the owner's license acceptance, or build on the laptop) for F-10-05 and F-10-01 on Windows.

## 2026-10-17 — Claude Code — packaged app rebuilt and re-verified; first two-Mac observations

`target/` had been cleaned; the `.app`/`.dmg` were rebuilt from 08e267e and passed `verify_packaged_app.py` (15/15) with real inference; the executables are byte-identical to the 2026-10-15 build installed in `/Applications`. Workspace 367 passed, desktop 77 passed (docs/VERIFICATION.md).

The installed app's audit log (`~/Library/Application Support/UCompute/logs/audit.jsonl`, 2026-10-15 22:12 to 2026-10-16 02:33 UTC) shows the first real two-machine use, with a Mac at 192.168.1.115 named "Kyle":
- pairing over the LAN succeeded (twice: revoked once, re-paired);
- that Mac connected to this one 10 times (inbound, reconnecting about hourly);
- this Mac's 441 outbound dials to its UDP 8193 all failed with `peer_lost`, and it did not answer ping on 2026-10-17. Likely its macOS firewall blocks incoming connections (stealth mode drops ping) or it was asleep; not confirmed from this side;
- no group contains it, so a remote request would have been refused (`peer_unauthorized`); none was attempted.
Not evidence for any ledger row yet: no remote inference ran across the two machines, and the other Mac's side was not observed.

Next: on the other Mac, check System Settings > Network > Firewall for `UCompute`/`ucompute` and allow incoming connections, create a group containing both devices with "run models" allowed, then run a remote request in both directions and collect both audit logs.

## 2026-10-15 (evening) — Claude Code — packaged desktop app fixed (stale bundled daemon)

A tester's packaged `UCompute.app` never started its daemon: the bundled `ucompute` predated `--exit-with-parent`, which the shell passes. `tauri build` now builds and checks the daemon from the same checkout (`apps/desktop/scripts/build-sidecar.sh`), and the daemon's arguments live in one file (`apps/desktop/src-tauri/daemon-args.txt`). Three more defects found while verifying were fixed: quit orphaned the engine (SIGKILL), a crashed app orphaned it too (SIGPIPE on the daemon's next write), and the bundle was not signed. Evidence: `docs/evidence/desktop-sidecar-fix-2026-10-15.md`; the rebuilt `UCompute_0.1.0_aarch64.dmg` passed all 15 checks of `apps/desktop/scripts/verify_packaged_app.py` with real inference. Workspace 367 passed, desktop 77 passed. Ledger unchanged (F-10-01 stays IMPLEMENTED_UNVERIFIED: fresh-account, UI-only first token not yet run; B-008).

Next: replace the old `/Applications/UCompute.app` with the new build; run the UI-only install → first token on a fresh macOS user account or a second Mac; ship testers the new .dmg with its sha256.

## 2026-10-15 — Claude Code — M3 UCompute Mesh: 13 rows VERIFIED (SINGLE-HOST), 3 open

**Counts:** VERIFIED 35 (+13), IMPLEMENTED_UNVERIFIED 9, IN_PROGRESS 3, NOT_STARTED 101, BLOCKED 0, INFEASIBLE 0. Commits 5034fb6 … 68e73f0.

**Built:** device identity (Ed25519, 0700/0600), peer store with groups and signed revocations, default-deny policy and per-peer quotas, append-only audit with rotation and redacted diagnostics, QUIC + TLS 1.3 raw-public-key transport (pinned and revoked keys refused inside the handshake, resumption included), signed capabilities, liveness and reconnect, signed mDNS discovery, SPAKE2 pairing bound to the TLS exporter (code or QR, QR identity checked in the handshake), Mode B remote execution, API routes, CLI commands, desktop Peers/Privacy/Run-on views.

**Verified (SINGLE-HOST, topology-independent criteria):** F-06-01, F-06-02, F-06-03, F-06-05, F-06-07, F-06-10, F-02-02, F-02-03, F-02-04, F-02-05, F-02-09, F-08-10, F-10-10. Evidence: `docs/VERIFICATION.md` (M3 sections), `docs/evidence/m3-single-host-2026-10-15.md`, `docs/evidence/m3-ui-pairing-and-defaults-2026-10-15.md`.

**Still open:** F-02-01 and F-10-05 need two machines (B-002). F-06-04 needs an owner decision on the default peer share (B-014) and has one unexplained `backend_failed` (B-015).

**Review history:** three adversarial reviews of the first M3 cut (45 findings, including a TLS-resumption bypass of the pinned verifier and lost cross-process revocations) and two of the follow-ups (16 findings, each checked by a skeptic) were fixed. Testing in the app found two more defects, both now fixed: network settings changes failed with EADDRINUSE on the fixed port, and the CLI mesh test leaked daemons and engines on failure.

**Suites at 68e73f0:** `UCOMPUTE_HOME=.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test --workspace --no-fail-fast` → 364 passed, 0 failed; clippy `-D warnings` and fmt clean; `apps/desktop` 77 passed, build ok; registry valid.

**Next:**
```
python3 scripts/validate_registry.py
cat docs/BLOCKERS.md                  # B-002 second machine, B-014 default peer share, B-015
UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test --workspace --no-fail-fast
M3_DIR=<scratch dir with a/ b/ bin/ucompute> python3 -I tests/distributed/single_host/m3_evidence.py step1   # two-daemon run
```
With a second machine: run the same driver across two hosts, pair two packaged desktop installs through the UI, and capture on a LAN interface; then F-02-01, F-10-05 and the M4 pooled-inference work (F-03-*) can start.

## 2026-10-14 (later) — Claude Code — M3 UCompute Mesh wave A committed (5034fb6)

Identity (Ed25519/ring, 0700/0600, redacted), policy (default-deny, 2×2,000-case property tests), audit + redacted diagnostics, and the QUIC transport core (rustls RFC 7250 raw public keys, TLS 1.3 only, pinned verifier inside the handshake, signed capabilities, prost framing, peer loss in 6.5 s) are implemented with 66 tests. Ledger: 11 rows moved to IMPLEMENTED_UNVERIFIED (F-06-01/02/03/04/07/10, F-02-03/04/05/09, F-08-10). In progress: discovery (mDNS), SPAKE2 pairing, remote execution (Mode B), then API/CLI/desktop integration, review and the SINGLE-HOST evidence run. Known gap: reqwest's feature unification compiles rustls tls12 into the binary although every UCompute TLS config pins 1.3.

## 2026-10-14 — Claude Code (owner's Mac, shell access) — M1 delivered, M2 desktop running, registry 22 VERIFIED

**Verified on the owner's MacBook Air (Apple M2, 8 GB, macOS 26.2):** 186 Rust tests with `UCOMPUTE_REQUIRE_FIXTURES=1` (no silent skips) + 43 desktop tests + 8 registry tests; `cargo fmt`/`clippy -D warnings` clean. Evidence per requirement in `docs/VERIFICATION.md` ("M1 evidence"), benchmarks in `benchmarks/results/`.

**Delivered:**
- Local inference runtime (`ucompute-runtime`): supervised pinned llama.cpp b11609, Metal by default with log-proven offload, CPU fallback with a surfaced reason, streaming, cancel (slot idle ~1.2 ms), typed timeouts, blob hash re-verified before every load, single-slot model manager that refuses to drop in-flight generations.
- Models (`ucompute-models`): bounds-checked GGUF reader (2×1,000 fuzz, no panics), memory estimate calibrated to within −4.6…+8.9 % of measured engine footprint on 14 combinations, context/KV planner with explicit capping notes, quant recommendation (library only).
- Artifacts (`ucompute-artifacts`): content-addressed store, https-only resumable verified downloads (real HF pull and kill/resume exercised), safe archive extraction + re-verification, 8-model catalog with licenses.
- API (`ucompute-api`): OpenAI-compatible (official Python and JS clients pass), SSE, admission control (10× overload bounded), promtool-valid metrics, validated OpenAPI 3.1, loopback-only default + CSRF guard + 2 MiB body bound.
- CLI (`ucompute`): hardware, models {list,search,pull,inspect,import,verify,rm}, run, chat, serve, setup, doctor, bench, replay; first run auto-provisions the engine; signals stop generation and reap the engine.
- Desktop (`apps/desktop`): Tauri 2 + React shell; live chat on Metal verified in the browser (DESKTOP-E2E); packaged `UCompute.app` + dmg built (ad-hoc signed; see F-10-01 evidence once recorded).
- Docs: ADR-0002 (accepted), ADR-0003 (proposed M3 design), threat model, README, CI workflow, cargo-deny config.

**Ledger counts:** VERIFIED 22 · IMPLEMENTED_UNVERIFIED 6 (F-01-06, F-01-07, F-01-12, F-10-01, F-10-04, F-10-11) · IN_PROGRESS 3 · NOT_STARTED 117 · BLOCKED 0 · INFEASIBLE 0. New blockers B-012 (second backend), B-013 (tool-call fixture).

**Known limitations:** only macOS arm64 exercised (Linux/Windows paths compile and are fixture-tested; B-004); `available_bytes` is conservative under memory pressure; no second backend; tool-call round trip untested; packaged app unsigned (B-008) and not yet tested from a fresh user account.

**Next actionable task (M2 close-out, then M3):** record F-10-01 evidence from the packaged app (sidecar health, select model, first token), test from a fresh macOS user account, add `ucompute models recommend` (F-01-07), pin a tool-capable fixture (B-013); then start M3 per ADR-0003: `ucompute-identity` (Ed25519 keys, F-06-01), `ucompute-network` (quinn+rustls RPK transport, mDNS discovery, SPAKE2 pairing), remote execution over the authenticated transport.

**Next commands:**
```sh
UCOMPUTE_HOME=$PWD/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test --workspace --exclude ucompute-desktop
python3 scripts/validate_registry.py
cargo build --release -p ucompute-cli && (cd apps/desktop && bash scripts/prepare-sidecar.sh && pnpm tauri build --bundles app,dmg)
open target/release/bundle/macos/UCompute.app
```

## 2026-10-14 — Claude Code (owner's Mac, shell access) — M1 implementation in progress (superseded by the entry above)

**Verified:** Phase 1 on the Mac (`docs/environment/`); Codex slice re-run (17/17 tests, 4 real-inference) and its history imported as `refs/recovered/codex-m1/*`; pinned `llama-server` b11609 reproduces the golden output on loopback (VERIFICATION: ENGINE-SMOKE).

**Implemented (Rust workspace, all tests run with the real engine + fixtures, `UCOMPUTE_HOME=.ucompute`):**
- `ucompute-core` (8 tests), `ucompute-hardware` (14), `ucompute-models` (19 incl. 2×1,000-iteration GGUF fuzz), `ucompute-artifacts` (29 incl. cut-and-resume download, malicious tar), `ucompute-runtime` (32 incl. 6 real-engine tests: CPU/Metal golden text, forced-GPU-failure fallback, conformance suite; cancel → slot idle in ~1.2 ms).
- Desktop app `apps/desktop` (Tauri 2 + React): 43 vitest tests, axe 0 violations, keyboard-only primary flow; `cargo check` clean. Not yet run as a packaged app.
- Docs: ADR-0002 (accepted), ADR-0003 (proposed), threat model (SD-01..SD-12), SECURITY/CONTRIBUTING/CODE_OF_CONDUCT, CI workflow (`.github/workflows/ci.yml`, cannot run until B-007), `deny.toml`.
- Registry: acceptance criteria strengthened for F-06-06, F-01-05, F-08-08, F-06-01 (threat-model gaps). Statuses not yet changed: evidence entries are written after the API/CLI land and the evidence workflow runs.

**In progress:** `ucompute-api` (OpenAI-compatible API, admission control, metrics, OpenAPI) and `ucompute-cli` (`ucompute hardware|models|run|chat|serve|setup|doctor|bench`), then adversarial review + fixes, then the evidence run (golden tokens, streaming, cancel latency, Metal vs CPU, memory-estimate calibration on ≥5 model/ctx combos, OpenAI client smoke, overload) and registry status updates.

**Ledger counts:** unchanged from the import (NOT_STARTED 143 · IN_PROGRESS 3 · IMPLEMENTED_UNVERIFIED 2) until evidence is attached.

**Next commands:**
```sh
UCOMPUTE_HOME=$PWD/.ucompute cargo test                      # whole daemon workspace
UCOMPUTE_HOME=$PWD/.ucompute cargo run -p ucompute-cli -- run stories15M-q4_0 -p "Once upon a time, a little rabbit" -n 32 --temp 0 --seed 42
UCOMPUTE_HOME=$PWD/.ucompute cargo run -p ucompute-cli -- serve  # then: cd apps/desktop && pnpm dev
```

## 2026-10-14 — Claude (claude.ai chat, cloud workspace) — registry + research import kit

**Produced (not on the Mac yet):**
- `docs/SPEC_HANDOFF_V2.md` — verbatim spec.
- `docs/REQUIREMENTS.csv` (148 rows, all spec section 21 fields + milestone, owner, section) and generated `docs/requirements/REGISTRY.md`.
- `scripts/gen_registry.py`, `scripts/validate_registry.py` (rules: 148 IDs match spec, descriptions verbatim, status vocabulary, VERIFIED needs evidence + commit + verified deps, no dependency cycles, deps not in later milestones, no vague acceptance criteria, generated files in sync).
- `docs/research/*` and `docs/decisions/ADR-0001-stack-and-license.md` — M0 report imported with markers preserved.
- `scripts/inspect-env.sh` — Phase 1 inspector. **Linux branch test-run only; macOS branch unexecuted.**
- `AGENTS.md`, `docs/{PROGRESS,DECISIONS,BLOCKERS,VERIFICATION}.md`.

**Ledger counts:** NOT_STARTED 143 · IN_PROGRESS 3 (F-06-11, F-06-12, F-07-12: literature only) · IMPLEMENTED_UNVERIFIED 2 (F-01-02, F-01-04: user-reported claim, unreconciled) · BLOCKED 0 · VERIFIED 0 · INFEASIBLE 0.

**Not done:** Phase 1 on the Mac; reconciliation with the existing repo; any M1 code. Cause: B-001.

**Next commands (on the Mac, in the repo root):**
```sh
scripts/inspect-env.sh                     # Phase 1 record
python3 scripts/validate_registry.py       # registry rules
# then reconcile F-01-*/F-09-*/F-08-* rows against the existing STATUS.md evidence
```

## 2026-10-14 — Codex (owner's Mac) — first local vertical slice (USER-REPORTED, UNRECONCILED)
Owner reports: real small GGUF model runs on Mac CPU and Apple GPU; 17 tests pass with repeatable output and saved evidence; local Git repo created; nothing pushed. Location reported: `/Users/nasr/Documents/Codex/2026-10-14/referenced-chatgpt-conversation-this-is-an/outputs/ucompute` (`README.md`, `STATUS.md`). That agent did not have the spec or M0 files. **Not inspected by the registry author.**

## 2026-10-13 — Claude (claude.ai chat) — M0 research
M0 research report produced (see `docs/research/README.md`). No code. No benchmarks run.
