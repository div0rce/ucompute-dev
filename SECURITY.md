# Security Policy

Common runs AI models on your own hardware and, when you choose, on devices you trust. Security problems in it can expose prompts, model files or your computer, so we take reports seriously.

## Reporting a vulnerability

- Email the maintainers (see the repository's GitHub profile) or open a **private** GitHub security advisory on `div0rce/common`. Do not open a public issue for an unpatched vulnerability.
- Include: affected version or commit, platform, steps to reproduce, impact, and any proof-of-concept.
- You will get an acknowledgement within 7 days and a status update at least every 14 days until the issue is resolved.
- Please give us 90 days before public disclosure unless we agree otherwise.

## Scope

In scope: everything in this repository (daemon, CLI, API, desktop app, protocols, packaging) and the way Common invokes the pinned inference engine.

Out of scope: vulnerabilities in third-party models' *behaviour*, and issues in the upstream inference engine (llama.cpp) unless Common's integration makes them exploitable. Report upstream issues upstream; tell us too so we can re-pin.

## Secure defaults you can rely on

See `docs/security/THREAT_MODEL.md`. In short: the local API binds `127.0.0.1` only; the inference engine's port is never exposed; no peer traffic without authentication and encryption; no off-device execution without explicit authorization; no prompt text in logs or metrics; model files are hash-verified before load.
