# Common local API (v1.0.0)

The daemon (`common serve`) exposes an OpenAI-compatible HTTP API on `http://127.0.0.1:8192` by default. The authoritative, versioned description is the OpenAPI 3.1 document served at `GET /openapi.json` and kept in `crates/common-api/openapi.json`; a test asserts every served route appears in it and vice versa (F-09-04), and `openapi-spec-validator` validates it.

## Secure defaults

- Binds `127.0.0.1` only. A non-loopback `--host` without `--api-key` is refused before anything is bound.
- With an API key configured, every route except `/health` requires `Authorization: Bearer <key>`.
- Browser requests from origins outside the allow-list (Tauri and the Vite dev server) are refused; request bodies are capped at 2 MiB; generation slots and the queue are bounded (429/503).
- The inference engine's own port is never exposed; prompts never appear in logs or metrics.
- Peer networking, serving peers and mDNS are off; privacy is `local_only` (see Common Mesh below).

## Routes

| Route | Purpose |
|---|---|
| `GET /health`, `GET /ready` | liveness; readiness of the loaded engine |
| `GET /metrics` | Prometheus text (`common_*`): requests, tokens, TTFT histogram, queue depth, rejections |
| `GET /v1/models`, `GET /v1/models/{id}` | OpenAI model list with a `common` extension (downloaded, license, bytes, loaded) |
| `POST /v1/chat/completions` | chat (streaming SSE with `stream: true`; `response_format` json_schema / json_object; `tools` passthrough) |
| `POST /v1/completions` | text completion |
| `POST /v1/embeddings` | embeddings (model loaded in embeddings mode) |
| `GET /api/hardware` | the same document as `common hardware --json` |
| `GET /api/status` | version, loaded model (device, offloaded layers, context note), queue |
| `GET /api/models` | catalog ∪ store with per-model memory estimate and fit verdict for this machine |
| `POST /api/models/{id}/pull` | SSE download progress (resumable, hash-verified) |
| `POST /api/models/{id}/load`, `POST /api/unload`, `DELETE /api/models/{id}`, `POST /api/models/import` | model lifecycle |
| `GET /openapi.json` | this API's OpenAPI document |

Errors use the OpenAI shape `{"error":{"message","type","code","param","hint"}}`; `code` is one of the stable codes in `crates/common-core/src/error.rs` (snapshot-tested).

## Clients

Unmodified official OpenAI clients work with only `base_url` changed:

```python
from openai import OpenAI
client = OpenAI(base_url="http://127.0.0.1:8192/v1", api_key="unused-on-loopback")
print(client.chat.completions.create(model="qwen2.5-0.5b-instruct-q4_k_m", messages=[{"role": "user", "content": "Hello"}]).choices[0].message.content)
```

See `examples/clients/openai-js/smoke.mjs` for the JavaScript client.

## Common Mesh (paired devices)

Peer networking is **off by default**. The settings live in `COMMON_HOME/config.json` (mode 0600): `privacy` (`local_only`), `network_enabled` (false), `serve_peers` (false) and `peer_quota`. Nothing binds the peer endpoint until `PUT /api/network {"enabled": true}` or `common serve --network`. Everything here is tested SINGLE-HOST only (B-002). Design and tests: `docs/decisions/ADR-0003-peer-transport-and-pairing.md` §9. Threats: `docs/security/THREAT_MODEL.md` §2.13.

### Mesh routes

`{id}` is a 64-hex peer id or the `xxxx-xxxx-xxxx` short label of a paired device.

| Route | What it does |
|---|---|
| `GET /api/network` | Status: `enabled`, `serve_peers`, `listen_addr`, `mdns`, `peer_id`, `short_label`, `connections`, `peer_quota`, `peer_slots`. |
| `PUT /api/network` | Body `{enabled?, serve_peers?}`. Saves the change, restarts the peer endpoint and closes any pairing window. If the endpoint cannot start, networking is saved as off and the error is returned. Audit `network_changed`. |
| `GET /api/peers` | `peers`: paired devices with `id`, `name`, `short_label`, `paired_at_utc`, `last_seen_utc`, `last_addr`, `connected`, `health` (`connected`, `unhealthy`, `reconnecting`, `disconnected`) and `groups`. `discovered`: devices seen over mDNS, never this one; `trusted` is a label, not a trust decision. `revocations`: the signed revocations this device holds. |
| `DELETE /api/peers/{id}` | Unpairs without revoking (the device may pair again) and closes its connections. 204. Audit `peer_removed`. |
| `POST /api/peers/{id}/revoke` | Unpairs, records a revocation signed by this device, closes every connection to the device and sends the revocation to the devices that shared a group with it. Returns the revocation. Its key now fails inside the TLS handshake. Audit `peer_revoked`. |
| `POST /api/pair/start` | Body `{name?}`. Opens a 120 s pairing window. Returns `code` (6 digits), `port`, `pair_addr` (the `ip:port` the other device dials), `qr_payload` and `expires_at`. 403 while networking is off; 409 `overloaded` while a window is open. |
| `GET /api/pair/status` | `{active, result}`. `result` is `null`, `{peer}` or `{error}`. |
| `POST /api/pair/join` | Body `{code, addr?, name?}`. `code` is either the 6 digits (`addr` then required) or the whole `common-pair:v1:…` QR payload (`addr` optional). With a QR payload only the device the QR names can finish the handshake. Returns the paired device. |
| `GET /api/groups` | All groups. |
| `PUT /api/groups` | Body `{name, members, policy: {allow_remote_inference, allow_pooled_inference}}`. Inserts or replaces one group and returns all. Name at most 64 bytes, at most 256 members. Policy fields default to false. Audit `group_updated`. |
| `GET /api/privacy` | `{mode, disclosure}`. |
| `PUT /api/privacy` | Body `{mode, acknowledged?}`. See the disclosure flow below. Audit `privacy_changed`. |
| `GET /api/audit?n=` | The last `n` audit events (1–1000, default 100), oldest first. Ids, counts, codes and durations; never prompt text. |

### Running a request on a paired device

`POST /v1/chat/completions` and `POST /v1/completions` accept a Common extension, `target`:
- `"local"`: run here. This is the default; a missing or empty value means local.
- `"peer:<64-hex id or short label>"`: run the whole request on that paired device (Mode B). It is not split across devices.

The same value may come in the `X-Common-Target` header. The body field wins when both are present. `POST /v1/embeddings` runs locally only; a peer target there is `invalid_request`. Every response, and every streamed chunk, carries `"target"` (`"local"` or `"peer:<64 hex>"`), so nothing runs elsewhere silently.

```sh
curl -s http://127.0.0.1:8192/v1/completions -H 'Content-Type: application/json' \
  -H 'X-Common-Target: peer:1a2b-3c4d-5e6f' \
  -d '{"model": "stories15M-q4_0", "prompt": "Once upon a time", "max_tokens": 32}'
```

The policy decides before admission, any load and any network activity. A peer target needs all of:
1. privacy `trusted_group` (see the disclosure flow);
2. peer networking on;
3. the device paired and not revoked;
4. a group that contains it and allows remote inference.

The serving device checks its own side too: networking and `serve_peers` on, this device paired there and in a group that allows remote inference there. It applies per-peer quotas (120 requests and 200,000 tokens a minute, 4 at once by default) and never lets peers take its last engine slot. The model must have a known SHA-256 (local manifest or catalog); the serving device refuses a different file and never substitutes one.

Both sides audit each request with the same `request_id`. The sending side writes its entry before the request leaves; if it cannot, nothing is sent. `/metrics` counts sent requests as `common_remote_requests_total{peer}`.

### Privacy disclosure flow

1. `GET /api/privacy` returns `mode` and `disclosure`. Show the disclosure to the user.
2. `PUT /api/privacy {"mode": "trusted_group"}` without `acknowledged` returns 403 `policy_denied`. The message is the disclosure text; the hint says to resend with `"acknowledged": true`.
3. `PUT /api/privacy {"mode": "trusted_group", "acknowledged": true}` succeeds and is saved.
4. `{"mode": "local_only"}` always succeeds. `{"mode": "swarm_allowed"}` is refused with 403 `policy_denied` until its security gate.

The disclosure says that a paired device decrypts the request and runs the model on the prompt in plain text, and can read, log or keep the prompt and the answer. The CLI (`common privacy trusted-devices`, asks y/N or takes `--yes`) and the desktop app show the same text.

### Mesh error codes

Same OpenAI error shape as above.

| Code | HTTP | When |
|---|---|---|
| `policy_denied` | 403 | privacy is `local_only` or `swarm_allowed`; peer networking is off; `trusted_group` was not acknowledged; the serving device does not serve peers |
| `peer_unauthorized` | 403 | the device is not paired, the short label is unknown, or no group allows remote inference with it (on either side) |
| `peer_revoked` | 403 | the device is revoked here, or it revoked this device |
| `pairing_failed` | 401 | wrong code, relayed connection, the QR names another device, or the window expired |
| `pairing_locked` | 429 | more than 5 failed attempts within 60 s; open a new window |
| `overloaded` | 429 | a peer quota, the serving device's peer share or its engine slots are used up (409 from `POST /api/pair/start` while a window is open) |
| `peer_lost` | 502 | no known address, the device is unreachable, or the connection dropped mid-request |
| `protocol_incompatible` | 426 | the devices speak different protocol majors; update Common |
| `frame_too_large` | 413 | a peer frame exceeded its bound |
| `integrity_mismatch` | 422 | the serving device has a different file for that model |
| `model_not_found` | 404 | this device knows no hash for the model |
| `timeout` | 504 | the peer sent nothing within the timeouts |
| `cancelled` | 499 | the client stopped; or a pairing window closed because networking was turned off |
| `invalid_request` | 400 | malformed `target`, a peer target on embeddings, bad `addr`, bad QR payload or bad group |
| `io` | 500 | the audit entry could not be written, so the request was not sent |
