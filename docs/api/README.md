# Common local API (v1.0.0)

The daemon (`common serve`) exposes an OpenAI-compatible HTTP API on `http://127.0.0.1:8192` by default. The authoritative, versioned description is the OpenAPI 3.1 document served at `GET /openapi.json` and kept in `crates/common-api/openapi.json`; a test asserts every served route appears in it and vice versa (F-09-04), and `openapi-spec-validator` validates it.

## Secure defaults

- Binds `127.0.0.1` only. A non-loopback `--host` without `--api-key` is refused before anything is bound.
- With an API key configured, every route except `/health` requires `Authorization: Bearer <key>`.
- Browser requests from origins outside the allow-list (Tauri and the Vite dev server) are refused; request bodies are capped at 2 MiB; generation slots and the queue are bounded (429/503).
- The inference engine's own port is never exposed; prompts never appear in logs or metrics.

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

M3 (Common Mesh) adds `/api/network`, `/api/peers`, `/api/pair/*`, `/api/groups`, `/api/privacy`, `/api/audit` and the `target` request field; they are documented in the OpenAPI file as they land.
