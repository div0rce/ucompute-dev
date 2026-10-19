# Common

**Run AI models on the best compute you have, with one install and no account.**

Common is an open-source (Apache-2.0) universal AI inference platform. Think of it as the VLC of AI compute: pick a model, press Run, and Common figures out the hardware, the backend, the memory budget and, with explicit authorization, a selected device you trust. Automatic multi-device planning remains future work.

> Status (2026-10-19): local CPU/Metal inference and trusted Mode B remote execution have recorded evidence, including a physical Mac↔Windows Arc/Vulkan pair, hotspot discovery, and installed macOS and Windows desktop apps that paired through their UIs; a prompt from the Windows app ran on the Mac and a Mac request ran on the Windows GPU ([evidence](docs/evidence/desktop-two-machine-2026-10-19.md), open defects listed there). Windows desktop qualification (clean-account install, uninstall, accessibility), Linux execution and genuine multi-device model partitioning remain incomplete. See [progress](docs/PROGRESS.md), [evidence](docs/VERIFICATION.md), and the [requirements ledger](docs/requirements/REGISTRY.md). No stable cross-platform release is claimed.

UCompute is the public-facing name in use for the open computing infrastructure program (owner directive 2026-10-19, D-018); ucompute.org, held by the owner, is its intended canonical site, built separately and governed by specification §29.5. Naming and trademark clearance remain open and are not claimed. Existing Common names and interfaces stay compatible. The [existing specification](docs/SPEC_HANDOFF_V2.md), sections 25–33, defines the product, device roles, developer platform, delivery backlog and release gates; [ADR-0004](docs/decisions/ADR-0004-infrastructure-program.md) records the integration decision. The 148 original P0 requirements are preserved, with five additive requirements for interoperability, workload contracts, compatibility evidence, independent developer adoption and unified compute credits with contributor consent. Principle ([D-019](docs/DECISIONS.md), specification §26.5): Contribute compute. Earn compute. Buy compute when you need more. Share your compute. Not your computer. Run, private Mesh and self-hosted operation stay free and open source. A future network would use one compute-credit balance: each device's owner chooses its sharing mode (private, donate or share for credits); credits are earned only when share-for-credits capacity serves credit-funded requests (donating earns contribution history, not credits), granted from budgeted fair-access pools, or, once authorized, purchased, with the price and a transparent platform fee shown before anything is spent. Cloud stays an optional managed service. This is a direction, not availability: no credits, purchases, Swarm or Cloud exist yet, and every economic figure in the specification is a hypothesis.

## What works today

| Product | Status |
|---|---|
| **Common Run** (local inference) | CLI + OpenAI-compatible API + desktop app on macOS arm64; real GGUF models through a pinned, hash-verified llama.cpp engine; CPU and Metal; streaming; cancellation; memory feasibility before load; Windows x64 desktop app EXPERIMENTAL: built from caf1497, installed and paired (evidence af3e37c); local first token through its UI not yet recorded |
| **Common Mesh** (trusted devices) | Opt-in authenticated pairing, encrypted remote inference, privacy policy and revocation; recorded Mac/Windows daemon and installed-desktop evidence. Whole requests run on one selected peer. Genuine pooling remains unimplemented |
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
