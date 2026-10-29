# UCompute

**Run AI models on the best compute you have, with one install and no account.**

UCompute is an open-source (Apache-2.0) universal AI inference platform. Think of it as the VLC of AI compute: pick a model, press Run, and it figures out the hardware, the backend, the memory budget and, with explicit authorization, a selected device you trust. Automatic multi-device planning remains future work.

> Status (2026-10-22): local CPU/Metal inference and trusted Mode B remote execution have recorded evidence, including a physical Mac↔Windows Arc/Vulkan pair, hotspot discovery, and installed macOS and Windows desktop apps that paired through their UIs; a prompt from the Windows app ran on the Mac and a Mac request ran on the Windows GPU ([evidence](docs/evidence/desktop-two-machine-2026-10-19.md)). Since then: the redesigned desktop app (UI 2.0) is verified on one Mac, including the packaged macOS app ([evidence](docs/evidence/desktop-ui2-2026-10-22.md)); on a network without mDNS, connect at an address restored a Mac-laptop connection and ran a remote request; the Windows firewall consent step created its scoped rule and pairing started on Windows succeeded ([evidence](docs/evidence/mesh-physical-2026-10-22-eduroam.md)). Windows desktop qualification (clean-account install, uninstall, accessibility), Linux execution, three-node runs and genuine multi-device model partitioning remain incomplete. See [progress](docs/PROGRESS.md), [evidence](docs/VERIFICATION.md), and the [requirements ledger](docs/requirements/REGISTRY.md). No stable cross-platform release is claimed.

UCompute is the product's name ([D-021](docs/DECISIONS.md), owner 2026-10-22; [D-030](docs/DECISIONS.md): the former name appears nowhere); ucompute.org, held by the owner, is its intended canonical site, built separately and governed by specification §29.5. Naming and trademark clearance remain open and are not claimed. Since [D-028](docs/DECISIONS.md) every code identifier says UCompute too: the `ucompute` CLI, the `UCompute.app` bundle and `UCompute_<version>_*` installers, data folders (`UCOMPUTE_HOME`), HTTP headers such as `X-UCompute-Target`, crate names and wire identifiers. There is no compatibility with builds that still carry the former name: they do not connect to renamed builds, so each device is reinstalled and paired again ([migration](docs/release/migration-to-ucompute.md)). The [existing specification](docs/SPEC_HANDOFF_V2.md), sections 25–33, defines the product, device roles, developer platform, delivery backlog and release gates; [ADR-0004](docs/decisions/ADR-0004-infrastructure-program.md) records the integration decision. The 148 original P0 requirements are preserved, with five additive requirements for interoperability, workload contracts, compatibility evidence, independent developer adoption and unified compute credits with contributor consent. Principle ([D-019](docs/DECISIONS.md), specification §26.5): Use your own compute for free. Share compute to earn credits. Let trusted people use your hardware for free. Spend credits to access compensated compute. Purchase credits when you need more. Share your compute. Not your computer. Run, private Mesh and self-hosted operation stay free and open source, and no one earns credits for computing for themselves. A future network would use one compute-credit balance: each device's owner chooses its sharing mode (private, donate or share for credits) and may allowlist trusted people for free access; credits are earned only when share-for-credits capacity serves verified work for an eligible independent requester (donating earns contribution history, not credits), granted from budgeted fair-access pools, or, once authorized, purchased, with the price and a transparent platform fee shown before anything is spent, and a free request never silently becomes paid. Cloud stays an optional managed service. This is a direction, not availability: no credits, purchases, trusted access, Swarm or Cloud exist yet, and every economic figure in the specification is a hypothesis.

## What works today

| Product | Status |
|---|---|
| **UCompute Run** (local inference) | CLI + OpenAI-compatible API + desktop app on macOS arm64; real GGUF models through a pinned, hash-verified llama.cpp engine; CPU and Metal; streaming; cancellation; memory feasibility before load; Windows x64 desktop app EXPERIMENTAL: built from caf1497, installed and paired (evidence af3e37c); a UI 2.0 build from 593d4ea is installed on the test laptop, with its firewall consent step evidenced; local first token through its UI not yet recorded |
| **UCompute Mesh** (trusted devices) | Opt-in authenticated pairing, encrypted remote inference, privacy policy and revocation; recorded Mac/Windows daemon and installed-desktop evidence. Whole requests run on one selected peer. Genuine pooling remains unimplemented |
| **UCompute Swarm** (opt-in network) | Not yet; disabled by design until security gates pass |
| **UCompute Cloud** | Not yet |

## Quick start (macOS, Apple Silicon)

```bash
git clone https://github.com/div0rce/ucompute && cd ucompute
cargo build --release -p ucompute-cli
export PATH="$PWD/target/release:$PATH"

ucompute setup                 # downloads + verifies the pinned llama.cpp engine (~12 MB)
ucompute models list           # catalog with size, license and "fits on this machine"
ucompute models pull qwen2.5-0.5b-instruct-q4_k_m   # shows the license, then downloads + verifies (491 MB)
ucompute run qwen2.5-0.5b-instruct-q4_k_m --chat -p "Explain unified memory in two sentences."
```

Other commands: `ucompute hardware --json`, `ucompute models inspect <id>`, `ucompute chat <id>`, `ucompute serve` (OpenAI-compatible API on `127.0.0.1:8192`), `ucompute bench <id>`, `ucompute doctor`.

Use it from any OpenAI client:

```bash
ucompute serve &
curl http://127.0.0.1:8192/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"model":"qwen2.5-0.5b-instruct-q4_k_m","messages":[{"role":"user","content":"Hello"}],"stream":true}'
```

Desktop app: a control center with seven sections (Dashboard, Devices, Models, Jobs, Chat, Activity, Settings) over the same API; see [apps/desktop/README.md](apps/desktop/README.md). Development loop:

```bash
ucompute serve                         # terminal 1
cd apps/desktop && pnpm install && pnpm dev   # terminal 2; or `pnpm tauri dev` for the native window
```

## Principles

Local-first, account-optional, secure by default (the API binds `127.0.0.1`; the engine port is never exposed; prompts are never logged), no silent model substitution, no fabricated benchmarks, no cryptocurrency. The threat model is in `docs/security/THREAT_MODEL.md`.

## Repository

- `crates/` — Rust workspace (`ucompute-core`, `-hardware`, `-models`, `-artifacts`, `-runtime`, `-api`, `-cli`)
- `apps/desktop/` — Tauri 2 + React desktop app
- `catalog/` — pinned models (revision + SHA-256 + license) and engine releases
- `docs/` — specification, requirements registry, decisions (ADRs), research, security, verification evidence
- `tests/`, `benchmarks/` — integration tests and benchmark results with hardware manifests
- `AGENTS.md` — how engineering sessions (human or AI) work in this repo

## Developing

```bash
scripts/fetch-fixtures.sh                 # engine + tiny test models into ./.ucompute
UCOMPUTE_HOME=$PWD/.ucompute cargo test       # includes real-inference tests
cargo clippy --all-targets -- -D warnings
python3 scripts/validate_registry.py      # requirements ledger must stay valid
python3 -m unittest discover -s tests/distributed/two_machine   # offline two-machine driver tests (synthetic fakes)
```

Runs on two physical machines (daemon API runs, not app UI runs), the test laptop's setup and the accessibility tree dumps: `docs/environment/windows-test-host.md`.

See `CONTRIBUTING.md`. Security reports: `SECURITY.md`.

## License

Apache-2.0. Model weights are governed by their own licenses, shown in the catalog and before every download. The bundled inference engine is [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT).
