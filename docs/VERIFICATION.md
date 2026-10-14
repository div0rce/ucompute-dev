# Verification

How evidence is recorded, and the current state of each quality gate (spec section 11).

## Evidence entry format
```
### <REQ-ID or GATE> — <short title>
- Date / commit: YYYY-MM-DD / <git sha>
- Host: <hostname> (<chip>, <memory>, <OS>)   label: REAL | SINGLE-HOST | SIMULATED
- Command: <exact command>
- Result: PASS | FAIL (+ numbers)
- Artifacts: <paths under benchmarks/results/ or tests/ output>
```
A row in `docs/REQUIREMENTS.csv` may be marked `VERIFIED` only when its `observed_evidence` points to an entry here covering every acceptance criterion.

## Quality gates
| Gate | Scope | State | Evidence |
|---|---|---|---|
| A — Build | clean build, lint, fmt, dependency/license scan, secrets check | Not assessed | — |
| B — Inference | real load, generation, streaming, cancellation, cleanup | Not assessed in this ledger (existing Mac repo reports partial; B-009) | — |
| C — Network | discovery, auth, revocation, disconnects, malformed messages | Not started | — |
| D — Pooling | genuine two-node partitioned execution, correctness, memory, failure semantics | Not started; real verification blocked (B-002) | — |
| E — Product | install, select, run, pair, pool, diagnose without developer | Not started | — |
| F — Public swarm | adversarial review, accounting, privacy, abuse, churn | Not started; disabled by default | — |
| G — Release | signed/checksummed, reproducible, compat matrix, CI, docs, benchmarks | Not started (B-007, B-008) | — |

## Evidence log

### REGISTRY — requirements ledger integrity
- Date: 2026-10-14 · commit: n/a (import kit, not yet in repo)
- Host: Claude cloud workspace (Linux x86_64) · label: REAL (tooling only; no product code)
- Command: `python3 scripts/validate_registry.py && python3 -m unittest discover -s tests/registry -v`
- Result: PASS — 148 P0 rows; NOT_STARTED=143, IN_PROGRESS=3, IMPLEMENTED_UNVERIFIED=2, others 0. Negative tests confirm the validator rejects unevidenced VERIFIED, missing rows, unknown statuses, cycles and stale generated files.
- Scope: proves ledger structure only. Proves nothing about the product.

### PHASE-1 — environment inspector
- Date: 2026-10-14 · label: Linux branch executed on cloud workspace; **macOS branch never executed**
- Command: `scripts/inspect-env.sh <dir>`
- Result: Linux report produced. macOS output unknown until run on the Mac.
