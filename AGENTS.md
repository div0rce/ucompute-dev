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
scripts/fetch-fixtures.sh                     # pinned llama.cpp engine + tiny fixtures into ./.common (idempotent)
COMMON_HOME=$PWD/.common cargo test           # unit + real-inference tests (daemon crates; desktop is built separately)
cd apps/desktop && pnpm install && pnpm test  # desktop UI tests (vitest + axe)
```

## Code map (M1)
- `crates/common-core` — stable error codes (F-08-03), `COMMON_HOME` layout, execution manifest (X-15), timeouts.
- `crates/common-hardware` — `probe()` (F-01-01). `crates/common-models` — GGUF reader, memory/context/quant planning.
- `crates/common-artifacts` — catalog (`catalog/models.json`), content-addressed store, verified resumable downloads, pinned engine provisioning (`catalog/backends.json`).
- `crates/common-runtime` — `InferenceBackend` trait, llama.cpp adapter = supervised pinned `llama-server` on 127.0.0.1 (ADR-0002), `ModelManager`, conformance suite.
- `crates/common-api` — axum OpenAI-compatible API on 127.0.0.1:8192 (+ `/api/*`, `/metrics`, `/openapi.json`). `crates/common-cli` — the `common` binary.
- `apps/desktop` — Tauri 2 + React shell over the API; bundles `common` as a sidecar.
- `.recovery/codex-m1/` (ignored) and `refs/recovered/codex-m1/*` — the earlier Python slice, kept as correctness oracle.
- Architecture decisions: ADR-0002 (engine), ADR-0003 (peer transport, proposed). Threat model: `docs/security/THREAT_MODEL.md`.

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
- Primary dev host: owner's MacBook Air (Apple M2, 8 GB unified memory, 10-core GPU, macOS 26.2). Run `scripts/inspect-env.sh` to (re)record `docs/environment/`. Disk is tight (B-010); the owner authorized deleting caches when needed.
- Test assets live in `COMMON_HOME` (default `./.common`, git-ignored): `backends/llama.cpp/b11609/`, `models/blobs/sha256-*`, `models/manifests/*.json`.
- Only one physical machine is currently available (see `docs/BLOCKERS.md`).
