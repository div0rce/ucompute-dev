# UCompute Desktop UI 2.0: data-source inventory and gaps

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

Repo `/Users/nasr/repos/ucompute`, branch `integration/recover-codex-m1`, HEAD `e76e7fc` plus the uncommitted working
tree (in-flight discovery/desktop fixes). Read-only survey; nothing was built, run or edited. No daemon was listening
on 127.0.0.1:8192 at survey time, so no live responses were sampled: every shape below comes from code.

Sources read: `crates/ucompute-api/src/{lib,admin,mesh,openai,metrics,admission}.rs`, `crates/ucompute-api/openapi.json`,
`crates/ucompute-network/src/{transport,proto,remote,health,discovery,pairing}.rs`, `crates/ucompute-runtime/src/{types,manager}.rs`,
`crates/ucompute-hardware/src/{lib,macos,windows}.rs`, `crates/ucompute-identity/src/lib.rs`, `crates/ucompute-observability/src/lib.rs`,
`crates/ucompute-artifacts/src/download.rs`, `crates/ucompute-cli/src/{main,run}.rs`, `apps/desktop/src-tauri/src/{lib,firewall}.rs`,
`apps/desktop/src/{api/client.ts,daemon.ts,state/stores.ts,views/*.tsx}`.

---

## 1. Ground rules that apply to every source

| Topic | Fact (code reference) |
|---|---|
| Transport | Plain HTTP/1.1 to `http://127.0.0.1:8192` (`daemon-args.txt`: `serve --host 127.0.0.1 --port 8192 --exit-with-parent`). The webview learns the URL from Tauri `daemon_info`. |
| Auth | Bearer `api_key` on every route except `/health` *only if* the daemon was started with `--api-key`; the sidecar is started without one (`lib.rs::auth`). |
| Origin guard | Browser `Origin` must be in `cors_origins` (`tauri://localhost`, `http(s)://tauri.localhost`, `http://localhost:1420`), else 403 `policy_denied` *before* the handler runs; `Sec-Fetch-Site: cross-site` without Origin is refused; without an API key `Host` must be loopback (DNS-rebinding guard). Non-browser clients (CLI, Tauri Rust side) send neither header and pass. |
| Limits | 2 MiB request body (`BODY_LIMIT`). Generation routes go through admission: `max_concurrent` running (desktop sidecar: `serve` default **4**), `max_queue` 32 waiting, `queue_timeout_ms` 30 000; 429 `overloaded` (queue full) / 503 (wait timed out), both with `Retry-After: 1`. |
| Errors | Always `{"error":{message,type,code,param,hint?}}`, HTTP status from the stable code. A failure *after* an SSE stream opened arrives as that object inside a `data:` line. |
| Update model | **Polling only.** The only server push is SSE on two request-scoped routes (pull, chat/completions streaming). No `/api/events`, no WebSocket, no Tauri events emitted by the shell. |
| Scope | **Every `/api/*` and `/v1/*` route describes this device only.** The daemon API is loopback-only; nothing a peer knows is fetched over HTTP. What this daemon knows about peers comes from the mesh (section 3.6) and only the parts listed there are surfaced. |
| Client timeouts | `client.ts::send` has no fetch timeout; a route that blocks (see `/api/status` during a load) stalls the poller. |

---

## 2. Daemon HTTP routes (`ROUTES` in `lib.rs`, 32 entries, all present in `openapi.json`)

Cost column: what the handler does per call (no measured latencies exist; bounds come from timeouts in code).

### 2.1 Health, status, telemetry

| Method / path | Response fields (type, meaning) | Update model | Cost / latency | Notes |
|---|---|---|---|---|
| `GET /health` | `status:"ok"`, `version:string` | poll | trivial | Exempt from auth. Tauri shell uses it (raw TCP) to decide whether to spawn the sidecar. |
| `GET /ready` | 200 `{status:"ready", model:string\|null}`; 503 `{status:"not_ready", reason}` (`"a model is loading"`, engine error, `"engine did not answer within 2 s"`) | poll | `try_lock` on the model slot + one tokenize round-trip to llama-server, bounded 2 s | **Only non-blocking signal that a load is in progress** (no model id given). |
| `GET /api/status` | `version`, `uptime_seconds:int`, `loaded: LoadedModelInfo\|null`, `queue:{inflight, queued, max_concurrent, max_queue}` | poll (desktop: every 2.5 s, `daemon.ts`) | cheap **except it awaits the model-slot mutex**, which `ModelManager::ensure_loaded` holds across hash-verify + engine start: the call blocks for the whole load | `queue.inflight` = engine-slot permits taken, **including peer-served generations** (peers `try_acquire` the same semaphore). `queued` = local waiters only. |
| `LoadedModelInfo` | `model_id`, `model_sha256`, `backend_name`, `backend_version`, `device_requested`, `device_actual` (proven, e.g. `Metal0 (Apple M2)` or `cpu`), `offloaded_layers`, `total_layers`, `n_ctx` (per slot), `threads`, `parallel_slots`, `fallback_reason:string\|null` (GPU→CPU, never silent), `context_note:string\|null` (planner capped/reduced ctx or KV), `load_ms:f64`, `pid:u32\|null` | | | `context_note` is missing from openapi `LoadedModelInfo` and from `client.ts::Loaded`. `LoadedModel::memory_rss_bytes()` and `in_flight()` exist but are **not** serialized. |
| `GET /metrics` | Prometheus text. Series: `ucompute_requests_total{route,status}`, `ucompute_requests_rejected_total{reason=queue_full\|queue_timeout}`, `ucompute_tokens_generated_total`, `ucompute_prompt_tokens_total`, `ucompute_ttft_seconds` histogram (buckets 0.01…30 s), `ucompute_decode_tokens_per_second` gauge (**most recent local request only**), `ucompute_queue_depth`, `ucompute_inflight`, `ucompute_remote_requests_total{peer=<short label>}` | poll; counters since daemon start | cheap (render registry) | Token/TTFT series count **local generations only**: tokens decoded by a peer for us are excluded, and generations this device serves *for* peers are not recorded at all (`remote::execute` has no metrics handle). Client parses with `parseMetrics`. |
| `GET /api/hardware` | `HardwareReport`: `host{os, os_version, arch, chip, memory_bytes}`, `cpu{model, physical_cores, logical_cores, arch, features[]}`, `memory{total_bytes, available_bytes, unified}`, `gpus[]{name, vendor, backend, memory_bytes?, cores?}`, `backends[]{name, available, reason?}` (available = detected **and** shipped by the pinned engine), `disk_free_bytes?`, `probe_ms` | poll (desktop: on view mount) | **Fresh probe every call** on a blocking thread: macOS `sysctl`+`vm_stat`+`ioreg`+`sw_vers` subprocesses; Windows one PowerShell CIM query (the expensive one); contract < 2 s | Do not poll fast. Note `ModelManager.hardware` (used for model fit) is a **startup snapshot**, not this live probe. |
| `GET /openapi.json` | Hand-written OpenAPI 3.1; `tests/api.rs::route_inventory_matches_openapi_document` keeps it equal to `ROUTES` | static | trivial | |
| `POST /api/shutdown` | 202 `{status:"shutting_down"}` | action | returns at once; daemon drains ≤ 5 s, closes mesh (mDNS goodbye), unloads engine | Used by the Tauri shell on Windows at quit. (working tree) |

### 2.2 Models and engine

| Method / path | Request / response | Update model | Cost / latency | Notes |
|---|---|---|---|---|
| `GET /api/models` | `{models: LibraryModel[]}`: `id`, `name`, `bytes`, `license`, `license_url`, `source`, `family?`, `parameters?`, `quantization?`, `chat:bool`, `redistribution`, `tags[]`, `downloaded:bool`, `loaded:bool`, `fit:{verdict:"fits",headroom_bytes} \| {verdict:"too_large",needed_bytes,available_bytes,reason}`, `estimate:{weights_bytes,kv_cache_bytes,compute_buffer_bytes,overhead_bytes,total_bytes,n_ctx}`, `estimate_basis:"gguf"\|"file_size"` | poll on demand | store list + GGUF header parse (cached by sha) on a blocking thread; **also awaits the model-slot mutex** (blocks during a load) | Store manifests first, then catalog entries not downloaded. `fit` uses the startup memory snapshot and `n_ctx ≤ 4096`. **No `sha256` field**, no partial-download size. `estimate_basis` absent from `client.ts::Model`. |
| `GET /v1/models`, `GET /v1/models/{id}` | OpenAI list/object + `ucompute:{downloaded, license, bytes, redistribution, loaded}` | poll | cheap; awaits model-slot mutex | For OpenAI clients; `/api/models` is the richer one. |
| `POST /api/models/{id}/pull` | **SSE**; each `data:` is `Progress{downloaded_bytes, total_bytes, network_bytes, resumed_from, done}` or an error object; last has `done:true` | stream (request-scoped) | network-bound; ≤ 2 concurrent pulls (`MAX_PULLS`, 429 beyond) | The download runs on a blocking task that **continues after the client disconnects** (sends are ignored); there is no route to list or re-attach to running downloads and **no cancel**. Partial files persist in `UCOMPUTE_HOME/models/partial/<sha>.part` (resumable) but are not reported. Stream ends at server shutdown. |
| `POST /api/models/{id}/load` | body `{device?: auto\|cpu\|gpu, n_ctx?}` → `LoadedModelInfo` | action | hash-verify of the blob (~1 s/GB per code comment) + engine start; blocks until loaded | One slot: loading another model unloads the current one; `overloaded` if the current one has streams in flight. |
| `POST /api/unload` | 204 | action | engine stop | `overloaded` while in use. |
| `DELETE /api/models/{id}` | 204 | action | unloads first if loaded | |
| `POST /api/models/import` | body `{path (absolute), id?, license?}` → `ModelManifest` | action | GGUF header check + hash + link | |

### 2.3 Inference (OpenAI-compatible)

| Method / path | Request | Response | Notes |
|---|---|---|---|
| `POST /v1/chat/completions` | OpenAI chat body + extension `target:"local"\|"peer:<64hex or short label>"` (or `X-UCompute-Target` header); `stream`, `stream_options.include_usage` | Non-stream: `chat.completion` + `usage{prompt_tokens, completion_tokens, total_tokens}` + `timings{time_to_first_token_ms, prompt_ms, prompt_tokens_per_second, decode_ms, decode_tokens_per_second}` (all nullable) + **`target`** echo (`"local"` or `"peer:<64hex>"`). Stream: SSE chunks each carrying `id` (`chatcmpl-<uuid>`) and `target`; with `include_usage` a final chunk with `usage` + `timings`; then `[DONE]` | Policy is evaluated before admission/load/network (I-05). A peer run takes **no local engine slot**; its `timings` are the **peer engine's** measurements. Cancellation: only by the HTTP client dropping the stream (`Guard::drop` cancels; for a peer run the Cancel frame/stream end reaches the peer). There is no cancel-by-id. |
| `POST /v1/completions` | same, `prompt` string | `text_completion` + same extras | |
| `POST /v1/embeddings` | `input`, `encoding_format` | OpenAI list + `usage` | Local only; a peer target is refused. Loads the model in embeddings mode (replaces a chat load). |

### 2.4 Mesh: network, devices, pairing, trust

| Method / path | Fields | Update model | Cost / latency | Notes |
|---|---|---|---|---|
| `GET /api/network` | `enabled:bool` (true only while the QUIC endpoint is bound), `serve_peers:bool`, `listen_addr:string\|null`, `dial_addr:string\|null` (routable LAN `ip:port`, null when unspecified) *(working tree)*, `device_name:string` (hostname / `COMPUTERNAME`) *(working tree)*, `mdns:bool` (advertiser active), `peer_id:hex64`, `short_label`, `connections:int` (live, non-retired), `peer_quota:{requests_per_minute, tokens_per_minute, max_concurrent}`, `peer_slots:int` (engine slots peers may use = min(quota, max_concurrent−1); sidecar default 3) | poll (desktop Peers: 5 s while enabled) | cheap; a throwaway UDP socket for the default-route probe | `peer_quota`/`peer_slots` are not in `client.ts::NetworkState`. |
| `PUT /api/network` | body `{enabled?, serve_peers?}` → NetworkStatus | action | restarts the Node (rebind retry ≤ 2 s), closes any pairing window, restarts mDNS | Audited `network_changed`. Persisted in `config.json`. |
| `GET /api/peers` | `peers[]`: `PeerRecord{id, name (as the peer named itself at pairing), paired_at_utc, last_seen_utc?, last_addr?}` + `short_label`, `connected:bool` (**only an open authenticated QUIC connection**), `health: connected\|unhealthy\|reconnecting\|disconnected`, `last_error: {addr, code, message, hint?, at_utc}\|null` (latest failed dial since last connection) *(working tree)*, `groups:[name]`; `discovered[]`: `{id, addr, addrs[] (≤2, best first) (working tree), pairing, pair_port?, proto, trusted (pinned here; never a trust decision), seen_utc}`; `revocations[]: {seq, peer_id, revoked_by, at_utc, signature}` | poll (desktop: 5 s; `RunOn` on mount) | cheap: peers.json is cached and re-read only when its stat changes | mDNS entries are listed while heard within 300 s; a peer without goodbye disappears when mdns-sd expires its SRV (120 s). **Discovered entries carry no device name** (mDNS advert has `id, pair, pairport, proto, ts, sig` only, by design). `revocations` missing from `client.ts`. |
| `POST /api/peers/{id}/connect` | body `{addr?: "ip:port"}` → `PeerRecord` | action | up to 4 sequential dials (typed addr, ≤2 mDNS addrs, pinned `last_addr`), each ≤ idle 4 s for silence / 10 s handshake: worst case well over 10 s | Clears redial backoff. Address is a hint; pinned key enforced. *(working tree)* |
| `DELETE /api/peers/{id}` | 204 | action | | Unpair (may re-pair); audited `peer_removed`, closes connections (`peer_disconnected`). |
| `POST /api/peers/{id}/revoke` | `Revocation` | action | | Signed; fanned out to group co-members; audited `peer_revoked`. |
| `POST /api/pair/start` | body `{name?}` → `{code (6 digits), port, pair_addr, qr_payload, expires_at (unix s)}` | action | opens a temporary QUIC endpoint (random UDP port; Windows firewall drops it without the rule) | 409 while a window is open; window 120 s. |
| `GET /api/pair/status` | `{active, result:null\|{peer:PeerRecord}\|{error:{code,message,hint?}}, port?, expires_at?}` | poll (desktop: 1 s while a code is shown) | trivial | |
| `POST /api/pair/join` | body `{code (6 digits or ucompute-pair:v1 QR payload), addr?, name?}` → `PeerRecord` | action | blocks through SPAKE2 | |
| `GET /api/groups` / `PUT /api/groups` | `Group{name (≤64 B), members:[hex64] (≤256), policy:{allow_remote_inference, allow_pooled_inference}}`; PUT upserts one, returns all | poll / action | cheap | **No delete route** (and `PeerStore` has no remove-group). Default policy denies. Pooled inference does not exist (flag only). |
| `GET /api/privacy` / `PUT /api/privacy` | `{mode: local_only\|trusted_group, disclosure:string}`; PUT body `{mode, acknowledged}` | poll / action | cheap (stat of config.json) | `trusted_group` requires `acknowledged:true`; `swarm_allowed` refused. Audited `privacy_changed`. |
| `GET /api/audit?n=1..1000` | `{events: AuditEvent[]}` oldest first: `{ts_utc, kind, peer_id?:hex64, fields:object}` | poll | reads backwards across `audit.jsonl` + rotations (≤ ~32 MiB retained) and merges `audit.security.jsonl` | Client always asks for the default 100. No server-side filter (kind/peer/time). |

### 2.5 Audit event kinds (the only persistent history)

`audit.jsonl` (0600) records **off-device actions only**: ids, counts, codes, durations, never prompt/completion text
(`record` refuses keys `prompt, content, messages, api_key, private_key, pkcs8` and strings > 2 KiB).

| kind | fields | emitted when |
|---|---|---|
| `pairing_started` | `port, window_secs` | window opened |
| `pairing_succeeded` | `role (initiator\|joiner), addr` | both sides pinned |
| `pairing_failed` | `role, addr, reason (code), detail, failures?` / `reason:"qr_peer_mismatch", expected` / `reason:"cancelled"` | wrong code, wrong device, window closed |
| `pairing_locked` | `failures` | too many wrong codes |
| `peer_connected` | `addr, direction (inbound\|outbound), minor` | authenticated connection registered |
| `peer_disconnected` | `reason (revoked\|unpaired), connections` | **deliberate** close only |
| `peer_rejected` | inbound: `addr, reason, detail, suppressed` (≤ 20/min); outbound dial: `addr, direction:"outbound", reason` (once per peer/addr/code until a connection succeeds); serve-side policy refusal: `reason, detail, request_id` | refused handshake, failed dial, refused remote request |
| `peer_revoked` | `seq, by, source (local\|peer)` | revocation issued or applied |
| `peer_removed` | `{}` | unpaired |
| `network_changed` | `enabled, serve_peers` | PUT /api/network changed something |
| `privacy_changed` | `from, to, acknowledged` | |
| `group_updated` | `name, members (count), allow_remote_inference, allow_pooled_inference` | |
| `remote_request` (sender) | `role:"client", phase:"sent", peer (short label), model_id` then `phase:"finished", request_id, peer, model_id, outcome (done\|incomplete\|cancelled\|<error code>), completion_tokens, duration_ms` | every `target: peer:` request |
| `remote_request` (server) | `request_id, peer, model_id, outcome (done\|ended\|client_gone\|<error code>), prompt_tokens, completion_tokens, duration_ms` (no `role`) | every request served for a peer; `request_id` joins the two sides |
| `audit_rotated` | `file, rotated_bytes, dropped_bytes` | rotation |

**Not audited:** a connection lost to timeout/reset (`health::watch` sees the close reason and only updates health),
local generations, model loads/unloads/downloads, mDNS appear/disappear.

### 2.6 Data exchanged over the mesh (held by this daemon, mostly not exposed)

| Wire item (`proto.rs`, `control.proto`) | Content | Held where | Exposed via API? |
|---|---|---|---|
| `Hello` | `major, minor, ucompute_version, peer_id, nonce` | `PeerConnection.negotiated_minor` | **No** (minor appears only in `peer_connected` audit) |
| `SignedCapabilities` → `Capabilities` (signed by the peer, ≤ 64 KiB, verified against the TLS key) | `schema_version, peer_id, ucompute_version, hardware_json` (the peer's full `HardwareReport` JSON), `free_memory_bytes`, `engines[]` (e.g. `llama.cpp b11609 macos-arm64`), `cached_model_sha256[]`, `groups[]` (names of *all* the peer's local groups, not membership), `issued_at` | `PeerConnection.capabilities: Mutex<Option<Capabilities>>`, live connection only (`Node::live(id)` is pub) | **No.** This is the only source of peer hardware and peer model lists. Freshness caveats: `hardware_json` and `free_memory_bytes` are probed once when the **peer's** Node started; `cached_model_sha256` is set at the peer's `apply_network` and not refreshed after a pull/remove; hashes only (ids must be resolved against the local store/catalog, which is also the set this device can route, since a request needs a pinned sha). |
| `Status` | `free_memory_bytes, active_sessions, queue_depth, loaded_model_id?` | — | **Defined, never sent**; `control_loop` ignores it ("ignored in this minor"), so older peers tolerate it. |
| `RemoteRequest` / `RemoteChunk` / `Cancel` / `Error` | request id, model id + sha, request JSON, privacy; streamed `GenerateEvent`s (`Done` carries peer `usage` + `timings`) | per stream | Indirectly: chat response `target` + `timings`; audit `remote_request`. Peer's `device_actual` (GPU/CPU) is **not** returned. |
| `RevocationMsg` | signed revocation | peers.json | `GET /api/peers.revocations` |
| QUIC connection | `remote_addr`, `outbound`, `quinn::Connection::rtt()`, `stats()` (`udp_tx/udp_rx.{datagrams,bytes}`, `path.{rtt,lost_packets,lost_bytes,cwnd,congestion_events,current_mtu}`), private `in_flight` count | `Node::connections()` | **No** (health only uses `udp_rx.datagrams`) |
| Health tracker | `connected / unhealthy / reconnecting / disconnected`, miss count | `Node.health` | `health` only |
| Dial failures | latest `{addr, code, message, hint, at_utc}` per peer/addr (≤ 64 kept) | `Node.dial_failures` | `last_error` (latest only) |
| Redial backoff | `(failures in a row, next dial Instant)` per pinned peer, 1…300 s | `AppState.redial` | **No** ("next retry in N s" not available) |

### 2.7 Tauri commands (`apps/desktop/src-tauri/src/lib.rs`, `firewall.rs`)

| Command | Returns | Notes |
|---|---|---|
| `daemon_info` | `{base_url:"http://127.0.0.1:8192", spawned:bool}` | `spawned:false` when a daemon was already serving (then the app will not stop it at quit). |
| `firewall_status` *(working tree)* | `{supported:bool, allowed:bool, rule_program:string\|null}` | Windows only (`netsh ... show rule`, English-locale parse); `supported:false` elsewhere. Read-only. |
| `allow_through_firewall` *(working tree)* | `Ok(())` / `Err("cancelled"\|message)` | UAC prompt; adds one inbound UDP Private-profile rule for the bundled daemon. |

No events are emitted; sidecar stdout/stderr go to the shell's stderr only (not readable by the UI). The daemon-exit
code is known to the shell (`CommandEvent::Terminated`) but not surfaced to the webview.

### 2.8 Client-only state (`apps/desktop/src/state/stores.ts`)

| Store | Content | Persistence |
|---|---|---|
| `settings` | `device, n_ctx, device_name` (pairing name override) | localStorage `ucompute.settings` |
| `chatHistory` | chat messages **with text** | localStorage `ucompute.chat` (plaintext on disk in webview storage) |
| `runs` | per chat run: `at, model, completion_tokens, ttft_ms, decode_tps, stopped` | session only |
| `daemon` | `reachable, version, status, spawned` | memory |
| `privacy`, `runTarget`, `lastError` | | memory |

`client.ts::api.diagnostics()` calls **`GET /api/diagnostics`, which does not exist**; `Diagnostics.tsx` falls back on the
404 to status + hardware. The redacted bundle exists only as `ucompute diagnostics` (CLI → `ucompute_observability::bundle`:
hardware, audit tail 200, last 100 lines of each `UCOMPUTE_HOME/logs/*.log`, redacted).

---

## 3. UI 2.0 needs: does the data exist?

Legend: **Yes** = served today; **Partial** = served with a stated limitation; **Held** = in daemon memory, not served;
**No** = not collected anywhere.

| Need | Status | Source today | Limitation / what the UI must say |
|---|---|---|---|
| This device: identity, name, addresses | Yes | `/api/network` (`peer_id, short_label, device_name, dial_addr, listen_addr, mdns, connections`) | |
| This device: hardware | Yes | `/api/hardware` | Expensive probe; poll ≥ 10–30 s, not on the 2.5 s tick. |
| This device: free memory live | Partial | `/api/hardware.memory.available_bytes` | Only via the full probe. |
| **Peer hardware** | **Held** | `PeerConnection.capabilities.hardware_json` | Not exposed. Snapshot from the peer's network start; self-reported (signed, not verified). Gone when disconnected. |
| **Peer model list** | **Held** | `Capabilities.cached_model_sha256` | Not exposed; hashes only; stale after the peer pulls/removes until its network restarts. `/api/models` has no `sha256` to join on client-side. |
| Peer software (version, engine, protocol minor) | Held | `Capabilities.ucompute_version, engines`, `negotiated_minor` | Not exposed. |
| Device trust state (trusted / untrusted / revoked) | Yes | `/api/peers.peers` (pinned = trusted), `.discovered[].trusted:false`, `.revocations` | |
| Device connection state | Yes | `connected`, `health`, `last_error` | `connected` is only a live QUIC connection; an mDNS record never sets it. |
| Device "busy" | Partial | Local: `/api/status.queue` (inflight vs max_concurrent, queued). Peers: **No** | Peer load needs the unused `Status` message. Without it, "busy" can only mean "running N requests from this computer" (needs the job registry). |
| Next redial time | Held | `AppState.redial` | Not exposed. |
| Connection details (addr, direction, RTT, bytes, loss) | Held | `PeerConnection` + quinn `stats()` | Not exposed. |
| Network activity history | Partial | audit `peer_connected`, `peer_disconnected` (deliberate only), `peer_rejected`, `pairing_*` | **Connection losses are not recorded**; bytes not recorded. |
| Dial failures | Yes (latest) | `last_error`; audit `peer_rejected` (deduped) | Only the latest per peer live. |
| Permissions: what this device grants | Yes | `/api/groups` (policy per group), `/api/privacy`, `/api/network.serve_peers`, `peer_quota`, `peer_slots` | Quota is read-only (config.json, applies at restart). No group delete. |
| Permissions: what a peer grants this device | No | — | Learnable only by trying (policy_denied in the stream / audit outcome). Protocol change to expose; defer. |
| Engine state: loaded model & proof | Yes | `/api/status.loaded` | |
| Engine state: loading | Partial | `/ready` 503 `"a model is loading"` | `/api/status`, `/api/models`, `/v1/models` **block** during the load; which model is loading is not reported. |
| Engine memory (RSS) / in-flight per model | Held | `LoadedModel::memory_rss_bytes()`, `in_flight()` | Not serialized. |
| Engine installed (nothing loaded) | No (API) | CLI `ucompute doctor` only | |
| **Jobs: in-flight requests** | Partial | `/api/status.queue.inflight/queued` (counts) | No per-request rows: no id, model, device, start time. |
| **Jobs: history** | Partial | audit `remote_request` (remote only, both directions) | Local requests leave no record (metrics aggregates only). UI's own `runs` store is session-only and covers only its own chats. |
| Jobs: per-request timings | Partial | chat response `usage` + `timings` (to the caller only); audit `duration_ms`, token counts for remote | Requests made by other clients (CLI, scripts) are invisible except as counters. |
| Jobs: device served | Yes for own requests | response `target`; audit `remote_request.peer` | Peer's GPU/CPU for a remote run not returned. |
| Jobs: cancellation | Partial | Caller aborts its own stream (desktop chat Stop works) | No cancel-by-id for requests the UI did not start; downloads cannot be cancelled. Do not show Cancel except on the UI's own streams. |
| Throughput / latency telemetry | Partial | `/metrics` (cumulative tokens, TTFT histogram, last decode tps, rejections, remote request counts per peer) | Rates must be derived from counter deltas; peer-served work is not in metrics; per-model breakdown absent. |
| Measured capabilities per device | Partial | Local: TTFT histogram / last decode tps; remote end-to-end: audit `finished.completion_tokens / duration_ms` per peer; peer-engine timings in each remote response | `ucompute bench` writes `benchmarks/results/` relative to the CLI's cwd: not reachable by the app. |
| Models: local library, fit, sizes, licence | Yes | `/api/models` | Fit uses startup memory snapshot. |
| Models: loading status | Partial | `loaded` flag; `/ready` | See engine. |
| Models: execution locations | Partial | local `loaded`/`downloaded` | Peer locations need peer capabilities. |
| Downloads: progress | Yes (own stream) | pull SSE | |
| Downloads: across view changes / reloads | No | — | Keep the pull stream in an app-level store (survives navigation in the SPA); a webview reload or CLI pull is invisible; partial `.part` size not reported. |
| Chat: local + remote, destination | Yes | `target`, `RunOn` uses `/api/peers` | Model picker for a peer cannot be filtered to models the peer has (needs peer models). |
| Chat: history | Client only | localStorage `ucompute.chat` | Plaintext prompts on disk; give Settings a clear-history control. |
| Activity: search/filter | Yes (client-side) | `/api/audit?n=1000` | Filter in the UI; no server filter needed at this size. |
| Diagnostics bundle | No (API) | CLI only | Client already calls the missing route. |
| Settings: networking/privacy/groups | Yes | PUT network/privacy/groups | |
| Settings: storage | Partial | `/api/hardware.disk_free_bytes`, sum of `/api/models[downloaded].bytes` | UCOMPUTE_HOME path not served (in `doctor` only). |
| Contributor / credit controls | No backend | — | Show nothing (directive). |
| Windows firewall | Yes | Tauri `firewall_status` / `allow_through_firewall` | Windows only. |

### Device state derivation (from data that exists today)

| UI state | Rule | Source |
|---|---|---|
| Trusted | in `peers[]` and not in `revocations[].peer_id` | `/api/peers` |
| Untrusted (nearby) | in `discovered[]` with `trusted:false` | `/api/peers` |
| Available | `connected && health=="connected"` (+ `promptReadiness` for "can run prompts") | `/api/peers`, `/api/groups`, `/api/privacy` |
| Busy | peers: **not derivable** (needs gap N2 or E3); local: `queue.inflight >= max_concurrent \|\| queued > 0` | `/api/status` |
| Unreachable | `!connected && last_error != null` | `/api/peers` |
| Disconnected | `!connected && last_error == null` (health `disconnected`/`reconnecting`) | `/api/peers` |
| Offline-but-recently-seen | never "connected"; may show "announced nearby N s ago" from `discovered[].seen_utc` | `/api/peers` |

The working-tree `Peers.tsx::connState` already encodes this (connected / unhealthy / connecting / reconnecting /
unreachable / offline / disconnected); reuse it.

---

## 4. Gaps and the smallest backend addition for each

Ordered by priority. "Essential" = UI 2.0 cannot meet the owner directive truthfully without it. All additions are
loopback API only, ids/counts/codes/durations only (same no-text rule as the audit), documented in `openapi.json`
(the route inventory test enforces route ↔ document parity), and tested.

### Essential

**E1. Peer capabilities and connection details in `GET /api/peers` (no new route, no protocol change).**
- Change: in `mesh::peers_get`, for each pinned peer with `node.live(&p.id)`, add
  - `capabilities: { ucompute_version, protocol_minor, engines[], issued_at_utc, free_memory_bytes, hardware: HardwareReport|null, models: [{sha256, id|null}] } | null`
    - `hardware`: `serde_json::from_str::<ucompute_hardware::HardwareReport>(&caps.hardware_json).ok()` (typed parse drops unknown fields, garbage → null; never pass raw peer JSON through).
    - `models`: each `cached_model_sha256` resolved to a local manifest or catalog id (`id: null` when unknown here; such a model cannot be targeted from this device anyway).
  - `connection: { remote_addr, direction: "outbound"|"inbound", rtt_ms, tx_bytes, rx_bytes, lost_packets } | null` from `conn.remote_addr`, `conn.outbound`, `conn.conn.rtt()`, `conn.conn.stats()` (quinn 0.11.12: `udp_tx.bytes`, `udp_rx.bytes`, `path.lost_packets`, all pub).
- `null` while not connected: the UI shows "hardware unavailable (not connected)", never stale data presented as live.
- Doc caveat to carry into the UI: hardware and free memory are as reported by the device when its networking started; models as of its last network start.
- Test (SINGLE-HOST, extend the `pair(...)` helper flow in `tests/mesh.rs`): pair two in-process daemons, connect, `GET /api/peers` → `peers[0].capabilities.hardware.cpu.logical_cores > 0`, `engines` non-empty, `models` contains the fixture sha with its id when present; after a remote completion `connection.rx_bytes > 0` and `direction` matches the dialler; a pinned-but-unconnected peer has `capabilities: null, connection: null`. Add `PeerCapabilities`/`PeerConnectionInfo` schemas to openapi.

**E2. Non-blocking engine state in `GET /api/status` (and `/api/models`, `/v1/models`).**
- Problem: `ModelManager::status()` awaits the slot mutex that `ensure_loaded` holds through hash-verify and engine start, so the 2.5 s status poll hangs for the whole load and the UI cannot show "Loading X".
- Change: `ModelManager` gets `loading: std::sync::Mutex<Option<(String, Instant)>>` set/cleared around the load inside `ensure_loaded`, and `status_now()` that uses `loaded.try_lock()` (falls back to "busy"). `/api/status` adds `engine: { state: "idle"|"loading"|"ready"|"busy", loading_model_id?, loading_for_ms? }`, and `loaded.rss_bytes` (`memory_rss_bytes()`), `loaded.in_flight` (`in_flight()`). `/api/models` and `/v1/models` use `status_now()`.
- Test: `ModelManager.loaded` is `pub`; the test holds `st.manager.loaded.lock().await`, then `GET /api/status` with a 1 s client timeout must return 200 with `engine.state` in {loading, busy} (fails today by timing out). With the stories15M fixture, a real load sets `engine.state:"ready"` and `loaded.rss_bytes > 0`.

**E3. Job registry: `GET /api/jobs` (in-memory, bounded, no text).**
- Rows: `id` (the response id `chatcmpl-…`/`cmpl-…`, so the UI joins its own chat to the row; served-for-peer rows use the peer's `request_id`), `kind: chat|completion|embeddings|served_for_peer`, `model_id`, `target: "local"|"peer:<hex>"`, `origin: "api"|"peer:<hex>"`, `state: queued|loading|running|done|error|cancelled`, `created_at_utc, started_at_utc?, finished_at_utc?`, `queue_ms?, ttft_ms?, duration_ms?`, `prompt_tokens?, completion_tokens?, decode_tokens_per_second?`, `finish_reason?`, `error_code?`, `request_id?` (remote, joins audit). Response `{active: [...], recent: [...]}`; recent capped (e.g. 256), lost at restart (persistent history stays the audit for remote work; say so in the UI).
- States are all real: `queued` = waiting in `Admission::acquire`, `loading` = inside `ensure_loaded`, `running` = generating.
- Hooks: one drop-guard struct (same pattern as `mesh::Outcome`) created in `openai::run`/`run_remote`/`embeddings`, updated on admit, load, first token, `Done`, error, drop. Served-for-peer rows: put the registry type in `ucompute-observability` (already a dependency of both crates) and pass an optional handle in `remote::ServeOptions`; `handle_stream` registers/finishes the row next to its existing `remote_request` audit line. Phase 1 can ship local + outbound only.
- Test (real engine, stories15M, `real_engine.rs` style): during a streaming chat `GET /api/jobs` shows one `active` row `state:"running", target:"local"`; after `[DONE]` it is in `recent` with `completion_tokens == usage.completion_tokens` and `ttft_ms` set; a queue-full rejection appears as `error_code:"overloaded"`; the serialized registry never contains the prompt canary string. SINGLE-HOST: a `peer:` request appears on the sender (`target: peer:…`) and on the server (`kind: served_for_peer`, same `request_id`).

**E4. Record connection losses in the audit (`peer_lost`).**
- Problem: Activity would show every connect but no disconnect unless it was a revoke/unpair, which misrepresents what the network did.
- Change: in `health::watch`, after the "another live connection" early return, `node.audit("peer_lost", Some(&peer), {"reason": "timed_out"|"reset"|"closed_by_peer"|"local_close", "addr", "connected_s"})`. Bounded by connection count (retired duplicates return early and are not logged).
- Test (SINGLE-HOST): pair + connect, stop daemon B's network; within idle + keep-alive (≤ 5 s) A's audit has one `peer_lost` for B; revoke path still yields `peer_disconnected` and no extra `peer_lost` for the closed-by-us case if that is excluded.

### Nice-to-have

| # | Gap | Smallest addition | Test idea |
|---|---|---|---|
| N1 | Cancel a job the UI did not start | `POST /api/jobs/{id}/cancel` cancelling the row's `CancellationToken` (local generation: `st.shutdown.child_token()` in `run`; outbound remote: the token passed to `generate_remote_audited`); rows carry `cancellable:bool` (false for queued and served-for-peer in phase 1). 404 unknown/finished. | start a long stream, cancel by id → stream ends with `cancelled`, row `state:"cancelled"`, `/api/status.queue.inflight` back to 0 within 200 ms (F-08-02 bound). |
| N2 | Peer busy / loaded model / live free memory | Send the existing `Status` message on the control stream every 5 s (and on load/unload); store the latest per connection; expose `status:{free_memory_bytes, active_sessions, queue_depth, loaded_model_id, received_at_utc}` in `/api/peers`. Older builds ignore it (`control_loop` drops `Status`). Owner decision: send only when `serve_peers` is on (it discloses the loaded model to every paired device). | SINGLE-HOST: load a model on B, A's `/api/peers` shows `status.loaded_model_id` within 6 s; an old-minor peer stays connected. |
| N3 | Last-known capabilities for disconnected peers | `AppState` map `PeerId → (Capabilities, received_utc)` filled in `attach`; `/api/peers` returns it as `last_capabilities` with `received_utc` when not connected. | disconnect B → A still lists B's hardware with `received_utc`, `capabilities: null`. |
| N4 | Downloads visible after a reload / from the CLI, partial size | `AppState` map `id → {state, downloaded_bytes, total_bytes, network_bytes, started_at_utc, error_code?}` updated by the existing progress callback; `GET /api/downloads`; `/api/models` adds `partial_bytes` (stat of `models/partial/<sha>.part`). No cancel (none exists). | start a pull, drop the SSE client, `GET /api/downloads` keeps advancing to `done`. |
| N5 | Diagnostics bundle from the app | `GET /api/diagnostics` → `spawn_blocking(ucompute_observability::bundle(&paths, {"settings", "status"}))` (client already calls it). | route inventory; response has `schema_version:1` and contains no 64-hex id (bundle redaction already unit-tested). |
| N6 | Stale advertised models / memory | Refresh `node.cached_models` after pull, import, remove (one helper in `admin.rs`); re-probe `memory.available_bytes` per handshake in `Node::capabilities`. Peers still see it only at the next connection unless N2. | pull on B, reconnect, A's `capabilities.models` includes it. |
| N7 | Engine install state with nothing loaded | `/api/status.engine.pinned: "llama.cpp b11609 macos-arm64"` (`transport::engines()`), `present: bool` (pinned dir exists; full verify stays in `doctor`). | fixture home → `present:true`; empty home → `false`. |
| N8 | Telemetry for work served to peers | `ucompute_peer_served_requests_total{peer,outcome}` and `ucompute_peer_served_tokens_total` (needs a metrics handle or the E3 registry in `ServeOptions`). | SINGLE-HOST remote completion increments B's counters. |
| N9 | Remove a group | `PeerStore::remove_group` + `DELETE /api/groups/{name}`, audited `group_updated` with `removed:true`. Today the UI can only empty it (members `[]`, policy false). | delete → `GET /api/groups` lacks it; audit line present. |
| N10 | Edit peer quota / engine slots | `PUT /api/network {peer_quota}` persisted to config.json, response flags `restart_required:true` (they are read at start). | round-trip through config.json; value applies after restart only. |
| N11 | Fit against current memory | `/api/models` uses a memory-only probe (`vm_stat` on macOS) instead of the startup snapshot. | none beyond existing fit unit tests. |
| N12 | Next redial time | `/api/peers[].redial: {failures, next_attempt_in_ms}` from `AppState.redial`. | unreachable pinned peer shows growing `next_attempt_in_ms`; connect clears it. |
| N13 | Remote run's device on the peer | Peer adds `device_actual` to the `Done` timings (serde ignores unknown fields on old receivers). Protocol-adjacent; only if the Jobs inspector needs it beyond E1 hardware. | golden remote test asserts the field. |

### Deliberately not proposed

- **Event stream (`/api/events` SSE):** loopback polling at 1–5 s is cheap for every route except `/api/hardware`; E2 removes the one blocking poll. Add only if poll fan-out becomes measurable.
- **Device name in mDNS adverts:** the advert omits it by design (host `ucompute-<12hex>.local.`); broadcasting hostnames to the LAN is a privacy decision for the owner, not a UI need. Nearby devices show short labels until paired.
- **What a peer permits this device:** needs a protocol field; until then the UI says "unknown until a request is tried" and can show the last `remote_request` outcome for that peer from the audit.
- **Server-side audit filters:** ≤ 1000 events filter fine in the UI.
- **Model partitioning / pooled execution, Swarm, credits:** no backend exists; show nothing.

---

## 5. Contract drift to fix with the redesign (client/doc only)

| Item | Where |
|---|---|
| `api.diagnostics()` calls a route that does not exist (404 fallback works) | `client.ts`, `Diagnostics.tsx` (or add N5) |
| `Loaded` lacks `context_note`; openapi `LoadedModelInfo` lacks it too | `client.ts`, `openapi.json` |
| `Model` lacks `estimate_basis` | `client.ts` |
| `Timings` lacks `prompt_ms`, `decode_ms`; all timing fields are nullable on the wire | `client.ts` |
| `NetworkState` lacks `peer_quota`, `peer_slots` | `client.ts` |
| `api.peers()` drops `revocations` | `client.ts` |
| `api.audit()` cannot pass `n` (route supports 1..1000) | `client.ts` |
| `Hardware.gpus[].memory_bytes/cores` and `disk_free_bytes` are optional on the wire (`Option`) but typed as numbers | `client.ts` |
| No fetch timeout: a blocking route stalls the poller | `client.ts::send` (add `AbortSignal.timeout` for polls) |

---

## 6. Polling plan the UI can use today (cost-aware)

| Data | Route | Interval |
|---|---|---|
| Reachability, engine, queue | `/api/status` (+ `/ready` while a load is running, until E2) | 2–2.5 s |
| Devices, nearby, health, last error | `/api/peers` | 2–5 s while networking is on; on demand otherwise |
| Network state | `/api/network` | 5 s, and after every PUT |
| Telemetry | `/metrics` (derive rates from deltas) | 5 s on Dashboard/Jobs only |
| Activity | `/api/audit?n=1000` | 5–10 s on Activity; 30 s elsewhere for the status bar |
| Groups, privacy | `/api/groups`, `/api/privacy` | on mount and after changes |
| Local hardware | `/api/hardware` | on mount, then ≥ 30 s (PowerShell probe on Windows) |
| Models | `/api/models` | on mount and after pull/load/remove (blocks during a load until E2) |
| Pairing outcome | `/api/pair/status` | 1 s while a code is shown |
| Downloads | pull SSE held in an app-level store | stream |
| Firewall (Windows) | Tauri `firewall_status` | on Devices mount and after allow |

> Update 2026-10-22: `GET /api/jobs` rows also carry state `connecting` (a request to a paired device until it is sent on the connection); see the plan section 4 E3.
