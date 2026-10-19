# Contributing to Common

Thanks for helping build the VLC of AI compute. This project is Apache-2.0 licensed; by contributing you agree your contribution is licensed the same way.

## Ground rules

1. **Real over fake.** No mocked inference presented as real, no fabricated benchmark numbers, no feature marked done without evidence. Read `AGENTS.md`; it applies to humans too.
2. **Requirements are tracked.** Every feature maps to a row in `docs/REQUIREMENTS.csv`. Status changes need evidence in `docs/VERIFICATION.md`. Run `python3 scripts/validate_registry.py` before you commit.
3. **Secure by default.** Never add a plaintext peer transport, a non-loopback default bind, or anything that sends prompts off-device without explicit consent.
4. **Licenses.** Do not copy code from GPL/AGPL projects. New dependencies must be permissively licensed (see ADR-0001) and justified in the PR.

## Development

```sh
cargo build
cargo test                     # unit tests
scripts/fetch-fixtures.sh      # pinned engine + tiny fixtures into ./.common
COMMON_HOME=$PWD/.common cargo test   # also runs real-inference tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

Decisions that change architecture get an ADR in `docs/decisions/`. Keep PRs focused; include tests; explain what you verified and on which hardware.

## Tests that need hardware

Tests that need a second physical machine, a GPU we do not have, or a mobile device are labelled in their names and skip with a message when the hardware is absent. Never make them pass by faking the hardware.

## Maintainers, decisions and releases (program extension, 2026-10-18)

Contributors retain ownership of their work and contribute under the existing Apache-2.0 terms. No copyright assignment, additional CLA or license change is introduced. Check the exact upstream revision and component license before importing code; a repository name is not a license determination. The dated research refresh records current Exo separately from historical ex-exo.

The repository owner is the current decision/merge authority; subsystem owners in the ledger are responsibilities, not a claim that a staffed team exists. A maintainer reviews correctness, security, evidence and license impact before merge. Security or protocol changes need a reviewer other than their author. Explain unresolved objections in the ADR; the owner decides after review, with a recorded rationale. Publishing, pushing, paid infrastructure and settlement still require explicit authorization.

Protocol/schema changes propose an ADR and compatibility/migration tests before implementation. Invite independent implementer review when available; publish conformance vectors under the project's open license, and keep independent clients possible without proprietary accounts or the official desktop. An extension registry must distinguish compatibility from security approval and must never execute an extension merely because it is listed. U-01 is the gate for independent peer interoperability, not the existing schema mirror test.

Use semantic package versions and explicit API/protocol versions. Before a supported release, record supported platforms/backend pins, artifact hashes, source commit, dependency/model notices, compatibility/deprecation policy, known issues and upgrade/rollback instructions. Rehearse installation, data retention, crash cleanup and tampered-update rejection on exact artifacts. The [specification](docs/SPEC_HANDOFF_V2.md) §§30–33 owns release gates and work packages; do not maintain a second roadmap here. No stable or public release is authorized by this text.

Triage bug reports by reproducible impact, not vote count; distinguish security reports, regressions, unsupported configurations and requests. Ask for redacted diagnostics, never private prompts or credentials. Beta targets: first response within seven days and clear escalation/ownership for critical regressions; disclose when volunteer capacity cannot meet them. Use [SECURITY.md](SECURITY.md) for confidential issues and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) for community behavior.

Each PR that changes behavior updates its contract, tests, relevant ledger evidence and user instructions. Good first contributions have a work-package/requirement link, bounded file scope, fixture instructions and a reproducible acceptance check. Maintain examples against the same declared compatibility matrix as the SDKs. Document review/release procedures so a second maintainer can perform a release rehearsal without the founder before stable promotion. The target is two active maintainers; this is not an assertion that they already exist. Any ucompute.org contribution page links to this file, SECURITY.md and CODE_OF_CONDUCT.md; these files, not the site, define contribution pathways.

Funding may include donations, support or optional managed services; useful account-free local/private operation, portable data and open interfaces remain part of the core. Under D-019 (specification §26.5), Run, private Mesh and self-hosted operation stay free and open source; any future compute-credit network uses one balance, owner-chosen per-device sharing modes, owner-granted trusted free access and a transparent platform fee, and earns no one credits for their own workloads; no contribution makes credits require cryptocurrency or blockchain or adds either at this stage, and none adds payments, payouts or transferable credits outside the stages and owner authorizations that section names. Record commercial conflicts in decisions and keep sponsored contributions subject to the same review. UCompute is the program's public-facing name in use (owner directive 2026-10-19, D-018) and ucompute.org, held by the owner, its intended canonical site; naming and trademark clearance remain open, and no trademark clearance or permission to imply endorsement is claimed. Plain descriptive compatibility statements are preferable to certification claims that have not passed conformance.
