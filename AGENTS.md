# AGENTS.md — Project Common

Read this first, every session, whichever agent you are (Claude, Codex, other). It is the restart point.

## Authority
1. `docs/SPEC_HANDOFF_V2.md` — authoritative product specification (verbatim copy of PROJECT_COMMON_AUTONOMOUS_HANDOFF_V2). All 148 capabilities are P0.
2. `docs/REQUIREMENTS.csv` — the single requirements ledger (spec section 21 fields). `docs/requirements/REGISTRY.md` is generated from it.
3. `docs/research/` — M0 research **baseline**, not verified truth. Keep its ✅/⚠️/`unknown/unverified` markers. Re-verify before relying on anything not ✅.
4. `docs/DECISIONS.md` + `docs/decisions/ADR-*.md` — architectural decisions.

## Session start
```sh
python3 scripts/validate_registry.py          # must pass
cat docs/PROGRESS.md docs/BLOCKERS.md         # where we are, what is stuck
git status && git log --oneline -10
# then run the project test suite (see README)
```

## Session end (spec section 16 step 8)
1. Build, lint, test. Record commands and results in `docs/VERIFICATION.md`.
2. Update ledger rows you touched in `docs/REQUIREMENTS.csv`; run `python3 scripts/gen_registry.py` then `python3 scripts/validate_registry.py`.
3. Append a dated entry to `docs/PROGRESS.md`: implemented / verified / blocked / infeasible counts, evidence links, limitations, exact next commands.
4. Commit locally if tests pass. **Never push, never create a remote**, unless the owner explicitly authorizes it in writing in this repo (`docs/DECISIONS.md`).

## Status rules (enforced by `scripts/validate_registry.py`)
- Allowed: `NOT_STARTED`, `IN_PROGRESS`, `BLOCKED`, `IMPLEMENTED_UNVERIFIED`, `VERIFIED`, `INFEASIBLE_WITH_EVIDENCE`.
- `VERIFIED` = every acceptance criterion in that row demonstrated, evidence path recorded, commit hash recorded, all dependencies `VERIFIED`. No mass-marking.
- `INFEASIBLE_WITH_EVIDENCE` is not done; it needs a written determination with sources.
- A claim from another agent or a chat summary is at most `IMPLEMENTED_UNVERIFIED` until you see the evidence yourself.

## Hard rules
- No mocked or simulated inference presented as real. Synthetic fixtures must be labelled as such in the test name and evidence.
- **Single-host multiprocess tests are labelled `SINGLE-HOST`** in test names, evidence and docs. They never satisfy a two-machine criterion (F-03-01, F-03-02, Gate D). Two independent physical machines are required.
- Never expose llama.cpp `rpc-server` (or any unauthenticated service) beyond loopback. Cross-machine traffic goes only through Common's authenticated, encrypted transport.
- Default API bind is `127.0.0.1`. No public ports, no paid services, no real-money settlement, no releases without explicit owner approval (spec section 4).
- No silent model, quantization, or privacy-mode substitution.
- Never copy code from GPL/AGPL projects (ex-exo, KoboldCpp). See ADR-0001.
- Never fabricate benchmark numbers. Results live in `benchmarks/results/` with hardware manifest, command line, versions.
- Preserve existing code; inspect before editing.

## Environment
- Primary dev host: owner's Apple Silicon MacBook. Run `scripts/inspect-env.sh` to (re)record `docs/environment/`.
- Only one physical machine is currently available (see `docs/BLOCKERS.md`).
