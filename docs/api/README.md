# UCompute local API (v1.0.0)

The daemon (`ucompute serve`) exposes an OpenAI-compatible HTTP API on `http://127.0.0.1:8192` by default. The authoritative, versioned description is the OpenAPI 3.1 document served at `GET /openapi.json` and kept in `crates/ucompute-api/openapi.json`; a test asserts every served route appears in it and vice versa (F-09-04), and `openapi-spec-validator` validates it.

Right after the bind, before peer networking starts, `ucompute serve` prints exactly `listening on http://<addr>` on stdout. The desktop shell counts a daemon as its own only after its own child prints that line (ffe1a47; it reads the child's stdout and stderr and matches after trimming trailing whitespace); a desktop test (`the_listening_line_for_our_address_marks_the_bind`, `apps/desktop/src-tauri/src/lib.rs`) reads the CLI's source line, no CLI test pins it.

## Secure defaults

- Binds `127.0.0.1` only. A non-loopback `--host` without `--api-key` is refused before anything is bound.
- With an API key configured, every route except `/health` requires `Authorization: Bearer <key>`.
- Browser requests from origins outside the allow-list (Tauri and the Vite dev server) are refused; request bodies are capped at 2 MiB; generation slots and the queue are bounded (429/503).
- The inference engine's own port is never exposed; prompts never appear in logs or metrics.
- Peer networking, serving peers and mDNS are off; privacy is `local_only` (see UCompute Mesh below).

## Routes

| Route | Purpose |
|---|---|
| `GET /health`, `GET /ready` | liveness; readiness of the loaded engine |
| `GET /metrics` | Prometheus text (`ucompute_*`): requests, tokens, TTFT histogram, queue depth, rejections |
| `GET /v1/models`, `GET /v1/models/{id}` | OpenAI model list with a `ucompute` extension (downloaded, license, bytes, loaded) |
| `POST /v1/chat/completions` | chat (streaming SSE with `stream: true`; `response_format` json_schema / json_object; `tools` passthrough) |
| `POST /v1/completions` | text completion |
| `POST /v1/embeddings` | embeddings (model loaded in embeddings mode) |
| `GET /api/hardware` | the same document as `ucompute hardware --json` |
| `GET /api/status` | version, uptime, `engine` (`state` `idle`, `loading` or `ready`; `loading_model_id`, `loading_for_ms`; read without waiting on a load; `last_exit`: null, or `{model_id, pid, reason, in_flight, at_utc}` for the last engine that exited on its own since the daemon started, such as `signal: 9 (SIGKILL)`), loaded model (device, offloaded layers, context note, `in_flight`, `loaded_for` `local` or `peer:<hex>`, `loaded_at_utc`), queue |
| `GET /api/models` | catalog ∪ store with per-model memory estimate and fit verdict for this machine: against free memory sampled for the request plus the loaded engine's resident size, since a load replaces that engine (on the M2 that overstates what stopping it frees by about 330 MiB; on Windows nothing is added) |
| `POST /api/models/{id}/pull` | SSE download progress (resumable, hash-verified) |
| `POST /api/models/{id}/load`, `POST /api/unload`, `DELETE /api/models/{id}`, `POST /api/models/import` | model lifecycle |
| `GET /api/jobs` | requests running (`active`, oldest first) and the last 256 finished (`recent`, newest first): every chat, completion and embeddings request run here or sent to a paired device, and every request served for one (`kind: served_for_peer`). State `queued`, `connecting` (a request to a paired device until it is sent), `loading`, `running`, `done`, `error` or `cancelled`; token counts, timings, `request_id`, `client_ref`. In memory only, cleared on restart; ids, counts, codes and durations, never prompt or completion text. Read an unknown state as not known, never as an outcome |
| `POST /api/shutdown` | stop the daemon gracefully (the path SIGTERM takes, for callers that cannot signal it, such as the desktop app on Windows): cancels generations, drains HTTP for up to 5 s, closes the peer endpoint (mDNS goodbye, QUIC close), stops the engine, exits. 202 `{"status":"shutting_down"}`; same auth, origin and Host checks as every route |
| `GET /openapi.json` | this API's OpenAPI document |

`POST /v1/chat/completions` and `POST /v1/completions` accept an optional `X-UCompute-Client-Ref` header (`^[A-Za-z0-9-]{1,64}$`, allowed in CORS preflights): the caller's own id for the request, shown as the job's `client_ref` in `GET /api/jobs`. Any other value is ignored, and so is a ref that a job still listed already holds (a ref names one request). It carries an id only, never content.

Errors use the OpenAI shape `{"error":{"message","type","code","param","hint"}}`; `code` is one of the stable codes in `crates/ucompute-core/src/error.rs` (snapshot-tested).

## Clients

Unmodified official OpenAI clients work with only `base_url` changed:

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8192/v1", api_key="unused-on-loopback")
print(client.chat.completions.create(model="qwen2.5-0.5b-instruct-q4_k_m", messages=[{"role": "user", "content": "Hello"}]).choices[0].message.content)
```

See `examples/clients/openai-js/smoke.mjs` for the JavaScript client.

## UCompute Mesh (paired devices)

Peer networking is **off by default**. The settings live in `UCOMPUTE_HOME/config.json` (mode 0600): `privacy` (`local_only`), `network_enabled` (false), `serve_peers` (false) and `peer_quota`. Nothing binds the peer endpoint until `PUT /api/network {"enabled": true}` or `ucompute serve --network`. SINGLE-HOST tests are supplemented by the bounded physical Mac/Windows Mode B and discovery run in [two-machine evidence](../evidence/m3-two-machine-2026-10-18.md). Installed macOS and Windows desktop apps paired through their UIs; the Windows app ran a prompt on the Mac; a Mac daemon-API request ran on the Windows GPU ([desktop two-machine evidence](../evidence/desktop-two-machine-2026-10-19.md)). On a network that drops multicast (eduroam), connect at an address restored a paired Mac-laptop connection and a remote run returned the golden output ([eduroam evidence](../evidence/mesh-physical-2026-10-22-eduroam.md)). Clean-account installs and model partitioning remain unverified. Design and tests: `docs/decisions/ADR-0003-peer-transport-and-pairing.md` §9. Threats: `docs/security/THREAT_MODEL.md` §2.13.

### Mesh routes

`{id}` is a 64-hex peer id or the `xxxx-xxxx-xxxx` short label of a paired device.

| Route | What it does |
|---|---|
| `GET /api/network` | Status: `enabled`, `serve_peers`, `listen_addr`, `mdns`, `peer_id`, `short_label`, `connections`, `device_name` (the host name: Computer Name on macOS, hostname on other unix, `COMPUTERNAME` on Windows; the default name a paired device sees), `dial_addr` (the `ip:port` other devices should dial; null while networking is off or when no route gives one), `peer_quota`, `peer_slots`. |
| `PUT /api/network` | Body `{enabled?, serve_peers?}`. Saves the change, restarts the peer endpoint and closes any pairing window. Devices disconnected with `…/disconnect` stay disconnected across a change that keeps networking on (Sharing); turning networking off ends every hold. If the endpoint cannot start, networking is saved as off and the error is returned. Audit `network_changed`. |
| `GET /api/peers` | `peers`: paired devices with `id`, `name`, `short_label`, `paired_at_utc`, `last_seen_utc`, `last_addr`, `connected`, `health` (`connected`, `unhealthy`, `reconnecting`, `disconnected`), `groups`, `last_error` (the latest failed dial since its last connection: `addr`, `code`, `message`, `hint`, `at_utc`; null when there is none or networking is off), `held` (disconnected by this device's user, see `…/disconnect`; false while networking is off), `capabilities` (what the device signed when this connection opened: `ucompute_version`, `protocol_minor`, `engines`, `issued_at_utc`, `free_memory_bytes`, `hardware`, `models`; verified against its pinned key, not independently checked; null while not connected) and `connection` (the live connection: `id`, `opened_at_utc`, `remote_addr`, `direction`, `rtt_ms`, `tx_bytes`, `rx_bytes`, `lost_packets`; null while not connected). `discovered`: devices seen over mDNS, never this one, each with `addr` and `addrs` (1-2 IPv4 addresses from its signed record, best first); `trusted` is a label, not a trust decision. `unreachable`: devices seen with a valid signed record but only at IPv6 addresses, never dialled: `id`, `reason` (`link_local_only`, or `other_family` when some address is routable IPv6), `trusted`, `seen_utc`. `revocations`: the signed revocations this device holds. |
| `DELETE /api/peers/{id}` | Unpairs without revoking (the device may pair again), closes its connections and ends any hold on it. 204. Audit `peer_removed`. |
| `POST /api/peers/{id}/revoke` | Unpairs, records a revocation signed by this device, closes every connection to the device, ends any hold on it and sends the revocation to the devices that shared a group with it. Returns the revocation. Its key now fails inside the TLS handshake. Audit `peer_revoked`. |
| `POST /api/peers/{id}/connect` | Body `{addr?}` (`ip:port`; not port 0, an unspecified or a multicast address). Connects to a paired device now, for networks that drop mDNS and devices whose IP changed: dials `addr` when given, then the addresses mDNS sees, then the pinned `last_addr`; an open connection is returned as is. Returns the device's record. See "Dialing a paired device" below. Ends any hold on the device and clears its redial backoff. 403 `peer_unauthorized` for an id not paired here (nothing is sent) or a device that did not prove its pinned key (including a refusal closed before it did); 403 `policy_denied` while networking is off; 403 `peer_revoked` when this device revoked it, or the device closed with that code after proving its key; 403 `peer_held` when the device disconnected this one and said so on the control stream; 426; 502 `peer_lost` when nothing answered; 504 `timeout` for a control handshake that did not finish. |
| `POST /api/peers/{id}/disconnect` | Closes every connection to a paired device (close code `CLOSE_HELD`; audit `peer_disconnected`, reason `by_user`) and holds it: this device no longer dials it (its own requests to it get 403 `policy_denied`) and refuses its connections (audit `peer_rejected`, reason `held`, once per hold). The other device reads `peer_held`. The hold ends with `…/connect`, unpair, revoke, turning networking off and on, or a service restart; a Sharing change keeps it. Revocations still cross it: each time the held device dials, the two exchange the revocations due at the other before the close (both on protocol minor 1 or later), so delivery waits for that device's next redial; two devices that hold each other exchange nothing until one connects. The device stays paired. Returns its record. 403 `peer_unauthorized` for a device not paired here; 403 `policy_denied` while networking is off. |
| `POST /api/pair/start` | Body `{name?}`. Opens a 120 s pairing window. Returns `code` (6 digits), `port`, `pair_addr` (the `ip:port` the other device dials), `qr_payload` and `expires_at`. 403 while networking is off; 409 `overloaded` while a window is open. |
| `GET /api/pair/status` | `{active, result}`. `result` is `null`, `{peer}` or `{error}`. |
| `POST /api/pair/join` | Body `{code, addr?, name?}`. `code` is either the 6 digits (`addr` then required) or the whole `ucompute-pair:v1:…` QR payload (`addr` optional). With a QR payload only the device the QR names can finish the handshake. Returns the paired device. |
| `GET /api/groups` | All groups. |
| `PUT /api/groups` | Body `{name, members, policy: {allow_remote_inference, allow_pooled_inference}}`. Inserts or replaces one group and returns all. Name at most 64 bytes, at most 256 members. Policy fields default to false. Audit `group_updated`. |
| `GET /api/privacy` | `{mode, disclosure}`. |
| `PUT /api/privacy` | Body `{mode, acknowledged?}`. See the disclosure flow below. Audit `privacy_changed`. |
| `GET /api/audit?n=` | The last `n` audit events (1–1000, default 100), oldest first. Ids, counts, codes and durations; never prompt text. |

### Dialing a paired device

Every address is only a hint. Whatever answers must present the key pinned at pairing inside the TLS handshake; another device at that address is refused (`peer_unauthorized`, "did not prove it is <name>") and nothing is stored. `last_addr` changes only after a handshake at that address returned the connection. An id that is not paired here is refused before any packet is sent.

- A refusal is the device's own answer only after this device has read the device's Hello on the control stream. Until then a close, a TLS alert such as `certificate_revoked` as well as an application code, cannot be told from one forged on the path (QUIC Initial packets, whose keys come from the public connection id, are still read), even when the pinned key was shown and the handshake completed. Such a refusal is reported as `peer_unauthorized` ("did not prove it is <name>") and the next address is tried. Since 780c2b9 a close that claims a protocol mismatch reads the same way, with the hint "if it is that device, update UCompute on both devices", and so does a frame-size close (`FT`), which before kept its `frame_too_large` diagnosis; 426 `protocol_incompatible` is only a proven mismatch (the device's Hello was read, or this device's own negotiation failed).
- A proven refusal ends the dial: `peer_held` sent on the control stream, a close after the Hello, or this device's own verdict (`peer_revoked` for a device revoked here). The remaining addresses are not tried, so a later timeout cannot replace that answer in `last_error`.
- So a device that revoked this one reads `peer_unauthorized` here on a new dial, not `peer_revoked`; and a device held by a minor-0 build (every build before 3df9438, which includes every installed app so far), which closes before its Hello, reads `peer_unauthorized`, not `peer_held`.
- The peer endpoint binds IPv4 only: `0.0.0.0` (the default, every IPv4 interface) or one IPv4 address. `::`, `[::]` and other IPv6 bind addresses are refused when `ucompute serve` starts (`invalid_request`). Discovery advertises and browses IPv4 only; discovered or pinned IPv6 addresses are not dialled. An address given in `…/connect` is always tried.
- Nearby list: a device re-signs and re-announces every 120 s and stays listed while heard within 175 s. When a listed device has not been heard for 125 s the browser re-queries (a fresh browse), at most once per 30 s, so missed announcements do not age a live device out; while every announcement arrives it sends no queries of its own (RFC 6762 §5.2).
- The daemon redials every paired device that has no connection, with backoff 1, 2, 4 … 300 s, reset by a connection, a new mDNS address or `…/connect`, and writes a `peer_rejected` audit line only for the first failure with a given code at a given address since the device's last connection, not one per redial.

### Running a request on a paired device

`POST /v1/chat/completions` and `POST /v1/completions` accept a UCompute extension, `target`:
- `"local"`: run here. This is the default; a missing or empty value means local.
- `"peer:<64-hex id or short label>"`: run the whole request on that paired device (Mode B). It is not split across devices.
- `"split:<64-hex id or short label>"`: split the model's blocks between this device and that paired device (WP-04 pooled inference, ADR-0005 Proposed; see [Pooled inference](#pooled-inference-the-split-target)).

The same value may come in the `X-UCompute-Target` header. The body field wins when both are present. A request carrying the former `X-Common-Target` header (a client from before the rename) is refused with `invalid_request` naming the new header, never run locally. `POST /v1/embeddings` runs locally only; a peer target there is `invalid_request`. Every response, and every streamed chunk, carries `"target"` (`"local"`, `"peer:<64 hex>"` or `"split:<64 hex>"`), so nothing runs elsewhere silently.

```sh
curl -s http://127.0.0.1:8192/v1/completions -H 'Content-Type: application/json' \
  -H 'X-UCompute-Target: peer:1a2b-3c4d-5e6f' \
  -d '{"model": "stories15M-q4_0", "prompt": "Once upon a time", "max_tokens": 32}'
```

The policy decides before admission, any load and any network activity. A peer target needs all of:
1. privacy `trusted_group` (see the disclosure flow);
2. peer networking on;
3. the device paired and not revoked;
4. a group that contains it and allows remote inference.

The serving device checks its own side too: networking and `serve_peers` on, this device paired there and in a group that allows remote inference there. It applies per-peer quotas (120 requests and 200,000 tokens a minute, 4 at once by default) and never lets peers take its last engine slot. The model must have a known SHA-256 (local manifest or catalog); the serving device refuses a different file and never substitutes one.

Both sides audit each request with the same `request_id`. The sending side writes its entry before the request leaves; if it cannot, nothing is sent. `/metrics` counts sent requests as `ucompute_remote_requests_total{peer}`.

### Pooled inference: the split target

`"target": "split:<peer>"` on `POST /v1/completions` or `POST /v1/chat/completions` runs one model split in two: the paired device (the stage host) holds the first `split_at` blocks, this device the rest. llama.cpp's RPC between the two engines travels only inside the authenticated QUIC connection; both engines listen on `127.0.0.1` only. Experimental: tested on one machine only (SINGLE-HOST), no two-machine run yet. Extra body fields:

| Field | Meaning |
|---|---|
| `split_at` | required: the first block of this device's stage, `0 < split_at < block_count` |
| `split_mode` | `"pooled"` (default, configuration P: the stage host holds blocks `[0, split_at)`, this device's GPU the rest and the output layer, so no logits cross the link; needs a GPU here, else 503 `gpu_unavailable`) or `"same_backend"` (configuration C: this device's CPU holds `[0, split_at)`, the stage host the rest and the output layer) |
| `acknowledged` | required `true`: without it the answer is 403 `policy_denied` whose message is the disclosure, and nothing is sent |

```sh
curl -sN http://127.0.0.1:8192/v1/completions -H 'Content-Type: application/json' -d '{
  "model": "qwen2.5-0.5b-instruct-q8_0", "prompt": "Once upon a time", "max_tokens": 64, "stream": true,
  "target": "split:1a2b-3c4d-5e6f", "split_at": 12, "split_mode": "pooled", "acknowledged": true}'
```

Before any connection, load or process: privacy `trusted_group`, networking on, the device paired, not revoked and not held, and a group with it that allows **pooled** inference (`allow_pooled_inference`; remote inference does not grant it). The stage host checks the same on its side (it must serve peers and allow pooled inference with this device), runs Linux in this build, and hosts one session at a time. The model must be downloaded here, and its catalog licence must be Apache-2.0 or MIT, because its weights for the stage are uploaded to the other device (`policy_denied` otherwise).

The disclosure (`acknowledged` accepts it): the other device receives the weights of its part and, for every token, intermediate data from which the prompt and the reply can be reconstructed; the connection is encrypted, the other device is not; pooling is no more private than Mode B on that device and is usually slower than a model that fits here.

The response and every chunk carry `"placement"`: the devices and stages the engine log proved, and the session id. A split loads the model as a split engine here; a later `local` request reloads it locally and the unused session closes 10 s later. Nothing falls back: when the session ends (the other device lost, revoked, held, its pooling turned off, privacy `local_only`, networking off, a byte cap, the silence watchdog) the split engine stops and in-flight requests fail with that reason (`peer_lost`, `peer_revoked`, `peer_held`, `policy_denied`, `overloaded`). `GET /api/status` has `split`: `peer`, `session`, `model_id`, `split_at`, `split_mode`, `stages`, `stage_device`, the tunnel byte counters `to_r` and `to_h`, direction `switches`, and `ended` (the end reason) — numbers and ids, never content. Both devices audit `session_open` and `session_close` with the same `session`. A peer that speaks protocol minor 1 cannot host a stage (`protocol_incompatible`).

### Privacy disclosure flow

1. `GET /api/privacy` returns `mode` and `disclosure`. Show the disclosure to the user.
2. `PUT /api/privacy {"mode": "trusted_group"}` without `acknowledged` returns 403 `policy_denied`. The message is the disclosure text; the hint says to resend with `"acknowledged": true`.
3. `PUT /api/privacy {"mode": "trusted_group", "acknowledged": true}` succeeds and is saved.
4. `{"mode": "local_only"}` always succeeds. `{"mode": "swarm_allowed"}` is refused with 403 `policy_denied` until its security gate.

The disclosure says that a paired device decrypts the request and runs the model on the prompt in plain text, and can read, log or keep the prompt and the answer. The CLI (`ucompute privacy trusted-devices`, asks y/N or takes `--yes`) and the desktop app show the same text.

### Mesh error codes

Same OpenAI error shape as above.

| Code | HTTP | When |
|---|---|---|
| `policy_denied` | 403 | privacy is `local_only` or `swarm_allowed`; peer networking is off; `trusted_group` was not acknowledged; the serving device does not serve peers; this device disconnected the target device (`…/disconnect`) |
| `peer_unauthorized` | 403 | the device is not paired, the short label is unknown, or no group allows remote inference (pooled inference, for a split target) with it (on either side); or the device that answered at an address did not prove it is the paired device (a refusal closed before it proved its key, a revocation by that device, a claimed protocol mismatch or a frame-size close included, reads this way) |
| `peer_revoked` | 403 | the device is revoked here, or it closed a connection with that code after proving its key (a revocation met on a new dial reads `peer_unauthorized`) |
| `peer_held` | 403 | the other device disconnected this one (`…/disconnect` there); it must connect again from its side. Builds without the hold read the same close as `peer_lost` ("closed by peer"); a minor-1 build held by a minor-0 build reads `peer_unauthorized`, because that holder closes before sending its Hello |
| `pairing_failed` | 401 | wrong code, relayed connection, the QR names another device, or the window expired |
| `pairing_locked` | 429 | more than 5 failed attempts within 60 s; open a new window |
| `overloaded` | 429 | a peer quota, the serving device's peer share or its engine slots are used up (409 from `POST /api/pair/start` while a window is open) |
| `peer_lost` | 502 | no known address, the device is unreachable, or the connection dropped mid-request |
| `protocol_incompatible` | 426 | the devices speak different protocol majors, proven: the other device's Hello was read, or this device's own negotiation failed (a mismatch claimed before the Hello reads `peer_unauthorized` with an update hint); update UCompute |
| `frame_too_large` | 413 | a peer frame exceeded its bound (a frame-size close before the other device's Hello reads `peer_unauthorized`) |
| `integrity_mismatch` | 422 | the serving device has a different file for that model |
| `model_not_found` | 404 | this device knows no hash for the model |
| `timeout` | 504 | the peer sent nothing within the timeouts |
| `cancelled` | 499 | the client stopped; or a pairing window closed because networking was turned off |
| `invalid_request` | 400 | malformed `target`, a peer target on embeddings, bad `addr`, bad QR payload or bad group; a split target without `split_at` or with one out of range |
| `gpu_unavailable` | 503 | `split_mode` `pooled` on a device with no GPU (use `same_backend`) |
| `io` | 500 | the audit entry could not be written, so the request was not sent |

## Infrastructure contracts (2026-10-18 program)

**Current implementation.** HTTP/OpenAPI and the peer protocol are different contracts. Existing official-client smoke tests establish the tested HTTP subset, not a complete OpenAI implementation or a UCompute SDK. The peer profile is the [control schema](../../proto/ucompute/v1/control.proto) plus [ADR-0003 §9](../decisions/ADR-0003-peer-transport-and-pairing.md): bounded protobuf envelopes, TLS 1.3 raw keys, signed capability bytes and JSON GenerateRequest/GenerateEvent payloads. Rust mirrors the protobuf schema by hand. Status is defined but not periodically sent. These are experimental contracts; a compatible minor-version test between two UCompute peers does not prove independent interoperability.

**Committed scope.** U-01/WP-07 requires a complete independently implementable profile: identity/pairing/signature transcript, exact JSON and binary schemas, units and limits, version/feature negotiation, timeout/error/cancel behavior, authorization/revocation, replay semantics, extension namespaces and published vectors. Unknown optional fields remain tolerable; unknown required capabilities are refused. A separate implementation sharing no ucompute-network code must exchange real work and pass negative conformance cases. No protocol stability promise is made before that evidence.

U-02/WP-10 will introduce a versioned workload envelope around existing inference, preserving its conformance tests. Workload schemas declare IO, runtime/artifact pins, hardware, permissions, quotas, deadline, privacy, lifecycle, integrity and accounting. Unknown workloads/permissions fail before spawn or network. No arbitrary-program endpoint is added by this design.

**Developer delivery.** F-09-05/WP-06 delivers versioned Python and TypeScript clients with live-server tests, streaming/cancel/typed-error examples, documented authentication, explicit execution targets and migrations. Keep current OpenAI-client examples working. Local mock fixtures must be labelled and must never count as inference evidence. U-04 adds an independent study: three developers, at least two separately maintained apps, two of three first real streams within 30 minutes without author help; all apps test cancellation, refusal and default-local behavior. [Specification](../SPEC_HANDOFF_V2.md) §§30–33 owns gate targets and evidence requirements.

**Compatibility policy target.** Once beta contracts are supported, publish current/previous minor support and migrations for at least 90 days after a successor, except disclosed urgent security removals. Breaking wire meanings require a new major; reserved tags cannot be reused. Version SDK packages independently and test their declared server matrix. Protocol decisions use the contribution/ADR process in [CONTRIBUTING](../../CONTRIBUTING.md), including independent implementer review. These are launch obligations, not retroactive guarantees for today's experimental protocol.
