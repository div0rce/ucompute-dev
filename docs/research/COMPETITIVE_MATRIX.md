# Competitive Matrix (spec section 5)

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

> For current decisions, read the dated 2026-10-18 primary-source refresh at the end. The preceding M0 sections remain a historical baseline; their license and absence-of-feature assumptions are not automatically current.

> Imported from the M0 Research Phase Report (2026-10-13). Research baseline, not independently verified. Markers: ✅ primary-confirmed, ⚠️ secondary/partial, `unknown/unverified`. See `README.md`.

## Headline finding

No surveyed project combines an install→select→run desktop UX, genuine layer- or tensor-partitioned pooling of one model across trusted LAN devices, and authenticated, encrypted transport.

- **Exo** has the UX and genuine partitioning, but MrTechKing's GitHub security scan of exo (Oct 3, 2026) reports the API server binds to 0.0.0.0:52415 with no authentication ⚠️ (secondary source; verify against exo's code before quoting externally).
- **llama.cpp RPC** does real partitioning but its own README calls it "fragile and insecure."

Implication recorded in the report: build UCompute's own authenticated transport and partition planner on top of llama.cpp/GGML; do not reuse the RPC wire protocol as-is; do not copy code from GPL/AGPL competitors.

## 1.1 Summary table

| Project | Source URL(s) | Latest version (date) | License (SPDX) | Reuse by Apache/MIT project | Distributed mode | Genuine partitioning? | OpenAI-compatible API | Maintenance |
|---|---|---|---|---|---|---|---|---|
| Petals | github.com/bigscience-workshop/petals; arxiv.org/abs/2312.08361 | v2.2.0 (Sep 6, 2023) ✅ | MIT ✅ (org page) | Compatible with attribution | Public/private swarm over the Internet | Yes: pipeline (transformer blocks served by different peers) | No (Python client and HTTP chat backend) | Dormant: repo last updated Sep 7, 2024; community PRs open into 2026 without merge |
| Exo | github.com/exo-explore/exo | EXO 1.0.x line; v1.0.71 newest release notes seen (date unknown/unverified) ⚠️ | Apache-2.0 ✅ (current); predecessor "ex-exo" is GPL-3.0 and archived ✅ | Current repo compatible (NOTICE obligations); do not copy from ex-exo | LAN cluster with automatic discovery | Yes: pipeline and tensor parallel (MLX), RDMA over Thunderbolt 5 | Yes, plus other API-compatible interfaces (per README) ⚠️ | Very active: repo updated Oct 13, 2026; 47.9k stars ✅ |
| llama.cpp (+ RPC backend) | github.com/ggml-org/llama.cpp; tools/rpc/README.md; SECURITY.md | Formal releases now annotated tags `v0.x` (Wikipedia lists v0.5.0, Sep 23, 2026 ⚠️); nightly `bNNNN` builds; rpc-server protocol "v3.6.0" observed at build b8241 ✅ | MIT ⚠️ (not re-read from LICENSE this pass) | Compatible | RPC backend exposes remote ggml devices to a host process | Yes: remote devices receive model tensors and compute their share of the graph ⚠️ | Yes (llama-server) | Very active |
| prima.cpp | github.com/OpenCPIL/prima.cpp (new official, per author); arxiv.org/abs/2504.08791; ICLR 2026 proceedings | No GitHub releases; original repo last push 2025-04-15 ⚠️ | MIT ⚠️ (ecosyste.ms metadata) | Likely compatible; verify LICENSE | Home cluster (laptops, desktops, phones), Wi-Fi | Yes: "piped-ring" pipeline parallelism with mmap | unknown/unverified | Research code; author reports original GitHub account blocked ⚠️ |
| distributed-llama | github.com/b4rtaz/distributed-llama | v0.16.5 (Feb 2, year likely 2026) ⚠️ | MIT (unverified) | Likely compatible | LAN root + workers | Yes: tensor parallel (per project description; not re-verified) | unknown/unverified | Active into 2026 ⚠️ |
| Ollama | github.com/ollama/ollama | v0.40.2 (Oct 8, 2026) per release aggregator; 0.40.3 listed Oct 11 by mise ⚠️ | MIT (unverified this pass) | Compatible | Single node | No | Yes | Very active |
| vLLM | pypi.org/project/vllm; github.com/vllm-project/vllm | 0.31.0 (Oct 5, 2026) ✅ | Apache-2.0 (unverified this pass) | Compatible | Multi-GPU/multi-node datacenter | Yes (TP/PP/EP within clusters) | Yes | Very active (~biweekly) |
| SGLang | github.com/sgl-project/sglang | v0.5.21 (date unknown) ✅ | Apache-2.0 (unverified) | Compatible | Datacenter | Yes (TP/PP/EP) | Yes | Active |
| TensorRT-LLM | github.com/NVIDIA/TensorRT-LLM | v1.3.0rc29 pre-release (Sep 29, 2026) ✅; latest stable on 1.2 line (exact version unknown) ⚠️ | Apache-2.0 (unverified) | Compatible; NVIDIA-only | Datacenter | Yes (TP/PP) | Via Triton/trtllm-serve | Active |
| MLX / mlx-lm | github.com/ml-explore/mlx; pypi.org/project/mlx-lm | MLX v0.32.3 newest tag seen ⚠️; mlx-lm 0.32.0 ✅ | MIT (unverified) | Compatible | `mx.distributed` (used by Exo for tensor parallel) | Yes (via MLX distributed) | mlx-lm has a server (compatibility unverified) | Active |
| Ray Serve | github.com/ray-project/ray | Ray 2.58.0 ✅ (date unknown) | Apache-2.0 (unverified) | Compatible | Cluster serving / replica scaling | Replication and routing; partitioning delegated to engine | Via integrations | Active |
| LocalAI | github.com/mudler/LocalAI | v4.11.0 (Oct 2, 2026) ✅ | MIT ⚠️ (README) | Compatible | P2P/federation features (not verified this pass) | unknown/unverified | Yes | Active |
| Jan | github.com/janhq/jan | v0.8.6 (hotfix) ⚠️ | Apache-2.0 (unverified; earlier versions AGPL-3.0) | Only code under current license; check per-file history | Single node | No | Yes (local server) | Active |
| LM Studio | lmstudio.ai | unknown/unverified | App proprietary; `lms` CLI and SDKs MIT (unverified) | App code not reusable | Single node | No | Yes | Active (unverified) |
| KoboldCpp | github.com/LostRuins/koboldcpp | koboldcpp-1.122.1 (Sep 26, 2026) ✅ | AGPL-3.0 ⚠️ | **Incompatible** for copying into a permissive core | Single node | No | Yes (plus KoboldAI API) | Active |

Fields required by spec section 5 that the report could not establish for a project are `unknown/unverified` by omission; the per-project notes below are the complete record.

## 1.2 Per-project notes

### Petals
- Decentralized swarm; each server hosts a contiguous span of transformer blocks; clients route activations through a chain of servers — genuine WAN pipeline partitioning.
- NeurIPS 2023 paper: fault-tolerant inference and load-balancing protocols that automatically assign devices to maximize total throughput; reports Llama 2 (70B) and BLOOM (176B) up to 10x faster than offloading for interactive generation (simulation plus two-continent real setup). Absolute tok/s not captured.
- Networking: hivemind DHT, NAT traversal and relays; v2.0 notes cite improved DHT stability with NAT-traversed/relayed servers and fewer excess block reassignments that had caused swarm downtime.
- Privacy: public swarm exposes activations to peers (see FRONTIER_FEASIBILITY (c)).
- Last release v2.2.0 (Sep 2023), repo updated Sep 2024 — treat as unmaintained reference design.

### Exo
- Python on MLX; automatic discovery; "Topology-Aware Auto Parallel"; pipeline and tensor sharding. Predecessor ex-exo (GPL-3.0) archived.
- **Vendor-reported benchmarks:**
  - README: tensor parallel up to 1.8x on 2 devices and 3.2x on 4 devices.
  - RDMA over Thunderbolt 5: 99% latency reduction; Exo Labs launch post quantifies 300 µs → 3 µs.
  - Exo Labs post: 2× M4 Pro Mac mini (24 GB each), Llama 3.1 8B fp16 — pipeline parallel 15.7 tok/s vs tensor parallel over RDMA 28.4 tok/s.
  - README large-model charts (Qwen3-235B 8-bit, DeepSeek v3.1 671B 8-bit, Kimi K2 Thinking 4-bit on 4× M3 Ultra) credit Jeff Geerling's blog — third-party, single setup, unreplicated.
- RDMA requires macOS 26.2+ and Thunderbolt 5. On Linux reportedly CPU only, GPU support in development ⚠️.
- **Security:** secondary scan (MrTechKing, Oct 3, 2026) states API/dashboard on port 52415 listens on all interfaces with no login ⚠️. Verify against source.
- v1.0.69 enabled continuous batching by default (incl. RDMA); v1.0.66 fixed an RDMA/tensor-parallel regression.
- Fault/churn behavior: unknown/unverified.

### llama.cpp RPC backend
- rpc-server exposes a host's ggml devices; client (`llama-cli`/`llama-server --rpc host:port`) loads tensors onto them; model split across local and remote devices — genuine partitioning, not request forwarding.
- **Official security posture:**
  - README: proof-of-concept stage, "fragile and insecure"; never run on an open network or sensitive environment.
  - SECURITY.md: do not use RPC backend, rpc-server and llama-server functionality in untrusted environments; encrypt data sent over the network.
  - Binary prints a warning never to expose the RPC server to an open network.
  - Discussion #14932 (Jul 29, 2025): RPC lacks authentication, has memory-address disclosure issues and no fuzzer; hardening done in the open rather than through advisories.
- Robustness issues: #15055 remote crash when model exceeds ~75% of server RAM; #13185 crash without cache; #20315 CUDA graph leak → OOM on rpc-server v3.6.0.
- Transport: plain TCP; no discovery, NAT traversal or encryption.
- **Implication:** reuse GGML's backend abstraction but wrap or replace the wire protocol with authenticated, encrypted transport. The RPC protocol is not a security boundary.

### prima.cpp
- ICLR 2026 paper, Li, Li, Feng et al. (MBZUAI, UESTC, CityU HK, La Trobe; arXiv 2504.08791); built on llama.cpp (~20K LoC). Heterogeneous home clusters (mixed CPU/GPU, Wi-Fi, cross-OS). Pipelined-ring parallelism with mmap; scheduler "Halda".
- Published: 70B on four home devices — 674 ms/token TPOT with <6% memory pressure; 26 tok/s (32B) and 2 tok/s (70B) with speculative decoding; 5–17× lower TPOT than llama.cpp, exo and dllama.
- The 10,120 → 674 ms/token comparison is against single-device llama.cpp thrashing via mmap, not against a device that fits the model.
- Secondary summary: a 3-device subset beat a 6-device cluster on Llama 3-70B because weak devices bottlenecked ⚠️ — direct evidence that adding devices can regress.
- Early versions CUDA-only for GPU ⚠️. Security, auth and churn: unknown/unverified.

### Ollama, LM Studio, Jan, KoboldCpp, LocalAI
- Local-first single-node runners; no genuine multi-device partitioning in mainline features.
- Exposed Ollama instances were the subject of a Cisco Talos/Shodan case study — default-open local APIs are a real risk.
- LocalAI advertises P2P/federation; whether partitioning or request distribution: unknown/unverified.

### vLLM, SGLang, TensorRT-LLM, Ray Serve
- Datacenter stacks with genuine TP/PP over NVLink/InfiniBand-class links; not designed for consumer LAN pooling.
- vLLM 0.31.0 (Oct 5, 2026) after 0.30.0 (Sep 22) and 0.29.0 (Sep 9).
- Ray Serve distributes requests across replicas, delegates partitioning to engines.

### Decentralized / verification-adjacent
- Prime Intellect runs a globally distributed pipeline-parallel inference stack (SYNTHETIC-2), verified with TOPLOC v2.
- Bittensor, Gensyn and other token networks: not researched (unknown/unverified).

## 1.3 Published benchmark evidence (as published; none replicated)

| Source | Hardware | Model / quant | Mode | Result | Flag |
|---|---|---|---|---|---|
| Exo Labs post (Dec 20, 2025) | 2× M4 Pro Mac mini 24 GB, Thunderbolt | Llama 3.1 8B fp16 | PP vs TP+RDMA | 15.7 vs 28.4 tok/s | Vendor-reported |
| Exo README | Not stated per figure | Not stated | TP | Up to 1.8× (2 dev), 3.2× (4 dev) | Vendor-reported |
| prima.cpp (ICLR 2026) | 4 home devices (2 laptops, desktop, phone) | Llama 70B, Q4 | PRP pipeline | 674 ms/token TPOT; TTFT < 2 s (v1 preprint) | Author-reported, peer-reviewed |
| prima.cpp | Same cluster class | 32B / 70B + speculative decoding | PRP | 26 tok/s / 2 tok/s | Author-reported |
| Petals (NeurIPS 2023) | Geo-distributed consumer GPUs | Llama 2 70B, BLOOM 176B | WAN pipeline | Up to 10× faster than offloading | Author-reported; absolute tok/s not captured |
| Network bytes/token | — | — | — | Not published by any project reviewed | unknown |

## Open verification items

- [ ] Exo: confirm API bind address and auth behavior from source (current `main`).
- [ ] llama.cpp: read LICENSE; confirm current release tag scheme.
- [ ] prima.cpp: read LICENSE at the new official repo.
- [ ] Ollama, vLLM, SGLang, TensorRT-LLM, MLX, Ray, Jan, KoboldCpp, distributed-llama: read LICENSE files.
- [ ] LocalAI P2P: determine partitioning vs request distribution.
- [ ] Head-to-head benchmarks only on matched hardware (spec section 10).

## 2026-10-18 primary-source refresh — infrastructure program

This dated refresh supersedes conflicting current-capability or license interpretations in the imported M0 material above; it does not erase that material or upgrade its benchmark markers. ✅ below means the cited primary page was read, **not** that UCompute reproduced its functionality. Mutable upstream branches/docs were inspected, not pinned releases or exhaustive source/security audits. Re-pin versions before comparisons or code reuse. No head-to-head UCompute advantage has been measured.

| Project / distribution and maturity evidence | Actual focus and hardware scope from primary sources | License / trust and DX limits | Implication for UCompute (inference, not a result) |
|---|---|---|---|
| Ollama — established downloadable macOS/Windows/Linux runner, CLI and client ecosystem ✅ | Local and optional cloud models; API, model lifecycle and integrations documented in [repository](https://github.com/ollama/ollama) | [MIT implementation](https://raw.githubusercontent.com/ollama/ollama/main/LICENSE); model licenses separate. This pass did not audit authentication/default exposure or claim unsupported features absent | Strong default for one-machine developers. “OpenAI API + local models” alone is weak differentiation |
| LM Studio — shipped desktop/developer product ✅ | Local/offline operation and developer APIs; current docs also describe [local, LM Link remote and cloud choices](https://lmstudio.ai/docs/bionic/models) | App under [vendor terms](https://lmstudio.ai/app-terms), distinct from open components. [Privacy](https://lmstudio.ai/app-privacy) distinguishes on-device operation from downloads/service use. No claim that remote execution is absent | Competes on accessible installation and remote UX already; compare independent usability, not screenshots |
| Exo — macOS app and source deployment ✅ | [Current README](https://raw.githubusercontent.com/exo-explore/exo/main/README.md) documents MLX, topology-aware splitting, tensor parallelism, device discovery and multiple API compatibilities. Linux README simultaneously describes CUDA extras and says Linux GPU support is under development: unresolved scope, verify exact release | Current [LICENSE](https://raw.githubusercontent.com/exo-explore/exo/main/LICENSE) is **Apache-2.0**. Do not generalize historical exo/ex-exo licenses to this current tree. Namespace isolation is not independently audited authentication; no new insecure-default claim made | Direct pooling competitor. Its published speedups remain author-reported; UCompute must demonstrate a useful advantage on matched hardware and trust policy |
| Petals — research system with public/private swarm and Python interface ✅ | [README](https://raw.githubusercontent.com/bigscience-workshop/petals/main/README.md) describes block-distributed inference/fine-tuning across participating machines; architecture/model/hardware support is bounded | [MIT](https://raw.githubusercontent.com/bigscience-workshop/petals/main/LICENSE). Participating providers process activations; public swarm should not be assumed confidential. No current full-platform matrix reproduced | Genuine distributed execution baseline; not merely forwarding. Compare feasible supported models and network conditions |
| llama.cpp — active engine/tooling project ✅ | [Repository](https://github.com/ggml-org/llama.cpp) covers broad CPU/GPU inference, GGUF, server and RPC; UCompute already uses this engine | [MIT](https://raw.githubusercontent.com/ggml-org/llama.cpp/master/LICENSE). Engine/runtime integration is not an authorization layer; UCompute retains loopback-only raw RPC rule | Direct pinned-engine baseline isolates UCompute orchestration/transport overhead; kernel speed is not UCompute's invention |
| OpenRouter — operated API routing service ✅ | [Privacy policy](https://openrouter.ai/privacy) states inputs reach selected/routed model providers; [ZDR explanation](https://openrouter.ai/blog/insights/zero-data-retention/) distinguishes retention/routing controls from application logs | Hosted service terms, not an open self-hosted compute runtime. Provider policy/configuration matters; this pass does not claim all routed requests are retained | Strong developer convenience alternative. Private device control and offline operation are different needs, not automatic superiority |
| BOINC — long-running volunteer computing client/server ecosystem ✅ | [Project](https://github.com/BOINC/boinc) targets independent work units across volunteer computers, including platform-specific clients; not evidence of interactive transformer partitioning | [COPYRIGHT](https://raw.githubusercontent.com/BOINC/boinc/master/COPYRIGHT) and component licenses require per-component review (LGPL/GPL material); no code reuse proposed. Work-unit validation does not solve arbitrary inference verification | Learn voluntary contribution/accounting and lifecycle patterns; avoid pretending a new public network is necessary for private usefulness |
| Ray — distributed runtime, serving and scheduling framework ✅ | [Project](https://github.com/ray-project/ray) provides general distributed runtime/AI libraries; Python/application deployment, not consumer model-install UX | [Apache-2.0 and bundled notices](https://raw.githubusercontent.com/ray-project/ray/master/LICENSE). [Security docs](https://docs.ray.io/en/latest/ray-security/index.html) require trusted code/controlled networks and external isolation; token auth is defense in depth | General workload contracts are established territory; borrowing the idea does not establish safe public arbitrary-code execution |
| vLLM — serving engine and hardware plugin ecosystem ✅ | [Current hardware documentation](https://docs.vllm.ai/en/stable/getting_started/installation/index.html) includes CUDA, ROCm, XPU, CPU and Apple Silicon through vLLM-Metal; feature support varies. Serving/distributed execution must be evaluated per backend | [Apache-2.0](https://raw.githubusercontent.com/vllm-project/vllm/main/LICENSE); [GPU matrix](https://docs.vllm.ai/en/latest/getting_started/installation/gpu/index.html) limits feature/backend combinations. No blanket “datacenter only” assertion | Serious throughput and extension baseline; UCompute's target is private device integration, not claiming better serving kernels |
| SGLang — serving framework and benchmark tooling ✅ | [Repository](https://github.com/sgl-project/sglang) supports LLM/multimodal serving; [prefill/decode disaggregation](https://docs.sglang.ai/backend/pd_disaggregation.html) documents specialized parallel deployment | [Apache-2.0](https://raw.githubusercontent.com/sgl-project/sglang/main/LICENSE); hardware/configuration-dependent setup. This pass does not audit network exposure or tenant isolation | Prefill/decode-aware scheduling is not novel by itself; compare setup, workload and measured cost |
| Akash — operating decentralized resource market ✅ | [Providers and leases](https://akash.network/docs/learn/core-concepts/providers-leases/) describes infrastructure offers, deployment leases and blockchain registration, including GPUs | [Provider implementation Apache-2.0](https://raw.githubusercontent.com/akash-network/provider/main/LICENSE); chain/payment/provider trust differ from account-free local compute; no vendor price or fraud-rate claim adopted | Marketplace ops/availability/settlement are real product costs. UCompute should validate private demand and useful-work economics first |
| Tailscale — deployed secure connectivity service/client ✅ | [Security architecture](https://tailscale.com/security) provides device connectivity and controls, not inference kernels or model placement | [Client BSD-3-Clause](https://raw.githubusercontent.com/tailscale/tailscale/main/LICENSE); hosted/control components and service terms are distinct. Transport security does not hide input from the compute endpoint | A local runner plus existing secure networking may already solve the user's problem; compare against that composition |
| Kubernetes — mature orchestration and extension ecosystem ✅ | [Multi-tenancy guidance](https://kubernetes.io/docs/concepts/security/multi-tenancy/) discusses namespace/cluster isolation tradeoffs, quotas and network policies | Open-source orchestration; full distribution/component license review not repeated here. Tenant isolation depends on configuration, not the scheduler label | Desired-state reconciliation is a useful pattern; making consumers administer a cluster would undermine UCompute's UX goal |

Defensible differentiation remains a **hypothesis**: useful private execution across unlike personally controlled devices, explicit local-only policy, simple install/pair/revoke, reproducible refusal of unhelpful distribution, and interfaces others can implement. Counterarguments: local runners already serve most single-device needs; LM Link addresses remote use; Exo already pools; a VPN plus runner may suffice; hosted APIs avoid hardware administration. H6–H9 and spec sections 30–33 turn these objections into decisions. Do not use unverified competitor security claims as marketing.
