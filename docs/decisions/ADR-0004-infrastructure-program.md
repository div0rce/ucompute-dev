# ADR-0004: Extend the platform into the UCompute infrastructure program

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

- **Status:** Accepted for program integration, 2026-10-18; new product behavior remains unimplemented until verified.
- **Amended 2026-10-19:** D-018 supersedes decision 2's “proposed public identity” wording: UCompute is the program's public-facing name in use (owner directive 2026-10-19) and ucompute.org (owner-held) its intended canonical site (spec §§26.1, 29.5); naming and trademark clearance remain open and are not claimed. [Desktop two-machine evidence](../evidence/desktop-two-machine-2026-10-19.md) (af3e37c) supersedes “Windows desktop unbuilt” below and narrows “two packaged UIs pairing” to the remaining F-10-05 checks (still IMPLEMENTED_UNVERIFIED). Decision 4's frozen digest also covers original milestones and dependencies. The context below remains the 2026-10-18 baseline.
- **Amended 2026-10-19 (D-019):** unified compute credits with contributor consent (spec §26.5; economics model §29.4) bind the program. Decision 3 gains a fifth additive row, U-05 (P1, M10). Payment operations under decision 7 stay separately authorized and follow the §26.5 stages: no production payments, contributor payouts or real-money settlement without owner authorization, and no credit transfers or cash redemption before the F-07-11 gates. Run, private Mesh and self-hosted operation stay free and open source; contributed capacity serves credit-funded requests only under its owner's share-for-credits consent, and donate-only capacity never does. Clarified by the owner the same day: self-compute exclusion, trusted-access allowlists without silent paid fallback, bounded payment-failure exposure and purchased credits as prepaid access. Self-hosted credit accounting, cross-network settlement, and ledger federation are outside the current committed scope. Their architecture and economic rules remain undecided. Original rows are unchanged; credits, payments, trusted access, Swarm and Cloud remain unimplemented.
- **Amended 2026-10-22 (D-021):** UCompute is the product's name, superseding D-018's "public-facing name in use"; code identifiers (CLI, bundle and installer names, data folders, headers, crates, wire names) kept the former name for now (renamed by D-028). Naming and trademark clearance remain open and are not claimed.
- **Inputs:** Owner's Operation UCompute directive; clean `integration/recover-codex-m1` at `ddc39f6609a60cdcb50da09136cddbc90324295f`; source, ledger, ADRs, research and committed evidence inspected before edits.

## Context and recovered baseline

The authoritative product document is [SPEC_HANDOFF_V2](../SPEC_HANDOFF_V2.md). Its original sections 1–24 remain intact. The root handoff is a historical input, not a second evolving specification. They already differed in Markdown formatting before this decision; the old “verbatim copy” description was inaccurate.

The ledger contains 148 original P0 requirements: 36 VERIFIED, 8 IMPLEMENTED_UNVERIFIED, 3 IN_PROGRESS, 101 NOT_STARTED, zero BLOCKED or INFEASIBLE_WITH_EVIDENCE. A blocker may exist on an implemented row without its status being BLOCKED. Counts describe recorded acceptance, not a fresh product certification.

Source inspection confirms eleven daemon crates, one llama.cpp backend, a thin Tauri desktop, explicit local/peer API targets, authenticated QUIC transport and default-deny policy. The scheduler, distributed executor, metering and orchestration crates listed in the original architecture do not yet exist. Existing types and policy switches do not prove those capabilities.

| Evidence / commitment | What it establishes | Boundary retained |
|---|---|---|
| ADR-0002; recovered Python slice and M1 evidence | Reuse supervised pinned engine; real CPU/Metal generation | No custom transformer engine or runtime rewrite |
| Desktop fixes `5f0c580`, `08e267e`; packaged verification | Older exact macOS artifact passed lifecycle and real-inference checks | Does not qualify current HEAD or a fresh-user UI-only journey |
| `bf40e20`; [two-machine evidence](../evidence/m3-two-machine-2026-10-18.md) | Mac M2 and Windows x64 Arc/Vulkan, bidirectional remote requests, privacy, revocation, recovery | Whole requests, not model partitioning; Windows desktop unbuilt |
| `88183a9`, `1a36b3c`; hotspot run | Two physical devices discover within the recorded bound; F-02-01 verified | Campus multicast remains blocked; no LAN kernel packet capture |
| `ddc39f6` | Portable sidecar build script and Windows parent-lifecycle implementation | Cross-target argument execution is skipped; target-native packaging still required |
| ADR-0003 §9 | Implemented protocol, hand-mirrored protobuf, JSON generation payloads, signed capabilities | No independent peer implementation; Status is not periodically sent |
| D-009 / LICENSE | Apache-2.0 adopted by owner | No license change; model licenses remain separate |

Unverified work includes second backend conformance, exposed quantization recommendations, real tool-call round trip, default-quota fairness, installers, hardware UI comparison, two packaged UIs pairing and cross-platform accessibility. B-014/B-015, Windows key ACL evidence, signing, Linux hardware, fixture licensing, and public execution gates remain material risks. No new hardware run was performed for this specification task.

Rejected/deferred approaches remain recorded: in-process inference FFI was rejected for M1; iroh/NAT traversal waits for M9 evaluation; keychain awaits signing; public computation, settlement and confidentiality claims wait for separate gates. Do not reinterpret process supervision as an untrusted-code sandbox.

## Decision

1. Extend the existing specification in sections 25–33. Preserve original requirements, acceptance criteria, milestones M0–M14, evidence and code. Planning work packages belong in section 30, not a parallel roadmap. Add one ADR because authority, requirement cohort and protocol evolution are architectural decisions; no separate PRD is needed.
2. UCompute is a **proposed public identity**, pending naming/trademark review. Keep Run/Mesh/Swarm/Cloud as the product hierarchy and the CLI name, the home variable, crate names, protocol names and repository paths unchanged (superseded by D-028). Core, Protocol, Scheduler and SDK name subsystems, not competing product editions.
3. Keep a single CSV and schema. Add U-01..U-04 for independent protocol implementation, a versioned workload contract, evidence-scoped participation and independent developer integration. P1 means required for the associated launch gate; P2 means committed later work, not a prerequisite for local/private alpha. Original P0 means mandatory full-product scope, not simultaneous execution or an alpha blocker. Report original/additional/total status counts separately.
4. Protect original IDs, priorities, descriptions and acceptance criteria with a baseline digest in the registry validator. Keep every original ledger field unchanged in this task. Changes to original acceptance require a future explicit decision; regeneration cannot silently weaken it.
5. Existing HTTP clients are the first integration surface. Document peer semantics and conformance vectors before promising a stable independent peer protocol. Add a workload envelope only around a proven inference path, with unsupported kinds rejected before execution. No arbitrary program runner is authorized by this decision.
6. Preserve F-03-03's exact bytes/token criterion. ADR-0003's proposed RPC carriage may establish F-03-01/02/04 but cannot automatically satisfy F-03-03. Keep it open until measured native framing meets the original criterion or an owner-authorized amendment separately preserves its history. A partial pooling experiment is not completion of M4 or Gate D.
7. Release cohorts are limited, evidence-qualified promotions within existing milestones. They do not waive full-product P0 scope. Public publication, infrastructure, trademark registration, outreach and payment operations remain separately authorized actions.

## File-by-file integration map (completed before modifications)

| File | Responsibility / change |
|---|---|
| `docs/SPEC_HANDOFF_V2.md` | Extend the authoritative product/program contract, device roles, architecture, work packages, gates, economics and metrics |
| `docs/decisions/ADR-0004-infrastructure-program.md` | This context map and integration decision; the only new narrative document |
| `scripts/registry_data.py` | Add design/test/acceptance fields for four new requirements; original tuples unchanged |
| `scripts/gen_registry.py` | Parse additive IDs/priorities; retain ledger fields; generate cohort totals |
| `scripts/validate_registry.py` | Validate additive cohort while protecting original scope and dependency rules |
| `tests/registry/test_validator.py` | Negative and preservation tests for the schema extension |
| `docs/REQUIREMENTS.csv` | Single ledger, additive rows only |
| `docs/requirements/REGISTRY.md` | Regenerated view, never hand-maintained |
| `docs/api/README.md` | Current protocol boundaries and planned developer/independent implementation contracts |
| `docs/security/THREAT_MODEL.md` | Dated corrections and future workload/provider/consumer/isolation threats |
| `CONTRIBUTING.md` | Maintainer, protocol, compatibility, release and contributor governance |
| `SECURITY.md` | Disclosure readiness, realistic response targets and trust boundaries |
| `docs/research/COMPETITIVE_MATRIX.md` | Dated primary-source refresh without erasing M0 uncertainty |
| `docs/research/SOURCES.md` | New source provenance and access dates |
| `docs/research/RESEARCH_GAPS.md` | Audience comparison, falsifiable adoption/distribution hypotheses |
| `docs/research/README.md` | Distinguish imported M0 from subsequent research |
| `README.md` | Accurate current capability summary and program entry point |
| `AGENTS.md` | Restart authority, original/additional cohorts and current hardware limits |
| `docs/DECISIONS.md` | Fix ADR-0003 index and link this decision |
| `docs/BLOCKERS.md` | Correct resolved discovery/remote facts; add protocol/ACL/release evidence blockers |
| `docs/PROGRESS.md` | Current counts, exact checks and next executable task |
| `docs/VERIFICATION.md` | Current gate summary, program validation, adversarial findings and limits |

## Consequences

The next implementation is Windows native desktop qualification, followed by Linux runtime/install coverage; current discovery evidence is reused. Default peer fairness and Windows key protection must be resolved before broad Mesh promotion. Pooling and SDK work can proceed independently after their own prerequisites. Neither commercial demand nor competitive superiority has been demonstrated.

Validation must show original-row preservation, unique IDs, dependency consistency, generated-file agreement, valid new document references and zero application-code changes. An independent review must challenge systems, security, product, maintainability, economics, competition and acceptance. Record rejected hypotheses as well as successes.
