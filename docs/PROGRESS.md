# Progress Log

Newest first. Every entry: date, agent, what changed, counts, evidence, next commands.

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
