# Blockers

Each blocker: affected requirements, exact reproduction, next action. Remove only when resolved, with a PROGRESS entry.

| ID | Blocker | Affects | Reproduction / evidence | Next action | Owner |
|---|---|---|---|---|---|
| B-001 | The claude.ai chat that holds the spec/research has no shell or folder access on the Mac (device linked, `connectedFolders: []`, no device shell tool) | Phase 1, reconciliation, all Mac-side builds from that chat | `get_device_info` on 2026-10-14 returned `mous-2-local`, darwin/arm64, no connected folders | Owner attaches the repo folder to the chat in the Claude desktop app, or runs the import on the Mac with the agent that has a shell | Owner |
| B-002 | Only one physical machine | F-03-01, F-03-02, Gate D (real), all multi-node benchmarks (two-node LAN, three-node heterogeneous) | Owner statement 2026-10-14 | Second machine (any OS) on the same LAN; until then only `SINGLE-HOST` tests | Owner |
| B-003 | No physical iOS/Android device | F-10-12, X-02 | — | Provide device or mark verification pending | Owner |
| B-004 | No Windows or Linux desktop test host for installers | F-10-01, F-10-07, F-10-11 cross-platform, Gate G compatibility matrix | — | VMs or CI runners (GitHub Actions needs remote + push authorization, see B-007) | Owner |
| B-005 | No confidential-computing hardware (H100/H200/Blackwell CC, TDX/SEV-SNP) | F-06-11 experiment | — | Rental requires explicit cost approval (spec section 4) | Owner |
| B-006 | Real-money settlement requires counsel | F-07-11 operation, F-07-12 final determination | FRONTIER_FEASIBILITY (e) | Engage qualified counsel before any monetary feature leaves the disabled gate | Owner |
| B-007 | No remote repository / push authorization | CI (GitHub Actions), cross-platform CI, release signing pipeline | Owner instruction 2026-10-14 | Owner creates the remote and authorizes pushes in `docs/DECISIONS.md` | Owner |
| B-008 | Code-signing identities (Apple Developer ID, Windows cert) absent | Signed installers (F-10-01, F-10-07, Gate G) | — | Unsigned but checksummed builds until provided | Owner |
| B-009 | Existing Mac repo evidence not reconciled into the ledger | F-01-02, F-01-04 and any other rows that repo covers | Its `STATUS.md` was not available to the registry author | Inspect repo, map each claim to a requirement ID, attach evidence or downgrade | Next agent on the Mac |
