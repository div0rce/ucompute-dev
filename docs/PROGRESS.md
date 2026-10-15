# Progress Log

Newest first. Every entry: date, agent, what changed, counts, evidence, next commands.

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
