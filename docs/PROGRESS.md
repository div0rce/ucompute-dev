# Progress Log

Newest first. Every entry: date, agent, what changed, counts, evidence, next commands.

## 2026-10-14 — Claude Code (owner's Mac, shell access) — M1 implementation in progress

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
