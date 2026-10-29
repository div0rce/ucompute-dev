# Platform readiness plan (2026-10-23)

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

Working plan produced by an engineering-session workflow on 2026-10-23 (read-only audit and synthesis; not a decision record). Decisions live in docs/DECISIONS.md and docs/decisions/; status lives in docs/REQUIREMENTS.csv and docs/VERIFICATION.md.

# UCompute: ranked milestone plan toward the definition of success

**Basis.** This plan is built from the four audit result sets (packaging and distribution, runtime and supervision, mesh, API fidelity). I also read HEAD `af1459c` read-only. That covered spec §2/§11/§31, the BLOCKERS list, the ledger rows F-03-01..04, F-06-06, F-08-04 and F-10-01, and the Core Lab's README and calibration runbook.

**Definition of success** (spec §1, §2, Gate E): a person can install, select a model, run it, pair devices and pool them, then diagnose problems, all without developer help. This must hold on each advertised platform. Every P0 row must be traced to evidence.

**Current state.** Run is verified on one machine (M2, Metal and CPU). Mesh has been run on two physical machines, but only with pre-fix builds. Nothing on Linux has evidence yet. Pooling has not started: F-03-01..04 are NOT_STARTED.

**Ranking.** Items come first when they produce real-hardware evidence now on this Mac and sentinel. Windows follows when the laptop returns. Distribution comes next, under owner gates. Pooling comes last and makes no claims until a genuine multi-device run exists.

**Preconditions for every milestone**
- Nothing on this Mac or on sentinel that builds, runs a daemon or loads the CPU starts until the in-progress Mac/sentinel two-machine run has finished and its raw output is saved. Until then, only read-only work and editing on a branch are allowed.
- SINGLE-HOST results are labelled SINGLE-HOST and never satisfy F-03-01/02, Gate D or any TWO-MACHINE criterion.
- No push, release, publication, paid service, sudo or root on sentinel, account creation, or security-setting change without the owner. Each such item is listed under "Needs the owner" and never blocks the steps around it.
- Core Lab boundary: engineering does not rebuild the placement planner, the local-versus-remote calibration, the RPC dataflow study or the MLX plan. Where these appear below, engineering only *executes* the Core Lab's handed-over artifacts or *integrates* them after owner approval.
- **Input truncated.** The fourth audit set (API) was cut off partway through the "Local API auth" gap. Any later gaps in that set are not reflected here. Re-run that audit's tail before treating M3 as complete.

---

## M1. Linux local inference evidence on sentinel (Run on a second OS)

**Goal.** Linux x64 CPU becomes an evidenced EXPERIMENTAL Run platform, using real inference on the D-024 pin.

**Gaps closed.** "Linux local inference has an engine pin but no committed evidence"; the threads hard-coded to 4; Linux memory calibration skipped; documentation drift (B-004, spec §27.1 Linux row, the `run_mac_linux.sh` header, the PROGRESS "Next" block).
Requirement rows: F-01-01, F-01-02, F-01-03, F-01-04, U-03.

**First steps (autonomous, after the current run ends)**
1. On sentinel, run `scripts/mac-host.sh run 'cd ~/src/ucompute && git status && git log -1'` (`MAC_HOST=192.168.1.167 MAC_USER=div0rce`). Confirm the checkout is at HEAD, or sync it.
2. Fetch fixtures and run the real tests:
   `UCOMPUTE_HOME=$HOME/.ucompute-test UCOMPUTE_REQUIRE_FIXTURES=1 scripts/fetch-fixtures.sh && UCOMPUTE_HOME=$HOME/.ucompute-test UCOMPUTE_REQUIRE_FIXTURES=1 cargo test -p ucompute-runtime -p ucompute-api -p ucompute-hardware --no-fail-fast`
3. Run the golden CLI check:
   `ucompute setup && ucompute run stories15M-q4_0 -p 'Once upon a time, a little rabbit' -n 32 --temp 0 --seed 42`
   Compare against the golden text. Record `ldd llama-server`, the glibc version, the kernel, and the engine sha256 `f95cbb27…`.
4. Code, small and testable on the Mac:
   - Make `estimate_vs_measured.rs` fail instead of printing SKIP when `UCOMPUTE_REQUIRE_FIXTURES=1`.
   - Add a Linux peak reader (`/proc/<pid>/status` VmHWM).
   - Default the thread count from `hardware.cpu.physical_cores` in `ModelManager::plan`, and fix the `run.rs:47` help text.
   - Re-run steps 2 and 3 on sentinel.
5. Run `ucompute bench` on sentinel at `-t 4` and at the new default. Commit the JSON to `benchmarks/results/` with a hardware manifest.
6. Make one documentation commit:
   - A VERIFICATION entry for this work.
   - B-004 updated, and spec §27.1 Linux row proposed as EXPERIMENTAL with OS, kernel, CPU and artifact hash.
   - The `run_mac_linux.sh` header fixed.
   - `docs/environment/linux-test-host.md` tracked.
   - The F-06-04 versus B-014 wording reconciled.
   - Then run `gen_registry.py`, `validate_registry.py` and the registry unit tests. Commit locally.

**Needs the owner (non-blocking).** Nothing for these steps. Later, the choice of glibc floor (build in an Ubuntu 22.04/24.04 container or state the 26.04 floor) is folded into the M6 D-entry.

**Evidence produced.** A VERIFICATION entry for "Linux x64 CPU, sentinel" covering golden text, conformance, cancellation, memory estimate versus measured within ±15%, and benchmark JSON.

**Does NOT claim.** Linux GPU, any Linux package or install path, a Linux desktop app, or SUPPORTED status. Everything is labelled EXPERIMENTAL and names the exact host.

---

## M2. Mac and Linux two-machine Mesh on HEAD builds (first fix-round build on two machines)

**Goal.** The first physical run of rounds 1–5 code, in both directions, with Linux serving remote requests.

**Gaps closed.** Mac-Linux discovery dead end at link-local addresses; Mac-Linux acceptance run not committed and runner stale; "no fix-round build has run on two machines" (the Mac/Linux half); CROSS_HOLD only proven SINGLE-HOST (Mac/Linux half).
Requirement rows: F-02-01, F-02-02, F-02-03, F-02-05, F-06-05, F-08-10, U-03.

**First steps**
1. Save the in-progress run's output under `docs/evidence/mesh-physical-2026-10-23-linux/` together with the link-local observation.
2. Do read-only diagnosis:
   - On sentinel, run `avahi-browse -rpt _ucompute._udp` (no sudo).
   - On the Mac, run `dns-sd -B _ucompute._udp` and `dns-sd -L <instance> _ucompute._udp`.
   - Run HEAD `ucompute serve --network -v` from Terminal with `RUST_LOG=mdns_sd=debug`. Comparing this with the app separates the app's Local Network privacy state from mdns-sd behaviour.
3. Code changes in `crates/ucompute-network/src/discovery.rs`:
   - Advertise and rank only addresses in the bound family. With the default 0.0.0.0 bind, IPv6 is neither advertised nor ranked.
   - Treat a peer with only link-local addresses as "discovered, no reachable address", with a typed reason.
   - Add a unit test next to `rank_addrs_keeps_two_routable_addresses`.
4. Fix the runner's B→A expectation now that Linux has an engine. Pre-pull the fixture on sentinel (`UCOMPUTE_HOME=$HOME/.ucompute-acceptance`).
5. Run `LINUX_HOST=192.168.1.167 LINUX_USER=div0rce OUT=<dir> NO_MDNS=1 CROSS_HOLD=1 tests/distributed/two_machine/run_mac_linux.sh`. Then run it again with mDNS on.
   The run covers: pairing both ways with a refused wrong code first; revocation and re-pair; kill and restart detection within 6 s; hold, and Connect ending a hold; golden output from Linux CPU both as JSON and as a stream.
6. Fix the `apply_peer_revocation` residual (disconnect the running Node under the net lock) with its test, then re-run the hold steps.
7. Execute the Core Lab's handed-over calibration (`ucompute-core-lab/calibration/RUNBOOK.md`, `tools/calibrate.py`) against the Mac and sentinel daemons. Run the lab's driver unmodified, and with host B changed from the laptop tunnel to sentinel only if the runbook allows it. Hand the raw data back to the Core Lab. Do not analyse it in this repository.
8. Commit the runner, the host document, the evidence and the record updates locally.

**Needs the owner (non-blocking).** Only if the Local Network privacy prompt is implicated: the owner reads or toggles System Settings > Privacy & Security > Local Network for UCompute.app. Separately, a decision on adding `NSLocalNetworkUsageDescription` and `NSBonjourServices` to the bundle.

**Evidence produced.** `docs/evidence/mesh-physical-2026-10-23-linux.md`, labelled TWO-MACHINE, ALL PASS with N/A only for unmet preconditions, naming the ffe1a47 app (or a HEAD binary) and sentinel at HEAD. The F-02-01 note records the root cause.

**Does NOT claim.**
- Windows currency.
- A three-node run.
- Pooling: remote execution of a whole model on one peer is not pooling.
- Re-certification of the VERIFIED rows from older commits. That happens only through a dated note per row.

---

## M3. Runtime hardening on hardware available now (Mac and sentinel)

**Goal.** Close the crash, memory, disk, cancellation and log classes that would fail the private-alpha bar of "20 quit/crash/restart cycles, no orphan" and the ≥99% crash-free sessions target. Also close the two API gaps that are security- or feature-critical.

**Gaps closed.**
- A dead engine stays in its slot (F-08-04, F-08-01).
- The memory fit uses a stale snapshot (F-01-03, F-01-08).
- No disk-full preflight and no ENOSPC handling (F-05-03, F-01-05).
- Downloads cannot be cancelled and duplicate pulls are not de-duplicated (F-01-05, F-10-02).
- Engine logs are never pruned and the diagnostics bundle grows with them (F-10-06).
- Daemon kill mid-job is undefined (F-08-04).
- Tool calling drops `tool_calls` and `tool_call_id` (F-01-12, B-013).
- No local API token (F-06-06, F-09-05). The ledger shows F-06-06 as VERIFIED, but the audit finds the installed daemon runs without a key. Re-check that row's acceptance against the installed configuration before relying on it.
- Four slots quarter the context (F-08-08).

**First steps.** Each item is small, test-first, and labelled REAL on the fixture.
1. `ensure_loaded_for`: check liveness with `process::alive(pid)` before returning a cached handle. Map connection-refused to an engine-exited error, and bound restarts at 3 per 60 s. Add the real test `engine_killed_is_reloaded_on_fixture`.
2. Add `ucompute_hardware::available_memory()` and call it in `ModelManager::plan`. Add a unit test with an injected memory source.
3. `download::fetch`: compare the bytes needed against a fresh free-disk reading, and map ENOSPC (28/112) to a typed error. Test on an `hdiutil create -size 64m` scratch image used as `UCOMPUTE_HOME`.
4. Pass a `CancellationToken` into `pull` and add `DELETE /api/models/{id}/pull`. Make pulls single-writer per id. Add `cancel_mid_pull_keeps_partial_and_stops_network` and `duplicate_pull_is_single_writer`, plus a desktop Cancel button with a vitest case.
5. Prune `logs/engine-*.log` to the newest N files. The bundle reads only the newest K files with a seek-from-end tail. Add a test with 1,000 synthetic logs.
6. Add a ucompute-cli integration test, following the `desktop_sidecar.rs:255` pattern: kill -9 the daemon mid-stream and mid-pull, then restart. Assert no orphan engine, no "running" jobs, and that the pull resumes. Document the client-visible contract in `docs/api/README.md`.
7. Tool calls:
   - Add optional `tool_calls`, `tool_call_id` and `name` to `ChatMessage`, with `skip_serializing_if` so peer-wire JSON stays byte-identical. Add a mesh round-trip test.
   - Forward `tool_choice` and `parallel_tool_calls`.
   - Check `llama-server --help` for `--jinja`.
   - Add a real `tool_call_roundtrip_openai_client` test on qwen2.5-0.5b-instruct, falling back to 1.5b, using the Python and JS clients.
   - Update openapi.json and B-013.
8. Local API token:
   - Write a short ADR covering scopes (inference versus admin/mesh), file location (0600, next to `serve.json`) and how OpenAI clients pass `api_key`.
   - Implement `--api-key-file` and `UCOMPUTE_API_KEY` for `serve`. The desktop and CLI read the token automatically.
   - Mode-loosening routes need more than a body flag.
   - Add tests showing that token-less and cross-uid requests are refused.
   - Update the THREAT_MODEL rows.
9. Slot and context default: measure qwen2.5-0.5b and 1.5b at 1, 2 and 4 concurrent requests on the Mac. Choose between n_ctx × slots and fewer slots. Add a real test that sends a 2,000-token history through the default ApiConfig. Feed the result into B-014; do not re-run the flood A/B.
10. Run the whole set on the Mac (on mains power, under caffeinate) and on sentinel with `UCOMPUTE_REQUIRE_FIXTURES=1`. Add VERIFICATION entries.

**Needs the owner (non-blocking).** Approval for the token ADR if it changes the documented API contract for integrators.

**Evidence produced.** Named REAL tests passing on macOS (Metal) and Linux (CPU), and a typed-error matrix. F-01-12 becomes eligible for VERIFIED once both clients pass. F-08-04 is evidenced for local jobs only.

**Does NOT claim.**
- Windows behaviour, deferred to M7.
- Remote job recovery: F-08-04 for mesh jobs remains open.
- Cache-cap eviction policy (F-05-03) beyond the preflight, unless it is implemented and tested.

---

## M4. Packaged-app lifecycle and build identity on macOS, leading to a private-alpha-ready macOS artifact

**Goal.** The installed app identifies itself, survives daemon death, uninstalls and upgrades predictably, and is exercised from a quarantined copy. The exact artifact is the one tested.

**Gaps closed.**
- Builds cannot be told apart by version (F-10-07, F-08-09).
- The app does not restart a dead daemon and does not persist daemon logs (F-08-04, F-10-06).
- Uninstall and data retention are undefined (macOS half).
- Gatekeeper first-run is untested (B-008, unsigned path).
- No upgrade or rollback check.
- Rename and migration decision outstanding (D-021).

**First steps (autonomous)**
1. Add a `build.rs` that embeds the short commit and a dirty flag in `ucompute --version` and `/api/status`. Version the app per build (`0.1.0-<n>`).
2. Persist the daemon log: a size-capped `UCOMPUTE_HOME/logs/daemon.log` with one rotation, included in the diagnostics bundle after redaction.
3. In `lib.rs`, on an unexpected `Terminated`, respawn the sidecar with a max-3-in-60-s policy. Expose a "restarting" state to the UI.
4. Extend `verify_packaged_app.py` with these stages:
   1. kill -9 the daemon only; `/health` must return within 10 s.
   2. Mark the copy with `xattr -w com.apple.quarantine` and record `spctl -a -vv` and `syspolicy_check distribution` output.
   3. Uninstall: remove the app, assert no process or port remains, and assert data is kept as documented.
   4. Reinstall: assert the same device id, pairings and models.
   5. Upgrade N-1 (the ffe1a47 DMG) to N, then roll back to N-1, asserting the documented outcome.
5. Write the per-OS retention table in `apps/desktop/README.md` and the threat model. It covers the app, data dir, WebView storage, identity and firewall rule, plus "erase this device" with revoke-first advice. Write the manual upgrade and rollback procedure for testers (spec §31 external alpha).
6. Set `bundle.macOS.minimumSystemVersion` to the oldest version tested.
7. Rebuild after M3 lands, then run the verifier (all stages) on this Mac. Optionally rebuild on sentinel's schedule so the Mac is free.

**Needs the owner (non-blocking: steps 1–7 proceed without these)**
- Create a standard (non-admin) test account on this Mac. In it, perform the UI-only path from a browser-downloaded or AirDropped DMG: Models > qwen2.5-0.5b > Download > Load > Chat. Record times and bandwidth, and capture `swift scripts/macos-ax-dump.swift ucompute-desktop 40 Models` output and screenshots.
- Decide whether to freeze the identifiers (bundle id, data folders) under the former name before any external install, or to rename first with a tested migration.
- Decide on Apple Developer ID and notarization. This is paid, so it needs spec §4 authorization. Until then, any build is a disclosed unsigned private-alpha exception.

**Evidence produced.**
- An updated `docs/evidence/desktop-packaged-mac-<commit>.json` with all stages.
- A fresh-account first-token record (when the owner does it) with hash, commit, OS build and timings. This advances F-10-01 for macOS only.

**Does NOT claim.** Notarization, Windows or Linux packaging, an auto-updater (F-10-07 stays NOT_STARTED until an owner-authorized endpoint and key custody exist), or public download.

---

## M5. Endurance and benchmark baseline on sentinel and the Mac

**Goal.** Measured stability and performance numbers that satisfy the spec's evidence rules, with real inference only.

**Gaps closed.** No soak evidence; benchmarks exist only for the Mac with tiny models; peer flood defaults unmeasured on a quiet host (WP-02, B-014); B-015 not reproduced.
Requirement rows: F-08-08, F-06-04, U-03, F-10-09.

**First steps**
1. Write `tests/soak/local_soak.py`, standard library only, loopback API only, fixed seed. It cycles load, unload, chat, stream, cancel, pull with cache hit, overload and engine-kill (from M3). Every 60 s it samples RSS, open file descriptors, log size, an error-code histogram and p95, written as JSONL.
2. Run the soak for 24 h on sentinel (`UCOMPUTE_HOME=~/.ucompute-soak`, its own port, no LAN listener). Run it for several hours on the Mac on mains power.
3. On sentinel (a quiet 8-core host), run `peer_flood_throttled_through_daemon_single_host` with the shipped 4/3 defaults, labelled SINGLE-HOST Linux. Report run count and spread.
4. Add a two-peer fairness test in `crates/ucompute-api/tests/mesh.rs` (per-peer cap of ceil(peer_slots/n_active)) and pick the default from the measurements.
5. Run a long flood soak on sentinel aimed at reproducing B-015.

**Needs the owner (non-blocking).** None.

**Evidence produced.** Soak JSONL with pass bounds stated in advance (RSS growth, zero untyped errors, zero orphans). `benchmarks/results/*.json` per platform. A B-015 outcome: captured, or bounded by N requests.

**Does NOT claim.** The ≥99% crash-free cohort metric, which needs real testers; the Windows Vulkan numbers (M7); or any latency or placement claim, which belongs to the Core Lab.

---

## M6. Linux headless install path (packaging, lifecycle, F-09-10 example)

**Goal.** An owner-approvable minimal Linux install: a user-level tarball plus a systemd user unit, verified on sentinel.

**Gaps closed.** "Linux: nothing packaged"; llama-server outliving a killed daemon on Linux; the systemd example for F-09-10.
Requirement rows: F-10-01 (Linux), F-01-02, F-09-10, U-03.

**First steps**
1. Draft `packaging/linux/ucompute.service`:
   `ExecStart=%h/.local/bin/ucompute serve --host 127.0.0.1 --port 8192`, `Restart=on-failure`, and the default `KillMode=control-group`.
2. Write a tarball script that packages the binary built `--locked`, LICENSE, generated notices and the unit file, plus a SHA256SUMS file.
3. Add `PR_SET_PDEATHSIG` for llama-server on Linux, with a test that SIGKILLs the daemon and finds no engine remaining.
4. Port `verify_packaged_app.py` to a Linux verifier. It checks:
   - a fresh HOME and XDG dirs;
   - that `ucompute setup` provisions the pinned engine and `ldd` resolves;
   - golden CPU completion;
   - that `systemctl --user stop` frees port 8192 with no llama-server left;
   - kill -9 recovery;
   - that uninstall keeps `~/.local/share/ucompute` as documented.
5. Run it on sentinel (user units need no sudo). Record the result as an EXPERIMENTAL U-03 entry.

**Needs the owner (non-blocking).**
- A D-entry approving the format (tarball plus user unit) and the glibc floor.
- Whether lingering is enabled, which may need `loginctl enable-linger` and possibly sudo.

**Evidence produced.** A Linux lifecycle verifier record naming Ubuntu 26.04 and the i9-9900K.

**Does NOT claim.** .deb, AppImage, the Linux desktop app, distributions other than the one tested, or GPU.

---

## M7. Windows return: preserve, then rebuild and requalify (when the laptop is online)

**Goal.** Bring Windows to HEAD with full verifier, ACL and two-machine evidence, without losing the only installed minor-0 build.

**Gaps closed.**
- Mixed protocol minor 0/1 never run on hardware (F-08-10, U-01).
- Windows installer from HEAD never built or verified (F-10-01, F-10-05, F-10-11).
- Fix-round currency on Windows.
- Windows key ACLs, B-017 (F-06-01).
- Windows uninstall layout, where the install folder appears to be the data folder.
- Vulkan local acceptance (F-01-04).
- Windows memory modelling (FreePhysicalMemory, missing `rss_bytes`).
- M3's runtime tests and M4's lifecycle stages on Windows.

**First steps, in this order**
1. **Before installing anything:**
   1. Copy the 593d4ea installer aside and record its sha256.
   2. Run `WIN_HOST=<laptop> OUT=<dir>/mixed-minor tests/distributed/two_machine/run_mac_windows.sh`, with the hold steps expected to read unauthorized or N/A.
   3. Record the result as a mixed-version run.
   4. Read the real install dir from `HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\*` and list its contents.
2. Write `scripts/windows-acl-check.ps1` on the Mac beforehand (Get-Acl, then open and replace attempts on `device.key`, `peers.json` and `config.json`).
3. Sync HEAD and run `pnpm tauri build --bundles nsis` natively. Pin `bundle.windows.nsis.installMode` and `webviewInstallMode` in `tauri.conf.json`.
4. Run `verify_windows_app.ps1` at the **default** install path with the app idle. Add the uninstall/reinstall stage (`uninstall.exe /S`) and the daemon-kill and upgrade stages from M4.
5. Read the firewall rule (`Get-NetFirewallRule -DisplayName 'UCompute (paired devices)'`). Document or implement removal of the rule on uninstall. Run `scripts\windows-uia-dump.ps1`.
6. Add a `cfg(windows)` Vulkan test mirroring the Metal one, run with and without forced GPU failure. Use AvailableBytes (free plus standby) for available memory and fill in `rss_bytes`. Run the M3 REAL tests natively.
7. Run `WIN_HOST=<laptop> OUT=<dir> CROSS_HOLD=1 tests/distributed/two_machine/run_mac_windows.sh` with HEAD on both sides, then the `NO_MDNS=1` variant.
8. Run the three-node check on the Mac, sentinel and the laptop: a revocation issued on A reaches C, with latency measured, and C refuses B's next dial. Use a small driver built from `mesh_two_machine.py` and `state.py`.

**Needs the owner (non-blocking).**
- Create a second standard local account for the ACL check and for the SmartScreen/Mark-of-the-Web install from a browser-downloaded copy.
- If ACL inheritance fails, approve setting a protected DACL in code. The code change itself is autonomous; the security-setting approval is the owner's.
- The by-hand app-UI pairing session (F-10-05): a wrong code typed by hand, pairing started from each side, group creation, and a remote run from the Mac Chat with no local model. This happens at the machines with screenshots, with no API or DevTools substitute.

**Evidence produced.**
- A mixed-minor TWO-MACHINE record.
- The Windows verifier ALL PASS at the HEAD commit.
- ACL evidence, which closes or updates B-017.
- Mac-Windows TWO-MACHINE evidence at HEAD.
- The first three-node revocation record (closes part of B-002).
- Dated notes on each VERIFIED mesh row (F-02-01, F-06-05, F-08-10…) stating that the row now rests on a commit at or after ffe1a47.

**Does NOT claim.** Windows 10, ARM64, NVIDIA or dual-GPU device selection, signed installers (B-008), or pooling.

---

## M8. Mesh robustness and privacy at scale (Mac and sentinel now, the laptop later)

**Goal.** Meet the measurable Stable-Mesh bars available to us: reconnection, revocation bounds, ≥1,000 authorized requests with zero bypass, and fairness.

**Gaps closed.** Reconnection after sleep, wake or IP change (F-02-05, F-02-10); revocation delivery latency to held or offline devices (F-06-05); privacy across a real LAN (F-06-07, F-06-03); the physical run of two peers flooding one host (F-06-04).

**First steps**
1. Reset a peer's backoff on a local IpAdd/IpDel and on a detected wall-clock jump. Add tests next to `backoff_resets_on_connection_or_discovery_change`.
2. Prompt-dial on revoke, or document the maximum delivery delay in ADR-0003.
3. Add a sleep step to the drivers (`pmset sleepnow` on the Mac side) and time reconnection on both sides at 2 and 10 minutes. Add a 10 s link-drop case during a remote stream, asserting a defined terminal job state.
4. Port the request mix of `local_only_never_leaves_device_10k_single_host` to a two-machine driver: ≥1,000 randomized requests across privacy modes, using the audit-by-marker checks.
5. After M7: have the laptop and sentinel both flood the Mac, recording typed 429s, zero 500s and local p95.

**Needs the owner (non-blocking).**
- A LAN packet capture (tcpdump needs root) on the Mac or sentinel, run by the owner or under a scoped approval.
- `rtcwake` on sentinel (sudo).
- Defining the UComputeTestWifi network-switch action on the laptop (a network setting).

**Evidence produced.** TWO-MACHINE reconnection timings, privacy counts with a capture scan (when approved), and fairness numbers.

**Does NOT claim.** NAT traversal or relay (F-02-06), more than one LAN segment, the ≥10 independent pairs cohort, or the impossibility of bypass. Zero observed bypasses is not proof.

---

## M9. Distribution and independent-verification readiness (owner-gated end to end)

**Goal.** A third party can obtain, verify and run a named artifact. The private alpha's WP-08 promotion record exists.

**Gaps closed.** Third parties cannot obtain or verify any artifact (F-10-01, U-03, U-04, U-01); no packaging CI; the model-licence problem (B-011 fixture in the verifiers).

**First steps (autonomous preparation only)**
1. Perform the redaction pass in `docs/release/public-source-readiness-2026-10-22.md` §2 (P1/P2) on a branch.
2. Generate notices (cargo-about, a pnpm licence report, Lucide ISC) into bundle resources.
3. Make the verifiers standalone (daemon args read from the bundle, model switchable to qwen2.5-0.5b, Apache-2.0).
4. Sign a SHA256SUMS file using `ssh-keygen -Y sign` or minisign. Draft a VERIFY guide.
5. Draft CI jobs for macOS, Windows and Ubuntu that run the three verifiers, with a `permissions:` block. **Do not push.** Meanwhile, maintain a local script that runs all three verifiers over SSH and writes one combined record.
6. Draft the WP-08 promotion record template in VERIFICATION. Fields: artifact, SHA-256, unsigned/ad-hoc status, Gates A/B, cohort.

**Needs the owner.**
- B-007 D-entry scoping any push.
- Key custody for the signing key.
- The private-alpha cohort (≥5 consented testers).
- Approval of each private-alpha build.
- Paid signing (B-008).
- The reproducibility check, which needs the second Mac (currently unreachable).

**Evidence produced.** Locally prepared release materials plus the record template. A WP-08 record appears only after owner approval.

**Does NOT claim.** Public download, Gate G, a signed or notarized artifact, or reproducible builds until two independent builds match.

---

## M10. Pooling (Gate D): genuine multi-device partitioned execution

**Goal.** One model split across two physical machines, numerically checked against local execution. Only this milestone may lead to any pooling claim.

**Gaps closed.** F-03-01, F-03-02, F-03-03, F-03-04 (all NOT_STARTED); B-016 (RPC carriage overhead).

**Preconditions.** M2 green (Mac/sentinel authenticated transport at HEAD) and M3 green (engine supervision). ADR-0003 rule: llama.cpp `rpc-server` is never exposed beyond loopback, and all cross-machine bytes go through UCompute's authenticated, encrypted transport.

**First steps**
1. Read the Core Lab's `REPORT.md` addenda (RPC dataflow, `--tensor-split` mapping) and `HANDOFF-FROM-ENGINEERING.md`. Agree on the integration boundary with the Core Lab. Engineering owns the transport and runtime wiring; the lab owns the planner and split mapping. Nothing of the lab's is reimplemented.
2. Prototype on the Mac and sentinel. A loopback-bound `rpc-server` on sentinel is reached only through a UCompute-authenticated stream. The Mac runs llama-server with `--rpc` pointing at its local end of that stream, using a fixed split taken from the Core Lab's mapping.
3. Write a correctness test: distributed versus local output with a fixed seed. Compare greedy tokens and logit tolerance on a fixture and on qwen2.5-0.5b.
4. Measure memory per device (distributed memory evidence, F-03-02) and bytes on the wire. Compare against B-016's criterion.
5. Failure semantics: kill the remote half mid-generation and record a typed terminal state, with no orphan on either host.
6. Record a TWO-MACHINE evidence file that states exactly which model was split and how.

**Needs the owner (non-blocking for prototyping).** Approval for any larger-model download used to show a model that does *not* fit on one device (disk B-010; sentinel has about 189 GB free). Approval of the MLX second backend (B-012), which is unrelated to pooling and follows the Core Lab plan.

**Evidence produced.** A Gate D evidence file for exactly two machines, one model and one split.

**Does NOT claim.** Pooling before step 6 passes; automatic partitioning (F-03-05/07, the Core Lab's research); tensor parallelism; dynamic repartitioning; peer-failure recovery (F-03-10); or any speed-up unless measured.

---

## Later work (tracked, not scheduled ahead of the above)

- **U-01, independent peer implementation (WP-07).** A profile plus test vectors from ADR-0003 and `close_codes_are_pinned_on_the_wire`, then a minimal Python aioquic peer on sentinel.
- **NAT and relay (F-02-06).** Decide between an in-house relay and an iroh adapter in ADR-0003 first. The netns NAT matrix on sentinel needs owner-granted root.
- **Linux desktop app (.deb/AppImage).** Needs owner-approved webkit2gtk installs on sentinel and a real desktop session for UI checks.
- **Larger catalog models.** Qwen2.5 3B and 7B Q4_K_M, with an 8 GB boundary test on the Mac (owner approval for disk) and `ucompute models recommend` (F-01-07).
- **Second backend (F-01-06).** MLX according to the Core Lab plan, after the owner approves installs and weights.
- **Updater (F-10-07).** Tauri updater with an owner-held Ed25519 key and a tampered-update test, after the hosting decision (B-007).
- **Package-manager distribution (owner request, 2026-10-29).** Install, update and uninstall through the usual package managers, the whole lifecycle and not only install: Homebrew (a cask for the desktop app and a formula or cask for the `ucompute` CLI and daemon), npm/yarn (a package that fetches the pinned, hash-checked binary for the platform, the pattern CLIs such as esbuild use), and later winget and apt. Each needs a public, versioned download with published checksums (B-007, Gate G), and Homebrew's cask rules expect a signed and notarized app (B-008), so it follows the first public release, not the private alpha. Each package gets its own verifier, as the DMG and the Linux tarball have, covering a clean install, an upgrade, an uninstall that keeps or removes data as documented, and the package's checksum. Owner's words, relayed by the website session: "we should be able to use shit like brew/npm/yarn to install this too. well i dont know if only install but like you know what i mean. full suite."

## Owner action queue (consolidated; none blocks M1–M3)

| # | Action | Unblocks |
|---|---|---|
| 1 | Create a standard test account on this Mac | M4 fresh-account UI first token (F-10-01 macOS) |
| 2 | Bring the Windows laptop online and create a second local account there | M7 entirely; ACL (B-017); SmartScreen first run |
| 3 | Decide: freeze identifiers or rename first (D-021) | M4/M9 first external install |
| 4 | D-entry for the Linux format and glibc floor | M6 |
| 5 | Scoped root approval for packet capture, `rtcwake` and netns on sentinel | M8 capture; F-02-06 lab |
| 6 | B-007 push scope; signing-key custody; Apple and Windows signing (paid) | M9, CI, Gate G |
| 7 | Downloads of larger models (disk) | Memory boundary test; M10 non-fitting model |
| 8 | MLX install and weights (Core Lab plan) | F-01-06 |
| 9 | By-hand UI pairing sessions at the machines; second Mac reachable | F-10-05; Mac-to-Mac rerun; reproducibility |

## What stays false at every milestone until its own evidence exists

- Pooling or "combined compute": only M10 step 6 can change this.
- Signed or notarized installers.
- Public downloads.
- Stable Run or Stable Mesh.
- Any SUPPORTED platform.
- Windows 10, ARM64, or GPU on Linux.
- Connectivity beyond one LAN.
- Credits, Swarm or Cloud.
- Any figure not committed under `benchmarks/results/` or `docs/evidence/`.
