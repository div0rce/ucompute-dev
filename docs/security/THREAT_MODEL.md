# Threat Model — Project Common

> Current reading note (2026-10-18): original M0/M1 actor tables and Appendix A retain historical planning labels. The dated implementation corrections and infrastructure extension below take precedence for current implementation; no new security tests or public-execution approval are implied.

- **Requirement:** F-06-06 (M0). Companion test: `default_config_binds_localhost` in `crates/common-api` (written separately).
- **Status:** living document; revise at every milestone that adds a trust zone (M3, M7, M9, M10, M12). Last revised 2026-10-19 (§6 infrastructure extension; R2 desktop update); 2026-10-15 for M3 as built.
- **Inputs:** `docs/SPEC_HANDOFF_V2.md` §7, §9, §18.6; `docs/REQUIREMENTS.csv`; ADR-0001; ADR-0002; ADR-0003 (§9, implementation notes); `docs/evidence/m3-single-host-2026-10-15.md`; `docs/research/COMPETITIVE_MATRIX.md` (llama.cpp RPC notes); `AGENTS.md`; decisions D-004, D-007; blockers B-002, B-005, B-006, B-007, B-008, B-011.
- **Method:** STRIDE per actor. S spoofing, T tampering, R repudiation, I information disclosure, D denial of service, E elevation of privilege.
- **Reading the mitigation tables.** `Requirement` is an exact ID from `docs/REQUIREMENTS.csv`; `—` means no row covers the mitigation (collected in §5.2). `Milestone` is the row's milestone. `Today` is the state in this branch: `present` (code + passing test), `present (SINGLE-HOST)` (code + passing test on one machine only; never two-machine evidence, B-002), `stub` (types/doc exist, logic unimplemented), `none`.

M3 (Common Mesh) is built: pairing, an authenticated QUIC peer endpoint, LAN discovery, revocation, policy, quotas, audit, and Mode B (one whole request on one paired device). It is **off by default**. With default settings the only listener is the loopback API and nothing leaves the device. All M3 evidence is SINGLE-HOST (B-002). §2.13 maps each mesh threat to its mitigation and test; the per-actor tables point there.

This revision updates M3 rows only. M1 rows keep their 2026-10-14 state and need their own re-check. Everything in M4 or later is **not mitigated today**.

## 1. Scope, trust zones and assets

### 1.1 System as built (M1 + M3)

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

M3 adds three listeners, and only after the user turns peer networking on:

```
paired device ◀── QUIC, TLS 1.3, raw Ed25519 keys, UDP 8193 on every interface ──▶ common-api (Node)
                  pinned keys only; QUIC Retry before TLS
another device ──▶ pairing endpoint, ephemeral UDP port, only during a 120 s window
LAN            ◀─▶ mDNS _common._udp.local., signed TXT (off with networking, or mdns = false)
```

State, all mode 0600: `identity/device.key` and `device.pub` (directory 0700), `identity/peers.json` (pins, groups, revocations), `config.json` (privacy, networking, serving, peer quotas), `logs/audit*.jsonl`.

### 1.2 Trust zones

| Zone | Who is in it | Execution modes (spec §7) | Trust assumption | Enabled today |
|---|---|---|---|---|
| Z0 Local device | This OS user, this daemon, the engine child process | A | Trusted. Other users/processes on the same host are **not** in Z0 (§2.1) | yes |
| Z1 Trusted peers | Devices paired by the owner (SPAKE2 code, pinned Ed25519 keys), grouped (F-02-09) | B, C, D, E | Authenticated, encrypted, authorized per group. Peer is trusted to **see** the work it executes (§4) | opt-in, off by default (M3: Mode B only) |
| Z2 Public swarm | Unpaired, pseudonymous contributors and requesters | F | Hostile by default. Every message is attacker-controlled | no (M9/M10, Gate F) |
| Z3 Managed cloud / enterprise | Cloud burst targets, enterprise federation, SSO IdP | G | Contractual; explicit cost and policy consent per use | no (M12) |

Data crosses a zone boundary only through Common's authenticated, encrypted transport (F-02-03, F-06-03). There is no plaintext peer path and llama.cpp's `rpc-server` is never reachable beyond loopback (D-007, `AGENTS.md`).

### 1.3 Assets

| Asset | Where | Confidentiality | Integrity | Availability |
|---|---|---|---|---|
| Prompts, activations, outputs | API request/response; engine memory; activation frames (M4) | high — user content | high — silent alteration = wrong answers | medium |
| Model weights + manifests | `models/blobs/sha256-*`, `models/manifests/*.json`, `catalog/*.json` | low–medium (license-dependent, X-08) | **critical** — wrong weights = silent model substitution | medium |
| Engine binaries + pins | `backends/<name>/<release>/`, `catalog/backends.json` | low | **critical** — runs as the user | medium |
| Device identity keys (M3) | `identity/device.key`, a 0600 file (no keychain, ADR-0003 D1) | **critical** | critical | medium |
| API keys | Common API key (config); engine key (process memory only) | high | — | — |
| Audit logs (M3) | `logs/audit*.jsonl`: rotated at 8 MiB, about 32 MiB kept, plus `audit.security.jsonl` | medium (metadata, no prompts) | high | low |
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
| Identity private key in a 0600 file in a 0700 directory; never logged | F-06-01 | M3 | present (SINGLE-HOST), §2.13 M3-26, M3-32 |
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
| Default-deny: with default config no request leaves the device | F-06-02 | M3 | present (SINGLE-HOST), §2.13 M3-19, M3-33 |
| Policy class (`local-only` / `trusted-group` / `swarm-allowed`) attached to every request and enforced in scheduler and transport; fuzz of 10,000 requests shows zero violations | F-06-07, I-05, F-04-01 | M3 / M5 / M5 | present for Mode B (SINGLE-HOST), §2.13 M3-19; scheduler none |
| Privacy mode selector with disclosure text before non-local modes | F-10-10 | M3 | present (API, CLI, desktop), §2.13 M3-20 |
| Encrypted, mutually authenticated transport; packet capture contains no plaintext prompt; relays see only ciphertext | F-02-03, F-06-03, F-02-06 | M3 / M3 / M9 | present (SINGLE-HOST relay capture), §2.13 M3-01..05 / relays none |
| Audit entry for every off-device request; diagnostics bundle scanned for prompts/keys | F-06-10 | M3 | present (SINGLE-HOST), §2.13 M3-26..28 |
| Contribution history and exports contain no prompt text | F-07-05 | M7 | none |
| Analytics off by default; DP aggregates only | X-14 | M13 | none |
| Attestation-gated confidential backend (research) | F-06-11 | M14 | research (B-005) |

**Not mitigated.** Anything a Z1/Z2 peer computes, it sees (§4). In Mode B a paired device sees the whole prompt and output in plain text; the disclosure says so. Execution records under `runs/` must not contain prompts; no row asserts this (gap 4).

### 2.6 Malicious peers (unpaired or public-swarm)

**Vectors.** Forged mDNS records; connection with an unpinned key; oversized or malformed capability/activation frames; serving corrupt chunks; returning fabricated outputs; eclipse of DHT routing tables; relay abuse.
**STRIDE.** S, T, I, D.
**Impact.** Wrong results, resource waste, DoS, exposure of the swarm to bad data.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Discovered-but-unpaired peers listed untrusted and never receive workloads; mDNS can be disabled | F-02-01 | M3 | present (SINGLE-HOST), §2.13 M3-02, M3-09..11 |
| Connections with unpinned keys refused; no plaintext fallback | F-02-03, F-06-03 | M3 | present (SINGLE-HOST), §2.13 M3-01..05, M3-08 |
| Capability messages bounded and versioned; oversize rejected and logged | F-02-04, F-08-10 | M3 | present (SINGLE-HOST), §2.13 M3-06, M3-07 |
| Activation frames length-prefixed, versioned, bounded; dimension mismatch rejected; stage accepts only authenticated peer | F-03-03, F-03-01 | M4 | none |
| Chunk hash verified on receipt; peer serving bad chunks penalised; non-redistributable models never served | F-05-09, F-05-05, F-05-06, X-08 | M6 | none |
| Signed, expiring discovery records; eclipse simulation documented | F-02-11 | M9 | none |
| Redundant execution / spot checks; cheating worker detected ≥95% within 100 jobs | X-07, F-07-09 | M10 | none |
| Reputation with confidence intervals | F-07-08, X-05 | M10 | none |
| Public swarm disabled until Gate F | F-07-01 (`default_off`), spec §22 | M7 | none (no swarm code exists) |

**Not mitigated.** Z2 does not exist in the codebase; the M3 rows cover unpaired devices on the LAN only. By default no listener accepts non-loopback traffic. With networking on, the peer endpoint accepts only pinned keys.

### 2.7 Compromised trusted peers

**Vectors.** A paired device is stolen, malware-infected or its key exfiltrated; it continues to receive workloads, reads activations of its layer range, returns altered logits, serves itself more quota, or seeds tampered chunks.
**STRIDE.** I, T, E (uses standing trust), R.
**Impact.** Silent exposure of every prompt routed through it; wrong outputs that pass as genuine.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| Revocation removes pinned key, closes sessions within 5 s, refuses reconnect, persists, propagates to group | F-06-05 | M3 | present (SINGLE-HOST), §2.13 M3-04, M3-16..18, M3-34 |
| Per-group authorization; workloads never dispatched to non-members | F-02-09, F-06-02 | M3 | present (SINGLE-HOST), §2.13 M3-21 |
| Per-peer quotas limit damage from a greedy trusted peer | F-06-04 | M3 | present (SINGLE-HOST), §2.13 M3-22, M3-23 |
| Correctness test against local execution catches gross tampering in development, not at runtime | F-03-04 | M4 | none |
| Activation-hash spot checks / redundancy for selected workloads (runtime detection) | X-07, F-06-12 | M10 / M14 | none / research |
| Audit log records which peer handled which request | F-06-10 | M3 | present (SINGLE-HOST), §2.13 M3-28 |

**Not mitigated.** Detection of a compromised peer that behaves correctly but copies activations is impossible without confidential computing (F-06-11, research). Owners must treat pairing as granting read access to whatever they route (§4).

### 2.8 Identity spoofing

**Vectors.** MITM during pairing; replayed pairing transcript; brute-forced short code; forged signed receipt; claiming another device's fingerprint in a discovery record; impersonating the local API client; impersonating an enterprise user.
**STRIDE.** S (primary), E.
**Impact.** Attacker becomes a trusted peer or bills work to another identity.

| Mitigation | Requirement | Milestone | Today |
|---|---|---|---|
| SPAKE2 pairing: wrong code fails, MITM fails key confirmation, >5 failures/min locks pairing | F-02-02 | M3 | present (SINGLE-HOST), §2.13 M3-12..15 |
| Ed25519 device identity; fingerprint is the peer ID; key persists across restart | F-06-01 | M3 | present, §2.13 M3-32 (`keygen_persist_load`) |
| Mutually authenticated transport with pinned keys | F-02-03 | M3 | present (SINGLE-HOST), §2.13 M3-02..05 |
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
| Z1 is Sybil-immune by construction: membership is owner-paired, not self-asserted | F-02-02, F-02-09 | M3 | present: in the product only a completed pairing creates a pin, §2.13 M3-12..15 |

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
| Per-peer token buckets on requests, tokens, bytes, sessions; p95 degrades <20 % under flood | F-06-04 | M3 | present for requests and tokens (SINGLE-HOST: local p95 +11.6 % under a flood), §2.13 M3-22; bytes and sessions wait for M4 |
| Capability and activation messages size-bounded | F-02-04, F-03-03 | M3 / M4 | present (capabilities), §2.13 M3-07 / none |
| QUIC Retry before TLS; inbound rejection audit lines capped at 20 a minute; at most 8 request streams per connection | — | M3 | present (SINGLE-HOST), §2.13 M3-08, M3-22 |
| Bandwidth probe capped | F-02-07 | M3 | none (no bandwidth probe is built) |
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
| Admission, quotas and timeouts bound the cost of one job | F-08-08, F-06-04, F-08-01 | M1 / M3 / M1 | stub / present (SINGLE-HOST, §2.13 M3-22, M3-23) / present |
| Browser contributors isolated by origin, limits documented | X-01 | M12 | none |
| Plugins declare capabilities; conformance suite | F-09-07 | M11 | none |
| Contribution scope (groups vs public) chosen at opt-in; audit-logged | F-07-01, F-06-10 | M7 / M3 | none |

**Not mitigated.** Contributor liability for generated content is a policy question, not a technical control; disclosure text (F-10-10, F-10-08) is the only planned measure.

### 2.13 Common Mesh (M3) as built: threat → mitigation → test

**Actors.** A passive or active attacker on the LAN; an unpaired device; a revoked device; a man in the middle during pairing; a paired device that floods or misbehaves; a hostile link aimed at the desktop app.
**Scope.** What runs in this branch. Every test is a unit test or **SINGLE-HOST** (one Mac). None is two-machine evidence (B-002). ADR-0003 §9 gives each test's file. The two-daemon run is `docs/evidence/m3-single-host-2026-10-15.md`.

| # | Threat | Mitigation as built | Tests | Requirement |
|---|---|---|---|---|
| M3-01 | Passive LAN observer reads prompts or completions | QUIC with TLS 1.3 only. The quinn `Endpoint` is the only socket type in `common-network`. 0-RTT off. | `packet_capture_contains_no_plaintext_prompt_single_host`, `no_plaintext_mode_exists` | F-02-03, F-06-03 |
| M3-02 | Unpaired device connects to the peer endpoint | The verifier rejects an unpinned key inside the TLS handshake (`unknown_ca`). No stream opens; no Capabilities are sent. | `unpinned_key_rejected` | F-02-03 |
| M3-03 | The dialled address answers with another key (spoofed record, hijacked address), or a party on the path forges a refusal | The dialler accepts only the one key it expects. A refusal is the device's own answer only after the dialler has read that device's Hello on the control stream: until then QUIC Initial packets (keys from the public connection id) are still processed, so any close, TLS alert or application code, can be forged even after the pinned key was shown and the handshake completed. An unproven refusal reads "did not prove it is <name>" (`peer_unauthorized`), never "revoked" or "disconnected by", and the next address is tried. A proven refusal (`ErrorMsg{peer_held}` on the control stream, a close after the Hello, or this store's own verdict) ends the dial. Every address is a hint. Cost: a genuine revocation by the other device, and a hold by a minor-0 build, read `peer_unauthorized` at the dialler. | `server_key_not_pinned_by_client_rejected`, `a_close_before_the_device_proved_its_key_proves_nothing`, `a_close_forged_after_the_genuine_flight_is_not_a_refusal_single_host`, `forged_revoked_alert_before_the_key_is_not_a_revocation`, `a_refusal_before_the_device_proved_its_key_does_not_end_the_dial_single_host`, `a_held_refusal_ends_the_dial_single_host` | F-02-03 |
| M3-04 | Revoked device reconnects, also with a cached TLS session | The verifier refuses revoked keys (`certificate_revoked`). No tickets, no resumption. The store is checked again after registration. | `revoked_key_rejected`, `revoked_client_cannot_resume_with_cached_session`, `revoked_key_cannot_reconnect_with_cached_session`, `revoked_during_control_handshake_is_closed` | F-06-05 |
| M3-05 | Downgrade to TLS 1.2 or an older protocol | Every config pins TLS 1.3. ALPN `common/1`. A major mismatch is `protocol_incompatible`. | `tls12_downgrade_attempt_fails`, `wrong_alpn_fails`, `protocol_major_mismatch_is_typed` | F-06-03, F-08-10 |
| M3-06 | Peer claims another identity in Hello, or alters a Capabilities record | Hello `peer_id` must equal the TLS identity. The Capabilities signature is checked over the exact bytes received, with a context prefix, before decoding. | `capabilities_signature_covers_the_bytes_sent`, `capabilities_exchanged_after_handshake` | F-02-04 |
| M3-07 | Oversized or malformed control frames | 1 MiB bound checked before allocation. Capabilities over 64 KiB → `frame_too_large`. Unknown fields and variants are skipped. | `frame_bound_checked_before_allocation`, `oversized_capability_message_rejected`, `unknown_field_tolerated`, `unknown_oneof_variant_tolerated` | F-02-04, F-08-10 |
| M3-08 | Flood of QUIC Initials from spoofed source addresses | QUIC Retry before any TLS work. Inbound `peer_rejected` lines capped at 20 a minute; the rest are counted. | `rejection_audit_is_rate_limited`, `rejection_audit_lines_are_capped_per_minute` (no test isolates the Retry) | — |
| M3-09 | Forged, tampered or replayed mDNS records | Ed25519-signed TXT that also covers the SRV port. Records more than 300 s off are dropped. At most 256 entries. A record for a known id at another address needs a newer `ts`. `trusted` is a label only. | `tampered_unsigned_and_foreign_records_rejected`, `stale_records_rejected_both_directions`, `table_is_bounded_and_replays_cannot_redirect` | F-02-01 |
| M3-10 | Discovery makes a device dial loopback or itself | Loopback and link-local addresses only when nothing else is advertised. Own id never listed. | `pick_addr_never_offers_loopback_or_link_local_when_a_lan_address_exists`, `browser_never_lists_own_id_single_host`, `peers_list_never_shows_this_device_as_discovered_single_host` | F-02-01 |
| M3-11 | mDNS leaks device details | TXT holds only `v`, `id`, `pair`, `pairport`, `proto`, `ts`, `sig`. No hostname beyond SRV, no models, hardware or user names. | `txt_sign_verify_round_trip` | F-02-01 |
| M3-12 | Man in the middle during pairing (relay terminating TLS on both legs) | The confirmation MAC binds the TLS exporter and both ids, so each leg fails. | `mitm_relay_detected`, `confirmation_mac_binds_side_exporter_and_ids` | F-02-02 |
| M3-13 | Online guessing of the 6-digit code | Single use, 120 s window, one attempt at a time. The 6th failure in 60 s locks pairing. | `brute_force_rate_limited`, `wrong_code_fails_and_pins_nothing` | F-02-02 |
| M3-14 | A wrong-code joiner learns the device name or port | Name and port are sent only after a verified MAC. | `wrong_code_learns_no_name_or_port` | F-02-02 |
| M3-15 | A QR is scanned but another device answers at that address | The joiner accepts only the QR's key, inside the handshake. Nothing is pinned; no revocation is cleared. | `qr_expected_peer_mismatch_pins_nothing_single_host`, `qr_join_pairs_and_a_qr_for_another_device_pins_nothing_single_host` | F-02-02 |
| M3-16 | Forged or replayed revocation (cut off an honest device, or undo a re-pair) | Signed with a domain prefix. Issuer pinned here and not revoked. Issuer and target share a group here. Superseded on re-pair. One record per target. | `bad_signature_rejected`, `revocation_requires_issuer_and_target_in_a_common_group`, `repaired_device_survives_replay_of_an_old_revocation`, `revocation_reaches_a_third_device_and_a_re_pair_survives_replay_single_host` | F-06-05 |
| M3-17 | A revocation misses a member, or reaches devices outside the group | Offered only to the group audience, again on every connection until the peer sends it back. The sync loop keeps a connection to every pinned device. A Disconnect hold (E5, `POST /api/peers/{id}/disconnect`) does not stop revocations: each time the held device dials, the holder answers its Capabilities with `peer_held`, the two exchange the revocations due at the other on the control stream, and the holder closes with `CLOSE_HELD` (protocol minor 1; a minor-0 peer gets no frame: the holder sends its Hello, verifies the peer's Capabilities and closes). The exchange re-checks the store before each write and each applied frame, revoke, unpair or a peer-sourced revocation closes it, and the revocation fan-out keeps state only for records the store holds, so a flood of rejected records cannot grow it. Residual: delivery waits for the held device's next redial (backoff at most 300 s), and two devices that hold each other exchange nothing until one connects; a held connection whose dial is cancelled mid-exchange stays open until the holder closes it (LOW; no work is routed over it); a peer's revocation applied during a Sharing restart can leave the new endpoint holding a device no longer paired (LOW, fails closed). | `revocation_goes_only_to_members_sharing_a_group_single_host`, `revocation_refused_once_is_offered_again_on_reconnect_single_host`, `revocation_fanout_offers_a_record_to_its_audience_until_it_comes_back`, `revocations_cross_a_hold_both_ways_single_host`, `a_sharing_change_keeps_a_hold_and_networking_off_ends_it_single_host`, `a_held_peer_of_each_minor_reads_the_hold_single_host`, `a_held_exchange_follows_the_store_single_host`, `peer_revocations_leave_state_only_for_stored_records_and_end_a_hold` | F-06-05 |
| M3-18 | A revoked device keeps an open session | Every connection to it closes at once with close code `RE`. | `disconnect_within_5s`, `pairing_pins_both_sides_and_revoke_closes_the_connection_single_host` | F-06-05 |
| M3-19 | A request leaves the device against the privacy setting | Default `local_only`, networking off. `evaluate` runs before any network activity. Privacy is checked again before a stream opens and again on the serving side. | `default_deny`, `local_only_never_leaves_device_fuzz`, `local_only_never_leaves_device_10k_single_host`, `privacy_mode_enforced_end_to_end_single_host`, `peer_target_is_denied_before_any_connection_when_network_is_enabled`, `privacy_gate_lets_only_trusted_group_leave_the_device` | F-06-02, F-06-07 |
| M3-20 | User leaves local-only without knowing a peer reads the prompt | Disclosure text. `acknowledged: true` required (API), y/N or `--yes` (CLI), a checkbox (desktop). `swarm_allowed` refused. Desktop chat: a paired device receives only the trailing run of messages already exchanged with that same device, plus the new prompt; nothing written under Local only, for this computer or for another device is sent to it, and the chat says so. | `disclosure_names_the_risk`, `privacy_mode_enforced_end_to_end_single_host`, `apps/desktop/src/test/privacy.test.tsx`, `apps/desktop/src/sections/chat/chat.devices.test.tsx` ("a picked device gets only the messages exchanged with it") | F-10-10 |
| M3-21 | A paired device without permission sends work | `may_serve`: networking and `serve_peers` on, peer pinned and not revoked, shared group allowing remote inference. | `unauthorized_peer_is_refused`, `may_serve_requires_serve_peers_and_network`, `workload_only_to_group_members`, `non_member_rejected` | F-06-02, F-02-09 |
| M3-22 | A paired device floods requests and starves local work | Per-peer buckets: 120 requests and 200,000 tokens a minute, 4 at once. At most 8 streams per connection. Peers never take the last engine slot and never queue. | `token_bucket`, `peer_flood_throttled`, `peer_flood_throttled_local_p95_within_20pct_single_host`, `peer_flood_throttled_through_daemon_single_host`, `peer_request_is_refused_while_local_work_holds_every_engine_slot_single_host`, `peers_never_get_the_last_engine_slot`, `peer_share_and_engine_slots_never_wait` | F-06-04 |
| M3-23 | A peer sends a grammar bomb or odd sampling, or asks for a different model file | The local API's bounds apply. The model hash is checked before and after load. No substitution. | `peer_requests_get_the_local_api_bounds`, `peer_supplied_grammar_refused_before_load`, `integrity_message_names_both_hashes_without_echoing_junk` | F-06-04 |
| M3-24 | A peer learns local paths from error messages | Generic messages for engine and filesystem codes. | `peers_never_see_local_paths` | — |
| M3-25 | An abandoned remote request holds an engine slot | The server cancels on `Cancel` or when the client's send side ends. | `remote_cancel_stops_server_generation_within_500ms` | F-08-02 |
| M3-26 | A prompt or the key reaches the audit log, logs or the bundle | Audit refuses content fields and strings over 2 KiB. The key is never formatted. A scan of real homes looks for every encoding of the key. | `audit_rejects_content_bearing_fields`, `private_keys_never_formatted`, `private_key_never_logged`, `private_key_absent_from_real_logs_and_bundle_single_host`, `diagnostics_bundle_redacted` | F-06-10, F-06-01 |
| M3-27 | The audit trail grows without bound, or a flood erases it | Rotation at 8 MiB, 3 files kept, each new file opens with `audit_rotated`. Security kinds have their own trail. Rejection lines are capped. | `rotation_keeps_three_files_and_0600`, `security_trail_survives_a_request_flood`, `concurrent_appends_survive_rotation` | F-06-10 |
| M3-28 | A request leaves the device with no record | The `sent` line is written before the frame leaves, else nothing is sent. `finished` is written once. The serving side logs the same `request_id`. | `audited_stream_records_one_outcome_whatever_ends_it`, `remote_completion_returns_golden_text_with_target_echoed_single_host` | F-06-10 |
| M3-29 | Crossed dials cut a running request or serve it twice | The lower id's connection survives. Only the initiator retires a duplicate, and only when it is idle. | `duplicates_resolve_to_the_lower_ids_connection`, `crossed_dials_keep_one_connection_single_host`, `busy_duplicate_is_retired_not_cut_single_host` | — |
| M3-30 | A dead or cut-off peer leaves a request hanging | Keep-alive 1 s, idle 4 s: worst case 5 s. Typed `peer_lost`. | `kill_peer_detected_within_bound_single_host`, `sigkill_peer_process_detected_within_bound_single_host`, `partition_detected_and_recovered_single_host`, `peer_lost_mid_stream_is_typed` | F-02-05 |
| M3-31 | A link points the desktop app at a hostile daemon | `?daemon=` works only in dev builds, only for `http` loopback with a port. | `apps/desktop/src/test/devdaemon.test.tsx` | — |
| M3-32 | Another OS user reads the identity key | `device.key` is 0600 in a 0700 directory, tightened on load. | `file_permissions_0600` | F-06-01 |
| M3-33 | Something listens although the user never turned networking on | `Node::start` refuses when disabled. Networking, serving and mDNS are off by default. | `network_disabled_refuses_to_listen`, `disabled_config_is_inert`, `settings_default_off_and_round_trip_0600` | F-06-02 |
| M3-34 | CLI and daemon race on shared state and lose a revocation or a privacy change | Writers lock and re-read; readers reload on change. CLI changes go through the running daemon. | `stale_handle_does_not_undo_another_handles_revocation`, `concurrent_handles_lose_no_update`, `privacy_changed_on_disk_applies_at_once_and_is_never_written_back`, `daemon_is_found_only_when_it_answers_with_this_device_id` | F-06-05 |

**Still open (M3).**
- **Two-machine evidence (R2, B-002).** Every row above ran on one Mac. Real LAN mDNS, Wi-Fi jitter against the 4 s idle timeout and a kernel capture on a LAN interface are untested. SINGLE-HOST evidence never satisfies a criterion that needs two machines (F-02-01 LAN discovery, F-03-01, F-03-02, Gate D, two installs pairing through the UI). Rows whose criteria do not depend on topology (pairing cryptography, policy, revocation, capture, liveness) were marked `VERIFIED` from SINGLE-HOST tests on 2026-10-15; see `docs/REQUIREMENTS.csv`.
- **rustls `tls12` is compiled in (R9).** reqwest (hyper-rustls) enables it. No Common config uses it.
- **`spake2` 0.4.0 is unaudited (R10)** and, by its own statement, probably not constant-time.
- A compromised paired device reads every prompt it runs (§4). Common cannot detect that.
- No kernel packet capture yet: `tcpdump` needs sudo. The in-process relay capture stands in.
- No test isolates the QUIC Retry. The pairing endpoint has no Retry; its lockout and one-at-a-time loop bound it.
- Private key bytes are not zeroized in memory.
- The `peer_rejected` cap is one global window: a flood hides other rejections for that minute (the count is kept).
- The revocation audience map lives in memory. A record this store never saw can re-revoke a re-paired device (fails closed).
- With networking on, the peer endpoint binds every interface. Common does not manage the OS firewall; the endpoint accepts only pinned keys.
- The CLI sends `COMMON_API_KEY` to whatever answers on the loopback port named in `serve.json` (same-user boundary, R6).
- No bandwidth probe (F-02-07), and no byte or session quotas yet.

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
13. **SD-13** Peer networking, serving peers and mDNS are off by default. Nothing listens beyond loopback until the user turns networking on.
14. **SD-14** Leaving `local_only` requires an explicit acknowledgment of the disclosure that a paired device reads the prompts it runs.

## 4. What TLS does not give you

- **A peer that executes a layer range sees the activations for that range.** For the first and last stages that is a thin transform away from the prompt and the output; for middle stages activations are still invertible in practice for many architectures. Encryption in transit (F-02-03) protects against observers on the wire and relays (F-02-06), not against the endpoint. Spec §9: "TLS does not hide prompts from a peer processing them."
- **Local-only is the only private mode.** Modes B–G disclose the workload to every executing peer. Pairing a device (F-02-02) is granting it read access to whatever policy allows routing to it. The privacy selector (F-10-10) and policy engine (F-06-07) exist to make that disclosure explicit and enforce the boundary, not to remove it.
- **Confidential computing (F-06-11) is research.** No attestation path exists; no CC hardware is available (B-005). Nothing in Common may claim confidential execution without verified attestation.
- **Verifiable inference (F-06-12) is research.** Redundancy (X-07) and activation-hash spot checks are statistical detection, not proof. They are never described as cryptographic proof unless they are.
- **Authentication is not authorization.** A valid peer identity (F-06-01) proves who; policy (F-06-02, F-06-07, I-05) decides whether. Both exist since M3: identity in the TLS verifier, policy in `PolicyEngine::evaluate` (sending) and `may_serve` (serving).
- **Mode B shows the whole prompt.** In M3 a paired device runs the whole request, so it reads the full prompt and output, not only activations. The disclosure says this before the user leaves `local_only` (SD-14).

## 5. Residual risks and open questions

### 5.1 Residual risks (accepted or unresolved)

| # | Risk | Why it remains | Owner action |
|---|---|---|---|
| R1 | **TOCTOU between hash verification and engine load.** `Store::verify` re-hashes the blob; the engine then opens the path by name. A same-user writer can swap the file in between. | Loopback-and-same-user is the OS trust boundary; `llama-server` takes a path, not an fd. Mitigation options: open once and hash the open fd, hard-link into a private per-run directory, or verify via `/proc/self/fd` style path. | Decide in M1 implementation; document in `common-artifacts`. |
| R2 | **Bounded physical coverage (updated 2026-10-19).** Mac/Windows Mode B, hotspot discovery and two installed desktop apps pairing through their UIs have evidence ([desktop two-machine](../evidence/desktop-two-machine-2026-10-19.md)); pooling, clean-account installs and Windows multi-user ACL isolation do not. | No LAN kernel packet capture; limited host/network configurations. | Retain SINGLE-HOST capture labels; see [two-machine evidence](../evidence/m3-two-machine-2026-10-18.md). Do not infer arbitrary platform or distributed-execution security. |
| R3 | **Public signing and executed release CI unavailable (B-007/008).** Mac artifact is ad-hoc signed; pipeline definitions exist, not a demonstrated release pipeline. | Developer ID/Windows signing and publication authorization absent; origin exists. | Qualify signed exact artifacts and target-native CI before public promotion. |
| R4 | **Upstream engine binary is trusted at pin time.** The SHA-256 pins what was downloaded, not what the source produces. | No reproducible build of llama.cpp releases. | Consider building from the pinned `source_commit` in CI once B-007 is resolved. |
| R5 | **llama.cpp memory-safety surface.** GGUF loader and `rpc-server` are C++ with known robustness issues (COMPETITIVE_MATRIX: #15055, #13185, #20315; no fuzzer upstream). | Not Common's code. | Process isolation now (ADR-0002); sandbox at M7 (F-06-08); never expose beyond loopback (D-007). |
| R6 | **Same-user local attacker.** Anything running as the user can read `$COMMON_HOME` and reach loopback ports. | Inherent to desktop OS model. | Document in user privacy notes. M3 chose a 0600 file over the keychain (ADR-0003 D1); revisit with signed builds (M13). |
| R7 | **Fixture license (B-011).** TinyStories weights have no verified license. | Upstream ambiguity. | Never bundle; download at test time. |
| R8 | **Engine secret handling (corrected 2026-10-18).** Code uses a temporary `--api-key-file`, not the secret in argv. | Same-user memory/file access remains; Windows ACL guarantees require native tests. | `crates/common-runtime/src/llamacpp/process.rs`; recorded `key_file_is_owner_only_and_never_overwritten` test in VERIFICATION. Do not equate Unix mode tests with Windows isolation. |
| R9 | **rustls `tls12` is compiled into the binary.** reqwest (hyper-rustls) enables the feature, so TLS 1.2 code ships although no Common config uses it. | Cargo feature unification: reqwest's `rustls-tls` turns it on through hyper-rustls. | Every peer config pins TLS 1.3 (`tls12_downgrade_attempt_fails`). Remove it when reqwest can be built without it. |
| R10 | **`spake2` 0.4.0 is unaudited** and, by its own statement, probably not constant-time. | Accepted in ADR-0003 D2 for a short-lived pairing code. | It protects only a one-time 120 s code; lockout after 5 failures; long-term trust rests on TLS 1.3 + pinned Ed25519 keys. Track `spake2` 0.5. |

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

- Transport: settled for M3 as `quinn` + `rustls` with pinned raw keys (ADR-0003 D3). Whether M9 adds an `iroh` adapter or its own relay is still open (F-02-06).
- Whether `rpc-server` is wrapped (tunnelled through Common's transport) or replaced with Common-native activation frames (F-03-03) — M4. COMPETITIVE_MATRIX recommends not reusing the wire protocol as-is.
- Key storage: settled for M3 as a 0600 file (ADR-0003 D1). Keychain support waits for signed builds (B-008).
- Whether redundancy (X-07) tolerances for cross-backend nondeterminism leave a usable detection margin for F-07-09.

## Appendix A — Test mapping for secure defaults

| Default | Test name | Location | Covers | State |
|---|---|---|---|---|
| SD-01 | `default_config_binds_localhost` | `crates/common-api` | F-06-06 | planned (in progress by API agent) |
| SD-01 | `external_interface_not_bound_by_default` | `crates/common-api` (integration: start `serve`, assert only 127.0.0.1 socket) | F-06-06 | planned |
| SD-01 | `api_binds_localhost_by_default` | `crates/common-api` | F-09-01 | planned (may alias SD-01 above) |
| SD-02 | `nonloopback_bind_without_key_is_policy_denied` | `crates/common-api` | gap 5 | planned |
| SD-03 | `no_plaintext_mode_exists`, `tls12_downgrade_attempt_fails`, `wrong_alpn_fails`, `unpinned_key_rejected`, `revoked_client_cannot_resume_with_cached_session`, `packet_capture_contains_no_plaintext_prompt_single_host` | `crates/common-network` | F-06-03, F-02-03 | present (SINGLE-HOST) |
| SD-04 | `default_deny`, `unauthorized_peer_request_refused` | `crates/common-policy` | F-06-02 | present |
| SD-04 | `defaults_are_local_only_and_a_peer_target_is_refused_without_any_network`, `local_only_never_leaves_device_10k_single_host` | `crates/common-api/tests/mesh.rs` | F-06-02, F-06-07 | present (SINGLE-HOST) |
| SD-04 | `local_only_never_leaves_device_fuzz` | `crates/common-policy` | F-06-07 | present |
| SD-05 | `default_off`, `no_contribution_without_opt_in` | `crates/common-metering` (M7) | F-07-01 | planned M7 |
| SD-06 | `gate_disabled_by_default` (settlement) plus a swarm-flag default test | `crates/common-metering`, `crates/common-network` | F-07-11; swarm flag has no row (spec §22) | planned M9/M10 |
| SD-07 | `engine_binds_loopback_with_random_key` | `crates/common-runtime` (`llamacpp.rs`) | gap 1 | planned M1 |
| SD-08 | `metrics_contain_no_prompts` | `crates/common-api` | F-09-03 | planned M1 |
| SD-08 | `errors_redact_secrets` | `crates/common-core` | F-08-03 | planned M1 (`error_codes_are_stable` present) |
| SD-08 | `engine_log_and_run_record_contain_no_prompt` | `crates/common-runtime` | gap 4 | planned M1 |
| SD-08 | `diagnostics_bundle_redacted`, `audit_rejects_content_bearing_fields` | `crates/common-observability` | F-06-10 | present |
| SD-08 | `private_key_absent_from_real_logs_and_bundle_single_host` | `crates/common-cli/tests/mesh_single_host.rs` | F-06-01, F-06-10 | present (SINGLE-HOST) |
| SD-09 | `hash_verify`, `corrupted_byte_rejected`, `manifest_signature_or_hash_required` | `crates/common-artifacts` | F-01-05, F-05-01 | planned M1 |
| SD-09 | `malformed_gguf_rejected_without_crash` | `crates/common-models` | F-01-02 | planned M1 |
| SD-10 | `safe_names` | `crates/common-core/src/paths.rs` | F-01-05 | **present** |
| SD-10 | `path_traversal_in_name_rejected` | `crates/common-artifacts` | F-01-05 | planned M1 |
| SD-10 | `archive_member_traversal_rejected`, `extracted_files_match_archive` | `crates/common-artifacts` (`backend.rs`) | gap 3 | planned M1 |
| SD-11 | `engine_env_is_scrubbed` | `crates/common-runtime` | gap 2 | planned M1 |
| SD-12 | `queue_bound`, `overload_returns_429`, `overload_does_not_oom` | `crates/common-api` | F-08-08 | planned M1 |
| SD-12 | `oversized_body_rejected` | `crates/common-api` | gap 6 | planned M1 |
| SD-12 | `oversized_capability_message_rejected`, `frame_bound_checked_before_allocation` | `crates/common-network` | F-02-04 | present |
| SD-12 | `oversized_or_mismatched_dim_frame_rejected` | `crates/common-distributed` | F-03-03 | planned M4 |
| SD-13 | `network_disabled_refuses_to_listen`, `disabled_config_is_inert`, `settings_default_off_and_round_trip_0600` | `crates/common-network`, `crates/common-api` | F-06-02 | present |
| SD-14 | `disclosure_names_the_risk`, `privacy_mode_enforced_end_to_end_single_host` | `crates/common-api` | F-10-10 | present (SINGLE-HOST) |

Present today: `safe_names`, `error_codes_are_stable`, `backoff_is_bounded`, `every_code_has_status_type_and_user_message`, `openai_json_shape`, and the rows marked `present` above (M3, 2026-10-15). M1 rows still marked planned were not re-checked in this revision. Tests marked M4+ cannot run until the corresponding crate exists; tests for gaps 1–6 need requirement rows or an amendment to an existing row before they can carry evidence in the ledger.


## 6. Infrastructure-program extension (2026-10-18)

### 6.1 Implementation corrections and evidence scope

These corrections come from inspected code and the existing verification record, not a new runtime run. [VERIFICATION](../VERIFICATION.md) F-06-06 records environment scrubbing, API auth, cross-origin/Host rejection, request size bounds, malicious archive rejection, key-file handling and redaction tests. They supersede original “stub/planned” labels for those specific implementations. Section 5.2 remains a historical gap list: engine loopback/auth/scrubbing/archive bounds are now represented by F-06-06/F-01-05 evidence; catalog signing, disk quota and verify-to-load TOCTOU still need their own proof. A coverage mapping is not a new acceptance claim.

The M3 trust profile is [ADR-0003 §9](../decisions/ADR-0003-peer-transport-and-pairing.md), including its differences from the proposal. Physical Mode B tests supplement SINGLE-HOST security tests; they do not establish model pooling, kernel LAN capture, Windows desktop qualification or Windows multi-user isolation. Unknown Windows key/config ACL behavior is a release blocker, not an assumed exploit or an assumed guarantee.

### 6.2 Additional threat-to-work mapping

All entries below are committed design/verification obligations. Public execution remains disabled; none is claimed implemented by this extension.

| Actor / attack | Asset / failure | Required control and falsifiable check | Work / requirements |
|---|---|---|---|
| Malicious provider or compromised trusted device; false hardware/load or result | Prompt/activation/result exposure, bad placement, incorrect output | Disclose endpoint plaintext; treat capability signatures as provenance, not truth; bounded independent probes, stale-claim refusal and correctness checks; inject false capacity and wrong output | WP-04/05/11; F-02-04, F-03-04, F-07-09, X-07 |
| Malicious consumer; unauthorized expensive requests | Device resources, battery, metered cost | Policy before admission/load/network; hard per-workload quotas and deadline; flood with local foreground work and assert shipped-default fairness, cancellation and resource release | WP-02/09; F-06-02/04, F-08-08, X-03 |
| Credential theft, impersonation or revoked/offline device | Device and group authority | Owner-only secret access on every OS; revoke active/resumed/new sessions; rejoin after offline revocation; no bearer secrets in diagnostics | WP-02/07; F-06-01/05/10, U-01 |
| Replay, downgrade, forged capabilities or protocol extensions | Authorization/lifecycle consistency | Publish canonical signatures, nonce/expiry/replay semantics, required-capability rules; independent negative vectors and no fallback to unauthenticated transport | WP-07; F-08-10, U-01 |
| Malicious model, backend, plugin or workload package | Provider host execution and supply chain | Pin/verify artifacts, license/notice review, bounded parsers, explicit permission manifest; reject unknown workload/permissions before spawn; extension listing never authorizes execution | WP-03/10; F-06-08/09, F-09-07/12, U-02 |
| Arbitrary code or cross-tenant escape | Host data, credentials and neighboring workloads | No arbitrary code until audited sandbox/isolation model; filesystem/network/process/device denial tests, escape/fault attempts and independent review before narrow public workload admission | WP-11; F-06-08, Gate F |
| Provider/consumer collusion, receipt replay, Sybil identities | Credits, reputation, payments | Signed accounting plus independent useful-work verification, replay protection, conservative reputation confidence and reconciliation; adversarial duplicate/wrong/fabricated job tests | WP-09/11; F-07-07..11, X-05/06/07 |
| Credit-economy abuse: credit fraud (forged, replayed or double-spent entries), Sybil grant and fair-access farming, fake or inflated contribution or metering, collusive price-setting, consent-mode bypass (including mode loosening through the local API, peers, coordinators or config import, and funding-class mislabelling by a scheduler), balance takeover by an unauthorized client or stolen device, chargeback abuse, liability run | Ledger integrity and backing, contributor consent and limits, requester funds, platform solvency, D-019 commitments | Append-only hash-chained double-entry ledger whose holder balances reconcile to funded issuance minus retirements (zero discrepancy over >=100,000 simulated transactions); issuance only on collected funded consumption or from an owner-signed grant budget, under a recorded issuer key; charges derived from requester-verifiable metered units within a published rate ceiling; earned credits held until F-07-09 spot checks and X-06 receipts clear and the funding payment is collected, fraudulent earnings reversed before they are spendable; chargebacks debit the purchaser's unspent balance, any shortfall held as a system-account receivable, never a negative holder balance, and honest contributors are protected by payment verification, fraud screening, exposure limits on credits spendable and earnings released before collection, a funded loss reserve, risk-based holds and provider terms shown at opt-in, not by an unlimited platform guarantee: a spend or release that would exceed a limit or the reserve is refused with a typed error; grant and fair-access allowances weighted by contribution history or Beta reputation, not identity count, with a per-period grant budget cap (30 % Sybils obtain <5 % of fair-access capacity and grants); sharing mode and owner limits checked before admission and again by the provider device's own default-deny policy against the requester-signed funding class, and recorded in every allocation and receipt (zero private or donate-only capacity on credit-funded work in >=10,000 adversarial attempts); mode loosening only by interactive owner confirmation on the device, other channels refused and audited (same-user processes remain R6); spending only by owner-granted clients, stopped on revocation; no transfers or cash-out before the F-07-11 gates, closing laundering exits; refunds only of unspent purchased credits, spend limits on newly purchased credits and chargeback losses carried in the §29.4 model; issuance and sales refused when the reserve would fall below the backing requirement of outstanding credits or capacity coverage falls below target, with a typed refusal rather than a silent downgrade | WP-09/11/12; F-06-01/02/05/10, F-07-01/05/07..11, X-05/06/07, I-03, U-05; spec §§26.5, 29.4 |
| Self-dealing and fabricated contribution: earning credits for one's own workloads (own devices, private Mesh, multiple accounts or device identities under common control, Sybil requesters), circular or repeated artificial transactions, fabricated workloads or fake contributions, manipulated metering, replayed or duplicated accounting records | Credit issuance integrity, grant budgets, fair-access and reciprocity weighting, D-019 self-compute exclusion | Earning only for verified work delivered to an eligible independent requester under a compensated arrangement; jobs from the provider owner's identity or devices registered to it classified own on the device and in the ledger (0 spent, 0 earned, 0 fee); an eligibility check before crediting refuses linked requester-provider pairs (same owner identity or its registered devices, F-07-08/09 linkage, with shared private-Mesh pairing a signal rather than proof), and separate usernames, accounts or device ids never count as independence by themselves; X-06 receipts signed by requester and provider with nonce and expiry, replays and duplicates rejected; circular flows rate-limited, flagged and held, repeated requester-provider pairs flagged and their earnings held for review, never refused on repetition alone, detected self-dealing earnings reversed while held; own and trusted-access jobs add no contribution history or reputation weight; checks: the U-05 self-compute list earns zero, and a >=10,000-cycle collusion simulation at a fixed attacker purchase and hardware budget ends with the adversary holding no more than its net collected purchases (after injected chargebacks and failed collections), grants and earnings from independent requesters plus its identities' released-earnings exposure limit, and with its share of grants, fair-access capacity and reciprocity priority rising by <=10% relative. An organization serving independent external customers from several servers is not self-dealing | WP-09/11; F-06-01/02, F-07-02/05/07/08/09, X-05/06/07, I-03, U-05; spec §§26.5, 29.4 |
| Trusted-access impersonation or scope escalation: an unlisted requester claiming an allowlisted identity by username, display name, IP address, LAN proximity, unauthenticated announcement or a stolen, forged or replayed credential, or by a coordinator, Cloud or identity-service assertion alone; self-enrolment on an allowlist or as an owner device, or an entry added or widened, through a non-owner channel (local API, SDK client, peer, coordinator, Cloud, update, pairing or config import); reuse of a grant on another provider or from the grantee's other accounts or, for a device-key grant, its other devices; use after revocation; resale of trusted, donated or fair-access capacity through the grantee's own share-for-credits provider; a trusted workload reaching the host beyond its permissions | Provider host and resources, the owner's consent, free-access accounting | Grants bound to authenticated, stable identities (pinned device key, or pinned account key with the device delegations it signed, verified on the provider device) and to one provider, checked by the provider device's own default-deny admission; allowlist entries and own-device registration change only by interactive owner confirmation on the provider device, other channels refused and audited (same-user processes remain R6); originating funding class carried end to end in X-06 receipts and credit-funded jobs forwarded into a trusted, donate or fair-access grant refused; removal stops new admission within 5 s; trusted requests stay within the owner's workload classes, limits and schedule and the workload permission set: no host administration, file system, shell, arbitrary code, desktop control, credentials or network beyond the workload; impersonation, cross-provider, post-revocation and escape attempts refused and audited (U-05 trusted-access checks). Device-key theft from the grantee remains possible and is handled by revocation (F-06-05) | WP-02/09/11; F-06-01/02/05/08/10, U-02, U-05; spec §26.5 |
| Free-to-paid fallback: a scheduler, coordinator or client reroutes an allowlisted or fair-access request to paid capacity when the free provider is unavailable, or charges a trusted request that exceeded its grant's limits or schedule, or labels an eligible free request credit-funded | Requester funds, billing precedence, trust in the free path | Free access takes precedence on the granting provider and the requester is shown before execution whether credits will be consumed; when the free provider is unavailable the request waits, fails with a typed refusal or uses another eligible free resource; paid execution only with the requester's explicit authorization for that request or under a requester-configured paid-fallback policy with per-request and period caps (a general spending cap is not such a policy); a debit without such a confirmation or policy reference, or on an eligible trusted request, is rejected and audited; check: with the granting provider offline or the grant's limits exhausted zero credits are spent without authorization and never above the policy caps | WP-05/09/11; F-06-02, U-05; spec §§26.5, 28.4 (spend consent as a hard constraint) |
| Relay/routing abuse, public ingress floods | Network availability, operator bandwidth bill | Authenticated bounded relay sessions, budgets/limits, no arbitrary destination proxying, isolation, operator shutdown and abuse response; churn/flood/cost-bound experiments | WP-11; F-02-06/11, F-06-04, Gate F |
| Privacy-policy bypass, logging or retention leak | Prompts, outputs, local files and metadata | Explicit target/permission flow, local-only packet-negative tests, no content in default logs, consented aggregate analytics, retention/deletion tests | WP-06/08/10; F-06-07/10, X-14, U-02 |
| Compromised update or rollback | Persistent unauthorized code or lost data | Signed artifacts, tamper refusal, pinned provenance, migration/rollback with user-data retention and malicious update tests | WP-08; F-10-07, F-06-09 |
| Windows Firewall rule for the bundled daemon (desktop consent step, 2026-10-21) | Host exposure, owner consent | Added only after the user presses "Allow on private networks" and confirms the Windows elevation (UAC) prompt; one inbound rule "Common (paired devices)" scoped to the bundled common.exe path, UDP, Private profile only, never Public/Domain or all programs; the elevated cmd.exe runs from System32 so a planted netsh.exe in the app folder cannot run elevated; the daemon still refuses unpaired keys before any work and pairing still needs the SPAKE2 code; the notice stays while Windows treats the current network as Public (the rule does not apply there). Detection is one read-only PowerShell query (`Get-NetFirewallRule -DisplayName` with its port and application filters, non-localized enum values, UTF-8, ending with an `end` marker); a read that fails or stops early is reported as not read, never as "no rule". Allow deletes only this program's own rule (`delete rule name=… program=<path>`), so other Windows accounts' rules of the same name stay. Both reads keep their errors and the script throws anything but a not-found error before `end` (round 2, 2026-10-22). When the app adopted a daemon it did not start, or the daemon it started has exited, it neither checks nor adds a rule (status `adopted`, Allow refused). Domain-profile networks are reported (`domain_networks`) with a note pointing to the administrator. Limits: a stale rule this account added for an older install path is no longer removed (Private UDP for a program that is gone); per-user uninstall cannot remove the rule (no elevation); the PowerShell read is unit-tested on its parser and script text only and still needs a physical Windows check (including an access-denied or provider-failure case), ideally on a localized Windows or under a non-ASCII user name | apps/desktop/src-tauri/src/firewall.rs; F-10-05; the consent step itself evidenced on the laptop 2026-10-22 (`docs/evidence/mesh-physical-2026-10-22-eduroam.md`, netsh-era detection) |
| Compromised or misleading public site, download page or DNS | Installer integrity, visitor privacy, user trust | Hashes and signatures published only from the docs/VERIFICATION.md promotion record and checked by the F-10-07 updater independently of the site; no third-party analytics or tracking; claim check before each publication; preview access enforced at the hosting layer | WP-08/13; F-10-07, F-06-09, X-14; spec §29.5 |

Residual risk (D-019 self-compute exclusion). Beneficial ownership and economic control cannot be established perfectly in a decentralized network: one person can operate unlinked accounts, devices, network paths and payment instruments that no proportionate check connects. The controls above detect common links and bound the gain rather than eliminate it. Under backed issuance an undetected self-dealing cycle with purchased credits retires the platform fee and creates no credits while its funding stays collected; if the funding purchase is charged back or a usage bill fails after the colluding provider's share is released, that share is a credit backed only by the loss reserve, bounded per requester and in total by the exposure limits X_t and dispute-window holds and counted in L_t. The remaining exposure is grant-funded cycles (bounded by per-identity and per-period grant budgets), contribution history, reputation and reciprocity priority bought with self-dealt volume at the cost of the platform fee (F-07-08 lower-confidence weighting bounds identity count, not history one identity buys; the U-05 collusion simulation bounds the resulting grant and fair-access gain), and distorted demand, liquidity and capacity-coverage measurements in the spec §29.4 model, which therefore report detected self-dealing volume and exclude it. Trusted access shares the same limit: an owner can allowlist identities it controls, which is harmless because trusted jobs earn nothing and add no contribution weight. A modified grantee client can strip the originating funding class and resell free capacity out of band; this creates no credits, because the buyer's spend backs the earning, but it breaches the granting owner's consent. It is bounded by that grant's limits and schedule, the owner's activity review and revocation, and for fair access by the Sybil-bounded share.

### 6.3 Separate promotion gates

Private Mesh promotion requires qualified identities/storage/transport/policy, default resource limits, consent, revocation and failure behavior on advertised physical platforms. A paired device remains capable of observing its workload; household trust is not enterprise tenant isolation. Resolve B-014/B-015 and Windows ACL evidence before broad promotion.

Public contribution additionally requires a narrow isolated workload allow-list, enforceable owner power/time/resource limits, accounting reconciliation, inference-verification limits, Sybil/fraud controls, churn/failure tests, abuse contacts and an independently reviewed incident shutdown path. Passing private Mesh tests or signing receipts cannot open public contribution. Confidential-computing and arbitrary-inference proof research remain F-06-11/12; no breakthrough is assumed.

[Specification](../SPEC_HANDOFF_V2.md) §§28/31 defines proposed measurable gate targets. They are not evidence of security or permission to deploy. Owner consent, security review and any required qualified settlement/legal assessment remain distinct conditions. Contributor revocation must stop new work and terminate admitted work within the applicable cancellation bound without silently moving private data elsewhere.
