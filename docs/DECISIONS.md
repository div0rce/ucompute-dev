# Decisions

Short decisions inline; anything architectural gets an ADR in `docs/decisions/`.

## ADR index
| ADR | Title | Status |
|---|---|---|
| ADR-0001 | Core stack versions and project license | Proposed; license clause superseded by D-009 (owner adopted Apache-2.0) |
| ADR-0002 | llama.cpp binding strategy: supervised pinned `llama-server` on loopback | Accepted 2026-10-14 |
| [ADR-0003](decisions/ADR-0003-peer-transport-and-pairing.md) | Peer identity, transport and pairing | Historical proposal plus §9 implemented M3 profile; M4 carriage remains planned |
| [ADR-0004](decisions/ADR-0004-infrastructure-program.md) | Additive UCompute infrastructure program | Accepted for program integration, 2026-10-18 |

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
| D-009 | 2026-10-14 | Project license is **Apache-2.0** (root `LICENSE`, commit 1ae8424, owner's choice); Cargo manifests say `license = "Apache-2.0"`. ADR-0001's dual-license proposal is superseded | Owner adopted Apache 2.0 before engineering started; Apache-2.0 carries the patent grant ADR-0001 wanted |
| D-010 | 2026-10-14 | Inference engine = pinned prebuilt llama.cpp release supervised as a `llama-server` child process on 127.0.0.1 with a per-process random API key; no in-process FFI in M1 (ADR-0002) | Verified this session: the pinned b11609 server reproduces the Codex golden output; process isolation; no C++ build on the 8 GB dev Mac |
| D-011 | 2026-10-14 | Data directory: `COMMON_HOME` override, else `~/Library/Application Support/Common` (macOS), `$XDG_DATA_HOME/common` (Linux), `%LOCALAPPDATA%\Common` (Windows); models content-addressed as `models/blobs/sha256-<hex>` + `models/manifests/<id>.json`; engines under `backends/<name>/<release>/` | One layout for CLI, API and desktop; content addressing gives F-05-01/F-05-07 for free |
| D-012 | 2026-10-14 | Common API default `127.0.0.1:8192`; a non-loopback bind is refused unless an API key is configured | Spec section 9 secure defaults; F-06-06 test asserts loopback |
| D-013 | 2026-10-14 | Codex M1 history preserved inside this repository as `refs/recovered/codex-m1/*` (imported from the owner's git bundle); the working copy stays in ignored `.recovery/codex-m1/` as correctness oracle | Preserve prior work without rewriting `main`; recovered refs are not pushed by default |
| D-014 | 2026-10-14 | Catalog model URLs are pinned to Hugging Face revision commits and SHA-256; every model carries a license field, `unknown` when not established; the TinyStories fixtures stay `unknown`/non-redistributable | F-05-04, X-08; TEST_MODELS.md flagged the llama2.c weight license as unverified |

| D-015 | 2026-10-18 | Extend the existing specification in §§25–33, preserve original 148 rows/criteria, add four U rows with declared P1/P2 priorities and separate cohort totals; root handoff stays historical | ADR-0004; no competing PRD or milestone renumbering |
| D-016 | 2026-10-18 | UCompute remains a proposed public identity; keep Common products, CLI, paths and wire names; no trademark clearance or license change | Avoid breaking existing installations or presenting a naming hypothesis as clearance |
| D-017 | 2026-10-18 | Current evidence summaries supersede stale single-host-only/M3-not-built summaries; old evidence remains dated and intact | Two-machine/hotspot reports exist; remote execution still does not prove partitioning |
