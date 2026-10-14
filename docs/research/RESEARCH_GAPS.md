# Research Gaps and Testable Differentiation Hypotheses

> Imported from the M0 Research Phase Report (2026-10-13). Research baseline, not independently verified. Markers: ✅ primary-confirmed, ⚠️ secondary/partial, `unknown/unverified`. No hypothesis below has been tested. Do not assert any of them as a result until its benchmark has run on matched hardware (spec sections 5, 10).

## 2.1 When does cross-device partitioning help?

### Activation size (arithmetic, not measured)
Pipeline parallelism carries one hidden-state vector per token per stage boundary: `hidden_dim × bytes_per_element`.

| Model class | hidden_dim | fp16 bytes / token / boundary |
|---|---|---|
| Llama-3-8B class | 4,096 | 8 KiB |
| 70B class | 8,192 | 16 KiB |

- Decode at 20 tok/s → under 1 Mbit/s per boundary. **Bandwidth is irrelevant for decode; per-hop latency dominates.**
- Prefill of a 2,048-token prompt on a 70B-class model → ~32 MiB per boundary ≈ 0.27 s at a nominal 1 Gbit/s, ≈ 1 s on a ~300 Mbit/s Wi-Fi link (back-of-envelope).

### Tensor parallelism is latency-bound
- Typically two all-reduce operations per layer per token → an 80-layer model performs ~160 synchronizations per decoded token.
- At ~300 µs TCP latency (Exo's Thunderbolt networking figure): ~48 ms of pure synchronization per token. At vendor-reported 3 µs RDMA: ~0.5 ms (back-of-envelope).
- Explains why Exo's TP speedups are published only with RDMA. **Implies TP over ordinary Wi-Fi/Ethernet usually regresses vs a single device that fits the model.**

### Measured evidence (as published)
- **Helps** when the model does not fit on one device and the baseline thrashes via mmap: prima.cpp 70B, 10,120 → 674 ms/token.
- **Helps** with very low-latency interconnects (Exo TP+RDMA, vendor-reported).
- **Regresses** when weak devices are added: prima.cpp reports 3 devices beating 6 for Llama 3-70B ⚠️.
- **Regresses** for small models that fit locally: prima.cpp matches llama.cpp at 8B because its scheduler places everything on one device.
- **WAN:** Petals shows WAN pipelines are usable for 50B+ models relative to offloading, not relative to a capable single GPU.

Scheduler consequence (feeds F-04-03, F-04-04, F-03-11): default to local; distribute only when predicted to help; refuse TP unless measured all-reduce latency permits.

## 2.2 Gaps, hypotheses, benchmarks

### G1 — Unified UX + genuine trusted-LAN pooling + authenticated pairing and encryption
- **Gap:** Exo has UX and partitioning but reportedly binds an unauthenticated API to all interfaces ⚠️ (secondary; verify). llama.cpp RPC has no auth or encryption. prima.cpp has no UX or security layer.
- **H1:** An authenticated, encrypted (QUIC/TLS 1.3 or Noise) activation channel adds ≤5% to decode TPOT vs plaintext TCP for the same 2-node layer split on 1 GbE.
- **Benchmark:** same two machines, same GGUF (e.g., Llama-3.1-8B Q4_K_M), same split point; llama.cpp RPC (plaintext) vs Common (encrypted). Report TTFT, prefill tok/s, decode tok/s, bytes/token. 10 runs, 95% CIs.
- **Falsified if:** overhead > 5%.
- **Also measure:** time-to-first-successful-pooled-token for novice users (task study, n ≥ 10) vs Exo and llama.cpp RPC.
- Registry: F-02-03, F-03-01, F-10-05.

### G2 — Network-aware scheduler that refuses distribution when it would regress
- **Gap:** no reviewed project publishes a "refuse to distribute" rule.
- **H2:** A cost model using measured RTT, bandwidth, per-device FLOPs and memory bandwidth predicts whether a split beats local with ≥90% accuracy across model sizes (1B–70B) × links (Wi-Fi 5/6, 1/2.5/10 GbE, Thunderbolt) × device pairs.
- **Benchmark:** exhaustive grid; ground-truth best plan vs predicted plan.
- **Falsified if:** accuracy < 90%, or any chosen plan regresses > 10% vs local.
- Registry: F-04-03, F-04-04.

### G3 — Prefill/decode-aware placement on heterogeneous consumer devices
- **H3:** Prefill on the highest-compute device, decode on the highest memory-bandwidth device, one KV transfer, reduces end-to-end latency for ≥2K-token prompts vs a static layer split.
- **Benchmark:** GPU desktop + Apple Silicon laptop; prompts 512/2K/8K tokens; vs static split and vs each device alone.
- **Falsified if:** KV transfer cancels the gain at 1 GbE.
- Registry: F-04-06, F-03-09.

### G4 — Content-addressed, peer-assisted model distribution
- **H4:** Chunked, hash-verified LAN transfer (BLAKE3 chunks; iroh-blobs is a candidate) reduces time-to-ready on the second and later devices by ≥5× vs independent Hugging Face downloads on a ≤100 Mbit/s WAN uplink, with zero integrity failures under injected corruption.
- **Benchmark:** 3 devices, 5 GB GGUF, wall-clock.
- Registry: F-05-05, F-05-06, F-05-09, I-02.

### G5 — Enforced privacy policy for off-device inference
- **Gap:** no reviewed project lets a user declare a prompt class may never leave the device and enforces it in the scheduler.
- **H5:** Policy enforcement (local-only, trusted-LAN-only, swarm-allowed) adds < 1 ms scheduling overhead and produces zero violations under a fuzzed request mix.
- **Benchmark:** audit-log diff against packet capture.
- Registry: F-06-07, I-05, F-10-10.
