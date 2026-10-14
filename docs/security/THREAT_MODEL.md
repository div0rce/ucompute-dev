# Threat Model — Project Common

- **Requirement:** F-06-06 (M0). Companion test: `default_config_binds_localhost` in `crates/common-api` (written separately).
- **Status:** living document; revise at every milestone that adds a trust zone (M3, M7, M9, M10, M12).
- **Inputs:** `docs/SPEC_HANDOFF_V2.md` §7, §9, §18.6; `docs/REQUIREMENTS.csv`; ADR-0001; ADR-0002; `docs/research/COMPETITIVE_MATRIX.md` (llama.cpp RPC notes); `AGENTS.md`; decisions D-004, D-007; blockers B-002, B-005, B-006, B-007, B-008, B-011.
- **Method:** STRIDE per actor. S spoofing, T tampering, R repudiation, I information disclosure, D denial of service, E elevation of privilege.
- **Reading the mitigation tables.** `Requirement` is an exact ID from `docs/REQUIREMENTS.csv`; `—` means no row covers the mitigation (collected in §5.2). `Milestone` is the row's milestone. `Today` is the state in this branch: `present` (code + passing test), `stub` (types/doc exist, logic unimplemented), `none`.

Only M0 and M1 are in progress. Everything in M3 or later is **not mitigated today**; the product today is Mode A only (local, no network), and the only network listener is the loopback API.

## 1. Scope, trust zones and assets

### 1.1 System as built (M1)

```
client (CLI, OpenAI SDK, desktop later)
   │ HTTP, 127.0.0.1:8192, optional API key
   ▼
common-api  ── admission control, typed errors, metrics ──┐
   │ HTTP on loopback, per-process random --api-key        │
   ▼                                                        ▼
llama-server (pinned b11609, 127.0.0.1:<ephemeral>,    $COMMON_HOME/
  --no-webui --offline --metrics, scrubbed env)           backends/   models/{blobs,manifests,partial}/
                                                          runs/  logs/  identity/ (M3)
```

The engine is a child process (ADR-0002). Its port is never exposed; its API key is generated per process and never persisted. Model and engine bytes are hash-verified against `catalog/models.json` and `catalog/backends.json` before use.

### 1.2 Trust zones

| Zone | Who is in it | Execution modes (spec §7) | Trust assumption | Enabled today |
|---|---|---|---|---|
| Z0 Local device | This OS user, this daemon, the engine child process | A | Trusted. Other users/processes on the same host are **not** in Z0 (§2.1) | yes |
| Z1 Trusted peers | Devices paired by the owner (SPAKE2 code, pinned Ed25519 keys), grouped (F-02-09) | B, C, D, E | Authenticated, encrypted, authorized per group. Peer is trusted to **see** the work it executes (§4) | no (M3) |
| Z2 Public swarm | Unpaired, pseudonymous contributors and requesters | F | Hostile by default. Every message is attacker-controlled | no (M9/M10, Gate F) |
| Z3 Managed cloud / enterprise | Cloud burst targets, enterprise federation, SSO IdP | G | Contractual; explicit cost and policy consent per use | no (M12) |

Data crosses a zone boundary only through Common's authenticated, encrypted transport (F-02-03, F-06-03). There is no plaintext peer path and llama.cpp's `rpc-server` is never reachable beyond loopback (D-007, `AGENTS.md`).

### 1.3 Assets

| Asset | Where | Confidentiality | Integrity | Availability |
|---|---|---|---|---|
| Prompts, activations, outputs | API request/response; engine memory; activation frames (M4) | high — user content | high — silent alteration = wrong answers | medium |
| Model weights + manifests | `models/blobs/sha256-*`, `models/manifests/*.json`, `catalog/*.json` | low–medium (license-dependent, X-08) | **critical** — wrong weights = silent model substitution | medium |
| Engine binaries + pins | `backends/<name>/<release>/`, `catalog/backends.json` | low | **critical** — runs as the user | medium |
| Device identity keys (M3) | `identity/`, OS keychain where available | **critical** | critical | medium |
| API keys | Common API key (config); engine key (process memory only) | high | — | — |
| Audit logs (M3) | append-only local log | medium (metadata, no prompts) | high | low |
| Credits ledger (M10) | local SQLite, hash-chained | low | high | low |
| User hardware, time, energy | CPU/GPU/RAM/disk/battery | — | — | **high** — the product's whole value exchange |

## 2. Actors and threats

### 2.1 Local attackers (other processes or users on the same machine)

**Vectors.** Connect to the API or engine port from another local user; read model cache, logs or identity keys from the data directory; plant `DYLD_*`/`LD_*`/`GGML_*`/`LLAMA_ARG_*` environment to alter the engine; race the hash check (§5.1); fill disk under `models/partial/`.
**STRIDE.** S (impersonate the local client), I (keys, prompts in logs), T (cache), E (env injection into the engine), D.
**Impact.** Full read of prompts and keys; arbitrary code in the engine via dynamic-loader env; silent model swap.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| API binds 127.0.0.1 unless configured; non-loopback bind without API key refused with `PolicyDenied` | F-06-06, F-09-01 | M0/M1 | stub (`ApiConfig::default()`); test being written |
| Engine on 127.0.0.1:ephemeral with per-process random `--api-key`; port never exposed | — (ADR-0002 §4; gap 1) | M1 | stub |
| Engine spawned with scrubbed environment (`DYLD_*`, `LD_*`, `GGML_*`, `LLAMA_ARG_*` removed) | — (ADR-0002 §1; gap 2) | M1 | stub |
| Identity private key in OS keychain or 0600 file; never logged | F-06-01 | M3 | none |
| Prompts absent from metrics, errors and diagnostics bundles | F-09-03, F-08-03, F-06-10 | M1 / M1 / M3 | stub |
| Model blob re-hashed against manifest before load | F-05-01, F-01-05 | M1 | stub (`Store::verify`) |
| Hardware probe runs unprivileged (no setuid helpers) | F-01-01 | M1 | stub |

**Not mitigated.** Same-user local processes: anything running as the same OS user can read `$COMMON_HOME` and connect to loopback; this is the OS trust boundary and Common does not claim to defend against it. Data-directory permissions (0700) are not required by any row (gap 7).

### 2.2 Malicious model file

**Vectors.** A GGUF crafted to crash or exploit the engine's parser (header, tensor dims, tokenizer vocab); a file whose size claim does not match content; a chat template containing injection text; an oversized context claim driving OOM.
**STRIDE.** T, D, E (parser memory-safety bug in llama.cpp), I (prompt exfiltration via template is out of scope — the model sees the prompt by design).
**Impact.** Engine crash (contained by process isolation), machine OOM, code execution inside the engine process.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Common parses GGUF metadata in Rust before handing the file to the engine; truncated/corrupt files return `ModelFileInvalid`, never crash (1,000-mutation fuzz) | F-01-02 | M1 | stub (`common-models/gguf.rs`) |
| Only hash-verified blobs reach the engine; untrusted import is explicit (`Store::import`) | F-05-01, F-01-05 | M1 | stub |
| Memory feasibility and context planning reject infeasible loads before spawn (`InsufficientMemory`, `ContextTooLong`) | F-01-03, F-01-08 | M1 | stub |
| Engine is a separate process: a crash surfaces as `BackendFailed`, not daemon death | F-01-06 (ADR-0002) | M1 | stub |
| Load/first-token timeouts bound a hung parser | F-08-01 | M1 | present (`Timeouts`, `backoff_is_bounded`) |
| Remote workloads run in a low-privilege sandboxed worker | F-06-08 | M7 | none |

**Not mitigated.** A memory-safety bug in llama.cpp's loader runs as the user; the M1 process boundary limits blast radius to the daemon's data, not the user's account. Sandboxing (F-06-08) is M7.

### 2.3 Malicious metadata (GGUF, manifest, catalog)

**Vectors.** Catalog entry with traversal in `id`/`file_name`; manifest whose `sha256` does not match the referenced blob; a `license`/`redistribution` field falsified to permit seeding; URL pointing to a mutable branch instead of a revision; archive member paths (`../`, absolute, symlink-out) in an engine release tarball; a wrong `bytes` field to defeat size bounds.
**STRIDE.** T, E (path traversal writes outside the data dir), R (license provenance).
**Impact.** Arbitrary file write on extraction; wrong model served under a trusted name; license violation.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Names must pass `common_core::paths::safe_name` (single component, no leading dot, ASCII set) | F-01-05 (`path_traversal_in_name_rejected`) | M1 | **present** (`safe_names` test) |
| Manifest hash is authoritative; mismatch fails the load; schema validated | F-05-01 | M1 | stub |
| Catalog URLs pinned to immutable revisions; manifest records `source_url` | F-05-10 | M6 | stub (doc comment only) |
| Engine archive: SHA-256 and byte count checked before extraction; members rejected on traversal/absolute/symlink-out; every extracted file re-verified against the archive | — (ADR-0002 §1–2; gap 3) | M1 | stub (`ensure_backend`, `verify_backend`) |
| License recorded, `unknown` never omitted; redistribution policy derived from license, defaults to `unknown` = never served | F-05-04, X-08 | M1 / M6 | present (schema default) / none |
| Catalog embedded at build via `include_str!` — not user-writable at runtime | F-10-02 | M2 | present (mechanism), catalog UI none |

**Not mitigated.** Catalog and manifests are not signed; integrity rests on the hash and on the repository build (F-05-01 `manifest_signature_or_hash_required` accepts hash alone). A remotely fetched catalog (M2+) would need signing (gap 8).

### 2.4 Model supply-chain attacks

**Vectors.** Compromised upstream release (llama.cpp binary or GGUF host); MITM on download; typosquatted model repo; vulnerable or license-incompatible Rust dependency; tampered desktop update; fixture weights with unclear license (B-011).
**STRIDE.** T, S (fake upstream), E (malicious binary runs as user).
**Impact.** Full compromise of the user account via the engine binary; silent model substitution; legal exposure.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Engine pin (release, source commit, URL, SHA-256, bytes) in `catalog/backends.json`; bump re-runs golden-token test | — (ADR-0002 §2, §Consequences; closest F-06-09 `unknown_model_hash_warns`) | M1 | present (pin) / stub (verify) |
| Model downloads require `https://`; SHA-256 verified; one flipped byte deletes the partial | F-01-05 (`tls_required_for_remote`, `corrupted_byte_rejected`) | M1 | stub |
| Unverified (unpinned) model files require explicit confirmation | F-06-09 | M6 | none |
| `cargo-deny` license allowlist + `cargo-audit` in CI | F-06-09 (ADR-0001 §2) | M6 | none (CI blocked, B-007) |
| Execution manifest pins model hash, backend version, quant, seed | X-15 | M1 | stub (`common-core/manifest.rs` roundtrip present) |
| Signed/checksummed installers; updater verifies Ed25519 signature; unsigned update rejected; rollback | F-10-01, F-10-07 | M2 / M13 | none (B-008: no signing identities) |
| Fixtures downloaded at test time, never bundled | — (B-011) | M1 | policy only |

**Not mitigated.** No code signing (B-008). No reproducible-build verification of the upstream llama.cpp binary — the pin trusts GitHub Releases at pin time. No CI dependency scanning until a remote exists (B-007).

### 2.5 Prompt / activation leakage

**Vectors.** Prompt written to engine stdout/log, API access log, metrics label, error message or diagnostics bundle; execution record (`runs/<id>/`) storing the prompt; scheduler routing a `local-only` request off-device; a peer in Z1 executing a layer range and retaining activations; relay or network observer reading plaintext; opt-in analytics shipping raw events.
**STRIDE.** I (primary), R (no record that data left the device).
**Impact.** Disclosure of user content to local log readers, peers or network observers.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| `/metrics` carries counters and histograms only, no prompt content | F-09-03 | M1 | stub |
| Errors redact secrets and never echo request bodies | F-08-03 (`errors_redact_secrets`) | M1 | present (code snapshot) / redaction stub |
| Engine runs without `--verbose`; API does not log request bodies | — (gap 4) | M1 | stub |
| Default-deny: with default config no request leaves the device | F-06-02 | M3 | none (Mode A only today, so trivially true) |
| Policy class (`local-only` / `trusted-group` / `swarm-allowed`) attached to every request and enforced in scheduler and transport; fuzz of 10,000 requests shows zero violations | F-06-07, I-05, F-04-01 | M3 / M5 / M5 | none |
| Privacy mode selector with disclosure text before non-local modes | F-10-10 | M3 | none |
| Encrypted, mutually authenticated transport; packet capture contains no plaintext prompt; relays see only ciphertext | F-02-03, F-06-03, F-02-06 | M3 / M3 / M9 | none |
| Audit entry for every off-device request; diagnostics bundle scanned for prompts/keys | F-06-10 | M3 | none |
| Contribution history and exports contain no prompt text | F-07-05 | M7 | none |
| Analytics off by default; DP aggregates only | X-14 | M13 | none |
| Attestation-gated confidential backend (research) | F-06-11 | M14 | research (B-005) |

**Not mitigated.** Anything a Z1/Z2 peer computes, it sees (§4). Execution records under `runs/` must not contain prompts; no row asserts this (gap 4).

### 2.6 Malicious peers (unpaired or public-swarm)

**Vectors.** Forged mDNS records; connection with an unpinned key; oversized or malformed capability/activation frames; serving corrupt chunks; returning fabricated outputs; eclipse of DHT routing tables; relay abuse.
**STRIDE.** S, T, I, D.
**Impact.** Wrong results, resource waste, DoS, exposure of the swarm to bad data.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Discovered-but-unpaired peers listed untrusted and never receive workloads; mDNS can be disabled | F-02-01 | M3 | none |
| Connections with unpinned keys refused; no plaintext fallback | F-02-03, F-06-03 | M3 | none |
| Capability messages bounded and versioned; oversize rejected and logged | F-02-04, F-08-10 | M3 | none |
| Activation frames length-prefixed, versioned, bounded; dimension mismatch rejected; stage accepts only authenticated peer | F-03-03, F-03-01 | M4 | none |
| Chunk hash verified on receipt; peer serving bad chunks penalised; non-redistributable models never served | F-05-09, F-05-05, F-05-06, X-08 | M6 | none |
| Signed, expiring discovery records; eclipse simulation documented | F-02-11 | M9 | none |
| Redundant execution / spot checks; cheating worker detected ≥95% within 100 jobs | X-07, F-07-09 | M10 | none |
| Reputation with confidence intervals | F-07-08, X-05 | M10 | none |
| Public swarm disabled until Gate F | F-07-01 (`default_off`), spec §22 | M7 | none (no swarm code exists) |

**Not mitigated.** All of it — Z2 does not exist in the codebase. The default that matters today is that no listener accepts non-loopback traffic.

### 2.7 Compromised trusted peers

**Vectors.** A paired device is stolen, malware-infected or its key exfiltrated; it continues to receive workloads, reads activations of its layer range, returns altered logits, serves itself more quota, or seeds tampered chunks.
**STRIDE.** I, T, E (uses standing trust), R.
**Impact.** Silent exposure of every prompt routed through it; wrong outputs that pass as genuine.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Revocation removes pinned key, closes sessions within 5 s, refuses reconnect, persists, propagates to group | F-06-05 | M3 | none |
| Per-group authorization; workloads never dispatched to non-members | F-02-09, F-06-02 | M3 | none |
| Per-peer quotas limit damage from a greedy trusted peer | F-06-04 | M3 | none |
| Correctness test against local execution catches gross tampering in development, not at runtime | F-03-04 | M4 | none |
| Activation-hash spot checks / redundancy for selected workloads (runtime detection) | X-07, F-06-12 | M10 / M14 | none / research |
| Audit log records which peer handled which request | F-06-10 | M3 | none |

**Not mitigated.** Detection of a compromised peer that behaves correctly but copies activations is impossible without confidential computing (F-06-11, research). Owners must treat pairing as granting read access to whatever they route (§4).

### 2.8 Identity spoofing

**Vectors.** MITM during pairing; replayed pairing transcript; brute-forced short code; forged signed receipt; claiming another device's fingerprint in a discovery record; impersonating the local API client; impersonating an enterprise user.
**STRIDE.** S (primary), E.
**Impact.** Attacker becomes a trusted peer or bills work to another identity.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| SPAKE2 pairing: wrong code fails, MITM fails key confirmation, >5 failures/min locks pairing | F-02-02 | M3 | none |
| Ed25519 device identity; fingerprint is the peer ID; key persists across restart | F-06-01 | M3 | none |
| Mutually authenticated transport with pinned keys | F-02-03 | M3 | none |
| Non-loopback API bind requires an API key; `Unauthorized` on mismatch | — (gap 5; F-09-01 covers default bind only) | M1 | stub |
| Signed, expiring discovery records | F-02-11 | M9 | none |
| Ed25519-signed receipts; forged receipt rejected | X-06 | M10 | none |
| OIDC/SAML SSO and RBAC; role escalation blocked | X-19 | M12 | none |

### 2.9 Sybil attacks

**Vectors.** One operator creates many identities to dominate scheduling, out-vote redundancy checks, accumulate credits, eclipse DHT neighbourhoods, or launder a bad reputation.
**STRIDE.** S, T (of accounting), D.
**Impact.** Verification by redundancy becomes meaningless; credit inflation; swarm partition.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Identity cost via pairing/attestation; beta-distribution reputation from spot checks; 30 % Sybils gain <5 % of credited work in simulation | F-07-08 | M10 | none |
| Redundant checks assigned across independent reputations | X-07 | M10 | none |
| Eclipse-attack simulation on discovery | F-02-11 | M9 | none |
| Z1 is Sybil-immune by construction: membership is owner-paired, not self-asserted | F-02-02, F-02-09 | M3 | none |

**Not mitigated.** No Sybil defence exists for Z2; Z2 stays disabled. The anti-Sybil target is a simulation result, not a proof.

### 2.10 Resource exhaustion

**Vectors.** Request flood on the API; unbounded request bodies or queues; many concurrent generations exhausting RAM; max-context prompts; a peer streaming oversized frames; download of a multi-GB file to a 9.5 GiB disk (B-010); bandwidth probes saturating a link; contribution draining a laptop battery; stalled backend holding a slot forever.
**STRIDE.** D (primary).
**Impact.** OOM kill, unusable machine, energy and money cost to the contributor.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Bounded queue (`max_queue` 32), `max_concurrent` 4, queue timeout 30 s → `Overloaded` 429/503; memory bounded at 10× load | F-08-08 | M1 | stub (config present) |
| Bounded request body size at the API | — (gap 6) | M1 | none |
| Timeouts on load, first token, inter-token; bounded retries for idempotent ops only | F-08-01 | M1 | present (policy) / stub (wiring) |
| Cancellation frees KV/context within 200 ms on disconnect | F-08-02 | M1 | stub |
| Feasibility filter refuses loads/contexts that do not fit | F-01-03, F-01-08 | M1 | stub |
| Per-peer token buckets on requests, tokens, bytes, sessions; p95 degrades <20 % under flood | F-06-04 | M3 | none |
| Capability and activation messages size-bounded | F-02-04, F-03-03 | M3 / M4 | none |
| Bandwidth probe capped | F-02-07 | M3 | none |
| Weighted fair queueing across peers | F-07-04 | M7 | none |
| Contribution off by default; CPU/GPU caps, schedules, battery/thermal pause, quiet hours | F-07-01, F-10-08, F-07-03, X-03, X-04, F-07-06 | M7 / M7 / M12 | none |
| Worker memory limit enforced by sandbox | F-06-08 | M7 | none |

**Not mitigated.** Disk exhaustion by downloads has no quota row (cache eviction F-05-03 is M6). Request body bound has no row.

### 2.11 Billing fraud

**Vectors.** Worker claims more compute than performed; requester disputes delivered work; tampering the local ledger; double-spending credits; fabricated receipts; triggering paid cloud without consent; settlement code reached without compliance gate.
**STRIDE.** R (primary), T, S.
**Impact.** Credit inflation, financial loss, legal exposure (money transmission, securities).

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Metered time within ±10 % of OS accounting; tokens counted exactly | F-07-02 | M7 | none |
| Ed25519-signed receipts (inputs hash, model hash, resources, outputs hash) | X-06 | M10 | none |
| Double-entry ledger, hash-chained, tamper detected; credits non-redeemable, UI says so | F-07-07, I-03 | M10 | none |
| Fraud detection via redundancy and spot checks | F-07-09, X-07 | M10 | none |
| Settlement unreachable without operator compliance gate; counsel review precondition | F-07-11 | M10 | none (B-006) |
| Token/crypto feasibility analysis; core works with zero tokens | F-07-12 | M10 | in progress (literature) |
| No paid cloud call without recorded approval matching the estimate | X-09 | M12 | none |
| Contribution history is local and exportable | F-07-05 | M7 | none |

**Not mitigated.** The economic plane does not exist. Nothing in the repo can move money; keep it that way until F-07-11's gate and B-006 are resolved.

### 2.12 Malicious workloads

**Vectors.** A requester sends a job designed to exploit the worker: adversarial GGUF or adapter the worker must load; job parameters that trigger engine bugs; prompt crafted to make the worker's model emit content that incriminates the contributor; jobs that exfiltrate worker files via a buggy backend; browser contributors escaping the WASM sandbox; plugin backends with undeclared capabilities.
**STRIDE.** E (primary), I, D, R.
**Impact.** Contributor's machine compromised or made liable for content it did not choose.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Worker loads only hash-pinned models already in its policy-allowed set; pinned execution manifests, no implicit upgrade | F-05-10, X-15, F-06-09 | M6 / M1 / M6 | none / stub / none |
| Untrusted remote workloads run in a low-privilege sandboxed process; cannot read outside model/cache dirs; killed on memory limit | F-06-08 | M7 | none |
| Every routing decision checks identity, policy and request class; denials logged | I-05 | M5 | none |
| Admission, quotas and timeouts bound the cost of one job | F-08-08, F-06-04, F-08-01 | M1 / M3 / M1 | stub / none / present |
| Browser contributors isolated by origin, limits documented | X-01 | M12 | none |
| Plugins declare capabilities; conformance suite | F-09-07 | M11 | none |
| Contribution scope (groups vs public) chosen at opt-in; audit-logged | F-07-01, F-06-10 | M7 / M3 | none |

**Not mitigated.** Contributor liability for generated content is a policy question, not a technical control; disclosure text (F-10-10, F-10-08) is the only planned measure.

## 3. Secure defaults

Verbatim list. A test suite should be able to assert each item; §Appendix A maps them to tests.

1. **SD-01** The Common API binds `127.0.0.1` by default (`ApiConfig::default().host == "127.0.0.1"`).
2. **SD-02** A non-loopback bind without an API key is refused at startup with `ErrorCode::PolicyDenied`.
3. **SD-03** There is no plaintext peer transport path; peer connections are mutually authenticated and encrypted, with no downgrade negotiation.
4. **SD-04** Off-device execution is default-deny: with default configuration no request leaves the device.
5. **SD-05** Resource contribution is off by default; a fresh install accepts zero external workloads.
6. **SD-06** Public swarm participation is disabled by default and remains disabled until Gate F.
7. **SD-07** Engine ports (`llama-server`, `rpc-server`) bind loopback only, carry a per-process random API key, and are never exposed by Common.
8. **SD-08** Prompts are never logged: not in engine logs, API logs, metrics, error messages, execution records or diagnostics bundles.
9. **SD-09** Model files are SHA-256-verified against their manifest before the engine is started on them; mismatch is `IntegrityMismatch`/`ModelFileInvalid`, not a load.
10. **SD-10** Archive extraction (engine releases) refuses path traversal, absolute paths and symlinks escaping the destination; names pass `safe_name`.
11. **SD-11** The engine process starts with a scrubbed environment: `DYLD_*`, `LD_*`, `GGML_*`, `LLAMA_ARG_*` removed.
12. **SD-12** Request bodies and queues are bounded; excess returns `Overloaded` (429/503), never unbounded memory growth.

## 4. What TLS does not give you

- **A peer that executes a layer range sees the activations for that range.** For the first and last stages that is a thin transform away from the prompt and the output; for middle stages activations are still invertible in practice for many architectures. Encryption in transit (F-02-03) protects against observers on the wire and relays (F-02-06), not against the endpoint. Spec §9: "TLS does not hide prompts from a peer processing them."
- **Local-only is the only private mode.** Modes B–G disclose the workload to every executing peer. Pairing a device (F-02-02) is granting it read access to whatever policy allows routing to it. The privacy selector (F-10-10) and policy engine (F-06-07) exist to make that disclosure explicit and enforce the boundary, not to remove it.
- **Confidential computing (F-06-11) is research.** No attestation path exists; no CC hardware is available (B-005). Nothing in Common may claim confidential execution without verified attestation.
- **Verifiable inference (F-06-12) is research.** Redundancy (X-07) and activation-hash spot checks are statistical detection, not proof. They are never described as cryptographic proof unless they are.
- **Authentication is not authorization.** A valid peer identity (F-06-01) proves who; policy (F-06-02, F-06-07, I-05) decides whether. Both are M3+.

## 5. Residual risks and open questions

### 5.1 Residual risks (accepted or unresolved)

| # | Risk | Why it remains | Owner action |
|---|---|---|---|
| R1 | **TOCTOU between hash verification and engine load.** `Store::verify` re-hashes the blob; the engine then opens the path by name. A same-user writer can swap the file in between. | Loopback-and-same-user is the OS trust boundary; `llama-server` takes a path, not an fd. Mitigation options: open once and hash the open fd, hard-link into a private per-run directory, or verify via `/proc/self/fd` style path. | Decide in M1 implementation; document in `common-artifacts`. |
| R2 | **Single-machine testing only (B-002, D-004).** Every multi-node security property (pairing, revocation, capture tests) can only be exercised `SINGLE-HOST`. | No second physical machine. | `SINGLE-HOST` label mandatory; nothing in §2.6–2.9 is `VERIFIED` until a two-machine run. |
| R3 | **No code signing (B-008), no CI (B-007).** Installers unsigned; dependency audit not automated. | Missing identities and remote. | Checksums only; owner to provide identities and authorize remote. |
| R4 | **Upstream engine binary is trusted at pin time.** The SHA-256 pins what was downloaded, not what the source produces. | No reproducible build of llama.cpp releases. | Consider building from the pinned `source_commit` in CI once B-007 is resolved. |
| R5 | **llama.cpp memory-safety surface.** GGUF loader and `rpc-server` are C++ with known robustness issues (COMPETITIVE_MATRIX: #15055, #13185, #20315; no fuzzer upstream). | Not Common's code. | Process isolation now (ADR-0002); sandbox at M7 (F-06-08); never expose beyond loopback (D-007). |
| R6 | **Same-user local attacker.** Anything running as the user can read `$COMMON_HOME` and reach loopback ports. | Inherent to desktop OS model. | Document in user privacy notes; keychain for keys at M3 (F-06-01). |
| R7 | **Fixture license (B-011).** TinyStories weights have no verified license. | Upstream ambiguity. | Never bundle; download at test time. |
| R8 | **Engine API key in process memory and command line.** `--api-key` passed as argv is visible to same-user `ps`. | llama-server accepts key via argv or env; env is scrubbed, argv is readable by the same user. | Same boundary as R6; prefer env var `LLAMA_API_KEY` if supported by the pinned build and re-add it after scrubbing. |

### 5.2 Requirement gaps (mitigation with no covering row)

1. Engine port loopback-only with per-process random API key (ADR-0002 §4; `rpc-server` D-007) — no row, no test name.
2. Engine environment scrubbing (ADR-0002 §1) — no row.
3. Archive extraction traversal/symlink refusal and post-extraction re-verification of engine binaries — F-01-05 `path_traversal_in_name_rejected` covers catalog names, not tar members.
4. "Prompts never logged" at M1 — F-09-03 covers metrics, F-06-10 (M3) covers bundles; engine stdout, API logs and `runs/` records have no row.
5. Non-loopback bind requires API key; API key comparison and `Unauthorized` — F-09-01 only asserts the default bind.
6. HTTP request body size bound — F-08-08 bounds queue and concurrency, not body bytes.
7. Data-directory permissions (0700 on `$COMMON_HOME`, 0600 on manifests/audit/ledger) — F-06-01 covers only the identity key.
8. Catalog/manifest signing for any remotely fetched catalog — F-05-01 accepts hash alone.
9. Disk quota / partial-download cleanup — F-05-03 (M6) is eviction policy, not a bound.
10. TOCTOU (R1) — no row requires verify-then-load atomicity.

### 5.3 Open questions

- Transport choice (`quinn`+`rustls` pinned keys vs `iroh` 1.0) — ADR-0003, M3. Affects F-02-03, F-02-06, F-06-03 test design.
- Whether `rpc-server` is wrapped (tunnelled through Common's transport) or replaced with Common-native activation frames (F-03-03) — M4. COMPETITIVE_MATRIX recommends not reusing the wire protocol as-is.
- Key storage: OS keychain availability on Linux headless hosts (F-06-01 allows 0600 file fallback).
- Whether redundancy (X-07) tolerances for cross-backend nondeterminism leave a usable detection margin for F-07-09.

## Appendix A — Test mapping for secure defaults

| Default | Test name | Location | Covers | State |
|---|---|---|---|---|
| SD-01 | `default_config_binds_localhost` | `crates/common-api` | F-06-06 | planned (in progress by API agent) |
| SD-01 | `external_interface_not_bound_by_default` | `crates/common-api` (integration: start `serve`, assert only 127.0.0.1 socket) | F-06-06 | planned |
| SD-01 | `api_binds_localhost_by_default` | `crates/common-api` | F-09-01 | planned (may alias SD-01 above) |
| SD-02 | `nonloopback_bind_without_key_is_policy_denied` | `crates/common-api` | gap 5 | planned |
| SD-03 | `no_plaintext_mode_exists`, `downgrade_attempt_fails`, `unpinned_key_rejected`, `packet_capture_contains_no_plaintext_prompt` | `crates/common-network` (M3) | F-06-03, F-02-03 | planned M3 |
| SD-04 | `default_deny`, `unauthorized_peer_request_refused` | `crates/common-policy` (M3) | F-06-02 | planned M3 |
| SD-04 | `local_only_never_leaves_device_fuzz` | `tests/security` (M3) | F-06-07 | planned M3 |
| SD-05 | `default_off`, `no_contribution_without_opt_in` | `crates/common-metering` (M7) | F-07-01 | planned M7 |
| SD-06 | `gate_disabled_by_default` (settlement) plus a swarm-flag default test | `crates/common-metering`, `crates/common-network` | F-07-11; swarm flag has no row (spec §22) | planned M9/M10 |
| SD-07 | `engine_binds_loopback_with_random_key` | `crates/common-runtime` (`llamacpp.rs`) | gap 1 | planned M1 |
| SD-08 | `metrics_contain_no_prompts` | `crates/common-api` | F-09-03 | planned M1 |
| SD-08 | `errors_redact_secrets` | `crates/common-core` | F-08-03 | planned M1 (`error_codes_are_stable` present) |
| SD-08 | `engine_log_and_run_record_contain_no_prompt` | `crates/common-runtime` | gap 4 | planned M1 |
| SD-08 | `diagnostics_bundle_redacted`, `secret_scan_of_bundle` | `crates/common-observability` (M3) | F-06-10 | planned M3 |
| SD-09 | `hash_verify`, `corrupted_byte_rejected`, `manifest_signature_or_hash_required` | `crates/common-artifacts` | F-01-05, F-05-01 | planned M1 |
| SD-09 | `malformed_gguf_rejected_without_crash` | `crates/common-models` | F-01-02 | planned M1 |
| SD-10 | `safe_names` | `crates/common-core/src/paths.rs` | F-01-05 | **present** |
| SD-10 | `path_traversal_in_name_rejected` | `crates/common-artifacts` | F-01-05 | planned M1 |
| SD-10 | `archive_member_traversal_rejected`, `extracted_files_match_archive` | `crates/common-artifacts` (`backend.rs`) | gap 3 | planned M1 |
| SD-11 | `engine_env_is_scrubbed` | `crates/common-runtime` | gap 2 | planned M1 |
| SD-12 | `queue_bound`, `overload_returns_429`, `overload_does_not_oom` | `crates/common-api` | F-08-08 | planned M1 |
| SD-12 | `oversized_body_rejected` | `crates/common-api` | gap 6 | planned M1 |
| SD-12 | `oversized_capability_message_rejected`, `oversized_or_mismatched_dim_frame_rejected` | `crates/common-network`, `crates/common-distributed` | F-02-04, F-03-03 | planned M3/M4 |

Present today: `safe_names`, `error_codes_are_stable`, `backoff_is_bounded`, `every_code_has_status_type_and_user_message`, `openai_json_shape`. Everything else in this appendix is planned. Tests marked M3+ cannot run until the corresponding crate exists; tests for gaps 1–6 need requirement rows or an amendment to an existing row before they can carry evidence in the ledger.
