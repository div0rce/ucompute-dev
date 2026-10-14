# Blockers

Each blocker: affected requirements, exact reproduction, next action. Remove only when resolved, with a PROGRESS entry.

| ID | Blocker | Affects | Reproduction / evidence | Next action | Owner |
|---|---|---|---|---|---|
| ~~B-001~~ | RESOLVED 2026-10-14: this session runs with a shell on the Mac (Claude Code desktop); Phase 1 executed (`docs/environment/`) | — | — | — | — |
| B-002 | Only one physical machine | F-03-01, F-03-02, Gate D (real), all multi-node benchmarks (two-node LAN, three-node heterogeneous) | Owner statement 2026-10-14 | Second machine (any OS) on the same LAN; until then only `SINGLE-HOST` tests | Owner |
| B-003 | No physical iOS/Android device | F-10-12, X-02 | — | Provide device or mark verification pending | Owner |
| B-004 | No Windows or Linux desktop test host for installers | F-10-01, F-10-07, F-10-11 cross-platform, Gate G compatibility matrix | — | VMs or CI runners (GitHub Actions needs remote + push authorization, see B-007) | Owner |
| B-005 | No confidential-computing hardware (H100/H200/Blackwell CC, TDX/SEV-SNP) | F-06-11 experiment | — | Rental requires explicit cost approval (spec section 4) | Owner |
| B-006 | Real-money settlement requires counsel | F-07-11 operation, F-07-12 final determination | FRONTIER_FEASIBILITY (e) | Engage qualified counsel before any monetary feature leaves the disabled gate | Owner |
| B-007 | No remote repository / push authorization | CI (GitHub Actions), cross-platform CI, release signing pipeline | Owner instruction 2026-10-14 | Owner creates the remote and authorizes pushes in `docs/DECISIONS.md` | Owner |
| B-008 | Code-signing identities (Apple Developer ID, Windows cert) absent | Signed installers (F-10-01, F-10-07, Gate G) | — | Unsigned but checksummed builds until provided | Owner |
| ~~B-009~~ | RESOLVED 2026-10-14: Codex slice inspected; its 17 tests (4 real-inference) re-run and passed on this Mac; golden output reproduced through the pinned `llama-server`; history imported as `refs/recovered/codex-m1/*`; see VERIFICATION.md `RECOVERY-CODEX-M1` | — | — | — | — |
| B-010 | Development Mac has 8 GB unified memory and had 9.5 GiB free disk at session start (17 GiB after cache cleanup authorized by the owner) | Large-model tests (>4 GB GGUF), concurrent Tauri + Rust builds | `df -h` 2026-10-14 | Keep fixtures small; evict `target/` and model blobs before large downloads; owner may free more | Owner |
| B-011 | TinyStories fixture weights (stories15M/260K) have no verified license; the model card says CI/demo use only | Redistribution of fixtures inside installers (F-10-01), X-08 | `docs/research/TEST_MODELS.md`; Codex STATUS.md | Fixtures are downloaded at test time, never bundled; find a licensed tiny fixture or get upstream clarification | Owner |

