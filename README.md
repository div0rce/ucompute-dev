# Common

**Run AI models on the best compute you have, with one install and no account.**

Common is an open-source (Apache-2.0) universal AI inference platform. Think of it as the VLC of AI compute: pick a model, press Run, and Common figures out the hardware, the backend, the memory budget and (soon) the other devices you trust.

> Status (2026-10-14): **M1, local inference, is implemented and tested on Apple Silicon.** The desktop app runs against the local daemon. Peer networking (M3), genuine multi-device model partitioning (M4) and the opt-in swarm (M9+) are designed (ADR-0003) but not built. See `docs/PROGRESS.md` for the honest ledger; every one of the 148 registered capabilities is tracked in `docs/REQUIREMENTS.csv`.

## What works today

| Product | Status |
|---|---|
| **Common Run** (local inference) | CLI + OpenAI-compatible API + desktop app on macOS arm64; real GGUF models through a pinned, hash-verified llama.cpp engine; CPU and Metal; streaming; cancellation; memory feasibility before load |
| **Common Mesh** (trusted devices) | Not yet. Design: `docs/decisions/ADR-0003-peer-transport-and-pairing.md` |
| **Common Swarm** (opt-in network) | Not yet; disabled by design until security gates pass |
| **Common Cloud** | Not yet |

## Quick start (macOS, Apple Silicon)

```bash
git clone https://github.com/div0rce/common && cd common
cargo build --release -p common-cli
export PATH="$PWD/target/release:$PATH"

common setup                 # downloads + verifies the pinned llama.cpp engine (~12 MB)
common models list           # catalog with size, license and "fits on this machine"
common models pull qwen2.5-0.5b-instruct-q4_k_m   # shows the license, then downloads + verifies (491 MB)
common run qwen2.5-0.5b-instruct-q4_k_m --chat -p "Explain unified memory in two sentences."
```

Other commands: `common hardware --json`, `common models inspect <id>`, `common chat <id>`, `common serve` (OpenAI-compatible API on `127.0.0.1:8192`), `common bench <id>`, `common doctor`.

Use it from any OpenAI client:

```bash
common serve &
curl http://127.0.0.1:8192/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"model":"qwen2.5-0.5b-instruct-q4_k_m","messages":[{"role":"user","content":"Hello"}],"stream":true}'
```

Desktop app (development loop):

```bash
common serve                         # terminal 1
cd apps/desktop && pnpm install && pnpm dev   # terminal 2; or `pnpm tauri dev` for the native window
```

## Principles

Local-first, account-optional, secure by default (the API binds `127.0.0.1`; the engine port is never exposed; prompts are never logged), no silent model substitution, no fabricated benchmarks, no cryptocurrency. The threat model is in `docs/security/THREAT_MODEL.md`.

## Repository

- `crates/` — Rust workspace (`common-core`, `-hardware`, `-models`, `-artifacts`, `-runtime`, `-api`, `-cli`)
- `apps/desktop/` — Tauri 2 + React desktop app
- `catalog/` — pinned models (revision + SHA-256 + license) and engine releases
- `docs/` — specification, requirements registry, decisions (ADRs), research, security, verification evidence
- `tests/`, `benchmarks/` — integration tests and benchmark results with hardware manifests
- `AGENTS.md` — how engineering sessions (human or AI) work in this repo

## Developing

```bash
scripts/fetch-fixtures.sh                 # engine + tiny test models into ./.common
COMMON_HOME=$PWD/.common cargo test       # includes real-inference tests
cargo clippy --all-targets -- -D warnings
python3 scripts/validate_registry.py      # requirements ledger must stay valid
```

See `CONTRIBUTING.md`. Security reports: `SECURITY.md`.

## License

Apache-2.0. Model weights are governed by their own licenses, shown in the catalog and before every download. The bundled inference engine is [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT).
