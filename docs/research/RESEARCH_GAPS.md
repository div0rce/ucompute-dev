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

## 2026-10-18 — adoption and infrastructure hypotheses

The M0 H1–H5 statements above are historical research hypotheses. In particular, bandwidth relevance depends on batching, framing, model and concurrency; the arithmetic example is not a universal finding. Exo's current README describes topology-aware splitting, and the refresh in COMPETITIVE_MATRIX.md invalidates any assumption that those concepts alone are unique. The H1 plaintext control must remain SINGLE-HOST: never expose unauthenticated RPC on a LAN to benchmark it.

### Beachhead comparison (decision under uncertainty)

| Candidate audience | Evidence supporting investigation | Likely friction / reason not first |
|---|---|---|
| Local-first developers with two owned machines | Existing OpenAI clients and Mac↔Windows Mode B evidence offer a concrete integration path; incumbent runner APIs demonstrate an established workflow | Demand and willingness to switch unmeasured; VPN+runner and LM Link are credible alternatives |
| Homelab / self-hosting community | Can reproduce heterogeneous device and private networking experiments | Tolerates manual setup, so success here can overstate consumer usability; support-heavy hardware variety |
| Research/university teams | Reproducibility, memory capacity and controlled data workflows align with X-15/20 | Campus multicast/firewall policy, research-specific model needs and administration delay adoption |
| Multi-computer AI enthusiasts | Underused hardware and bigger-model interest fit future pooling | Pooling not delivered, added devices may slow inference, novelty may not produce retention |
| General consumers | Local model UI is a familiar distribution category | Mature alternatives and current installer gaps; weak differentiation before qualified Mesh/pooling |
| Portable inference/enterprise developers | Stable contracts and fleet policy could create maintained integrations | Compatibility/support commitments exceed today's evidence; sales/compliance burden before validated demand |

Choose the first segment for a limited experiment, with homelab users as reproducibility partners. This is an evidence-informed hypothesis about accessible technical value, **not** market research establishing customer demand. No interviews, recruitment, outreach or payments occurred in this task.

### H6 — private developer workflow earns repeat use

- Owner: product/devex; work packages WP-06/08; requirements F-09-05, U-04.
- Recruit, only after authorization, 10 independent developers with two controlled computers; record current runner/VPN/cloud workaround and one useful weekly task before showing the product.
- Target: at least 6/10 complete that task unaided through the chosen local/remote interface, and at least 4/10 repeat useful use in week 4. U-04's separate 3-developer/2-app acceptance still applies.
- Compare time, failures and maintainability with their existing workflow, not a deliberately poor baseline. Report all withdrawals and support interventions.
- Falsified for this segment if either target fails after one documented onboarding correction; prioritize the observed missing value or narrow the segment, rather than buying more acquisition.

### H7 — second-device value creates a private growth loop

- Owner: network/product; WP-01/02/08; F-10-05, F-06-04.
- Mechanism: one useful local install motivates adding a privately owned device; that device's availability provides recurring value. Preconditions: qualified installers, informed sharing, useful model/device advantage and stable availability.
- Target: at least 8/10 independent pairs complete a useful authorized peer request within 10 min after installation, at least 5/10 use the second device on three separate days in the following fortnight, with no local-only violation or failed revocation.
- Falsified if setup/availability/latency harms the baseline task or repeat use misses target. More discovered peers without useful work is not success. Benchmark a supported runner over an already-authorized private network as an alternative.

### H8 — independent integrations and contributions compound usefulness

- Owner: devex/maintainers; WP-06/07/12; U-01/04, F-09-07/12.
- Mechanism: independently maintained apps bring users; reproducible tests make additional platforms cheaper to maintain; contributions improve those apps. Preconditions: compatibility policy, easy conformance, reviews and maintainer capacity.
- After U-04, target >=3 independently maintained useful integrations still passing live compatibility tests at 90 days, >=2 contributors completing a reviewed change without founder debugging, and a second maintainer performing a release rehearsal.
- Falsified if integrations stop working after minor upgrades, users require the official desktop, or support/review labor grows faster than accepted useful contributions. Fork counts and generated example apps do not count as independent adoption.

### H9 — optional services or public supply have sustainable economics

- Owner: product/operations; WP-11/12; F-07-02/09/11, X-06/09.
- Mechanism: reliable compatible supply attracts eligible jobs and paid operational convenience funds maintenance. Preconditions: demonstrated demand, isolation/privacy/abuse gates, credible metering, provider availability, legal approval and sufficient matching density for the actual workload.
- First test: cost >=100 representative successful and failed jobs across cold/warm runs, plus 10 buyer interviews; obtain >=3 written scoped pilot interests before building paid service infrastructure. No binding prices or payments in this research stage.
- Compare marginal and fully loaded useful-job costs with best feasible private device and managed alternative; include verification, provider payout, network, support, acquisition and failure costs. Stress utilization/egress/churn under spec §29.4. Pilot interest is not willingness to pay demonstrated by a transaction.
- Falsified if measured costs exceed the price buyers actually accept under pessimistic plausible assumptions, or verification/abuse costs overwhelm useful work. Stop the service hypothesis; do not weaken security or require a token to subsidize it.
