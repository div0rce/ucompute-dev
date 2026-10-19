# Security Policy

Common runs AI models on your own hardware and, when you choose, on devices you trust. Security problems in it can expose prompts, model files or your computer, so we take reports seriously.

## Reporting a vulnerability

- Email the maintainers (see the repository's GitHub profile) or open a **private** GitHub security advisory on `div0rce/common`. Do not open a public issue for an unpatched vulnerability.
- Include: affected version or commit, platform, steps to reproduce, impact, and any proof-of-concept.
- Maintainer response targets are acknowledgement within 7 days and a status update at least every 14 days; these are volunteer targets, not a contracted SLA. Before external beta, maintainers must verify the private reporting route and name a responsible responder.
- Please give us 90 days before public disclosure unless we agree otherwise.

## Scope

In scope: everything in this repository (daemon, CLI, API, desktop app, protocols, packaging) and the way Common invokes the pinned inference engine.

Out of scope: vulnerabilities in third-party models' *behaviour*, and issues in the upstream inference engine (llama.cpp) unless Common's integration makes them exploitable. Report upstream issues upstream; tell us too so we can re-pin.

## Secure defaults you can rely on

See `docs/security/THREAT_MODEL.md`. In short: the local API binds `127.0.0.1` only; the inference engine's port is never exposed; no peer traffic without authentication and encryption; no off-device execution without explicit authorization; no prompt text in logs or metrics; model files are hash-verified before load.

## Trust boundaries and launch readiness

A trusted compute provider sees workload data it processes; encrypted transport does not provide confidential execution. Same-user processes are within the present local OS trust boundary. Backend process supervision provides crash containment, not an arbitrary-code sandbox or verified tenant isolation. Public contribution and arbitrary downloaded programs remain disabled until their separate gates are satisfied.

Unix 0700/0600 checks do not establish Windows ACL protection. Windows native multi-user access tests remain a Mesh promotion obligation (B-017). Signing/notarization and reproducible build provenance are separate from a checksum: an ad-hoc signed Mac artifact is not a publicly trusted installer. The current residual risks and the new provider/consumer threats are recorded in [the threat model](docs/security/THREAT_MODEL.md).

Before external distribution, test the private advisory/contact route with a harmless report, assign an incident owner/backup, document supported versions, rehearse patch/revocation/rollback and retain only redacted incident evidence. Do not promise a staffed SLA or security certification before it exists. Publication and infrastructure authorization remain separate from passing technical checks.
