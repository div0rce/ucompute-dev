# ADR-0003: Peer identity, pairing, transport and discovery (M3); carriage of llama.cpp RPC (M4)

- **Status:** Proposed
- **Date:** 2026-10-14
- **Inputs:** `docs/SPEC_HANDOFF_V2.md` sections 6, 7, 9; ledger rows F-02-01..12, F-06-01/02/03/05/10, F-08-10, F-03-01, F-03-03, X-20; ADR-0001 (stack table), ADR-0002 (supervised `llama-server`, D-007); `docs/research/COMPETITIVE_MATRIX.md`, `RESEARCH_GAPS.md`; crates.io metadata and downloaded crate sources read on 2026-10-14 (section 11).
- **Markers:** ✅ verified this session from a primary source · ⚠️ secondary/partial · `unverified` not checked.
- **Implementation:** M3 was built on 2026-10-15 and tested SINGLE-HOST only. Section 9 describes the code. Where section 9 and sections 2–8 disagree, section 9 is what runs.

## 1. Context

M3 must let trusted devices on one LAN discover each other, pair with a short code, authenticate each other's keys, and talk over an encrypted transport with no plaintext path, offline (X-20), with no central service (spec section 6). M4 must carry llama.cpp's RPC layer-offload traffic between two hosts through that transport, because `ggml-rpc-server` is unauthenticated plaintext TCP and its own README says never expose it (COMPETITIVE_MATRIX; D-007). Only one physical machine exists (B-002), so every test in this ADR is labelled `SINGLE-HOST` unless stated otherwise (D-004).

Threat model for M3 (spec section 9), what this ADR defends against: a passive or active attacker on the LAN (reads, injects, replays, spoofs mDNS); an unpaired device trying to connect; a revoked device trying to reconnect; an active MITM during pairing; a peer flooding pairing or streams. **Not defended** and documented honestly: a trusted peer that is compromised sees, inside its own process, the activations and the layer weights it computes on; TLS does not hide prompts from a peer processing them (spec section 9). Confidential computing and verifiable inference remain M14 research (F-06-11, F-06-12).

## 2. Verified crate facts (crates.io API + `cargo info`, 2026-10-14)

| Crate | Stable version (date) | License (SPDX) | MSRV | Notes |
|---|---|---|---|---|
| quinn | 0.11.12 (2026-09-14) ✅ | MIT OR Apache-2.0 ✅ | 1.85 ✅ | Already in `Cargo.lock` transitively ✅. `quinn-proto` 0.11.19 (2026-09-30) requires `ring ^0.17` ✅ |
| rustls | 0.23.45 (2026-09-14) ✅; **0.24.0-dev.2 published 2026-10-14** ✅ | Apache-2.0 OR ISC OR MIT ✅ | 1.71 ✅ | Already in `Cargo.lock` ✅. RFC 7250 raw-public-key API present in 0.23.45 source: `ClientCertVerifier::requires_raw_public_keys`, `AlwaysResolvesServerRawPublicKeys`, `AlwaysResolvesClientRawPublicKeys`, `verify_tls13_signature_with_raw_key`, `ring::sign::any_eddsa_type` ✅ |
| ring | 0.17.14 (2025-03-11) ✅ | Apache-2.0 AND ISC ✅ (both apply; both in the ADR-0001 allowlist) | 1.66 ✅ | Self-described "An experiment"; 19 months without a release ⚠️ maintenance risk |
| iroh | 1.3.0 (2026-09-28) ✅ (1.0 was 2026-06 ⚠️) | MIT OR Apache-2.0 ✅ | **1.91** ✅ | Depends on `noq` 1.3.0 (its own quinn fork) ✅, `rustls ^0.23.33`, `ed25519-dalek 3` ✅. RFC 7250 raw public keys in `src/tls.rs` ✅. `RelayMode::Disabled`, `presets::{Minimal,N0,N0DisableRelay}` ✅; `N0` preset uses n0.computer DNS/pkarr/relays ✅. mDNS is an external crate (`iroh-mdns-address-lookup` 0.6.0 / `swarm-discovery` 0.6.4) `unverified` |
| libp2p | 0.57.0 (2026-09-11) ✅ | MIT ✅ | 1.88 ✅ | DCUtR/relay maturity `unverified` |
| spake2 (RustCrypto) | 0.4.0 (2023-07-28) ✅; 0.5.0-pre.0 (2026-01-25) ✅ | MIT OR Apache-2.0 ✅ | 1.60 ✅ | Key = SHA-256(H(pw)‖H(idA)‖H(idB)‖X‖Y‖K) (source) ✅; **no key confirmation** (caller's job) ✅; self-declared unaudited and probably not constant-time ✅; deps `curve25519-dalek ^4`, `hkdf 0.12`, `sha2 0.10` ✅ |
| snow (Noise) | 0.10.0 (2025-07-19) ✅ | Apache-2.0 OR MIT ✅ | 1.85 ✅ | Not chosen (section 5) |
| mdns-sd | 0.21.5 (2026-10-05) ✅ | Apache-2.0 OR MIT ✅ | 1.71 ✅ | No async-runtime dependency; actively released ✅ |
| prost / prost-build | 0.14.4 (2026-06-07) ✅ | Apache-2.0 ✅ | 1.85 ✅ | tonic 0.14.6 (MIT, MSRV 1.88) ✅ **not** used on the peer path |
| postcard / bincode | 1.1.3 (2025-07) / 3.0.0 (2025-12) ✅ | MIT OR Apache-2.0 / MIT ✅ | unknown / 1.85 ✅ | Rejected (section 5) |
| ed25519-dalek | 3.0.0 (2026-07-06) ✅ | BSD-3-Clause ✅ | 1.85 ✅ | Not needed: `ring` already does Ed25519 keygen/sign/verify |
| keyring | 4.2.0 (2026-08-29) ✅ | MIT OR Apache-2.0 ✅ | 1.88 ✅ | Not adopted in M3 (section 3) |
| rcgen | 0.14.10 (2026-08-28) ✅ | MIT OR Apache-2.0 ✅ | 1.88 ✅ | Not needed: raw public keys mean no X.509 at all |

Dependency-count proxy for binary size (`cargo tree -e normal`, scratch projects, **no build**): quinn 0.11.12 + rustls(ring) + spake2 + mdns-sd + prost = **91** crates; iroh 1.3.0 default = **315**; libp2p 0.57 with quic/tls/noise/mdns/identify/ping/dcutr/relay = **254**. Binary size itself `unverified` (no build permitted this session).

Pinned engine facts: `llama-b11609-bin-macos-arm64` ships `ggml-rpc-server` and `libggml-rpc` ✅; `ggml-rpc-server --help` shows `-H/--host (default 127.0.0.1)`, `-p/--port (default 50052)`, `-d/--device`, `-t/--threads`, `-c/--cache`; **no memory-cap flag** ✅. Started on this Mac it listened on `127.0.0.1:50999` only (lsof) ✅. `llama-server` accepts `--rpc host:port[,…]`, `-ngl`, `-ts/--tensor-split` ✅. RPC protocol version string of b11609 `unverified` (stdout buffering swallowed the banner; COMPETITIVE_MATRIX saw v3.6.0 at b8241 ⚠️).

## 3. Decision

### D1 Identity (F-06-01)
1. One **Ed25519 device keypair**, generated and used through `ring` (same crypto provider as TLS, one Ed25519 implementation in the binary). `PeerId = [u8; 32]` raw public key.
2. **Storage: file, not keychain.** `Paths::identity()` already resolves `COMMON_HOME/identity` (common-core). Layout: `identity/device.key` (PKCS#8 v2 DER, mode `0600`, directory `0700`, written tmp+rename), `identity/device.pub` (64 lowercase hex). Secret type has no `Serialize`, `Debug` prints `Ed25519Secret(<redacted>)`; the F-06-01 grep test runs over logs and diagnostics bundles.
   - Keychain (macOS Keychain / Secret Service / Windows Credential Manager via `keyring` 4.2.0) rejected for M3: Keychain ACLs bind to the code signature and we have no signing identity (B-008), so every rebuilt unsigned daemon re-prompts or is denied; headless/launchd daemons and Linux servers without a D-Bus session cannot use it; MSRV 1.88 > workspace 1.85; `0600` is what the F-06-01 criterion also accepts. Revisit when signed installers exist (M13).
3. **Fingerprint shown to users:** the key itself, no hash step. Canonical id = 64 lowercase hex (same encoding as iroh's `EndpointId` display ⚠️, so a later iroh adapter needs no conversion). Short label = first 12 hex as `xxxx-xxxx-xxxx` (labels only; never a trust decision). The pairing confirmation screen shows the full 64 hex in 8 groups of 8.

### D2 Pairing (F-02-02)
Protocol `common-pair/1`, run over a QUIC connection on a **separate pairing endpoint** (own UDP port) that exists only while the device is in pairing mode (default 120 s window):

| Step | Initiator A ("pair this device", shows code) | Joiner B (enters code) |
|---|---|---|
| 0 | `common pair` → generates code, opens pairing endpoint, advertises `pair=1,pairport=` in mDNS TXT | picks A from discovery (or QR) |
| 1 | QUIC/TLS 1.3 handshake, raw public keys both ways, verifier **accepts any key on the pairing endpoint only**; both learn the other's `PeerId` from the TLS RPK | same |
| 2 | SPAKE2 `start_a(pw=code, idA=PeerId_A, idB=PeerId_B)` → 33-byte message | `start_b(...)` → message |
| 3 | `K = finish(msg_B)`; `ekm = conn.export_keying_material(32, "common-pair/1 confirm", "")` (quinn `Connection::export_keying_material` ✅) | same on its own TLS session |
| 4 | sends `HMAC-SHA256(K, "A" ‖ ekm ‖ PeerId_A ‖ PeerId_B)` | sends the `"B"` variant; each verifies the other |
| 5 | pins `PeerId_B` into SQLite `peers` (name, group, paired_at); code invalidated; audit `pairing_succeeded` | pins `PeerId_A` |

- **MITM detection:** a relay that terminates TLS on both legs presents its own RPK, so `idA/idB` and `ekm` differ per leg; without the code it cannot compute `K`, and even with the code the confirmation MACs bind each leg's exporter value, so step 4 fails on both sides (test `mitm_relay_detected`).
- **Code:** 6 decimal digits, single use, valid 120 s. QR payload = `common-pair:v1:<PeerId_A hex>:<ip:pairport>:<128-bit secret hex>`; the secret replaces the 6-digit code as the PAKE password.
- **Rate limit:** more than 5 failed confirmations within any 60 s window → pairing endpoint closes, code discarded, `PairingLocked` for the remainder of the window, audit `pairing_locked` (test `brute_force_rate_limited`). Expected attacker success per window ≤ 5 × 10⁻⁶.
- **Why spake2 0.4.0 despite its caveats:** the PAKE protects a one-time 120 s secret and only authenticates the pairing transcript; long-term security rests on TLS 1.3 + Ed25519 pinning. Its key already hashes both identities and both messages (transcript binding ✅). We add the confirmation round it leaves to the caller. Not RFC 9382 wire-compatible (python-spake2 variant) ✅, which is irrelevant for Common-to-Common pairing. Re-evaluate `spake2` 0.5 when released; OPAQUE/CPace rejected (no need for augmented PAKE; CPace has no mature Rust crate `unverified`).

### D3 Transport (F-02-03, F-06-03): **QUIC via quinn 0.11 + rustls 0.23, raw public keys, mutual auth, TLS 1.3 only**
- `rustls` features `["ring","std","logging"]`, **no `tls12`, no `aws-lc-rs`** (avoids the C/cmake build on the 8 GB dev Mac; one feature flag switches provider if `ring` stalls). `quinn` features `["runtime-tokio","rustls-ring","log"]`, no `platform-verifier`.
- Server: `ServerConfig::builder_with_provider(ring).with_protocol_versions(&[&TLS13]).with_client_cert_verifier(PinnedPeerVerifier).with_cert_resolver(AlwaysResolvesServerRawPublicKeys)`. Client: `.dangerous().with_custom_certificate_verifier(PinnedPeerVerifier).with_client_cert_resolver(AlwaysResolvesClientRawPublicKeys)`. `PinnedPeerVerifier::requires_raw_public_keys() = true`, `client_auth_mandatory() = true`, schemes `[ED25519]`, verification via `verify_tls13_signature_with_raw_key`; it parses the SPKI into a `PeerId` and accepts iff `peers` contains it and `revoked` does not. **An unpinned key fails inside the TLS handshake** (test `unpinned_key_rejected`). No X.509, no CA, no rcgen.
- Two endpoints, one verifier policy each: workload endpoint (strict, ALPN `common/1`, default UDP 8193, created only when `[network] enabled = true`, default `false`) and pairing endpoint (accept-any, ALPN `common-pair/1`, ephemeral port, exists only in pairing mode). Avoids ALPN-conditional verification inside one verifier.
- 0-RTT disabled (no `into_0rtt`, `max_early_data_size = 0`), datagrams disabled, `keep_alive_interval = 2 s`, `max_idle_timeout = 6 s`, `stream_receive_window = 8 MiB`, `receive_window = 64 MiB`, `max_concurrent_bidi_streams = 64` (all `[network.transport]` config; defaults chosen for F-02-05, tune after measurement).
- **No plaintext path, by construction and by test:** `common-network` owns exactly one socket kind (the quinn `Endpoint`). The only TCP code in `common-distributed` is the loopback RPC bridge (D6), whose bind/connect asserts `ip.is_loopback()`. CI test `no_plaintext_mode_exists` greps `TcpListener::bind|TcpStream::connect|UdpSocket::bind` across both crates and fails on anything outside that module. QUIC has no cleartext mode; a client offering another ALPN or TLS 1.2 fails the handshake (`downgrade_attempt_fails`).
- **Proving a capture contains no plaintext (F-02-03 acceptance):** run a remote inference whose prompt and expected completion contain unique canaries (`CANARY-<random>`); `tcpdump -i lo0 -w` (SINGLE-HOST) or on the LAN interface; assert zero matches for the canaries (UTF-8 and UTF-16) in every UDP payload. Positive control in the same test: the identical search against a capture of the local `llama-server` HTTP hop on loopback **must** find the canaries, proving the search works. Debug-only `rustls::KeyLogFile` + Wireshark decryption may be used to show the ciphertext did carry the canaries (never in release builds). QUIC Initial packets are only obfuscated, not secret: they carry the ClientHello (ALPN, no SNI secret); the test asserts no canary there too.
- **M9 (F-02-06) adds**, without changing M3 code paths: a `Transport` seam at the level of `(Connection, open_bi, accept_bi)` so that iroh (`iroh::endpoint::Connection` has the same shape) can be a second implementation for hole punching and relays. Identity is already byte-compatible (Ed25519, hex id, RFC 7250 on both sides ✅). The alternative is a dumb UDP relay of our own: QUIC end-to-end encryption continues through it, so the relay sees ciphertext only, by construction. Decide in M9 with a NAT-matrix test.

### D4 LAN discovery (F-02-01): `mdns-sd` 0.21.5
- Service `_common._udp.local.`, instance `<short id 12 hex>`, SRV → workload port, TXT (all ≤ 255 B, record < 1472 B): `v=1`, `id=<64 hex PeerId>`, `pair=0|1`, `pairport=<n>` (only while pairing), `proto=1.0`, `ts=<unix s>`, `sig=<Ed25519 over the other TXT values, base64>`. **No hostname beyond SRV, no model names, no hardware, no user names** — those go only over the authenticated channel.
- Unsigned, badly signed or > 300 s old records are ignored; records whose `id` is not pinned are listed as **untrusted** and can never receive work (the verifier enforces this independently). Spoofing only yields a failed handshake.
- Config: `[network] mdns = false` disables advertising and browsing; `COMMON_MDNS=0` for tests; `[network] enabled = false` (default) implies no mDNS.

### D5 Protocol framing and versioning (F-08-10, F-03-03): **length-prefixed protobuf (prost) for control; fixed binary header for bulk**
- Control stream = first bidirectional stream opened by the connector. Frame = `u32 BE length ‖ protobuf ControlMessage { oneof: Hello, Capabilities, Status, SessionOpen/Accept/Close, StreamOpen, Revocation, Error }`. Hard bound 1 MiB per control frame (close with `FRAME_TOO_LARGE`); `Capabilities` ≤ 64 KiB (F-02-04).
- `Hello{major=1, minor, common_version, peer_id, nonce}` first in both directions. Major is also the ALPN (`common/1`): an incompatible major fails ALPN negotiation and maps to `ErrorCode::ProtocolIncompatible` with a message naming both versions and the device to update. Compatible minors use `min(minor)`. Unknown protobuf fields and unknown `oneof` variants are skipped by prost ✅ (not preserved on re-encode ⚠️, acceptable).
- Schemas live in `proto/common/v1/*.proto`; **generated Rust is committed** (regenerated by `scripts/gen_proto.sh`), so no build-time `protoc` dependency. tonic/gRPC is **not** on the peer path: gRPC needs HTTP/2 over TCP or HTTP/3, i.e. a second transport stack.
- Bulk/activation frames (Common-native stage protocol, later than M4): fixed 32-byte header `magic "CACT", u8 version, u8 dtype, u16 flags, u32 request_id, u32 seq, u32 n_tokens, u32 hidden_dim, u32 payload_len, u32 crc32c` + raw payload → framing overhead ≤ 64 B as F-03-03 requires; payload bound 64 MiB default; wrong `hidden_dim` or oversize is rejected without a crash.
- postcard/bincode rejected: not self-describing, adding a field breaks old peers, which F-08-10 forbids; bincode 3.0 is a 10-month-old major with a new API. Protobuf also serves the mobile companion and web app.

### D6 Carrying llama.cpp RPC for genuine layer partitioning (M4, F-03-01, F-03-03)
```
head host H                                   remote host R
llama-server --rpc 127.0.0.1:pL ──TCP lo0──▶ common RpcBridge(listen 127.0.0.1:pL)
                                             │ one QUIC bidi stream per accepted TCP connection
                                             ▼  (authenticated, encrypted; StreamOpen{RPC_TUNNEL, session_id})
                              common ◀──QUIC/UDP LAN──▶ common ──TCP lo0──▶ ggml-rpc-server -H 127.0.0.1 -p pR
```
- **Remote:** Common supervises `ggml-rpc-server -H 127.0.0.1 -p <ephemeral> [-d dev] [-t n]` exactly like `llama-server` in ADR-0002 (pinned archive, scrubbed env, log parsing); startup test asserts the listening socket is loopback (`rpc_server_binds_loopback_only`). It is started per accepted `SessionOpen{model_sha256, layer_range, est_bytes, engine_build}` and only for a peer whose group policy allows `pooled_inference` (F-06-02 default deny); `engine_build` must equal the local pinned build, since the ggml RPC protocol version is coupled to the llama.cpp build.
- **Head:** `RpcBridge` listens on `127.0.0.1:<pL>` and the supervised `llama-server` is started with `--rpc 127.0.0.1:<pL>` plus `-ngl`/`--tensor-split` from the plan. Each accepted TCP connection → `open_bi()` on the peer connection, `StreamOpen` frame, then `tokio::io::copy_bidirectional`. TCP FIN ↔ stream FIN; TCP RST ↔ `reset(code)`. The number of TCP connections ggml opens is an implementation detail we do not depend on (`unverified`; bounded by `max_tunnels_per_session = 8`).
- **Bounds:** per-session `max_tunnels` 8; tunnel idle timeout 120 s (a long remote `GRAPH_COMPUTE` is silent on the socket ⚠️ tune by measurement); per-peer bytes/s token bucket from F-06-04 policy (default unlimited on LAN; model-load bursts of GBs must not be throttled by default); QUIC flow-control windows from D3 bound memory in flight.
- **Failure semantics:** peer connection lost (6 s idle) → bridge resets all local TCP connections → ggml's RPC client errors; llama.cpp aborts on many RPC errors ⚠️ → the ADR-0002 supervisor sees the engine exit, fails in-flight requests with `ErrorCode::PeerLost`, and re-plans. Automatic fallback to local-only is allowed **only** with the identical model hash and quantization and only if it fits; otherwise the error is surfaced (spec: no silent substitution; F-02-10). Remote side: tunnel closed → rpc-server client disconnect; if rpc-server exits (known crash reports, COMPETITIVE_MATRIX) the supervisor restarts it and the session is marked failed. Both sides write audit entries.
- **Honest data-exposure note (spec section 9):** R receives, in plaintext inside `ggml-rpc-server`, the weights of its layer range (uploaded by the head at load, or read from its `-c` tensor cache on disk `unverified` details) and every activation crossing the boundary. Encryption protects the wire, not the peer. The UI must say "device R will compute part of this model and can see intermediate data" before the first pooled run; the tensor cache on R is a model-redistribution event for license policy (X-08).
- **Measurement plan.** The bridge counts bytes per direction per tunnel and session, exported as Prometheus metrics (crate already in the workspace) and written into the run record with `quinn::Connection::stats()` (udp tx/rx, lost, rtt ✅).
  - *bytes per token per boundary (F-03-03):* after prefill, decode N = 64 tokens greedily; `(Δbytes_tx, Δbytes_rx)/N`. Compare with the arithmetic floor `hidden_dim × element_size` (llama.cpp inter-layer activations are f32 ⚠️, e.g. 4096 × 4 = 16 KiB). Run two models with different `hidden_dim` to separate the per-token constant (ggml re-serialises the graph per compute call ⚠️) from the activation term. Report both.
  - *activation transfer overhead (bench):* TPOT with the split vs the same model local-only (when it fits); and `llama-server --rpc` **directly** to `ggml-rpc-server` over loopback TCP (plaintext control, SINGLE-HOST only) vs through the Common tunnel over loopback QUIC → isolates CPU cost of QUIC + AEAD, the lower bound for H1 (≤ 5 %). The LAN number needs the second machine (B-002).
  - *load transfer:* bytes and seconds for cold (weights uploaded) vs warm (`-c` cache) load.
  - **Ledger consequence:** F-03-03's "equal to hidden_dim × element_size (+ ≤ 64 B)" is satisfiable only by the Common-native frame of D5, not by RPC carriage, which has its own protocol overhead. Proposal for the owner: split F-03-03 into (a) native frame format (unit-testable in M4 without a second host) and (b) measured bytes/token on RPC carriage with the overhead ratio reported. Not changed here (D-005: no silent criterion changes).

### D7 Health, capabilities, revocation, audit
- **Health (F-02-05):** liveness = QUIC keep-alive 2 s / idle 6 s; three missed keep-alives = connection timeout ≤ 6 s (`kill_peer_detected_within_bound`). State machine `Connected → Unhealthy → Reconnecting (1,2,4,8…30 s backoff) → Connected`. A `Status` control message every 10 s carries volatile load (free memory, active sessions, queue depth); it refreshes capability fields, it is not the liveness signal.
- **Capabilities (F-02-04), sent right after `Hello`, ≤ 64 KiB, Ed25519-signed so M9 can relay/cache it (F-02-11):**
  ```proto
  message Capabilities { uint32 schema_version=1; bytes peer_id=2; string common_version=3;
    repeated ProtocolVersion protocols=4;        // name, major, minor
    Hardware hardware=5;                         // same struct as `common hardware --json` (F-01-01)
    uint64 free_memory_bytes=6; repeated EngineBuild engines=7;   // "llama.cpp", "b11609", "macos-arm64", rpc_proto
    repeated bytes cached_model_sha256=8; repeated string groups=9; uint64 issued_at=10; bytes signature=11; }
  ```
  Test: the record received equals the peer's own `common hardware --json`; a 64 KiB + 1 record is rejected and logged.
- **Revocation (F-06-05):** `common peers revoke <id>` deletes the pin, inserts into `revoked(peer_id, seq, revoked_by, sig, at)`, closes every connection to that peer with close code `REVOKED` (≤ 5 s via `Connection::close`), and sends `Revocation{...}` signed by the revoker to every connected group member, which verifies the signature against a pinned member key and applies the same steps. On every reconnect peers exchange `revocations_since(seq)` so offline members converge. The verifier consults `revoked` on each handshake, so reconnection fails (`revoked_key_cannot_reconnect`). M3 policy: any member of a group may revoke any other member (household model); per-group admin policy is an open question (F-02-09).
- **Key rotation (spec section 9):** `Rotation{old_id, new_id, sig_old, sig_new}` over the control stream, treated like a pin update; designed here, implemented when F-06 rows require it.
- **Audit hooks (F-06-10):** `common_observability::audit::record(AuditEvent)` appends one JSON line to `COMMON_HOME/logs/audit.jsonl` (0600). Events: `pairing_{started,succeeded,failed,locked}`, `peer_{connected,disconnected{reason},rejected{reason}}`, `peer_revoked`, `session_{open,close}{peer_id,model_sha256,layers}`, `rpc_tunnel_{open,close}{bytes_tx,bytes_rx,duration}`, `remote_request{request_id,peer_id,bytes}`. Never prompt text, token strings or key material; the bundle redaction scan (F-06-10) runs over this file too.
- **Error codes** added to the existing `common_core::ErrorCode`: `PeerUnauthorized`, `PeerRevoked`, `PairingLocked`, `PairingFailed`, `PeerLost`, `ProtocolIncompatible`, `FrameTooLarge`.

## 4. Alternatives considered

| Criterion | **quinn 0.11 + rustls 0.23 (chosen)** | iroh 1.3.0 | rust-libp2p 0.57 | Noise (snow 0.10) over TCP |
|---|---|---|---|---|
| License / MSRV | MIT OR Apache-2.0 / 1.85 ✅ (workspace stays at 1.85) | MIT OR Apache-2.0 / **1.91** ✅ (forces MSRV bump) | MIT / 1.88 ✅ | Apache-2.0 OR MIT / 1.85 ✅ |
| Maturity | QUIC stack used by iroh itself (forked as `noq`) ✅; 0.11 since 2024 | 1.0 in 2026-06, 1.3.0 by 2026-09: fast-moving ⚠️ | mature but large | mature crypto, no transport |
| Handshake control | Full: pinned set and revocation enforced **inside** TLS ✅ | Any key completes TLS; authorise after `accept()` via `remote_id()` ✅ (then close) | libp2p-tls X.509 with extension; `PeerId` is a multihash, not raw Ed25519 | Full, but X25519 static keys ≠ Ed25519 identity (conversion wart) |
| Dependency count (proxy) | 91 (quinn/rustls/ring already in `Cargo.lock` ✅) | 315 | 254 | small, but we must build streams, flow control, keep-alive ourselves |
| Central services by default | none | `presets::N0` → n0.computer DNS/pkarr/relays ✅; must use `presets::Minimal` + `RelayMode::Disabled` for X-20 | none required | none |
| LAN discovery | mdns-sd (separate, ours) | external crate `unverified` | built-in mDNS behaviour | mdns-sd |
| NAT traversal / relays (M9) | **not included**: own UDP relay or iroh adapter later | included; ~90 % hole-punch vendor-reported ⚠️; relays carry ciphertext (by construction, `unverified` by us) | DCUtR + circuit relay `unverified` | none |
| Two QUIC stacks risk | — | `noq` fork beside quinn if mixed with reqwest/h3 ⚠️ | uses quinn ✅ | — |

**Why not iroh now:** M3 is LAN-only and offline-first, where iroh's differentiators (hole punching, relays, global discovery) are unused, while its costs (MSRV 1.91, 3.5× dependency count, central-service presets that must be switched off, authorisation after the handshake, a forked QUIC stack) are paid immediately. Because both use RFC 7250 Ed25519 raw public keys, the M9 switch, if measurements favour it, is an adapter, not a redesign. **Why not libp2p:** we need two protocols between a handful of trusted devices, not a swarm framework; identity encoding and X.509 shim add friction. **Why not Noise:** QUIC gives streams, flow control, loss recovery, keep-alive and idle timeout (F-02-05) for free.

## 5. Test plan

| Test (ledger name) | SINGLE-HOST (two daemons, `COMMON_HOME=/tmp/{a,b}`, UDP 40001/40002, distinct identities) | Needs second physical machine (B-002) |
|---|---|---|
| `keygen_persist_load`, `file_permissions_0600`, `private_key_never_logged` | yes, fully | — |
| `two_processes_discover_within_5s` (mDNS) | yes, labelled SINGLE-HOST; both bind 5353 on the real interface ⚠️ confirm mdns-sd multi-instance behaviour | real L2/Wi-Fi discovery time |
| `pair_two_peers_end_to_end`, `wrong_code_fails`, `brute_force_rate_limited`, `pake_transcript_binding` | yes | — |
| `mitm_relay_detected` | yes: in-process relay terminating TLS on both legs | — |
| `encrypted_stream_roundtrip`, `unpinned_key_rejected`, `downgrade_attempt_fails`, `no_plaintext_mode_exists` (grep) | yes | — |
| `packet_capture_contains_no_plaintext_prompt` (+ positive control) | yes on `lo0`; **needs `sudo tcpdump`** (`/dev/bpf*` is root-only on this Mac ✅; Linux CI: `CAP_NET_RAW`) | capture on the LAN NIC for the real session |
| `capabilities_exchanged_after_handshake`, `oversized_capability_message_rejected`, `version_negotiation`, `old_new_peer_interop`, `unknown_fields_ignored_safely` | yes | — |
| `kill_peer_detected_within_bound` (SIGKILL), `revoked_peer_disconnected_within_5s`, `revoked_key_cannot_reconnect`, persistence across restart | yes | partition (unplug) variant |
| `rpc_server_binds_loopback_only`, tunnel open/close, idle timeout, byte counters | yes | — |
| Two-node split generates tokens; per-process memory per layer range | yes **but recorded SINGLE-HOST only**; never satisfies F-03-01 | **required** for F-03-01, Gate D |
| bytes/token/boundary; encrypted vs plaintext overhead | loopback numbers = CPU-cost lower bound only | **required** for bench:two-node-lan, H1 ≤ 5 % on 1 GbE |
| X-20 WAN-blocked LAN cluster | not meaningful on one host | **required** |

Fixtures: the TinyStories models (`docs/research/TEST_MODELS.md`) for all SINGLE-HOST functional tests; no mocked inference (AGENTS.md).

## 6. Consequences
- New direct dependencies: `quinn`, `rustls`, `ring` (already resolved in `Cargo.lock`), `spake2`, `mdns-sd`, `prost`, `hmac`/`sha2` (RustCrypto, for confirmation MACs), `rusqlite` (bundled) for `peers`/`revoked`. Workspace `rust-version = "1.85"` is unchanged. No C/cmake builds added.
- New crates per spec section 6: `common-identity` (D1, D2, pins, revocation), `common-network` (D3–D5, D7), `common-distributed` (D6, M4), `common-observability::audit`, `proto/` with committed generated code.
- Every pin here is re-verified against the upstream LICENSE file before Gate G (ADR-0001 rule); cargo-deny allowlist must include `ISC` and `BSD-3-Clause` (already proposed).
- The remote peer's plaintext view of activations/weights is a documented product fact (UI consent text, X-08 policy), not a bug.

## 7. Risks and open questions
1. **rustls 0.24** (dev release the day of this ADR) and the quinn release that follows: pin `0.23`/`0.11`, budget one migration before M13. **ring** inactivity: provider switch to `aws-lc-rs` is one feature flag but reintroduces a C build.
2. **spake2 0.4.0** is 3 years old, unaudited, not constant-time (self-declared). Mitigated by one-time short-lived codes, lock-out and transcript-only use; track 0.5.
3. `ggml-rpc-server` has **no memory cap flag** in b11609; remote memory is bounded only by the session estimate and RSS monitoring. llama.cpp aborts on RPC errors ⚠️ → every peer loss is an engine restart until a native stage protocol exists.
4. The ggml RPC protocol version is coupled to the engine build; only identical pinned builds may pool. Capability record enforces it; cross-platform builds need B-004 hosts.
5. mDNS with two instances on one host, Wi-Fi power-save RTT jitter versus the 6 s idle timeout, macOS UDP throughput (GSO/GRO) — all unmeasured; defaults are configurable.
6. Open: who may revoke in enterprise groups (F-02-09 policy); whether M9 uses an iroh adapter or an own relay; whether to split F-03-03 as proposed in D6; whether the desktop app's embedded daemon needs the pairing endpoint on IPv6 link-local too.

## 8. Verification record
- **Verified (✅):** all versions, dates, licenses and MSRVs in section 2 (crates.io API + `cargo info`, 2026-10-14); rustls RPK API and iroh RPK/presets/`RelayMode::Disabled` (downloaded crate sources); quinn `peer_identity`/`export_keying_material`/`stats` (source + docs.rs via Context7); spake2 0.4.0 key derivation and absence of confirmation (source + docs.rs); `ggml-rpc-server` flags, default loopback bind and presence in the pinned archive (run on this Mac); dependency counts (`cargo tree`, no build); `/dev/bpf0` root-only; quinn/rustls/ring already in `Cargo.lock`.
- **Unverified / secondary:** binary-size deltas; iroh relay ciphertext-only claim and hole-punch rate (vendor); iroh mDNS crate behaviour; b11609 RPC protocol version string; ggml RPC connection count, graph re-serialisation per compute, inter-layer activation dtype, `-c` cache semantics, abort-on-error behaviour; mdns-sd multi-instance on one host; prost unknown-field handling details beyond "skipped on decode".

## 9. Implementation notes (2026-10-15)

What M3 built, decision by decision, and the tests that hold each rule in place. Every test is a unit test or a **SINGLE-HOST** test on the owner's one Mac. None is two-machine evidence (B-002). SINGLE-HOST evidence never satisfies a criterion that needs two machines (F-02-01 LAN discovery, F-03-01, F-03-02, Gate D, two installs pairing through the UI). Rows whose criteria do not depend on topology (pairing cryptography, policy, revocation, capture, liveness) were marked `VERIFIED` from SINGLE-HOST tests on 2026-10-15; see `docs/REQUIREMENTS.csv`. Test names are function names; the file is given once per group. The 2026-10-15 two-daemon run is in `docs/evidence/m3-single-host-2026-10-15.md`.

### 9.1 D1 Identity
- One Ed25519 key per device, through `ring`. `identity/device.key` is PKCS#8 v2, mode 0600, in a 0700 directory, written tmp + rename. A loosened mode is tightened on load. A corrupt key file is an error, never a silent new identity.
- The trust store is `identity/peers.json` (0600), not SQLite. It holds pins, groups, revocations, superseded revocations and the next revocation `seq`.
- Writers take `peers.json.lock`, re-read the file, then write by rename. Readers reload when the file changes. So `common peers revoke` in another process reaches the daemon's TLS verifier at its next handshake.
- The secret has no `Serialize`; `Debug` prints `Ed25519Secret(<redacted>)`. Key bytes are not zeroized on drop (open, 9.15).
- Tests, `crates/common-identity/src/lib.rs`: `keygen_persist_load`, `file_permissions_0600`, `corrupt_key_file_is_an_error_not_a_new_identity`, `private_key_never_logged`, `debug_redacts_secret`, `sign_verify_roundtrip`, `spki_round_trip`, `peer_store_persists_across_reopen`, `stale_handle_does_not_undo_another_handles_revocation`, `concurrent_handles_lose_no_update`, `failed_write_leaves_store_unchanged`. `crates/common-network/tests/invariants.rs`: `private_keys_never_formatted`. `crates/common-cli/tests/mesh_single_host.rs`: `private_key_absent_from_real_logs_and_bundle_single_host`, with its positive control `key_scan_catches_every_claimed_encoding_synthetic_key`.

### 9.2 D2 Pairing
The order, as built:
1. The initiator A opens a pairing endpoint: its bind address, an ephemeral UDP port, ALPN `common-pair/1`, a verifier that accepts any key. The window is 120 s. mDNS advertises `pair=1` and `pairport` while it is open.
2. The joiner B dials from its own workload endpoint, so pairing adds only A's socket. With a QR payload, B's verifier accepts only the key in the QR. Any other device fails **inside the TLS handshake**, before SPAKE2: `pairing_failed`, audit reason `qr_peer_mismatch`, nothing pinned on either side, no revocation touched.
3. B → A: one selector byte (0 = 6-digit code, 1 = QR secret) and SPAKE2 `msg_B`. A → B: `msg_A`. Frames are `u8 length ‖ bytes`.
4. Both read `ekm` = TLS exporter, 32 bytes, label `common-pair/1 confirm`. Each sends `HMAC-SHA256(K, side ‖ ekm ‖ idA ‖ idB)` and checks the other's MAC in constant time.
5. **Only after a verified MAC** do both send `u16 workload port ‖ device name` (at most 64 bytes, control characters removed). A party without the code never learns either.
6. Both call `pin_after_pairing` (which also supersedes local revocations of that id, 9.9) and audit `pairing_succeeded`.

Bounds:
- The code is 6 digits, uniform (rejection sampling), single use. The QR payload is `common-pair:v1:<id hex>:<ip:port>:<32 hex secret>`. When the endpoint binds every interface, the QR carries this host's LAN address instead of `0.0.0.0` (if it has one).
- A handles one attempt at a time; each has 10 s. Every failed exchange counts, not only wrong MACs. The 6th failure within 60 s closes the endpoint with `pairing_locked` (close code `PL`).
- The pairing endpoint does not send QUIC Retry (9.3). The lockout and the one-at-a-time loop bound it.
- Turning networking off closes an open window (`cancelled`). `common peers pair|join` refuse while a daemon runs; pairing then goes through the daemon API.
- Tests, `crates/common-network/tests/pairing.rs`: `correct_code_pairs_and_pins_both_sides_single_host`, `wrong_code_fails_and_pins_nothing`, `brute_force_rate_limited`, `wrong_code_learns_no_name_or_port`, `mitm_relay_detected`, `qr_payload_from_wildcard_bind_pairs_single_host`, `qr_expected_peer_mismatch_pins_nothing_single_host`. `crates/common-network/src/pairing.rs`: `code_and_secret_have_the_documented_shape`, `qr_payload_parses_and_rejects_garbage`, `qr_address_is_routable`, `confirmation_mac_binds_side_exporter_and_ids`, `names_are_printable_and_bounded`. `crates/common-api/tests/mesh.rs`: `pairing_pins_both_sides_and_revoke_closes_the_connection_single_host`, `qr_join_pairs_and_a_qr_for_another_device_pins_nothing_single_host`, `turning_networking_off_closes_an_open_pairing_window`. `crates/common-api/src/mesh.rs`: `routable_replaces_only_the_unspecified_address`.

### 9.3 D3 Transport
Built as decided: quinn 0.11.12, rustls 0.23.45 with `ring`, RFC 7250 raw public keys both ways, mutual auth, TLS 1.3 only, Ed25519 only, no X.509. In detail:
- **The verdict is inside the handshake.** Unpinned key → alert `unknown_ca` → `peer_unauthorized`. Revoked key → `certificate_revoked` → `peer_revoked`. No stream opens. A dialler accepts only the one key it expects; another pinned key fails too.
- **No session resumption.** No tickets, no server session cache, client resumption off. A resumed TLS 1.3 handshake skips the client verifier, so a revoked device could otherwise return with a cached ticket.
- 0-RTT off, datagrams off. Windows: 8 MiB per stream, 64 MiB per connection, 64 bidirectional streams.
- **QUIC Retry before TLS.** The workload endpoint answers every Initial from an unvalidated source address with a Retry. A spoofed source cannot answer it, so it costs no TLS work and no audit line.
- Inbound admission order: Retry → TLS verifier → store check → Hello/Capabilities within 10 s → register → store check again. The last check closes a connection whose device was revoked between the handshake and registration.
- Inbound `peer_rejected` audit lines: at most 20 per minute in one global window. The rest are counted, and the count rides on the next written line.
- rustls's `tls12` feature **is compiled in**: reqwest (hyper-rustls) enables it. Every Common config pins TLS 1.3, and a TLS 1.2-only config cannot start a QUIC session. Section 3's "no `tls12`" holds for configs, not for the binary.
- The workload endpoint binds `0.0.0.0:8193` only when networking is on. It is off by default. The only socket type in `common-network/src` is the quinn `Endpoint`.
- The capture test uses an in-process UDP relay that records every datagram, not `tcpdump` (it needs sudo). The scan covers UTF-8 and UTF-16LE. Its positive control finds the canary in the plaintext frames.
- Tests, `crates/common-network/src/tls.rs`: `spki_round_trip_and_rejects_other_algorithms`, `handshake_pinned_keys_succeeds_single_host`, `unpinned_key_rejected`, `revoked_key_rejected`, `revoked_client_cannot_resume_with_cached_session`, `server_key_not_pinned_by_client_rejected`, `wrong_alpn_fails`, `tls12_downgrade_attempt_fails`. `crates/common-network/tests/peer_transport.rs`: `network_disabled_refuses_to_listen`, `handshake_pinned_keys_succeeds_single_host`, `unpinned_key_rejected`, `revoked_key_rejected`, `revoked_key_cannot_reconnect_with_cached_session`, `revoked_during_control_handshake_is_closed`, `rejection_audit_is_rate_limited`. `crates/common-network/src/transport.rs`: `rejection_audit_lines_are_capped_per_minute`. `crates/common-network/tests/invariants.rs`: `no_plaintext_mode_exists`. `crates/common-network/src/lib.rs`: `config_default_is_off_and_scan_finds_both_encodings`. `crates/common-network/tests/remote_single_host.rs`: `packet_capture_contains_no_plaintext_prompt_single_host`, `decode_tpot_overhead_measured_single_host`.
- No test isolates the Retry. Every handshake test against a `Node` passes through it.

### 9.4 Connection dedup (not in the proposal)
Both daemons' sync loops dial each other, so crossed dials are normal. The rule:
- The connection initiated by the **lower `PeerId`** survives. Of two from the same initiator, the older survives.
- Only the initiator of the losing connection retires it, and only once it holds both. So no side cuts a handshake the other is still finishing.
- A retired connection gets no new work. It closes with code 0 at the first 500 ms check that finds no request in flight. A request already running on it finishes.
- `Node::connect` returns the live connection instead of dialling when one exists.
- Known limit: a request opened on the duplicate at the exact moment of an idle check fails with `peer_lost`. A GOAWAY message would fix it.
- Tests, `crates/common-network/src/transport.rs`: `duplicates_resolve_to_the_lower_ids_connection`. `crates/common-network/tests/peer_transport.rs`: `crossed_dials_keep_one_connection_single_host`, `busy_duplicate_is_retired_not_cut_single_host`. `crates/common-api/src/mesh.rs`: `survivor_is_the_connection_the_lower_id_dialled`.

### 9.5 D4 Discovery
The record, as built:
- Service `_common._udp.local.`. Instance = first 12 hex of the id. Host `common-<12 hex>.local.`. SRV port = workload port.
- TXT: `v=1`, `id=<64 hex>`, `pair=0|1`, `pairport=<n>` (only while pairing), `proto=1.0`, `ts=<unix s>`, `sig`. Nothing else: no hostname beyond SRV, no models, no hardware, no user names.
- `sig` is hex (not base64) Ed25519 over the canonical string `v=1\nid=…\nport=<SRV port>\npair=…\npairport=…\nproto=…\nts=…`. The SRV port is signed, so a replay cannot point a live entry at another port.
- The advertiser re-signs and re-announces every 120 s, so `ts` stays fresh. A specific bind address is the only address advertised; `0.0.0.0` advertises every interface address.

The browser:
- Drops a record with an unknown version, a missing field, a bad signature, or `ts` more than 300 s from its own clock in either direction.
- Keeps at most 256 entries and evicts stale ones. An entry for a known id at another address is replaced only by a strictly newer `ts`.
- Never lists this device's own id (its advert comes back over multicast loopback).
- Picks loopback and link-local addresses only when nothing else is advertised; then IPv4 before IPv6; then the lowest address.
- Marks `trusted` only when the id is pinned here. That is a label; the TLS verifier makes the trust decision.
- Is off when networking is off, `mdns = false`, or `COMMON_MDNS=0`.

The daemon dials a pinned device at its fresh mDNS address first, then at the pinned `last_addr`. `last_addr` changes only when this device dialled that address and that dial's connection is the one in use.
- Tests, `crates/common-network/src/discovery.rs`: `txt_sign_verify_round_trip`, `tampered_unsigned_and_foreign_records_rejected`, `stale_records_rejected_both_directions`, `disabled_config_is_inert`, `table_is_bounded_and_replays_cannot_redirect`, `pick_addr_prefers_ipv4`, `pick_addr_never_offers_loopback_or_link_local_when_a_lan_address_exists`. `crates/common-network/tests/discovery.rs`: `two_instances_discover_within_5s_single_host`, `two_processes_discover_within_5s_single_host`, `browser_never_lists_own_id_single_host`. `crates/common-api/tests/mesh.rs`: `peers_list_never_shows_this_device_as_discovered_single_host`. `crates/common-api/src/mesh.rs`: `dial_order_prefers_the_fresh_mdns_address`.

### 9.6 D5 Framing, versioning, capabilities
- Frame = `u32 BE length ‖ protobuf ControlMessage`. The 1 MiB bound is checked before allocation; the writer refuses to send more.
- `Hello{major, minor, common_version, peer_id, nonce}`, connector first. `peer_id` must equal the TLS identity. Major must match (it is also the ALPN), else `protocol_incompatible` naming both versions. Minor = min of the two.
- **`SignedCapabilities` over the exact bytes.** Capabilities travel as `SignedCapabilities{record, signature}`. `record` is the encoded `Capabilities` exactly as sent. `signature` is Ed25519 over `"common/1 capabilities\0" ‖ record`. The receiver checks the signature under the TLS identity over the bytes it received, then decodes, then checks `record.peer_id` equals the TLS identity. Nothing is re-encoded, so a newer minor's unknown fields still verify. The context prefix stops the signature being replayed as an mDNS or revocation signature.
- The `Capabilities` frame is bounded at 64 KiB (`frame_too_large`); a sender refuses to build a larger one. Fields: `schema_version`, `peer_id`, `common_version`, `hardware_json` (exactly `common hardware --json`), `free_memory_bytes`, `engines` (e.g. `llama.cpp b11609 macos-arm64`), `cached_model_sha256`, `groups`, `issued_at`.
- In `control.proto`, tag 2 (the old unsigned Capabilities) and field 10 (the old in-record signature) are reserved. `SignedCapabilities` is tag 9, so an old peer sees an unknown variant.
- The Rust types in `proto.rs` mirror `control.proto` by hand; no generated code, no `protoc`. A test fails when a name or tag differs.
- `Status` is defined but not sent yet. Capabilities are not refreshed after the handshake. The bulk activation frame waits for M4.
- Tests, `crates/common-network/src/proto.rs`: `every_message_round_trips`, `frame_bound_checked_before_allocation`, `unknown_field_tolerated`, `unknown_oneof_variant_tolerated`, `control_proto_mirrors_proto_rs`. `crates/common-network/src/transport.rs`: `negotiate_major_mismatch_is_typed_and_minor_is_min`, `capabilities_signature_covers_the_bytes_sent`, `engines_names_the_pinned_build`. `crates/common-network/tests/peer_transport.rs`: `capabilities_exchanged_after_handshake`, `oversized_capability_message_rejected`, `protocol_major_mismatch_is_typed`, `minor_negotiation_is_min`, `old_new_peer_interop_single_host`.

### 9.7 D6 RPC carriage
Not built (M4). No `rpc-server` code exists. M3 built Mode B instead: one whole request on one paired device (9.10).

### 9.8 D7 Health and the liveness bound
- Defaults: keep-alive **1 s**, idle timeout **4 s** (`keep_alive_ms`, `idle_timeout_ms`).
- Worst case, step by step. The peer dies just after its last datagram, at t0. Our next keep-alive PING goes out at most 1 s later. Sending it restarts our idle timer once (RFC 9000 §10.1: the first ack-eliciting packet sent after a receipt restarts it). Nothing comes back, so the timer fires 4 s after that. Close ≤ t0 + 1 s + 4 s = **5 s**, inside the 6 s of F-02-05.
- The proposal's 2 s / 6 s gives 2 + 6 = 8 s worst case. 6.50 s was measured (`docs/VERIFICATION.md`), so the defaults moved.
- A watcher samples the connection's received-datagram count once per keep-alive interval. Three intervals with nothing → `unhealthy` (about 3 s). Idle-timeout close → `reconnecting`. A deliberate close (revoked, refused, shut down) → `disconnected`.
- Reconnect backoff 1, 2, 4, 8, 16, then 30 s. Supervision stops for good on `peer_revoked`, `peer_unauthorized` or `protocol_incompatible`. The daemon's sync loop dials every pinned device that has no connection, with the same backoff.
- Measured in the two-daemon run: SIGKILL of a daemon noticed in 4.855 s (SINGLE-HOST).
- Tests, `crates/common-network/src/health.rs`: `three_misses_then_unhealthy_then_recovers`, `lost_means_reconnecting_until_heartbeat`, `loss_reconnects_deliberate_close_does_not`, `backoff_doubles_and_caps_at_30s`. `crates/common-network/src/transport.rs`: `transport_config_builds_from_config`. `crates/common-network/tests/peer_transport.rs`: `kill_peer_detected_within_bound_single_host`, `sigkill_peer_process_detected_within_bound_single_host`, `partition_detected_and_recovered_single_host`, `supervise_stops_when_the_peer_refuses`.

### 9.9 D7 Revocation
Issuing (`POST /api/peers/{id}/revoke`, `common peers revoke`):
- Remove the pin and every group membership. Append `Revocation{seq, peer_id, revoked_by, at_utc, signature}`. The signature is Ed25519 over `"common-revocation/1" ‖ seq (u64 BE) ‖ peer_id ‖ at_utc`.
- Close every connection to the device with close code `RE`. From then on the verifier refuses its key. `pin` refuses a revoked id; only a new pairing restores it.

Acceptance rules for a received record (`PeerStore::apply_revocation`), in this order:
1. Already held → no-op, no audit line.
2. Superseded → refused. `pin_after_pairing` moves this store's revocations of a re-paired id to `superseded`, so a replay cannot undo the re-pair.
3. Issuer revoked here → refused. **Issuer not pinned here → refused.**
4. Signature must verify under the issuer's key.
5. A device revoking itself → refused. Target already revoked here → refused, so one record per target per store.
6. **Issuer and target must share a group here.**

A record that names this device is ignored. A wire record is ignored unless both ids are 32 bytes, the signature is 64 bytes and `at_utc` is at most 64 bytes.

Sending rules (one control loop per connection):
- On every new connection, offer every revocation the store holds; then each new one, local or applied from a peer.
- Offer a record only to its audience: the devices that shared a group with the target here when it was revoked or applied. Never to the target. Never back to a peer that sent it here.
- No audience recorded (revoked before this daemon started): offer it to any peer in one of this device's groups.
- A finished write is not delivery. The record is offered again on every new connection until that peer sends it back. A peer that holds it ignores it silently.
- An applied record closes the target's connections, is audited `peer_revoked` (`source: peer`) and is passed on by the same rules.

Known limits: the audience map lives in memory. `seq` is per issuer, so peers always send from 0. A record this store never saw can re-revoke a re-paired device that is back in a group with the issuer (fails closed; a pairing epoch would fix it).
- Tests, `crates/common-identity/src/lib.rs`: `revocation_round_trip`, `bad_signature_rejected`, `revoked_key_cannot_be_repinned_without_new_pairing`, `revocation_requires_issuer_and_target_in_a_common_group`, `idempotent_apply_does_not_rewrite_the_file`, `repaired_device_survives_replay_of_an_old_revocation`. `crates/common-network/tests/peer_transport.rs`: `disconnect_within_5s`. `crates/common-api/src/mesh.rs`: `revocation_fanout_offers_a_record_to_its_audience_until_it_comes_back`, `revocation_message_round_trip_and_bounds`. `crates/common-api/tests/mesh.rs`: `pairing_pins_both_sides_and_revoke_closes_the_connection_single_host`, `revocation_reaches_a_third_device_and_a_re_pair_survives_replay_single_host`, `revocation_goes_only_to_members_sharing_a_group_single_host`, `revocation_refused_once_is_offered_again_on_reconnect_single_host`. `crates/common-cli/tests/mesh_single_host.rs`: `two_installs_pair_run_remote_and_revoke_single_host`.

### 9.10 Mode B, peer admission and quotas (F-06-02, F-06-04, F-06-07)
Sending side (API `target`, `common run --peer`), in order:
1. `PolicyEngine::evaluate` runs before admission, any load or any network activity (I-05). Local is always allowed. A peer needs privacy `trusted_group`, networking on, the peer pinned and not revoked, and a shared group that allows remote inference.
2. The model hash comes from the local manifest, else the catalog. No hash → `model_not_found`.
3. Use the live connection, else dial (mDNS address, then `last_addr`).
4. Write the `remote_request` audit line, phase `sent`. If it cannot be written, nothing is sent (`io`).
5. `generate_remote` checks the privacy string again before it opens a stream.
6. Phase `finished` is written once, with the outcome and `request_id`, however the request ends.

Serving side, per request stream, in order:
1. At most 8 request streams per connection. The 9th gets `overloaded` without being read.
2. `request_id` must be 1–64 safe characters.
3. `may_serve`: networking on, `serve_peers` on, peer not revoked, pinned, and in a group allowing remote inference; and the request's `privacy` field must be `trusted_group`. A refusal sends an error frame and audits `peer_rejected`; nothing is parsed or loaded.
4. Per-peer token buckets: 120 requests a minute, 4 at once (defaults; `config.json` `peer_quota`).
5. The model id must be a safe name. The requested hash must equal the local manifest's. No substitution.
6. The request is held to the local API's bounds: no raw grammar, `max_tokens ≥ 1`, temperature in [0, 2], top_p in [0, 1], at most 16 stop strings of 256 bytes, a bounded JSON schema, and this device's own timeouts.
7. Engine capacity, never a wait: one permit of the peer share and one free engine slot. The peer share is `min(max_concurrent, engine slots − 1)`, so one slot always stays with local work and a one-slot engine serves no peers. Either missing → `overloaded`.
8. Load, then check the loaded model's hash again.
9. Charge the whole `max_tokens` to the 200,000 tokens-a-minute bucket.
10. Stream. Cancel on a `Cancel` frame or when the client's send side ends for any reason.

Error frames carry the code. For engine and filesystem codes the message is generic, so a peer never sees local paths.
- Tests, `crates/common-policy/src/lib.rs`: `policy_evaluation`, `unauthorized_peer_request_refused`, `workload_only_to_group_members`, `non_member_rejected`, `may_serve_requires_serve_peers_and_network`, `default_deny`, `local_only_never_leaves_device_fuzz`, `policy_overhead_p99_under_1ms`, `token_bucket`, `peer_flood_throttled`. `crates/common-network/src/remote.rs`: `privacy_gate_lets_only_trusted_group_leave_the_device`, `request_ids_are_fresh_and_validated`, `admission_guard_releases_its_slot_on_drop`, `peer_requests_get_the_local_api_bounds`, `peers_never_see_local_paths`, `peer_share_and_engine_slots_never_wait`, `integrity_message_names_both_hashes_without_echoing_junk`. `crates/common-network/tests/remote_single_host.rs`: `remote_golden_text_single_host`, `remote_streaming_delivers_before_done`, `unauthorized_peer_is_refused`, `local_only_privacy_never_leaves_device`, `remote_cancel_stops_server_generation_within_500ms`, `peer_lost_mid_stream_is_typed`, `peer_supplied_grammar_refused_before_load`, `peer_flood_throttled_local_p95_within_20pct_single_host`. `crates/common-api/src/mesh.rs`: `peer_quota_defaults_and_partial_config`, `peers_never_get_the_last_engine_slot`, `audited_stream_records_one_outcome_whatever_ends_it`. `crates/common-api/tests/mesh.rs`: `defaults_are_local_only_and_a_peer_target_is_refused_without_any_network`, `peer_target_is_denied_before_any_connection_when_network_is_enabled`, `remote_completion_returns_golden_text_with_target_echoed_single_host`, `privacy_mode_enforced_end_to_end_single_host`, `local_only_never_leaves_device_10k_single_host`, `peer_flood_throttled_through_daemon_single_host`, `peer_request_is_refused_while_local_work_holds_every_engine_slot_single_host`.

### 9.11 Audit (F-06-10)
- `logs/audit.jsonl`, mode 0600, one JSON line per event. `record` refuses fields named `prompt`, `content`, `messages`, `api_key`, `private_key` or `pkcs8`, and any string over 2 KiB.
- **Rotation.** At 8 MiB the file moves to `audit.1.jsonl`, then `.2`, `.3`; the oldest is dropped. The trail is bounded at about 32 MiB: retained, not append-only forever. Each new file starts with an `audit_rotated` line (`file`, `rotated_bytes`, `dropped_bytes`), so a drop is always visible.
- Kinds a peer cannot produce at will (`pairing_started`, `pairing_succeeded`, `pairing_locked`, `peer_revoked`, `peer_removed`, `privacy_changed`, `network_changed`, `group_updated`) are also copied to `audit.security.jsonl`, rotated on its own. A flood of refused requests cannot rotate them away. `tail` merges them back.
- Events built: `pairing_{started,succeeded,failed,locked}`, `peer_{connected,rejected,disconnected,revoked,removed}`, `remote_request`, `privacy_changed`, `network_changed`, `group_updated`, `audit_rotated`. `session_*` and `rpc_tunnel_*` wait for M4. A request that policy stops on the sending side writes no line: nothing left the device.
- Tests, `crates/common-observability/src/lib.rs`: `audit_record_and_tail`, `audit_rejects_content_bearing_fields`, `rotation_keeps_three_files_and_0600`, `tail_reads_across_rotation`, `security_trail_survives_a_request_flood`, `tail_adds_back_security_copies_rotated_away`, `tail_is_bounded_on_a_large_file`, `concurrent_appends_survive_rotation`, `diagnostics_bundle_redacted`.

### 9.12 Error and close codes
- Error codes as proposed: `peer_unauthorized` (403), `peer_revoked` (403), `pairing_failed` (401), `pairing_locked` (429), `peer_lost` (502), `protocol_incompatible` (426), `frame_too_large` (413).
- QUIC application close codes: `RE` 0x5245 revoked, `UA` 0x5541 unauthorized, `PV` 0x5056 protocol, `FT` 0x4654 frame too large, `PF` 0x5046 pairing failed, `PL` 0x504C pairing locked, 0 normal.
- Tests, `crates/common-network/src/transport.rs`: `connection_errors_map_to_stable_codes`. `crates/common-network/src/remote.rs`: `error_codes_round_trip_through_their_strings`.

### 9.13 Desktop: dev-only daemon URL
- `?daemon=http://127.0.0.1:<port>` (or `http://localhost:<port>`) points one browser tab at another local daemon, so two tabs can drive two daemons in SINGLE-HOST UI testing.
- Only plain `http`, only those two hosts, only a port in 1–65535, no path or query. Anything else is ignored.
- Only when `import.meta.env.DEV` is true. Production builds compile it out and ask the Tauri shell (`daemon_info`). Diagnostics labels a chosen URL "development only".
- Tests: `apps/desktop/src/test/devdaemon.test.tsx` (suites `devDaemonUrl` and `app with ?daemon=`).

### 9.14 Where the code differs from sections 2–8
| Proposal | Built |
|---|---|
| keep-alive 2 s, idle 6 s | 1 s and 4 s; worst case 5 s (9.8) |
| SQLite `peers` / `revoked` tables | `identity/peers.json` + `peers.json.lock` (9.1) |
| mDNS `sig` base64 over the other TXT values | hex over a canonical string that includes the SRV port (9.5) |
| `signature` field inside `Capabilities` | `SignedCapabilities` envelope over the exact bytes (9.6) |
| generated prost code via `scripts/gen_proto.sh` | hand-mirrored `proto.rs`, checked by a test (9.6) |
| rustls without `tls12` | `tls12` compiled in through reqwest; every config pins TLS 1.3 (9.3) |
| `Status` every 10 s | defined, not sent (9.6) |
| any group member may revoke any other | issuer pinned here and sharing a group with the target here (9.9) |
| one connection assumed per peer | crossed dials resolved by the lower-id rule (9.4) |
| `tcpdump` capture | in-process UDP relay capture (9.3) |
| D6 RPC carriage | not built; Mode B instead (9.7, 9.10) |
| test names in section 5 | the names in this section (e.g. `downgrade_attempt_fails` is `tls12_downgrade_attempt_fails`) |

### 9.15 Still open
- **Two-machine evidence (B-002).** Everything above ran on one Mac. Real LAN mDNS, Wi-Fi jitter against the 4 s idle timeout, and a kernel packet capture on a LAN interface are untested.
- **`tls12` compiled in** through reqwest. No Common config enables it, but nothing in the build stops a future config from doing so.
- **`spake2` 0.4.0** is unaudited and, by its own statement, probably not constant-time. One-time 120 s codes, the lockout and transcript-only use limit the exposure. Track 0.5.
- No test isolates the QUIC Retry. The pairing endpoint has no Retry.
- Private key bytes are not zeroized in memory.
- The `peer_rejected` cap is one global window, so a flood hides other rejections for that minute (the count is kept).
- The revocation audience map is in memory (9.9).
- The QR carries one address; a multi-homed host may pick the wrong LAN.
