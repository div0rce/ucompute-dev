# Progress Log

Newest first. Every entry: date, agent, what changed, counts, evidence, next commands.

## 2026-10-21 — Claude Code — discovery/reconnection fixes, desktop defects, Windows firewall consent step (unit + SINGLE-HOST)

- Implemented (not yet physically verified): the discovery and reconnection fixes, the six desktop defects from docs/evidence/desktop-two-machine-2026-10-19.md, the Windows Firewall consent step, clearer connection states. Details and commands: docs/VERIFICATION.md (2026-10-21 entry).
- Verified on one Mac: fmt and clippy clean (also for the Windows target), 407 Rust tests passed plus the fixed CLI test, desktop 111/111; one load-sensitive latency test (F-06-04 p95) fails on this loaded machine with and without the change.
- New test host: Kyle's MacBook Pro (M2 Pro, 16 GB, macOS 15.7.9) reachable over SSH with Kyle's consent (Common project scope); Rust 1.98.0, Node 26.10.0 and pnpm installed in Kyle's home folder. Mac to Mac remote runs worked both ways with the installed apps (Qwen2.5 0.5B).
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

- Recorded the owner's revised 2026-10-19 decision, "Contribute compute. Earn compute. Buy compute when you need more. Share your compute. Not your computer.", as binding principle D-019, with four guardrails added by the engineering session and labelled as such in D-019 and §26.5. It replaces in place the same-day, uncommitted "Free Compute Commons" draft (community compute barred from paid work); ids D-019 and U-05 are kept and no trace of the two-economy version remains. Edited: spec §25 amendment note and U-05 declaration, §26.1 product purpose/cost and messages (owner slogan and taglines), §26.3 participant-without-hardware row, §26.5 rewritten as "Compute credits and contributor consent" (stages 0–3 with preconditions; eight rules, each with a falsifiable U-05 check), §29.1/29.2/29.3 alignment (network platform fee and compute-credit rows), §29.4 credit economy model (issuance, redemption, contributor share, platform fee, outstanding liability, reserve, grant budget, refunds/chargebacks, fraud and verification cost, capacity coverage; the owner's $10 → $8.50 + $1.50 example worked through as a hypothesis, showing an illustrative margin of about −$1.71 at the verification rate F-07-09's acceptance implies; falsification conditions), §29.5 product-page rule, WP-09/11/12/13, §31 Public contributor network and Commercial managed service gates; ADR-0004 amendment; threat model §6.2 credit-economy abuse row; README, AGENTS hard rule, CONTRIBUTING.
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

The `common` daemon now runs on Windows 11 (x64/arm64 pins). Tested on the owner's HP laptop (Core Ultra 7 258V, Arc 140V), driven from the Mac over SSH: setup, golden inference on the Arc GPU via Vulkan, and the full M3 flow against the Mac on eduroam — pairing, remote execution both ways, local_only refusal, revocation, loss detection and reconnection — PASS twice. Discovery fails on eduroam (the network drops mDNS between clients). Three Windows defects fixed on the way (disk free, GPU name parsing, engine outliving a killed daemon). Evidence: `docs/evidence/m3-two-machine-2026-10-18.md`. Ledger statuses unchanged; two-machine evidence added to F-02-02/03/05, F-06-02/05/10; F-02-01 and F-10-05 still open (B-002, B-004 updated).

Next:
```
# on a network that carries mDNS (home Wi-Fi or a phone hotspot), with both daemons running:
python3 -I tests/distributed/two_machine/mesh_two_machine.py --a http://127.0.0.1:18301 --b http://127.0.0.1:18302 --out <dir>
```
Then the Windows desktop build (Tauri: MSVC toolchain needs the owner's license acceptance, or build on the laptop) for F-10-05 and F-10-01 on Windows.

## 2026-10-17 — Claude Code — packaged app rebuilt and re-verified; first two-Mac observations

`target/` had been cleaned; the `.app`/`.dmg` were rebuilt from 08e267e and passed `verify_packaged_app.py` (15/15) with real inference; the executables are byte-identical to the 2026-10-15 build installed in `/Applications`. Workspace 367 passed, desktop 77 passed (docs/VERIFICATION.md).

The installed app's audit log (`~/Library/Application Support/Common/logs/audit.jsonl`, 2026-10-15 22:12 to 2026-10-16 02:33 UTC) shows the first real two-machine use, with a Mac at 192.168.1.115 named "Kyle":
- pairing over the LAN succeeded (twice: revoked once, re-paired);
- that Mac connected to this one 10 times (inbound, reconnecting about hourly);
- this Mac's 441 outbound dials to its UDP 8193 all failed with `peer_lost`, and it did not answer ping on 2026-10-17. Likely its macOS firewall blocks incoming connections (stealth mode drops ping) or it was asleep; not confirmed from this side;
- no group contains it, so a remote request would have been refused (`peer_unauthorized`); none was attempted.
Not evidence for any ledger row yet: no remote inference ran across the two machines, and the other Mac's side was not observed.

Next: on the other Mac, check System Settings > Network > Firewall for `Common`/`common` and allow incoming connections, create a group containing both devices with "run models" allowed, then run a remote request in both directions and collect both audit logs.

## 2026-10-15 (evening) — Claude Code — packaged desktop app fixed (stale bundled daemon)

A tester's packaged `Common.app` never started its daemon: the bundled `common` predated `--exit-with-parent`, which the shell passes. `tauri build` now builds and checks the daemon from the same checkout (`apps/desktop/scripts/build-sidecar.sh`), and the daemon's arguments live in one file (`apps/desktop/src-tauri/daemon-args.txt`). Three more defects found while verifying were fixed: quit orphaned the engine (SIGKILL), a crashed app orphaned it too (SIGPIPE on the daemon's next write), and the bundle was not signed. Evidence: `docs/evidence/desktop-sidecar-fix-2026-10-15.md`; the rebuilt `Common_0.1.0_aarch64.dmg` passed all 15 checks of `apps/desktop/scripts/verify_packaged_app.py` with real inference. Workspace 367 passed, desktop 77 passed. Ledger unchanged (F-10-01 stays IMPLEMENTED_UNVERIFIED: fresh-account, UI-only first token not yet run; B-008).

Next: replace the old `/Applications/Common.app` with the new build; run the UI-only install → first token on a fresh macOS user account or a second Mac; ship testers the new .dmg with its sha256.

## 2026-10-15 — Claude Code — M3 Common Mesh: 13 rows VERIFIED (SINGLE-HOST), 3 open

**Counts:** VERIFIED 35 (+13), IMPLEMENTED_UNVERIFIED 9, IN_PROGRESS 3, NOT_STARTED 101, BLOCKED 0, INFEASIBLE 0. Commits 5034fb6 … 68e73f0.

**Built:** device identity (Ed25519, 0700/0600), peer store with groups and signed revocations, default-deny policy and per-peer quotas, append-only audit with rotation and redacted diagnostics, QUIC + TLS 1.3 raw-public-key transport (pinned and revoked keys refused inside the handshake, resumption included), signed capabilities, liveness and reconnect, signed mDNS discovery, SPAKE2 pairing bound to the TLS exporter (code or QR, QR identity checked in the handshake), Mode B remote execution, API routes, CLI commands, desktop Peers/Privacy/Run-on views.

**Verified (SINGLE-HOST, topology-independent criteria):** F-06-01, F-06-02, F-06-03, F-06-05, F-06-07, F-06-10, F-02-02, F-02-03, F-02-04, F-02-05, F-02-09, F-08-10, F-10-10. Evidence: `docs/VERIFICATION.md` (M3 sections), `docs/evidence/m3-single-host-2026-10-15.md`, `docs/evidence/m3-ui-pairing-and-defaults-2026-10-15.md`.

**Still open:** F-02-01 and F-10-05 need two machines (B-002). F-06-04 needs an owner decision on the default peer share (B-014) and has one unexplained `backend_failed` (B-015).

**Review history:** three adversarial reviews of the first M3 cut (45 findings, including a TLS-resumption bypass of the pinned verifier and lost cross-process revocations) and two of the follow-ups (16 findings, each checked by a skeptic) were fixed. Testing in the app found two more defects, both now fixed: network settings changes failed with EADDRINUSE on the fixed port, and the CLI mesh test leaked daemons and engines on failure.

**Suites at 68e73f0:** `COMMON_HOME=.common COMMON_REQUIRE_FIXTURES=1 cargo test --workspace --no-fail-fast` → 364 passed, 0 failed; clippy `-D warnings` and fmt clean; `apps/desktop` 77 passed, build ok; registry valid.

**Next:**
```
python3 scripts/validate_registry.py
cat docs/BLOCKERS.md                  # B-002 second machine, B-014 default peer share, B-015
COMMON_HOME=$PWD/.common COMMON_REQUIRE_FIXTURES=1 cargo test --workspace --no-fail-fast
M3_DIR=<scratch dir with a/ b/ bin/common> python3 -I tests/distributed/single_host/m3_evidence.py step1   # two-daemon run
```
With a second machine: run the same driver across two hosts, pair two packaged desktop installs through the UI, and capture on a LAN interface; then F-02-01, F-10-05 and the M4 pooled-inference work (F-03-*) can start.

## 2026-10-14 (later) — Claude Code — M3 Common Mesh wave A committed (5034fb6)

Identity (Ed25519/ring, 0700/0600, redacted), policy (default-deny, 2×2,000-case property tests), audit + redacted diagnostics, and the QUIC transport core (rustls RFC 7250 raw public keys, TLS 1.3 only, pinned verifier inside the handshake, signed capabilities, prost framing, peer loss in 6.5 s) are implemented with 66 tests. Ledger: 11 rows moved to IMPLEMENTED_UNVERIFIED (F-06-01/02/03/04/07/10, F-02-03/04/05/09, F-08-10). In progress: discovery (mDNS), SPAKE2 pairing, remote execution (Mode B), then API/CLI/desktop integration, review and the SINGLE-HOST evidence run. Known gap: reqwest's feature unification compiles rustls tls12 into the binary although every Common TLS config pins 1.3.

## 2026-10-14 — Claude Code (owner's Mac, shell access) — M1 delivered, M2 desktop running, registry 22 VERIFIED

**Verified on the owner's MacBook Air (Apple M2, 8 GB, macOS 26.2):** 186 Rust tests with `COMMON_REQUIRE_FIXTURES=1` (no silent skips) + 43 desktop tests + 8 registry tests; `cargo fmt`/`clippy -D warnings` clean. Evidence per requirement in `docs/VERIFICATION.md` ("M1 evidence"), benchmarks in `benchmarks/results/`.

**Delivered:**
- Local inference runtime (`common-runtime`): supervised pinned llama.cpp b11609, Metal by default with log-proven offload, CPU fallback with a surfaced reason, streaming, cancel (slot idle ~1.2 ms), typed timeouts, blob hash re-verified before every load, single-slot model manager that refuses to drop in-flight generations.
- Models (`common-models`): bounds-checked GGUF reader (2×1,000 fuzz, no panics), memory estimate calibrated to within −4.6…+8.9 % of measured engine footprint on 14 combinations, context/KV planner with explicit capping notes, quant recommendation (library only).
- Artifacts (`common-artifacts`): content-addressed store, https-only resumable verified downloads (real HF pull and kill/resume exercised), safe archive extraction + re-verification, 8-model catalog with licenses.
- API (`common-api`): OpenAI-compatible (official Python and JS clients pass), SSE, admission control (10× overload bounded), promtool-valid metrics, validated OpenAPI 3.1, loopback-only default + CSRF guard + 2 MiB body bound.
- CLI (`common`): hardware, models {list,search,pull,inspect,import,verify,rm}, run, chat, serve, setup, doctor, bench, replay; first run auto-provisions the engine; signals stop generation and reap the engine.
- Desktop (`apps/desktop`): Tauri 2 + React shell; live chat on Metal verified in the browser (DESKTOP-E2E); packaged `Common.app` + dmg built (ad-hoc signed; see F-10-01 evidence once recorded).
- Docs: ADR-0002 (accepted), ADR-0003 (proposed M3 design), threat model, README, CI workflow, cargo-deny config.

**Ledger counts:** VERIFIED 22 · IMPLEMENTED_UNVERIFIED 6 (F-01-06, F-01-07, F-01-12, F-10-01, F-10-04, F-10-11) · IN_PROGRESS 3 · NOT_STARTED 117 · BLOCKED 0 · INFEASIBLE 0. New blockers B-012 (second backend), B-013 (tool-call fixture).

**Known limitations:** only macOS arm64 exercised (Linux/Windows paths compile and are fixture-tested; B-004); `available_bytes` is conservative under memory pressure; no second backend; tool-call round trip untested; packaged app unsigned (B-008) and not yet tested from a fresh user account.

**Next actionable task (M2 close-out, then M3):** record F-10-01 evidence from the packaged app (sidecar health, select model, first token), test from a fresh macOS user account, add `common models recommend` (F-01-07), pin a tool-capable fixture (B-013); then start M3 per ADR-0003: `common-identity` (Ed25519 keys, F-06-01), `common-network` (quinn+rustls RPK transport, mDNS discovery, SPAKE2 pairing), remote execution over the authenticated transport.

**Next commands:**
```sh
COMMON_HOME=$PWD/.common COMMON_REQUIRE_FIXTURES=1 cargo test --workspace --exclude common-desktop
python3 scripts/validate_registry.py
cargo build --release -p common-cli && (cd apps/desktop && bash scripts/prepare-sidecar.sh && pnpm tauri build --bundles app,dmg)
open target/release/bundle/macos/Common.app
```

## 2026-10-14 — Claude Code (owner's Mac, shell access) — M1 implementation in progress (superseded by the entry above)

**Verified:** Phase 1 on the Mac (`docs/environment/`); Codex slice re-run (17/17 tests, 4 real-inference) and its history imported as `refs/recovered/codex-m1/*`; pinned `llama-server` b11609 reproduces the golden output on loopback (VERIFICATION: ENGINE-SMOKE).

**Implemented (Rust workspace, all tests run with the real engine + fixtures, `COMMON_HOME=.common`):**
- `common-core` (8 tests), `common-hardware` (14), `common-models` (19 incl. 2×1,000-iteration GGUF fuzz), `common-artifacts` (29 incl. cut-and-resume download, malicious tar), `common-runtime` (32 incl. 6 real-engine tests: CPU/Metal golden text, forced-GPU-failure fallback, conformance suite; cancel → slot idle in ~1.2 ms).
- Desktop app `apps/desktop` (Tauri 2 + React): 43 vitest tests, axe 0 violations, keyboard-only primary flow; `cargo check` clean. Not yet run as a packaged app.
- Docs: ADR-0002 (accepted), ADR-0003 (proposed), threat model (SD-01..SD-12), SECURITY/CONTRIBUTING/CODE_OF_CONDUCT, CI workflow (`.github/workflows/ci.yml`, cannot run until B-007), `deny.toml`.
- Registry: acceptance criteria strengthened for F-06-06, F-01-05, F-08-08, F-06-01 (threat-model gaps). Statuses not yet changed: evidence entries are written after the API/CLI land and the evidence workflow runs.

**In progress:** `common-api` (OpenAI-compatible API, admission control, metrics, OpenAPI) and `common-cli` (`common hardware|models|run|chat|serve|setup|doctor|bench`), then adversarial review + fixes, then the evidence run (golden tokens, streaming, cancel latency, Metal vs CPU, memory-estimate calibration on ≥5 model/ctx combos, OpenAI client smoke, overload) and registry status updates.

**Ledger counts:** unchanged from the import (NOT_STARTED 143 · IN_PROGRESS 3 · IMPLEMENTED_UNVERIFIED 2) until evidence is attached.

**Next commands:**
```sh
COMMON_HOME=$PWD/.common cargo test                      # whole daemon workspace
COMMON_HOME=$PWD/.common cargo run -p common-cli -- run stories15M-q4_0 -p "Once upon a time, a little rabbit" -n 32 --temp 0 --seed 42
COMMON_HOME=$PWD/.common cargo run -p common-cli -- serve  # then: cd apps/desktop && pnpm dev
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
Owner reports: real small GGUF model runs on Mac CPU and Apple GPU; 17 tests pass with repeatable output and saved evidence; local Git repo created; nothing pushed. Location reported: `/Users/nasr/Documents/Codex/2026-10-14/referenced-chatgpt-conversation-this-is-an/outputs/common` (`README.md`, `STATUS.md`). That agent did not have the spec or M0 files. **Not inspected by the registry author.**

## 2026-10-13 — Claude (claude.ai chat) — M0 research
M0 research report produced (see `docs/research/README.md`). No code. No benchmarks run.
