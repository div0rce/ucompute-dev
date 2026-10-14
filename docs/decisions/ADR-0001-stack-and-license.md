# ADR-0001: Core stack versions and project license

- **Status:** Proposed — pending primary-source LICENSE verification and reconciliation with the existing Mac repository's stack
- **Date:** 2026-10-14
- **Inputs:** M0 Research Phase Report Part 4 (2026-10-13). Markers: ✅ primary-confirmed, ⚠️ secondary/partial, `unknown/unverified`.

## Context
Spec section 6 prefers Rust/Tokio/Serde/Tonic+Protobuf/Axum/rustls/QUIC/mDNS/SQLite/tracing+OTel, llama.cpp/GGML as first adapter, Tauri+React+TypeScript+pnpm, nextest/Criterion/GitHub Actions, and requires license and version validation before pinning (section 16.2).

## Observed versions (as of research date)

| Item | Version (Oct 2026) | SPDX license | MSRV | Notes |
|---|---|---|---|---|
| Rust stable | 1.98.1 seen pinned by a third-party project ⚠️; 1.99 expected by cadence; exact current stable unknown/unverified | MIT OR Apache-2.0 | — | Pin via `rust-toolchain.toml` after checking `rustup check` on the Mac |
| tokio | 1.53.1 (Jul 20, 2026) ⚠️ | MIT | 1.85 ✅ | LTS 1.47.x ended Sep 2026 ✅ |
| serde | 1.0.x (unknown/unverified) | MIT OR Apache-2.0 | — | — |
| tonic / prost | 0.14.6 (May 7, 2026) ✅ / 0.14.4 ⚠️ | MIT / Apache-2.0 (unverified) | tonic 1.88 ✅ | 0.14 moved codegen to `tonic-prost-build`; adds `tonic-prost` runtime |
| axum | 0.8.9 (Apr 14, 2026) ✅ | MIT | 1.80 ⚠️ | main carries unreleased 0.9 changes ⚠️ |
| rustls | 0.23.44 ✅ | Apache-2.0 OR ISC OR MIT ✅ | 1.71 ✅ | — |
| quinn | 0.11.11 (Jun 22, 2026) ✅; 0.11.12 reported ⚠️ | MIT OR Apache-2.0 ✅ | 1.85 (crates.io) vs 1.74.1 (README), conflicting | — |
| mdns-sd | 0.21.4 ✅ | Apache-2.0 OR MIT (unverified) | 1.71 ✅ | — |
| rusqlite | 0.40.2 (2026 dependabot bump) ⚠️; crates.io listing showed 0.38.0, conflicting | MIT (unverified) | unknown | Use `bundled` feature |
| sqlx | 0.9.0 ✅ | MIT OR Apache-2.0 (unverified) | unknown | — |
| tracing / opentelemetry | 0.1.44 ✅ / 0.32.0 ✅ | MIT ✅ / Apache-2.0 (unverified) | 1.65 ✅ / ~1.75 ⚠️ | — |
| criterion | unknown/unverified | Apache-2.0 OR MIT (unverified) | — | — |
| cargo-nextest | 0.9.148 ✅ | Apache-2.0 OR MIT ✅ | 1.91 ✅ | — |
| cargo-deny / cargo-audit | 0.20.2 ✅ / 0.22.2 ✅ | MIT OR Apache-2.0 (unverified) | — / 1.74 ✅ | — |
| llama-cpp-2 (utilityai/llama-cpp-rs) | unknown/unverified | MIT OR Apache-2.0 (unverified) | — | Features: cuda, cuda-no-vmm, metal, vulkan, rocm, opencl, openmp, mtmd, dynamic-backends, system-ggml ✅; builds llama.cpp via cmake from a git submodule; README warns it is tightly coupled to llama.cpp and not simple to use |
| llama.cpp / GGUF | v0.x tags | MIT ⚠️ | — | GGUF is the stable successor to GGML (Aug 21, 2023); new architectures need runtime updates; RPC is not a security boundary (COMPETITIVE_MATRIX) |
| Tauri | 2.11.5 (Jul 1, 2026) ✅ | MIT OR Apache-2.0 (unverified) | — | Updater verifies Minisign Ed25519 signatures ✅; signing keys generated with empty passwords between v2.9.3 and v2.10.0 were broken and must be regenerated ✅; mobile support and built-in rollback unknown/unverified |
| React / TypeScript / pnpm | unknown/unverified | MIT / Apache-2.0 / MIT (unverified) | — | — |
| iroh | 1.0 (Jun 15, 2026) ✅ | MIT OR Apache-2.0 ⚠️ | unknown | QUIC + TLS 1.3, Ed25519 identities, NAT traversal, multipath, stateless relays; ~90% hole-punch success (vendor-reported); 0.96 had a hole-punch regression |
| rust-libp2p | 0.57.0 ✅ | MIT ✅ | 1.88 ✅ | DCUtR/relay maturity not re-verified |
| snow (Noise) / spake2 | unknown/unverified / 0.4.0 ⚠️ | Apache-2.0 OR MIT (unverified) / MIT OR Apache-2.0 (unverified) | — | spake2 listing may be stale |

## License flags
- No incompatible license found in the proposed core stack.
- **Never copy code from** ex-exo (GPL-3.0) or KoboldCpp (AGPL-3.0). Jan's history includes AGPL-3.0 periods; check per-file provenance. LM Studio's app is proprietary.

## Decision (proposed)
1. **Project license: `MIT OR Apache-2.0` (dual), Rust ecosystem convention.** Apache-2.0 contributes an express patent grant and patent-retaliation clause, which matter for scheduling, quantization and transport techniques. MIT alone has no express patent grant. MPL-2.0's file-level copyleft complicates enterprise embedding. GPL/AGPL would block permissive downstream use.
2. Enforce with `cargo-deny` license allowlist (MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib, Unicode-3.0, MPL-2.0 only for unmodified dependencies) and fail CI on GPL/AGPL.
3. Inference: llama.cpp/GGML first. Binding choice (`llama-cpp-2` vs a thin in-repo `bindgen` wrapper vs the Mac repo's existing integration) is **deferred to ADR-0002** after inspecting the existing code.
4. Transport: evaluate `quinn`+`rustls` with pinned Ed25519 keys vs `iroh` 1.0 in M3 (ADR-0003). Do not use llama.cpp RPC across machines without wrapping it in the authenticated transport.
5. Exact pins are set in `Cargo.toml`/`Cargo.lock` only after `cargo update` on the Mac confirms availability; this table is not a pin list.

## Consequences
- Every ⚠️/unverified license must be confirmed from the upstream LICENSE file before release (Gate G).
- If the existing Mac repo uses a different stack, record the delta and either migrate or amend this ADR — do not maintain two inference stacks.
