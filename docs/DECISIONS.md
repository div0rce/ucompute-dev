# Decisions

Short decisions inline; anything architectural gets an ADR in `docs/decisions/`.

## ADR index
| ADR | Title | Status |
|---|---|---|
| ADR-0001 | Core stack versions and project license (`MIT OR Apache-2.0`) | Proposed |
| ADR-0002 | llama.cpp binding strategy | Not written — needs inspection of existing Mac repo |
| ADR-0003 | Peer transport (quinn+rustls pinned keys vs iroh 1.0) | Not written — M3 |

## Decision log
| ID | Date | Decision | Rationale |
|---|---|---|---|
| D-001 | 2026-10-14 | `docs/REQUIREMENTS.csv` is the single ledger; `docs/requirements/REGISTRY.md` is generated, never hand-edited | Spec section 2 requires one authoritative registry; CSV is diffable and machine-checkable |
| D-002 | 2026-10-14 | Generator preserves ledger columns (status, evidence, blocker, commit) from the existing CSV | Regenerating design fields must never erase verification progress |
| D-003 | 2026-10-14 | Registry milestones follow spec section 14; validator rejects a dependency placed in a later milestone | Keeps "dependency-ordered" checkable |
| D-004 | 2026-10-14 | Single-host multiprocess tests are named and recorded `SINGLE-HOST`; they cannot satisfy two-machine criteria | Owner instruction 2026-10-14; spec section 10 simulation vs hardware |
| D-005 | 2026-10-14 | M0 report is a baseline with preserved markers; claims are re-verified before use, never silently upgraded | Owner instruction 2026-10-14 |
| D-006 | 2026-10-14 | User-reported or other-agent claims enter the ledger as at most `IMPLEMENTED_UNVERIFIED` | Spec section 21: per-item evidence |
| D-007 | 2026-10-14 | llama.cpp `rpc-server` never bound beyond loopback; cross-host traffic only via Common's authenticated encrypted transport | llama.cpp's own security docs (COMPETITIVE_MATRIX) |
| D-008 | 2026-10-14 | No remote repository and no push without explicit written owner authorization | Spec section 4; owner instruction |
