PROJECT COMMON — AUTONOMOUS ENGINEERING HANDOFF v2.0

ROLE AND MANDATE

You are the autonomous founding engineering organization responsible for designing, implementing, testing, securing, benchmarking, documenting and preparing Common, a free/open-source universal AI inference platform. Act as principal distributed systems architect, inference/runtime engineer, Rust engineer, C/C++ performance engineer, GPU engineer, networking/protocol engineer, security engineer, desktop engineer, developer-experience engineer, SRE/release engineer, technical product manager, adversarial reviewer, licensing/compliance analyst and QA lead.

Build actual working software, not a proposal or a demo. Never call mocked, simulated, partially implemented or untested functionality production-ready. Use subagents for independent work when available, but preserve a single authoritative requirements registry and avoid conflicting edits.

1. PRODUCT VISION AND ANALOGIES

Common is to AI compute what VLC is to media playback: install, select a model, press Run. Abstract model formats, inference runtimes, hardware and distributed execution. Support local-only, trusted remote, trusted pooled, public opt-in and managed enterprise execution where safe and technically feasible. Never claim arbitrary models can run on arbitrary devices.

Borrow design principles, not necessarily protocols, from:

• VLC/FFmpeg: universal compatibility and automatic backend selection.
• BitTorrent/IPFS: peer discovery, content-addressed chunks, integrity checks, resumable transfers and seeding.
• Bitcoin mining pools/BOINC/SETI@home: independent work units, voluntary resource pooling, metering and verification. Bitcoin proof-of-work does not validate arbitrary inference.
• Kubernetes: desired state, scheduling, admission control, health checks, reconciliation and recovery.
• Cloudflare/CDNs: latency-aware routing, warm caches, geographic placement.
• Tailscale/zero-trust networks: simple pairing, identity, encryption and revocation.
• Steam/package managers: one-click install, updates, model catalog, dependency compatibility.
• Docker/Nix/Git: portable, versioned, reproducible execution manifests.
• Stripe: stable, simple APIs over complex infrastructure.
• Linux: modular open standards and third-party ecosystem.
• Adaptive video streaming: adapt to network conditions, but never silently change model, precision or privacy policy.

2. PRIORITY POLICY: ALL FEATURES P0

Every item in this specification is P0 mandatory product scope. P0 does not mean execute simultaneously or bypass prerequisite gates. Use dependency-ordered milestones, with complete traceability. Every item must have an ID, owner, status, dependency list, test/verification procedure, implementation reference and evidence. Status is one of NOT_STARTED, IN_PROGRESS, BLOCKED, IMPLEMENTED_UNVERIFIED, VERIFIED, or INFEASIBLE_WITH_EVIDENCE. INFEASIBLE is not equivalent to DONE. If a research frontier has no defensible implementation, document the limit and do not counterfeit success. Do not declare the entire project complete until all feasible P0s are verified and infeasible items are explicitly disclosed.

3. NONNEGOTIABLE PRINCIPLES

Free open-source core; local-first; no mandatory account or cloud; cross-platform; open protocols; secure defaults; explicit permission for off-device inference; hardware-aware and network-aware scheduling; reproducible benchmarks; deterministic tests where practical; observability; no hidden resource usage; no silent model substitution; no fabricated benchmark claims; no unnecessary recreation of mature inference kernels; no mandatory cryptocurrency or blockchain; no vendor lock-in; legal and license compliance; maintainability; accessible UX; explainable failures. Prefer actual correct functionality over superficial feature checkboxes.

4. AUTONOMOUS EXECUTION CONTRACT

Proceed without asking about routine implementation decisions. Record assumptions and ADRs. You may edit files, install project-local dependencies, test, benchmark, create branches and commits where authorized. Do NOT delete unrelated data, expose secrets, open public ports, incur paid charges, publish releases, push externally or operate real-money settlement without explicit approval. Never execute untrusted scripts without inspection. Never claim hardware testing without hardware access. When blocked, implement independent work and record exact reproduction and next action. Keep AGENTS.md, docs/PROGRESS.md, docs/DECISIONS.md, docs/BLOCKERS.md, docs/VERIFICATION.md and docs/REQUIREMENTS.csv current. Commit after validated milestones if authorized. Leave restartable instructions at every stop.

5. COMPETITIVE RESEARCH

Inspect actual code, docs, licenses, issues and benchmarks (as available) for Petals, Exo, llama.cpp/RPC, prima.cpp, Ollama, vLLM, SGLang, TensorRT-LLM, MLX/MLX-LM, Ray Serve, LocalAI, Jan, LM Studio, KoboldCpp and relevant decentralized inference research. Record source URL, access date, version, hardware, model formats, local/distributed modes, scheduling, networking, privacy, fault handling, APIs, onboarding, license and benchmark evidence in docs/research/COMPETITIVE_MATRIX.md. Record testable differentiation hypotheses in docs/research/RESEARCH_GAPS.md. Do not assert superiority before reproducible head-to-head tests.

6. STACK AND ARCHITECTURE

Prefer Rust stable/Tokio/Serde, Tonic+Protobuf where warranted, Axum, rustls, QUIC, mDNS, SQLite, tracing/OpenTelemetry, llama.cpp/GGML as first inference adapter, Tauri+React+TypeScript+pnpm, cargo test/nextest, Criterion and GitHub Actions. Validate licenses and version compatibility before pinning. Architecture: control plane (policy, scheduling, identity, orchestration), data plane (model execution, activations, KV cache, transport), artifact plane (manifests/chunks/cache), trust plane (identity/privacy/security), economic plane (metering/fairness/settlement behind gates). Do not make central service mandatory for local or trusted LAN mode.

Rust workspace crates: common-core, common-hardware, common-models, common-runtime, common-network, common-scheduler, common-distributed, common-api, common-cli, common-artifacts, common-identity, common-policy, common-metering, common-orchestrator, common-observability, common-bench. Apps: apps/desktop, apps/mobile-companion (where viable), optional apps/web. Keep transport and inference backend adapters replaceable. Version protocol schemas and provide compatibility negotiation.

7. EXECUTION MODES

A: Local inference, no network. B: Trusted remote execution on one device. C: Genuine trusted pooled model inference across two or more devices with partitioned layers/tensors, intermediate activation transfer and verifiable memory placement. D: Independent batch/request distribution to multiple workers (mining-pool analogy). E: Hybrid placement combining clustered large-model inference and independent work units. F: Public opt-in swarm only after threat-model, abuse, privacy, verification and operational gates. G: Enterprise federation and managed cloud bursting only with explicit policy and cost consent. Document and test each separately. Forwarding a whole prompt to one peer is not pooled inference.

8. SCHEDULER AND OPTIMIZATION

Inputs: model architecture, quantization, context, KV-cache, available RAM/VRAM/unified memory, backend capabilities, CPU/GPU throughput, network RTT/bandwidth, peer churn, artifact locality, trust policy, energy, cost and contribution limits. Optimize measured time-to-first-token, decode latency, throughput, reliability and resource use under hard feasibility/privacy constraints. Implement deterministic baseline, candidate generation, cost model, plan validation, execution feedback and automatic fallback. Never assume adding peers improves speed. Separate prefill/decode cost, compute versus transfer time and cold/warm model state. Compare optimizations against baselines; reject regressions.

9. SECURITY, PRIVACY AND ECONOMICS

Threat-model malicious peers, compromised trusted peers, prompt/activation leakage, malicious metadata, model supply-chain attacks, resource exhaustion, identity spoofing, Sybil attacks, billing fraud and malicious workloads. Implement authenticated pairing, encrypted transport, key rotation/revocation, bounded messages, quotas, sandboxing where feasible, least privilege, secret redaction, local-only binding defaults, audit logs and explicit trust policies. TLS does not hide prompts from a peer processing them. Confidential computing and cryptographic proof of inference are research targets, not assumed solved. Credit systems must meter useful work, account for cheating and never promise redeemable value without compliance and fraud controls. No mandatory token.

10. BENCHMARK AND TEST PROGRAM

Measure TTFT, prefill and decode tok/s, p50/p95 latency, RAM/VRAM, network bytes/token, model load, discovery, handshake, power where measurable, failure detection/recovery and per-request cost. Scenarios: CPU-only, accelerated GPU, two-node LAN, three-node heterogeneous LAN, WAN emulator, bandwidth/latency stress, peer disconnect, memory pressure, concurrent requests, batch embeddings, model chunk distribution, mixed workloads and security abuse cases. Pin prompts/models/seeds/configs; distinguish simulation from actual hardware. Preserve JSON/CSV results, hardware manifests, command lines and environment versions. Compare against supported versions of Petals, Exo, prima.cpp, llama.cpp and local baseline on matched hardware and workloads. Never fabricate numbers.

11. QUALITY GATES

Gate A build: clean builds, lint, formatting, dependency/license scans, secrets checks. Gate B inference: real model loading, generation, streaming, cancellation and cleanup. Gate C network: discovery, authentication, revocation, disconnects and malformed message defense. Gate D pooling: genuine two-node partitioned execution, numerical correctness, distributed memory evidence and failure semantics. Gate E product: install, select, run, pair, pool, diagnose without developer intervention. Gate F public swarm: adversarial security review, verified accounting, privacy disclosure, abuse protection and realistic churn tests. Gate G release: signed/checksummed artifacts, reproducible builds where practical, compatibility matrix, release notes, CI, documentation and benchmarks. Block promotion if any gate fails.

12. USER EXPERIENCE

Primary: Install → Select Model → Run. Secondary: Add Device → Authorize → Pool → Run Larger Model. Third: Opt in to contributing resources → Set battery/power/time/privacy limits → View usage receipts. Expose speed/cost/privacy/energy policies, not obscure scheduler knobs by default. Desktop views: Home, Models, Chat, Hardware, Peers, Cluster, Jobs, Contributions, Performance, Privacy, Settings, Diagnostics. Provide understandable errors for unsupported model, unavailable GPU, inadequate memory, poor network, disconnection, license restrictions and privacy conflicts. Explicit consent before public compute or paid resources.

13. REPOSITORY LAYOUT

common/ root with AGENTS.md, README.md, LICENSE, CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md, Cargo.toml, Cargo.lock, package.json, pnpm-workspace.yaml, rust-toolchain.toml, .github/workflows, crates/ modules above, apps/, proto/, examples/, tests/{integration,distributed,security,fixtures}, benchmarks/{harness,scenarios,results}, scripts/, packaging/, docs/{architecture,research,security,protocols,benchmarks,decisions}, docs/REQUIREMENTS.csv, docs/PROGRESS.md, docs/DECISIONS.md, docs/BLOCKERS.md, docs/VERIFICATION.md. Adapt only with ADR.

14. MILESTONES (ALL P0; ORDER IS DEPENDENCY, NOT PRIORITY)

M0 Research, scaffold, licenses, threat model, benchmark harness and requirement IDs.
M1 Real local GGUF inference, hardware detection, streaming, CLI/API and tests.
M2 Desktop application, model library, hardware diagnostics, installers.
M3 Trusted LAN discovery, pairing, identity, encrypted networking and remote inference.
M4 Genuine two-node partitioned inference and correctness/memory tests.
M5 Hardware/network-aware scheduler, adaptive placement and comparative benchmarks.
M6 Model chunking/seeding, cache, deduplication, integrity and distribution.
M7 Distributed independent jobs, batch inference, embeddings and contribution metering.
M8 Reliability, self-healing, checkpoints, migration, fault injection and observability.
M9 WAN networking, NAT traversal, public swarm identity, trust and abuse defenses.
M10 Credits, reputation, verified receipts, economic mechanisms and gated settlement.
M11 Multimodal, MoE-aware, speculative decoding, advanced KV and adapter ecosystem.
M12 Mobile/browser contributors, power/thermal controls and enterprise federation.
M13 Security review, cross-platform CI, docs, release packaging and competitive beta.
M14 Advanced research validation: confidential computing, verifiable inference, token economics feasibility and other frontier work. Do not counterfeit unavailable breakthroughs.

15. DOCUMENTATION

Provide quickstart, installation, hardware/model compatibility matrix, CLI/API/SDK reference, peer pairing, private pooling, public-swarm warnings, contributor controls, privacy, troubleshooting, architecture, threat model, protocol specs, benchmarking, release process, developer guide and contribution guide. Every feature has a demonstration and verification procedure.

16. AUTONOMOUS EXECUTION PROCEDURE

1. Inspect OS, CPU, RAM, disk, GPU, toolchains, Git, network and available permissions.
2. Research ecosystem and select verified dependency versions/licenses.
3. Create docs/REQUIREMENTS.csv from every numbered capability below, all priority P0; assign stable IDs.
4. Scaffold repository and tests; implement real local inference first.
5. Work through dependency-ordered milestones without requesting routine guidance.
6. After each milestone build, test, lint, scan, benchmark, record evidence, fix failures, update docs and commit if authorized.
7. If blocked by credentials, hardware, legal compliance, paid infrastructure or research limitations, document precise blocker and continue independent work.
8. On session end report implemented/verified/blocked/infeasible counts, test evidence, benchmark evidence, limitations, artifacts and exact next commands. Never mark unverified work done.

17. FINAL MANDATE

Build Common as maintainable real software. Do not stop at architecture, scaffolding or UI. Do not confuse remote execution with pooled inference. All capabilities below are P0 product scope, but implementation is dependency-ordered and security-gated. Continue autonomously as far as environment and permissions allow. The ambition is universal AI execution; the obligation is demonstrable correctness.

18. COMPLETE P0 FEATURE REGISTER (120 CAPABILITIES)

Every item below is mandatory P0 scope and requires an individual acceptance test and evidence link.

18.1. Universal model execution

• P0 / F-01-01: Automatic hardware and backend detection.
• P0 / F-01-02: GGUF model loading and streaming inference.
• P0 / F-01-03: Automatic memory feasibility estimates.
• P0 / F-01-04: CPU/GPU acceleration and fallback.
• P0 / F-01-05: Model download, integrity checks, and cache.
• P0 / F-01-06: Multi-backend adapter system.
• P0 / F-01-07: Automatic quantization recommendations.
• P0 / F-01-08: Context-length and KV-cache planning.
• P0 / F-01-09: Multimodal model support.
• P0 / F-01-10: Speculative decoding integration.
• P0 / F-01-11: LoRA adapter management.
• P0 / F-01-12: Structured output and tool-calling compatibility.

18.2. Peer discovery and networking

• P0 / F-02-01: Automatic LAN peer discovery.
• P0 / F-02-02: Authenticated device pairing.
• P0 / F-02-03: Encrypted peer-to-peer transport.
• P0 / F-02-04: Peer capability exchange.
• P0 / F-02-05: Health checks and disconnect detection.
• P0 / F-02-06: NAT traversal and relay fallback.
• P0 / F-02-07: Peer latency and bandwidth profiling.
• P0 / F-02-08: Topology-aware routing.
• P0 / F-02-09: Trusted device groups.
• P0 / F-02-10: Connection quality adaptation.
• P0 / F-02-11: Distributed peer discovery.
• P0 / F-02-12: Geographically aware peer selection.

18.3. Distributed inference

• P0 / F-03-01: Genuine two-node model partitioning.
• P0 / F-03-02: Distributed memory placement.
• P0 / F-03-03: Activation transfer and synchronization.
• P0 / F-03-04: Correctness testing against local execution.
• P0 / F-03-05: Automatic layer partition optimization.
• P0 / F-03-06: Pipeline parallel execution.
• P0 / F-03-07: Heterogeneous device balancing.
• P0 / F-03-08: Request-level load balancing.
• P0 / F-03-09: Distributed KV-cache strategies.
• P0 / F-03-10: Peer failure recovery.
• P0 / F-03-11: Tensor parallel execution.
• P0 / F-03-12: Dynamic repartitioning.

18.4. Scheduling and optimization

• P0 / F-04-01: Resource-aware feasibility filtering.
• P0 / F-04-02: Deterministic baseline scheduler.
• P0 / F-04-03: Local versus distributed decision.
• P0 / F-04-04: Placement cost estimation.
• P0 / F-04-05: Latency-aware peer selection.
• P0 / F-04-06: Prefill/decode-aware scheduling.
• P0 / F-04-07: Adaptive throughput optimization.
• P0 / F-04-08: Model locality and warm-cache preference.
• P0 / F-04-09: Energy-aware scheduling.
• P0 / F-04-10: Reliability-aware placement.
• P0 / F-04-11: Predictive capacity planning.
• P0 / F-04-12: Multi-objective optimization policies.

18.5. Model distribution and storage

• P0 / F-05-01: Content checksums and model manifests.
• P0 / F-05-02: Resumable model downloads.
• P0 / F-05-03: Local cache eviction policies.
• P0 / F-05-04: Model license metadata.
• P0 / F-05-05: Content-addressed model chunks.
• P0 / F-05-06: Peer-assisted model distribution.
• P0 / F-05-07: Deduplicated model storage.
• P0 / F-05-08: Partial shard retrieval.
• P0 / F-05-09: Verified chunk transfer.
• P0 / F-05-10: Model version pinning.
• P0 / F-05-11: Rare-shard replication.
• P0 / F-05-12: Predictive prefetching.

18.6. Security privacy and trust

• P0 / F-06-01: Authenticated peer identities.
• P0 / F-06-02: Explicit execution authorization.
• P0 / F-06-03: Encryption in transit.
• P0 / F-06-04: Resource quotas and rate limits.
• P0 / F-06-05: Peer revocation.
• P0 / F-06-06: Threat model and secure defaults.
• P0 / F-06-07: Prompt privacy policy engine.
• P0 / F-06-08: Workload isolation and sandboxing.
• P0 / F-06-09: Supply-chain verification.
• P0 / F-06-10: Audit trails and redacted diagnostics.
• P0 / F-06-11: Confidential-computing backend research.
• P0 / F-06-12: Cryptographic verification of arbitrary inference: research and implement if feasible; otherwise document impossibility/limits.

18.7. Contributor economy

• P0 / F-07-01: Opt-in idle resource contribution.
• P0 / F-07-02: Contribution time and usage accounting.
• P0 / F-07-03: Electricity and power-limit controls.
• P0 / F-07-04: Fair-share resource allocation.
• P0 / F-07-05: Contribution history and transparency.
• P0 / F-07-06: Resource availability schedules.
• P0 / F-07-07: Compute-credit ledger.
• P0 / F-07-08: Peer reputation and anti-Sybil controls.
• P0 / F-07-09: Contribution verification and fraud detection.
• P0 / F-07-10: Marketplace price discovery.
• P0 / F-07-11: Paid contributor settlements with legal/compliance gates.
• P0 / F-07-12: Tokenized incentives feasibility analysis; no mandatory token or cryptocurrency.

18.8. Reliability and orchestration

• P0 / F-08-01: Timeouts and bounded retries.
• P0 / F-08-02: Graceful cancellation.
• P0 / F-08-03: Clear error propagation.
• P0 / F-08-04: Deterministic recovery states.
• P0 / F-08-05: Desired-state cluster reconciliation.
• P0 / F-08-06: Checkpoint-aware job restart.
• P0 / F-08-07: Replica placement.
• P0 / F-08-08: Admission control and backpressure.
• P0 / F-08-09: Rolling peer upgrades.
• P0 / F-08-10: Protocol compatibility negotiation.
• P0 / F-08-11: Automatic workload migration.
• P0 / F-08-12: Multi-region failover.

18.9. Developer ecosystem

• P0 / F-09-01: CLI and OpenAI-compatible API.
• P0 / F-09-02: Streaming responses.
• P0 / F-09-03: Health and metrics endpoints.
• P0 / F-09-04: Versioned API documentation.
• P0 / F-09-05: Python and TypeScript SDKs.
• P0 / F-09-06: Container deployment examples.
• P0 / F-09-07: Plugin and backend extension SDK.
• P0 / F-09-08: Webhooks and event streams.
• P0 / F-09-09: Observability integrations.
• P0 / F-09-10: Infrastructure-as-code examples.
• P0 / F-09-11: Third-party marketplace integrations.
• P0 / F-09-12: Community extension registry.

18.10. Consumer experience

• P0 / F-10-01: One-click desktop installation.
• P0 / F-10-02: Model catalog and search.
• P0 / F-10-03: Integrated chat interface.
• P0 / F-10-04: Hardware compatibility diagnostics.
• P0 / F-10-05: Peer pairing UI.
• P0 / F-10-06: Plain-language failure explanations.
• P0 / F-10-07: Automatic updates with rollback.
• P0 / F-10-08: Visual resource contribution controls.
• P0 / F-10-09: Model comparison and benchmarking UI.
• P0 / F-10-10: Privacy mode selector.
• P0 / F-10-11: Accessible cross-platform design.
• P0 / F-10-12: Mobile companion application.

19. ADDITIONAL P0 CAPABILITIES (20)

• P0 / X-01: Browser-based compute contributors with explicit browser sandbox limitations.
• P0 / X-02: Smartphone idle compute participation with charging-only option.
• P0 / X-03: Battery-aware and thermal-aware contribution.
• P0 / X-04: Scheduled compute availability and quiet hours.
• P0 / X-05: Peer reputation with statistical confidence intervals.
• P0 / X-06: Verifiable execution receipts and signed accounting.
• P0 / X-07: Redundant result checking for selected workloads.
• P0 / X-08: Model redistribution license policy and opt-in.
• P0 / X-09: Cost-aware cloud bursting with explicit user approval.
• P0 / X-10: On-premises enterprise federation.
• P0 / X-11: Distributed embedding pipelines.
• P0 / X-12: Distributed batch inference.
• P0 / X-13: Dataset locality scheduling.
• P0 / X-14: Privacy-preserving aggregate analytics.
• P0 / X-15: Reproducible execution manifests and environment pinning.
• P0 / X-16: Opt-in community hardware benchmark submissions.
• P0 / X-17: Workload-specific optimization profiles.
• P0 / X-18: Distributed speech and image workloads.
• P0 / X-19: Enterprise governance, SSO and audit policy.
• P0 / X-20: Offline-first local mesh clusters.

20. CROSS-DOMAIN INNOVATIONS (8 ADDITIONAL P0 CAPABILITIES)

• P0 / I-01: AI compute autopilot with speed/cost/privacy/energy objectives.
• P0 / I-02: BitTorrent-style model seeding and rare-shard replication.
• P0 / I-03: Compute reciprocity and fair-share credits without mandatory cryptocurrency.
• P0 / I-04: Kubernetes-like desired-state swarm reconciliation.
• P0 / I-05: Zero-trust workload routing policies.
• P0 / I-06: CDN-like warm model routing and prefetch.
• P0 / I-07: Mixture-of-experts-aware expert placement.
• P0 / I-08: Adaptive-streaming-inspired network speed governor without silent model changes.

21. FEATURE ACCEPTANCE STANDARD

For every F/X/I item, create a requirement record with fields id, priority=P0, description, analogy, dependencies, design, implementation_paths, unit_tests, integration_tests, security_tests, benchmark_scenario, acceptance_criteria, observed_evidence, status, blocker, last_verified_commit. Acceptance criteria must be falsifiable and concrete; no generic ‘works as intended’. Research-only features require a literature review, experimental protocol and an honest feasibility determination; implementation remains open if feasible. No mass-marking features VERIFIED without per-item evidence.

22. INTEGRATION AND NONREGRESSION

Build a cross-feature integration matrix: privacy policy × execution mode × hardware backend × model type × network topology × failure scenario. Validate no public peer receives local-only workloads; no resource contribution exceeds quotas; no model redistribution violates policy; no paid cloud is invoked without approval; no fallback changes model/quantization without consent. Preserve tests for all earlier milestones while adding new features. Keep public-swarm services disabled by default until gated.

23. BUSINESS MODEL AND OPEN SOURCE GOVERNANCE

Maintain useful, unrestricted open-source local and private-cluster functionality. Optional commercial offerings may include managed clusters, enterprise fleet administration, SLAs and compliance support, but cannot make the open core a crippled demo. Select a defensible license after dependency review. Document governance, contributor agreements if required, security disclosure, trademark use and third-party license obligations. Evaluate costs and market fit separately from technical claims; do not assert unicorn valuation as a deliverable.

24. FIRST COMMANDS

Inspect environment and repository; write docs/PROGRESS.md, docs/REQUIREMENTS.csv, docs/DECISIONS.md and AGENTS.md. Begin M0 and M1 immediately. If the project already contains code, preserve it and inspect before editing. Report only substantive milestone outcomes, blockers or permissions requiring human intervention

25. ADDITIVE INFRASTRUCTURE REQUIREMENTS

Extension accepted 2026-10-18 under docs/decisions/ADR-0004-infrastructure-program.md. Sections 1–24 and all 148 original F/X/I requirements retain their original scope and P0 priority. This extension supplies the product and delivery program for that scope. The root PROJECT_COMMON_AUTONOMOUS_HANDOFF_V2.md remains a historical input. P1 additions are required at their named release gates; P2 additions are committed later work. Neither changes the original denominator or makes all P0 features prerequisites for a limited alpha. Amended 2026-10-19 (D-018): public naming and component definitions (section 26.1), canonical website (section 29.5, WP-13), package coverage of every open row (section 30) and corrections for the af3e37c desktop evidence; sections 1–24 and the original rows are unchanged.

• P1 / U-01: Independently implementable peer protocol and cross-implementation interoperability.
• P2 / U-02: Versioned workload contract preserving the existing inference adapter.
• P1 / U-03: Evidence-scoped device participation and compatibility matrix.
• P1 / U-04: Independent developer integration without author assistance.

Acceptance, dependencies, owners, test targets and evidence live in the existing docs/REQUIREMENTS.csv, generated from this specification and scripts/registry_data.py. Do not mark these four requirements implemented because their design is documented. Commercial experiments below are hypotheses, not additional engineering requirement rows.

26. PRODUCT IDENTITY, USERS AND OUTCOMES

26.1 Mission and naming

Make supported computing resources accessible across hardware, operating systems, applications, networks and ownership boundaries through an open, secure, interoperable runtime and protocol. The infrastructure outcome is useful applications that can depend on authorized computation, including applications maintained independently of this project. Linux is an analogy for portability and ecosystem independence, not a plan to replace operating systems or a prediction of adoption.

UCompute is the program's public-facing name in use (owner directive 2026-10-19, D-018). The owner holds ucompute.org as its intended canonical site (section 29.5); naming and trademark clearance remain open and are not claimed, and holding a domain is not such clearance. Common remains the implementation name. Keep the common CLI, COMMON_HOME, crate names, wire identifiers, data paths and repository unchanged until a separately tested migration is justified.

| Existing product | Purpose | Architectural subsystems | §7 modes / §14 milestones / §30 packages / §31 cohort and §11 gates |
|---|---|---|---|
| Common Run | Local inference without account or cloud | Core runtime, model/artifact management, API/SDK, desktop | Mode A; M1-M2, M13; WP-01, WP-03, WP-06, WP-08; Stable Run, Gates A/B/E/G |
| Common Mesh | Explicitly authorized private remote and pooled execution | Core, Protocol, policy, Scheduler, private orchestration | Modes B-E among authorized devices; M3-M8; WP-02, WP-04, WP-05, WP-07, WP-09; Stable Mesh, Gates C/E/G (Gate D only when pooling is offered) |
| Common Swarm | Future opt-in independent resource contribution | Mesh plus isolation, metering, verification, abuse response and public routing | Mode F; M7, M9-M10, M12, M14; WP-09 contribution controls, WP-10, WP-11; Public contributor network, Gate F; disabled |
| Common Cloud | Optional managed execution, coordination and enterprise operations | The same open contracts plus operated services; no required cloud for Run/Mesh | Mode G; M12; WP-12; Commercial managed service; no paid service authorized |

Core, Protocol, Scheduler and SDK are subsystems shared by these products, not four extra products. Core is the portable execution runtime: hardware/backend probing, model and memory planning, the verified artifact store, pinned engine provisioning and supervised execution through the common-runtime InferenceBackend boundary, with execution manifests (section 6 data and artifact planes, section 28.1; crates common-core, common-hardware, common-models, common-artifacts, common-runtime; F-01-*, F-05-01/02, F-08-01..03, X-15; WP-03, WP-10). Protocol is the set of open versioned contracts: the local HTTP API (crates/common-api/openapi.json) and the experimental peer profile (section 28.2, U-01, F-08-10; WP-07), later extended by the U-02 workload envelope (section 28.3). Scheduler covers feasibility, placement and local-versus-distributed decisions (sections 8 and 28.4; F-04-*; WP-05); the section 6 common-scheduler crate does not exist, so the user still selects the target explicitly. SDK is the developer interface for independent applications over the same local API (section 29.2; F-09-*, U-04; WP-06). Public materials may label the products UCompute Run, Mesh, Swarm and Cloud and the subsystems UCompute Core, Protocol, Scheduler and SDK; each name maps one-to-one to the Common product or subsystem used in code, CLI, wire identifiers and docs/REQUIREMENTS.csv, and implies no separate edition. Current status is recorded in docs/REQUIREMENTS.csv, the section 30 package statuses, section 27.1, docs/VERIFICATION.md gates and docs/PROGRESS.md; README summarizes them. Developer message: “A local-first execution interface for supported models and authorized devices.” Consumer message: “Run a supported model here, or on a device you choose and trust.” Future pooling demonstrations must explicitly say when one model is split. Never advertise arbitrary hardware/model support, complete provider privacy or guaranteed speedups.

26.2 Reality and scope labels

Every claim must be labelled by context: verified reality (specific evidence and configuration), current implementation (code exists, acceptance incomplete), committed engineering scope (ledger/backlog), or strategic hypothesis (experiment/decision). A backend capability flag, successful compilation, an agent report, and a research paper are not interchangeable evidence. Current implementation and verification are summarized in docs/PROGRESS.md and docs/VERIFICATION.md; this section does not certify a release. Public pages, downloads, release notes and demonstration media use these same labels and the section 27.1 support terms (the roadmap may also show section 2 ledger statuses); they add no other status vocabulary. A recorded demonstration is verified reality only for its named builds, hardware and configuration; it is not an offer to download those builds, which stay within their section 31 cohort.

26.3 Initial and future users

The initial beachhead hypothesis is local-first application developers with two personally controlled computers, especially a laptop plus a capable desktop. They can evaluate useful Mode B execution before pooling exists, and their applications can test the interface beyond the official UI. Evidence supports technical feasibility on the recorded Mac/Windows pair; it does not yet establish demand. Audience comparisons and falsification experiments live in docs/research/RESEARCH_GAPS.md.

| User / stage | Job to be done | Observable value / requirement linkage |
|---|---|---|
| Initial developer | Use available private compute from an application without writing peer authentication and lifecycle code | SDK stream, cancellation and typed refusal; F-09-01/04/05, U-04 |
| Multi-computer owner / homelab | Use a stronger trusted desktop while working on a laptop | Visible chosen target, no hidden off-device prompt, revocable trust; F-02-02, F-06-02/05/07 |
| Consumer | Install, choose an appropriate model, get useful output | Fresh-account UI-only first token, clear memory/backend errors; F-10-01/02/03/04/06 |
| Research team / university, later | Reproduce runs and use a private cluster within network policy | Pinned manifests, offline mesh, pooling evidence; X-15/20, F-03-01..04; campus multicast constraints explicit |
| Contributor, later | Share bounded spare capacity with a readable receipt | Opt-in, enforceable time/power/quota limits and revocation; F-07-01..06, X-03/04/06 |
| Enterprise, later | Operate fleets with policy, audited access and support | Federation, SSO, cost consent; X-09/10/19; no present compliance certification |

26.4 Journeys and nonfunctional obligations

| Journey | Success and important failure behavior | Gate / linkage |
|---|---|---|
| Install → select → run | Supported artifact on clean account, license/size/fit visible, real first token; corrupt or too-large model rejected without substitution | M2, F-10-01, F-01-03/05 |
| Add device → authorize → run remotely | Explain what peer sees; wrong code refused; group and sender/receiver policies enforced; chosen target shown; disconnect terminates with a typed error; the model need not be loaded on, or fit, the requesting device | M3, F-10-05/10, F-06-02/07 |
| Pool → run larger model | Display feasible partition, per-device placement and disclosure; reject poor network/insufficient memory; both physical devices compute portions of this model | M4/M5, F-03-01..04, F-04-03 |
| Integrate application | Versioned docs and clean example produce real streamed output; cancellation and errors remain usable without official desktop | M13, F-09-04/05, U-04 |
| Contribute → inspect usage → stop | Default off; owner sets hard limits before admission; stop cancels and frees resources; receipts reconcile | M7/M10/M12, F-07-01/02, X-06; not a current flow |
| Managed execution | Explicit account/service choice and maximum spend; identity and data scope shown; approval refusal performs no paid action | X-09/19, Mode G; not a current flow |

Accessibility requires keyboard and assistive-technology checks on actual supported platforms, beyond jsdom scans. Reliability includes bounded admission, timeouts, cleanup and recoverable errors under load. Privacy requires policy before network activity and no content in default diagnostics. Maintainability requires one authoritative wire description, tests and migration notes per contract change. Resource efficiency is evaluated against a matched baseline, not a fixed claim that distributed execution is better. Exact numeric feature criteria remain in the ledger; release cohort targets are in section 31.

Non-goals for current delivery: a new OS, custom inference kernels, arbitrary executable workloads, public anonymous RPC, mandatory accounts/tokens, mobile always-on servers, universal model compatibility, or rewriting the functional llama.cpp adapter. Batch/scientific/compilation/rendering/data-processing/specialized GPU work remains future evaluation; local embeddings are not evidence of distributed embedding pipelines.

27. DEVICE PARTICIPATION AND COMPATIBILITY

27.1 Roles and support policy (U-03)

Consumer submits permitted workloads; provider executes supported workload kinds; coordinator plans and supervises authorized work; relay transports encrypted traffic with bounded routing permissions and cannot independently authorize computation. A device may hold several roles. Being a consumer does not require a GPU or provider capability. Current daemon control is not proof of a general distributed coordinator. No public relay implementation is claimed.

SUPPORTED means an exact platform/backend/artifact/role combination passed the relevant stable release gate with reproducible evidence and a named maintainer. EXPERIMENTAL means code or bounded real evidence exists but the support gate is incomplete. PLANNED means committed scope without qualified implementation. UNSUPPORTED means prohibited or outside the present execution contract, not necessarily technically impossible forever. A compiled target, downloadable engine pin or installed GPU is not a successful backend execution.

The following is the 2026-10-18 baseline; the Windows and NAS rows were updated 2026-10-19. No full platform is yet declared SUPPORTED. Each future support record must name OS/build, architecture, GPU/driver, backend pin, artifact hash, role, test commit, evidence, limitations and maintainer. U-03 makes these records machine-checkable; this table is the current manual baseline.

| Platform / hardware | Current status and roles | Evidence / limitation and qualification work |
|---|---|---|
| macOS 26.2, Apple M2 | EXPERIMENTAL consumer/provider, local daemon control | CPU/Metal and older packaged artifact verified in docs/VERIFICATION.md; new HEAD package, clean-account UI, signing/update qualification pending |
| Other Apple Silicon / supported Intel macOS | PLANNED consumer/provider | M2 evidence does not establish other GPU/OS combinations; backend pin/build, native probe and install tests required |
| Windows 11 x64, Core Ultra 7 258V / Arc 140V | EXPERIMENTAL desktop and daemon consumer/provider | bf40e20 daemon Mode B/Vulkan; native NSIS build from caf1497 (sha256 df13fe79…; evidence af3e37c), per-user install and UI pairing with the macOS app; the Windows app ran a prompt on the Mac; a Mac daemon-API request ran on the Windows GPU (docs/evidence/desktop-two-machine-2026-10-19.md, open defects listed there); clean-account install, uninstall/data retention, signing, accessibility and key ACL validation pending |
| Windows ARM64 | EXPERIMENTAL provisioning target only; consumer/provider PLANNED | Engine pin exists; no native execution evidence; cross-build alone does not qualify |
| Linux x64 desktop/server | EXPERIMENTAL hardware parsing; consumer/provider PLANNED | Linux probe/CI definitions exist; no Linux engine pin or physical inference/installer evidence; headless daemon first |
| Linux ARM64 server / single-board computer | PLANNED consumer, selective provider | Native backend availability, RAM, storage/thermal endurance and CPU instruction support must be measured |
| NAS / embedded appliances | PLANNED consumer, conditional provider (via WP-03 headless Linux x64/ARM64 records) | Vendor OS/container permissions and CPU/GPU drivers vary; qualifies only as a WP-03 headless Linux/container record naming the exact vendor OS; no vendor-specific package committed, no arbitrary binary install recommendation or unrestricted host access |
| Android | PLANNED consumer; provider research under M12 | App lifecycle, foreground-service limits, battery/thermal/charging policies; physical-device tests before advertising contribution |
| iOS/iPadOS | PLANNED consumer; always-on arbitrary-code provider UNSUPPORTED | App-store execution/background restrictions; bounded supported workload experiments only after platform review |
| Browser | PLANNED consumer / sandboxed contribution experiment | Origin/security, WebGPU availability, memory and tab lifetime are separate limits; no native peer daemon assumption |
| Cloud VM / headless GPU server | PLANNED private provider/coordinator; public relay PLANNED M9 | Native OS/backend qualification, explicit network policy and owner approval for any paid deployment |
| Heterogeneous CPU/GPU pair | EXPERIMENTAL Mode B only | Mac/Arc pair does not prove CUDA/ROCm, partitioning, tensor parallelism or general placement |

Platform restrictions were checked against Apple's App Review Guidelines §2.5.2/2.5.4 and Android foreground-service restrictions; sources and access dates are in docs/research/SOURCES.md. These restrictions support a consumer-first mobile strategy, not a claim that every form of mobile compute is forbidden.

28. ARCHITECTURE EVOLUTION AND OPEN CONTRACTS

28.1 Preserve the existing boundaries

The control plane authorizes and admits before execution; the data plane uses the pinned llama.cpp adapter; artifacts retain checksums, version and license metadata. Existing common-runtime InferenceBackend/LoadedModel traits remain the concrete inference boundary. The desktop continues to use the same public local API as external applications. Add new crates only when a work package needs an implemented boundary; the architecture list in section 6 is not a list of delivered crates.

28.2 Open protocol (U-01)

Current peer wire behavior is recorded in proto/common/v1/control.proto, crates/common-network/src/proto.rs and ADR-0003 §9. It uses bounded protobuf control envelopes, signed capability bytes and JSON payloads tied to GenerateRequest/GenerateEvent. Hand-mirrored field/tag tests are useful but do not prove independent implementation. Existing HTTP OpenAPI compatibility is a separate contract from peer interoperability.

Before a stable protocol claim, publish an implementation-independent profile covering: TLS identity and revocation checks including resumption; pairing transcript and exporter binding; ALPN/major/minor negotiation; exact framing, size and timeout bounds; required/optional capability semantics; nested JSON schemas and units; authoritative versus advisory resource claims; lifecycle/stream/error/cancellation rules; replay/idempotency behavior; signed message canonicalization; scheduling constraints; extension namespaces; reserved fields; and retention/observability rules. Unknown optional fields must be tolerated; unknown required capabilities, workload kinds or incompatible major versions must fail before work. Transport alternatives must retain the same authentication and authorization properties; no downgrade to plaintext.

Conformance includes a separately built peer/client implementation sharing no common-network code, tested against the Rust reference using pinned versions and golden vectors. Cover good/bad identity, unknown field, incompatible major, malformed lengths, forged/stale capability data, real generation/streaming, cancellation, revocation during and after a session, and deterministic refusal. A client that merely wraps the official daemon is not an independent peer implementation. Stable versions require two independently maintained implementations and a published compatibility window; until then mark the peer profile experimental. Schema release tags, migration guides and governance belong with docs/api/README.md and CONTRIBUTING.md, not a new competing protocol.

28.3 Portable workload contract (U-02)

Introduce a schema envelope incrementally around existing inference requests, only after backend conformance remains intact. Required concepts: workload kind/schema version; request identity; content-addressed input/model references and license; typed input/output schemas; pinned runtime/backend; supported CPU/GPU/memory/context; resource quotas/deadline/priority; device/filesystem/network permissions; privacy/trust/retention constraints; cancellation and terminal outcome; integrity evidence; resource/usage accounting. Inputs and credentials must not enter general audit records.

Inference remains the first real adapter. Preserve its golden tokens, streaming, cancellation, target selection and errors on the same pinned model/backend. An envelope does not authorize arbitrary commands or downloaded code. Fail closed on unknown kind, incompatible schema, unapproved permission, impossible memory or privacy policy. Result hashes prove byte integrity, not truthful computation. New workload adapters require a separate threat model, isolation test and measured utility before admission; do not add a universal executor now.

28.4 Pooling and scheduling

Modes A/B/C/D keep their meanings: local, whole-request remote, one workload partitioned, independent jobs. M4 first measures a static two-host layer partition using the existing backend and encrypted private carriage. Verify exact model hash, layer ownership, device memory, actual inter-host activation traffic, real tokens and local correctness. Record both application-payload and network bytes, setup/cold-load versus decode costs. KV ownership is per stage initially; migration/repartitioning waits for its M8/M11 criteria. Tensor parallelism requires measured collective costs; heterogeneous throughput, unified-memory accounting and network jitter can make local placement preferable.

F-03-03 remains unchanged: ggml RPC graph/protocol overhead is not the specified native activation framing. A bridge experiment may advance other pooling rows; it does not complete that row or all of M4. Never expose raw RPC beyond loopback to obtain a benchmark baseline. Plaintext controls may run only SINGLE-HOST; cross-device comparisons use authenticated private transport.

M5 candidate filtering treats backend compatibility, memory, trust/privacy, quotas, power/battery/thermal limits, availability and spend consent as hard constraints. Score only feasible plans using measured CPU/GPU throughput, RTT/bandwidth, artifact locality, cold/warm state, prefill/decode costs, reliability and user priority. Decline distributed execution when predicted overhead exceeds benefit; expose the reason. Start deterministic before predictive optimization. Hold out configurations for evaluation, compare local-only and manual peer/split choices, publish estimation error and regretted decisions. Stale/missing resource claims require a bounded fresh probe or conservative refusal, never assumed capacity. Energy is measured with disclosed method or reported unavailable.

29. DISTRIBUTION, ECOSYSTEM AND SUSTAINABILITY

29.1 Staged adoption

Distribution begins with a reproducible private developer preview of Run/Mode B on qualified combinations, not a global marketplace. Before recruitment, complete installer and disclosure checks in section 31. The intended canonical public site is ucompute.org (section 29.5); it presents this specification, the ledger and committed evidence and is not a second source of truth. Other planned channels are project documentation/GitHub, reproducible engineering articles, relevant self-hosting and local-AI communities, and opt-in researcher/hardware collaborations. No outreach or publication is authorized by this document. Existing users' devices, prompts and contact details are not a marketing dataset.

| Stage | Offer / proof | Channel and next conversion | Failure signal |
|---|---|---|---|
| Private alpha | A clean install and real local token with honest supported matrix | Owner-approved testers → first usable run | Frequent terminal rescue, unclear license/fit or lifecycle failures |
| Developer alpha | Python/TypeScript examples and explicit target policy | Developer communities/research contacts → independently maintained application | Author help needed for routine streaming/cancel/errors |
| Private Mesh beta | Two-device install, discovery or explained manual pairing, revoke/recover | Existing Run users → authorized useful second-device workload | Pairing succeeds but no useful repeated workload, local latency harmed |
| Pooling demonstration | One model partitioned across real devices, correctness and memory proof | Reproducible technical article → independent replication | No capacity benefit or slower than a feasible single device without a justified capacity tradeoff |
| Public developer beta | Qualified artifacts, support contact, migration/rollback and published limitations | Maintained examples/integrations → returning applications/users | Bugs/support burden exceed capacity, compatibility breaks |
| Contributor / managed pilots | Bounded workload classes, receipts, explicit costs and incident owner | Only after trust/economics gates; buyer/provider interviews first | Verification and operations cost erase useful compute value |

Current demonstration (verified reality, Mode B only): two installed desktop apps, macOS and Windows, paired through their UIs on one network, with prompts run on the other device (docs/evidence/desktop-two-machine-2026-10-19.md, or a newer committed run). It previews the Private Mesh beta offer above; no alpha or beta cohort has started (section 31). Public demos and the website describe only what that evidence records: each request runs whole on one chosen device; no model is split or pooled. Name any direction not driven through the app UI (the 2026-10-19 Mac-to-Windows request used the Mac daemon API). Throughput measured on the stories15M fixture is a path check, not performance; show any figure only with model, hardware and commit, and never redistribute the fixture (B-011). Open defects and limitations stay in the evidence file and docs/BLOCKERS.md.

Proposed distribution loops are testable mechanisms, not assumed network effects: (a) a useful application makes the runtime worth installing, but fails if integration maintenance costs exceed compute benefit; (b) a user's second trusted device makes the first more useful, but fails if availability/latency or setup cost dominates; (c) a reproducible benchmark attracts a contribution improving compatibility, but fails if maintainers cannot review/support it; (d) future supply attracts demand only for compatible, reliable capacity with positive measured unit economics. Experiments, thresholds and owners are in research H6–H9. No public supply loop is needed for Run/Mesh to be useful.

29.2 Developer and governance program

Use the existing HTTP API and error vocabulary first. Deliver versioned Python/TypeScript SDKs, real-server examples, local development fixtures explicitly labelled synthetic, diagnostics without prompts, cancellation/backpressure examples, migration notes and a container example under F-09-04..07. U-04 measures independent completion, not SDK build success. Document core behavior without requiring the official desktop. Protocol implementation/compatibility governance and contribution ownership live in CONTRIBUTING.md; release/disclosure readiness lives in SECURITY.md. Before stable release, at least two maintainers must demonstrate a review and release rehearsal; no fabricated organization or promised staffing.

Apache-2.0 remains appropriate to the program's permissive implementation and independent-adoption goals; retaining it is a product decision, not a legal opinion. Its express patent terms do not clear third-party patents, model redistribution or trademarks. Keep dependency/model notices and license review. A proprietary optional service cannot make local/private functionality account-gated. No license, CLA, trademark ownership or external project policy is changed here.

29.3 Revenue hypotheses (not demonstrated willingness to pay)

All prices are experimental structures, not quoted market rates. Record actual cost and interview evidence before choosing amounts. Gross margin means net realized revenue less attributable delivery cost, including provider payouts, energy, network, verification, support and abuse losses; exclude tax collected on behalf of others. Founder labor is not free.

| Model / buyer | Value and pricing hypothesis | Cost / operational constraint | Alternative / channel | Dependencies and falsification experiment |
|---|---|---|---|---|
| Managed private infrastructure / small team | Operated deployments; monthly base plus measured usage | Provisioning, idle reserve, upgrades, incident response; low utilization can erase margin | DIY private servers or cloud; developer integrations | Qualified Core/Mesh + X-19; interview 10 teams, require 3 written scoped pilot interests before building hosted service |
| Enterprise administration / IT | Fleet policy/audit; annual fleet/support agreement | Identity integration, security review, support commitments and deployment variance | Existing fleet/cloud tooling; referrals | X-10/19, U-01; test policy/admin prototype with 3 teams, reject if no unmet operational task |
| Hosted coordination / team admin | Optional availability/coordination; per active team/device band | Metadata retention, outage impact, account security | Self-host coordinator/VPN; existing Mesh users | M9 reliability, offline fallback; test whether 5 teams prefer paid operation to documented self-hosting |
| Optional managed relay / mobile or NAT-constrained user | Authorized relay access; metered bandwidth with cap | Egress/abuse/denial-of-service dominate; no promised cheap flat rate | VPN/relay alternatives; failed-direct-connect journey | F-02-06, Gate F security; measure per successful session cost and connection benefit before pricing |
| Premium developer service / integration team | Hosted compatibility tests and diagnostics; seats or CI usage | Build minutes, artifact storage, secret handling and maintainer time | Self-host test suite; SDK adopters | F-09-05, U-01/04; 3 independent apps must request a paid service beyond open conformance tools |
| Reliability/support contract / business operator | Response and repair capacity; retainer with defined scope | On-call staffing, incident load; no SLA until staffed and measured | Upstream/community support; private beta | Stable relevant gate; cost 30 days of support tickets and compare offer with cost under pessimistic load |
| Usage-based execution / application operator | Compatible authorized inference without owning hardware; measured workload units | GPU utilization, load/cold starts, egress, provider quality and privacy | Managed inference APIs; SDK integrations | Mode G, explicit cost consent, threat review; compare cost per successful task on matched model/quality, not token price alone |
| Marketplace infrastructure / provider and buyer | Discovery/receipts/disputes; service fee on settled useful work | Fraud, verification, payments, refunds, liquidity and legal operations | Existing compute markets; targeted provider pilots | M9/M10 + Gate F, B-006; closed non-monetary simulation must reconcile costs and fraud before authorized monetary pilot |
| Contributor credits / private group | Transparent reciprocity; non-transferable usage units initially | Meter calibration, fairness, gaming and administration | Informal shared server; existing trusted groups | F-07-02/07/09, X-06; test fairness and usefulness with 3 groups; no redeemable value or revenue assumed |

29.4 Compute economics protocol

For each supported workload measure successful jobs and useful tokens, provider busy/idle time, utilization, model-loading and warm-cache cost, failures/retries, electricity (meter method or unavailable), network ingress/egress, verification/replication, fraud/dispute losses, payment processing if ever authorized, support time and acquisition effort. Report amortized hardware cost under explicit lifetime/utilization assumptions separately from marginal electricity cost. Never compare a sunk-cost home GPU to a fully staffed managed service without showing both accounting boundaries.

Cost per useful job = (compute + energy + network + load/cache amortization + verification + retries + operations + fraud/payment costs) / successfully accepted useful jobs. Report distributions and sensitivity to 10/30/60% utilization, doubled egress/support, provider churn and failed verification; these are scenarios, not observations. Provider availability is compatible capacity during requested windows, not registered devices. Marketplace liquidity is the fraction of eligible requests matched within deadline at consented cost, not sign-ups. Customer acquisition cost includes labor and is compared with retained contribution margin only after retention is observed.

No payment, transferable token or public credit promise is enabled. Any future currency/settlement proposal needs a separate technical/economic/abuse/legal decision and explicit operational authorization (F-07-11/12, B-006). Negative margins or verification costs exceeding useful compute value falsify the corresponding service, not the usefulness of local/private compute.

29.5 Canonical website and public surfaces

ucompute.org is the intended canonical public site (owner-held domain, D-018). A separate website workstream presents this repository; it never defines requirements, priorities, dates, support levels, contracts or claims of its own. Where the site differs from this specification, docs/REQUIREMENTS.csv or committed evidence, the repository governs and the site is corrected. A site need that implies product scope enters through section 30 or an ADR first. Website source does not use apps/web, which section 6 reserves for an optional product client; if the source enters this repository, record its location by ADR (section 13). DNS, hosting, deployment, access control, contact addresses and each public page or download are separately owner-authorized (section 4, B-007); each authorization is recorded (date, scope) in docs/DECISIONS.md or the WP-13 docs/VERIFICATION.md entry by the engineering session; domain ownership is not trademark clearance (section 26.1). Access-controlled previews follow the same claim rules, with access enforced at the hosting layer rather than by unlisted links. The site uses no third-party analytics, tracking cookies or prompt collection; any measurement follows section 32. Repository links point only to content the owner has published (B-007). WP-13 tracks delivery.

| Surface | Source of truth | Allowed content now | Before expanding |
|---|---|---|---|
| Product pages | Section 26.1 products and subsystems, section 26.2 labels, README status | Run and Mesh as evidenced, each claim labelled; Core/Protocol/Scheduler/SDK as subsystems; Swarm and Cloud as gated future work with no date | New claims cite committed evidence at a commit |
| Downloads and releases | WP-08 promotion record in docs/VERIFICATION.md: exact artifact, SHA-256, signature status, platform/backend, cohort | No binary download | A private-alpha development build only after its record (artifact, SHA-256, unsigned/ad-hoc status, Gates A/B, cohort) is in docs/VERIFICATION.md and the owner approves it; a public download also needs Gate G |
| Compatibility | Section 27.1; U-03 records once they exist | Section 27.1 rows with their date; no platform is SUPPORTED | SUPPORTED only from a U-03 record that passed its stable gate |
| Developer documentation | docs/api/README.md, crates/common-api/openapi.json, examples/ at a tag or commit | HTTP API as documented; SDKs PLANNED (F-09-05, U-04) | Rendered from tagged files, not a hand-maintained fork |
| Protocol documentation | proto/common/v1/control.proto, ADR-0003 §9, the U-01 profile when published | EXPERIMENTAL peer profile; no stability or independent-interoperability claim | U-01 evidence (section 28.2) |
| Demonstrations | docs/evidence/ | The section 29.1 current demonstration, with its limits | One committed evidence file per demo; pooling only after Gate D, stating when one model is split |
| Roadmap | Section 14 milestones, section 30 packages, ledger counts from scripts/validate_registry.py | Statuses and dependencies; no dates | Owner-approved dates only; never ahead of ledger status |
| Contribution and community | CONTRIBUTING.md, CODE_OF_CONDUCT.md, SECURITY.md | Links once repository content is published | A tested private security/conduct route before advertising one |
| Benchmarks | benchmarks/results/ with manifests | Committed results with hardware, model, command and commit; fixture (stories15M) results labelled path checks, not performance (section 29.1) | Section 33 comparison rules |

30. DEPENDENCY-ORDERED DELIVERY BACKLOG

M0–M14 in section 14 remain the milestone system. The following work-package IDs refine those milestones; they do not renumber requirements or form a second completion ledger. Priority here is execution order within mandatory scope. A package is complete only when its acceptance and evidence are recorded; it cannot promote linked requirements whose other criteria remain unmet.

Common fields for every package: status below is a planning status, not a ledger promotion; evidence destination is prospective unless explicitly linked; no application changes are made by this specification task. Security and performance acceptance are part of each package. Independent contributors may take separable packages with disjoint files; integration tests and reviews remain mandatory.

| ID / parent / priority / status | Requirements / owner | Objective, existing base and dependencies | Scope, acceptance and verification | Security, performance, blockers and evidence |
|---|---|---|---|---|
| WP-01 / M2-M3 / P0 / IMPLEMENTED_UNVERIFIED | F-10-01/04/05/11; desktop | Qualify Windows native desktop so owners can actually use the second device. Daemon/Arc and Node sidecar step exist. Depends on native toolchain and test host, not pooling. | Build on Windows; verify staged daemon provenance/arguments; clean-account install → license/model → real first token via UI; wrong-code/refused then successful pairing with Mac UI; quit/crash/restart/uninstall with explicit data retention; keyboard and screen-reader flow. Extend existing packaged verifier with target-native evidence, no manual API substitute. A remote run does not require the model to be loaded on, or fit, the requesting device; device wording matches the host platform. | Private firewall scope, no new public service; hash/OS/driver/commit, first-token and shutdown times. B-004/008; desktop evidence under docs/evidence/, raw run manifest under benchmarks/results/. Partial: af3e37c docs/evidence/desktop-two-machine-2026-10-19.md (native build, staged-daemon arguments, crash cleanup, wrong-code message, UI pairing, remote first token). |
| WP-02 / M3 / P0 / IMPLEMENTED_UNVERIFIED | F-06-01/04, F-08-08; security/runtime | Close promotion risks around default peer fairness and Windows key ACLs. Existing quotas, audit and Windows daemon; depends on native host access. | Test different Windows user cannot read/replace identity/config secrets; owner can reload. Measure shipped 4/3 slot defaults under the existing flood test, preserve <20% local p95 degradation criterion; capture/reproduce B-015 and fix or keep gate closed. Choose ordinary default policy from measurements. | No weakened quota acceptance or assumed Unix mode equivalence; measure errors/resource use and no peer quota bypass. B-014/015 plus ACL evidence; docs/security/THREAT_MODEL.md and docs/evidence/. |
| WP-03 / M1-M2-M13 / P0 / NOT_STARTED | F-01-01/02/04/07, F-10-01/11, U-03; release | Linux supported path and continuous platform testing. Probe and CI definitions exist; missing engine pin/native inference. Can proceed alongside WP-01. | Select and verify Linux engine artifact/license; native CPU inference first, GPU per tested backend; desktop install and headless startup/stop; require fixtures rather than silent skips; add native Windows/Linux packaging and lifecycle CI when push authorized. | No unauthenticated LAN service; measure install/load/peak RSS; preserve user data on upgrade. B-004/007/008; exact platform evidence and artifact hashes. ARM, NAS/container hosts and extra GPUs require separate records. |
| WP-04 / M4 / P0 / NOT_STARTED | F-03-01/02/03/04/06; distributed | Static physical pooling, using current backend and authenticated Mesh. Two-host Mode B is base, not pooling. Depends on same compatible engine build, partition feasibility and private policy. | Implement scoped RPC supervision/carriage or native stage adapter; prove layer placement and transfer on two physical hosts; >=64 greedy tokens and ledger logit tolerances; per-host memory and failure/cancel checks. F-03-03 native framing criterion stays open if RPC cannot meet it; F-03-06 overlap measured separately. | Explicit activation/model disclosure and redistribution rights; RPC loopback only; limit sessions/bytes/memory; cold/warm/network bytes and local baselines. B-016 protocol-overhead issue; tests/distributed/ and benchmarks/results/. |
| WP-05 / M5 / P0 / NOT_STARTED | F-04-01..12, F-02-07/08/10, F-03-05/07, F-10-09, X-17, I-01/05/08; scheduler | Automatic placement that improves selected workloads, not device-count maximization. Memory planning exists; scheduler absent. Depends on WP-04 measurements for split scoring, F-02-07 profiling and policy. | First feasibility filter/deterministic local-versus-peer scheduler; then cost/partition planner; held-out model/link/device grid versus local and best measured manual plan; record all regressions and apply each row's criterion. | Filter privacy/spend/quota before scoring; stale data fails conservatively; no model/precision substitution. Network/hardware availability limits grid; benchmarks/results/ with latency/throughput/energy method. |
| WP-06 / M13 / P1 / NOT_STARTED | F-01-12, F-09-04/05/06/10/11/12, U-04; devex | Independent useful applications. OpenAPI and official-client smoke examples exist; Common SDKs absent. Depends on stable tested local API; can start before pooling. | Ship versioned Python/TS SDK source, streaming/cancel/errors examples and container example; live-server integration tests; 3 independent developers, >=2 separately maintained applications and >=2/3 unaided real first streams <=30 min. All exercise cancellation and refusal. | Examples default local; no credentials/prompt capture; measure time and issue count with consent. Recruitment/publication authorization distinct; docs/api/README.md and consented summaries in docs/evidence/. |
| WP-07 / M3-M13 / P1 / NOT_STARTED | F-08-10, U-01; network | Independent peer implementation and migration contract. Current protobuf/JSON and ADR exist. Depends on current auth/policy tests; can start alongside WP-06. | Document full profile and nested schemas, test vectors and extensions; separate implementation (no common-network reuse); positive/negative compatibility suite and real streamed inference/cancel/revoke; versioned release/deprecation policy. | Reject downgrade, malformed lengths and unsupported required capabilities; no stable claim based on mirror test; profile framing/auth overhead. Evidence: protocol vectors/tests plus docs/api/README.md. |
| WP-08 / M2-M3-M13 / P1 / NOT_STARTED | F-10-01/05/07/11, F-06-09, X-14/16, U-03/04; product/release | Private beta → developer beta → scoped stable releases. Depends on WP-01/02/03 and scope-specific WP-06/07, not every P0. | Execute section 31 cohorts; signed update/tamper rejection/rollback; compatibility matrix and release notes; independent install/support rehearsal; enumerate omitted capabilities. | Signing, security contact, backup/retention and no default telemetry; measure failures including abandonment. B-007/008; docs/VERIFICATION.md promotion record with exact artifacts. |
| WP-09 / M6-M7-M8 / P0 / NOT_STARTED | F-05-03..12, F-03-08/10/12, F-07-01/02/04/05, F-08-04..09/11, F-09-08/09, F-10-08, X-08/11/12/13/20, I-02/04/06; artifacts/orchestrator | Useful private job pools, cache distribution and recovery before public supply. Existing whole-model store/remote path. Depends on licensed transfers, reliable admission and explicit consent. | Split into chunking+corruption tests, bounded work queue/load distribution, embedding/batch adapters, owner contribution controls, then recovery/checkpoint/fault-injection suites; each linked row's numerical criteria apply. | Do not equate signed receipts with truthful compute; stop on quota/revoke; measure cache hit, useful throughput, retry/recovery, partial-download disk limits. Model licenses and third host needed for certain tests; benchmarks/results/. |
| WP-10 / M11-M12 / P2 / NOT_STARTED | U-02, F-01-06/09..11, F-03-09/11, F-07-03/06, F-09-07, F-10-12, X-01..04/18, I-07; runtime/mobile | Incremental workload/adapters and mobile roles without a universal executor. Inference traits exist. Depends on second real backend/conformance and explicit workload isolation design. | Versioned envelope and rejection tests; preserved golden inference; separately qualify each backend/workload; mobile consumer first, provider experiment only charging/thermal bounds and lifecycle proof on physical device. | No arbitrary downloaded code or implied confidential compute; report envelope overhead and thermal/battery behavior. B-003/005/012; tests/ plus docs/evidence/ and updated compatibility matrix. |
| WP-11 / M9-M10-M14 / P0 / NOT_STARTED | F-02-06/11/12, F-06-08/11/12, F-07-07..12, F-08-12, X-05/06/07, I-03; network/security | Gated public-network feasibility, not a launch date. Private Mesh only today. Bounded private laboratory work depends on its applicable authorization/isolation controls; its experiments establish Gate F. Public participation additionally depends on passing Gate F, meaningful demand and explicit owner approval. | Separate NAT/relay lab, threat-reviewed narrow workloads, churn/abuse/fault experiments, signed accounting reconciliation, verification-cost study; public participation only after section 31 gate and owner consent. | Public ingress/tenant isolation, budgets, operator shutdown, fraud and privacy disclosure; receipts != correctness, TLS != confidentiality. B-005/006/007; frontier reviews and benchmarks/results/. |
| WP-12 / M12-M13 / P1 / NOT_STARTED | X-09/10/19, U-01/04; product/maintainers | Sustainable managed services and independent ecosystem. No demand proof. Interviews, cost modelling and maintainer rehearsals may begin before stability and produce H6-H9 evidence; commercial service pilots depend on stable relevant scope, that evidence and section 29 economics. | Non-monetary interviews/cost model first; 2 independent maintainers release rehearsal; 3 useful independent integrations retained for 90 days before ecosystem-maturity claim; enterprise pilot only with explicit approvals. | No disabled-open-core upsell or hidden cloud spend; model gross margin including labor/failure/verification; reject unsupported pricing hypotheses. Legal/service ownership blockers; research experiment reports, not fabricated revenue. |
| WP-13 / M2-M13 / P1 / IN_PROGRESS | Presents F-09-04, F-10-01/07, U-01/03 evidence; advances no ledger row; release/docs | Intended canonical public site at ucompute.org (section 29.5), built in a separate site workspace; site work began 2026-10-19 and no site evidence is recorded here yet. Base: README.md, docs/api/README.md, crates/common-api/openapi.json, sections 14/27/30, CONTRIBUTING.md, SECURITY.md, CODE_OF_CONDUCT.md, docs/evidence/. Downloads depend on WP-08 promotion; each public surface depends on owner authorization. | Every surface follows its section 29.5 source and current limit; every capability statement carries a section 26.2 label and cites a ledger row ID or committed evidence path at a commit (as a link only once that content is published, B-007); the roadmap reproduces sections 14/30 and ledger counts without dates; compatibility derives from section 27.1 and later U-03 records; protocol pages are marked experimental until U-01 is VERIFIED. A claim check over the built site fails on pooling/split/combined-device, SUPPORTED, speedup, privacy-guarantee or Swarm/Cloud-availability wording without matching ledger evidence, and on any download link absent from the docs/VERIFICATION.md promotion record. | No third-party analytics, tracking cookies or prompt collection; measurement only under section 32 and X-14; HTTPS only; preview access enforced at the hosting layer; published hashes equal docs/VERIFICATION.md and the F-10-07 updater verifies signatures independently of the site; DNS, hosting and cost are owner-authorized. B-007/008. Evidence: site source commit and claim-check output recorded in docs/VERIFICATION.md; owner authorizations recorded as section 29.5 requires. |

Every open ledger row is named in at least one package; the ledger's milestone, dependency and acceptance fields stay authoritative, and naming a row here does not lower its criteria.

Next action: finish WP-01's remaining checks (clean-account install, quit/restart/uninstall data retention, keyboard and screen-reader flow; repeat the wrong-code case and the Mac-side group/run steps through the apps' UIs, without API or DevTools substitutes) and close the defects listed in docs/evidence/desktop-two-machine-2026-10-19.md; native build and UI pairing are evidenced (af3e37c). WP-02 is a Mesh promotion prerequisite and may proceed in parallel; Windows ACL failures take priority over wider testing. Hotspot discovery is already evidenced, so do not redo it as an implementation prerequisite; retain it as regression coverage on a permitted network. WP-03 then broadens qualified platform coverage. WP-04/05 and WP-06/07 are distinct tracks; independent SDK work does not wait for pooling, marketplace or advanced research. WP-13 proceeds independently and does not block or promote any other package.

31. RELEASE COHORTS AND QUALITY GATES

Sections 11 and 14 remain the quality gates and milestones. The following are proposed target thresholds, not measurements or launch commitments. Gate A/B always apply to executable promotions; C for Mesh; D only when pooling is advertised; E for the journeys in scope; G for external artifacts; F for public untrusted execution. A limited release names omitted capabilities and leaves their P0 ledger rows open. Whole-product completion still uses section 2.

For cohort metrics, report numerator, denominator, platform mix, retries, abandonment and confidence limits. A successful retry does not erase an initial failure. Test the exact release artifact, not a developer checkout. Install-to-first-use time includes model/engine download; also report download time separately with bandwidth and pre-cached runs labelled. Qualify each advertised platform/backend independently; aggregate averages cannot hide a failing platform.

| Cohort | Admission and measured targets | Promotion authority / blockers |
|---|---|---|
| Internal development | Registry/build checks; all new behavior labelled; no public exposure; required real fixtures for inference checks | Current program, not a release certification |
| Private alpha | >=5 consented testers; >=4/5 clean-account installs and UI first inference within 15 min using a pinned <=1 GB licensed model at recorded bandwidth; 20 quit/crash/restart cycles with no orphaned engine; wrong-code and local-only negative tests where Mesh offered | Maintainer review; unsigned exceptions explicitly disclosed for private local testing only; no stable support claim |
| External developer alpha | U-04 target study; live Python+TS suites on each advertised platform; typed errors and cancellation; tested security contact; no critical/high unresolved issue affecting offered scope | Owner approves external distribution; document experimental API compatibility and manual rollback procedure |
| Public developer beta | >=20 independent testers across advertised OS set, >=90% install/first-use success per OS; >=500 completed/failed sessions recorded with consent, >=99% crash-free sessions per OS; update/tamper/rollback tests; docs/support triage rehearsal; U-03 records complete for advertised combinations | Gate A/B/G, C for Mesh; B-007/008 and security gaps resolved; signed artifacts; no unknown model redistribution license |
| Stable Run | >=30 days beta; >=99% successful eligible local requests over >=1,000 attempts per supported combination; no known critical/high security or data-loss defect; retained data survives update/rollback, documented supported API compatibility window; accessibility on each platform | Run-specific A/B/E/G, no requirement to launch Swarm or claim pooling |
| Stable Mesh | Stable Run plus >=10 independent pairs across >=2 network classes; >=95% paired first-use success; revoke/disconnect within existing ledger bounds; default quota fairness criterion met; >=1,000 authorized requests with zero observed local-only/policy bypass; U-01; platform key protections verified | C/E/G and WP-02; D/F-03 rows only if pooling offered. Report sample limits; zero observed violations is not proof of impossibility |
| Public contributor network | Independent security review with no unresolved critical/high findings; narrow isolated workload allow-list, provider consent/limits, signed metering and verification; >=10,000 fault/abuse attempts with zero escape/unauthorized admissions; 24 h churn/kill/quota run, abuse response/kill-switch rehearsal and positive measured useful-work economics | Gate F plus A/B/C/G, F-06-08, F-07-01/02/09, X-06/07; explicit owner public-service approval; no launch on receipt signatures alone |
| Commercial managed service | Relevant stable scope; >=30 days costed pilot/SLO evidence; 3 scoped buyer pilot commitments; staffed incident and support plan, privacy/retention/contract review, tested spend caps and cancellation, sensitivity model supports stated margin | Mode G consent and owner paid-infrastructure authorization; qualified legal review before settlement; a plan or interview is not paid demand |

Each promotion inherits the preceding applicable cohort obligations; public beta cannot bypass the external-alpha security and support checks. A release may omit Mesh or pooling, but must say so explicitly. U-03 records can remain EXPERIMENTAL during beta; only stable, evidence-qualified combinations become SUPPORTED, avoiding circular qualification.

Public beta compatibility policy target: publish a versioned supported matrix and migration guide before changing an external contract; maintain current and previous supported minor versions for at least 90 days after a successor release, except disclosed urgent security removals. Do not promise this window for today's experimental peer protocol. A failed security/update gate blocks promotion even if adoption targets are met.

32. MEASUREMENT AND FEEDBACK

Telemetry remains off by default; local/private operation never requires analytics. Initial studies use voluntary exports and interviews. Proposed instrumentation belongs to X-14/X-16 and future work, not this task. Document event schema and deletion/export before collection: random per-study IDs, coarse platform/backend and durations/counts/error codes only, no prompt/output/model paths, peer addresses/identities, credentials or stable cross-study tracking. Aggregate only cohorts large enough to avoid singling out devices (target >=10); retain consented raw study data <=30 days, then anonymized aggregates <=12 months or shorter stated policy. Withdrawal removes identifiable records; selection bias and missing data are reported. Tests must prove opt-out sends no analytics traffic.

| Metric / denominator | Decision use / initial target | Owner |
|---|---|---|
| Discovery-to-install: consenting qualified visitors who attempt install / consenting qualified visitors | Compare channels, do not infer demand from stars/downloads; H6 study | Product |
| Activation: first real useful workload / all install attempts, with time and abandoned attempts | Section 31 thresholds; repair biggest failed step before acquiring more users | Desktop/release |
| Mesh activation: useful authorized peer request / users attempting a second device | >=80% in initial 10-pair study within 10 min after both installations; target pending study | Network/product |
| Retention: activated users or apps with useful execution in week 4 / week-0 activated cohort | H6 threshold; separate users, apps and scripted test traffic | Product |
| Reliability: successful eligible requests / all eligible requests; crash-free sessions / all sessions | Include timeouts/retries/restarts; separate correct policy refusals and infrastructure failures | Runtime/SRE |
| Developer adoption: separately maintained apps with a reproducible live integration | U-04; ecosystem maturity >=3 retained for 90 days; count ownership, not forks | Devex |
| Contribution: available compatible authorized capacity-hours and accepted useful jobs / offered capacity-hours | M7+; powered-on devices alone are not useful supply | Metering |
| Utilization and efficiency: useful compute time / allocated time, cost per accepted job, bytes/token, energy/job | Choose placement using matched cold/warm baseline, including failed work | Scheduler |
| Economics: net contribution margin and measured acquisition labor per retained buyer | Section 29 sensitivity; no projection from free beta to revenue | Product/operations |
| Community: independent reviewed contributions, review latency and active maintainers | Target two maintainers, median first response <=7 days during beta; disclose capacity | Maintainers |

Review feedback weekly during an active cohort, after each release/fault/security incident, and before changing a launch target. Record hypothesis, cohort, evidence, decision and next experiment in research/PROGRESS; do not lower targets retroactively to call a failed cohort successful.

33. VERIFICATION PROGRAM AND CONTINUATION

33.1 Comparisons

Use real models/hardware and pinned versions. Compare local inference with direct pinned llama.cpp and appropriate local runners on identical model/hash/quant/context/prompt/seed/settings. Compare serving/batch workloads with appropriate vLLM/SGLang/Ray configurations only where supported; match output quality, batching and hardware, not just API names. Compare trusted remote with manual authorized peer selection. Compare pooling with best feasible single device and measured manual split; a model that cannot fit locally has a capacity result, not an infinite speedup. Exo/Petals comparisons need comparable models, precision, hardware and trust conditions. Record non-comparable cases explicitly.

For each configuration record at least 10 timed runs after separate cold-load runs; publish all failures, median/p95 and confidence intervals with raw data, environment, exact command, model/runtime/license, commit and hardware manifests. Measure install friction, load time, TTFT, prefill/decode, correctness, peak RSS/VRAM (shared unified memory counted once), CPU/GPU utilization where available, bytes/token and transfer overhead, recovery/cancel, useful utilization and power with instrument/method or unavailable. Small-model numbers cannot predict large-model behavior. Installation and developer effort are independent-user studies, not inferred from kernel benchmarks.

Scheduler testing uses a held-out grid of models, device pairs and network conditions, including a weak peer that should be rejected. Report the best feasible baseline and every regression; retain existing F-04 acceptance thresholds. Security evaluation includes local-only negative capture, unauthorized/revoked peers, malformed data, exhausted resources, default settings, backend crashes and update tampering. Full cross-feature matrix remains section 22. Stage recovery must show what happened to in-flight output and KV state, not just a reconnected socket.

33.2 Evidence and execution discipline

Committed historical evidence under docs/evidence/, docs/VERIFICATION.md and benchmarks/results/ remains unchanged unless a dated correction is necessary. New runs include exact artifact hashes, code commit, OS/GPU/driver, fixture provenance, command, raw-result location, sample counts and limitations. Label SINGLE-HOST, simulated network and synthetic fixtures explicitly. Preserve originals' VERIFIED semantics; new documentation does not re-verify implementation. A missing Linux host, signing key, external tester or independent reviewer is an explicit blocker, not a reason to fabricate a pass.

Before each next work package: read AGENTS, this program, linked ledger rows and current blockers; validate the registry; inspect Git and relevant code; confirm required hardware/resources; implement/tests; record evidence; regenerate ledger; commit locally when checks pass. WP-01 continues from the af3e37c evidence and apps/desktop/README.md Packaging on the Windows host. Do not assume a historical SSH address or host is currently reachable. Independent agents may implement WP-02 or prepare WP-06 without blocking on WP-01.

Review this program from systems, security, product, open-source maintainability, economics, competition and requirements perspectives. Material findings and exact validation commands belong in docs/VERIFICATION.md. Do not treat reviews as product verification. Public artifacts, commercial pilots and public networking remain authorization boundaries, even when documentation and local code are ready.
