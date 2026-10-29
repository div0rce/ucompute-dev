# Research-Frontier Feasibility Reviews

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

> Imported from the M0 Research Phase Report (2026-10-13). Research baseline, not independently verified. Markers: ✅ primary-confirmed, ⚠️ secondary/partial, `unknown/unverified`. Determinations are **provisional**: per spec section 21, each requires a literature review, an executed experimental protocol, and an honest feasibility determination. Only the literature step has been done here, partially.

## (a) F-06-12 — Cryptographic verification of arbitrary inference

### Literature
- **zkLLM** (Sun, Li & Zhang, University of Waterloo; CCS 2024; arXiv 2404.16109): for 13B-parameter LLMs, a correctness proof for the entire inference process in under 15 minutes; proofs under 200 kB; verification 1–3 s. Secondary breakdown: 986 s commitment + 803 s proving per forward pass on LLaMA-2-13B ⚠️. Another paper notes the 2,048-token LLaMA-2-13B proof takes ~15 min vs < 0.1 s/token for plain inference.
- **zkGPT:** proves GPT-2 inference in ~25 s.
- **Soundness caveat** ("Hollow-LLM Attack", arXiv 2607.28884, 2026): a ZK proof of inference proves *some* committed weights produce the output; it does not prove the weights are the trained model or that the claimed computation occurred. Computationally trivial weights can satisfy the relation.
- **TOPLOC** (Ong et al., Prime Intellect; arXiv 2501.16007): locality-sensitive hashing of top-k last-hidden-state values. Claims detection of model, prompt or precision changes with 100% accuracy in their empirical evaluations; 258 bytes per 32 new tokens vs 262 KB for raw embeddings (Llama 3.1-8B-Instruct); validation up to 100× faster than generation.
  - Production: validated all decentralized rollout workers in INTELLECT-2 (32B RL run). TOPLOC v2 extends to pipeline-parallel inference and sampling proofs via reproducible Gumbel noise.
  - Critique (EZKL's Dante Camuto): guarantees are mostly empirical; a motivated malicious prover could alter intermediate activations.
- **opML / optimistic fraud proofs:** not researched in depth (unknown/unverified).
- **Redundancy:** see (d).

**Largest model proven:** 13B (zkLLM inference). zkLLMPoT (arXiv 2610.08258) proves covered operators of a training-attribution relation at 13B in 131 s — not full inference proving.

### Candidate protocol
1. TOPLOC-style commitments on every pooled request.
2. Randomly selected verifier peer re-runs prefill on a sampled fraction *p* of requests.
3. Measure detection rate against injected faults (swapped quantization, truncated layers, altered prompt) and false-positive rate across GPU, Metal and CPU backends.

### Provisional determination
- **ZK proofs for multi-billion-parameter LLMs on consumer hardware: not feasible** (minutes per forward pass on datacenter GPUs; limited soundness).
- **Activation-hash spot-checking: feasible to prototype.** Unknown: cross-backend robustness — TOPLOC was tested across GPU types and kernels, not across GGML CPU/Metal/Vulkan quantized kernels (unknown/unverified).
- Must never be described as cryptographic proof.

## (b) F-06-11 — Confidential-computing backends

### Measured overhead (NVIDIA datacenter parts)
- Zhu, Yin, Deng et al. (Phala Network, Fudan, io.net), arXiv 2409.03992: penalty mainly from data transfer; for most typical LLM queries overhead < 7%, near zero for larger models and longer sequences; H200 shows more overhead than H100; smallest model (Llama-3.1-8B) highest overhead.
- arXiv 2509.18886: CPU TEEs (Intel TDX/SGX with AMX) under 10% throughput and 20% latency overheads; H100 CC throughput penalties 4–8%, diminishing with batch/input size.
- arXiv 2607.19353 (H100 under Intel TDX, Mistral-7B, Qwen3-30B-A3B): consistent latency penalty, stable throughput gap, earlier saturation for the larger model. Secondary summary: ~21–28% TTFT overhead, 17–21% throughput loss ⚠️.
- arXiv 2505.16501: with model swapping, slowdown concentrates in model loading (encrypted CPU↔GPU I/O).

### Attestation flow
CPU TEE (TDX or SEV-SNP) attests the CVM; GPU attests CC mode; relying party verifies both before releasing prompts. EnclaveX (arXiv 2606.31408) adds no inference overhead over the CVM baseline beyond attestation.

### Availability
No GPU CC mode on consumer GeForce or Apple hardware as far as the research found. ARM CCA consumer availability and Apple Private Cloud Compute design documents not re-fetched (unknown/unverified).

### Provisional determination
Feasible only as an enterprise-federation backend on rented H100/H200/Blackwell CC instances. Not feasible for consumer LAN pooling.

### Protocol
UCompute worker in a TDX CVM with an H100 in CC mode; TTFT and decode with CC on vs off at batch 1/8/32; verify attestation end to end. **Blocked:** no such hardware; any rental requires explicit cost approval (spec section 4).

## (c) Activation and prompt privacy in pipeline-parallel inference

### Attacks
- **Prompt Inversion Attack** (Qu et al., NUS, UC Berkeley, NTU; arXiv 2503.09022): malicious downstream participant recovers prompts from transmitted activations — 88.4% token accuracy on Skytrax with Llama-65B when inverting the maximum number of layers, vs 22.8% for the best baseline.
- Related publication: a 4,112-token medical prompt nearly perfectly inverted (86.88 F1 token matching) from the middle layer of Llama-3 ⚠️ (attribution via secondary listing).
- Split-learning inversion (arXiv 2501.05965): representations closer to input or output layers are more susceptible.
- Fission (IACR ePrint 2025/653): attacks on split inference and PermLLM extract input information from intermediate activations.

### Mitigations and costs
Information-theoretic adapters (arXiv 2606.11592) trade utility for leakage; differential-privacy noise degrades quality; confidential computing see (b); MPC/HE orders of magnitude slower (unquantified).

### Determination (design rule)
**Treat activations as plaintext prompts.** Pipeline pooling only among authenticated, user-trusted devices. Any public-swarm mode must display an explicit warning that peers can reconstruct the prompt, enforced by the policy engine (F-06-07).

## (d) F-07-08 / X-05 — Sybil resistance and reputation

- **BOINC:** credit only for validated results via replication with quorum; homogeneous redundancy sends a job only to numerically equivalent hosts for bitwise comparison; adaptive replication drives overhead toward one for trusted hosts; credit is the average or minimum of claimed credits.
- **Gaming observed:** CreditNew design admits host normalization is vulnerable to cherry-picking; Einstein@Home found the Whetstone benchmark vulnerable to collusion-based cheating; Bitcoin Utopia showed single-unit credit breaks under ASICs — fewer than 1,000 users were granted more credit than 300,000 users of all other projects combined.
- **Petals:** "bloom points" incentive on roadmap; completion unknown/unverified.
- **Bittensor, Gensyn:** not researched (unknown/unverified).
- **Lessons:** per-(host, backend) reputation with Beta-distribution trust updated by spot checks; adaptive replication rates; identity cost (pairing or attestation) before any public swarm; never denominate rewards in a single gameable unit.

## (e) F-07-11 / F-07-12 — Paid settlements and tokenized incentives (US)

**This is not legal advice.** General background, not verified against primary legal sources.

- Money transmission: paying contributors on behalf of users can implicate FinCEN money-services-business rules and state money-transmitter licensing.
- Tokens may be analyzed as securities under the Howey test.
- Contributor payouts trigger IRS information reporting (e.g., W-9/W-8BEN, 1099-series) and OFAC sanctions screening.

**Required before any real-money operation:** qualified fintech/securities counsel; money-transmission analysis or a licensed payment partner; tax-reporting design; KYC/AML and sanctions procedures; written legal opinion on any token.

**Recommendation:** keep any swarm non-monetary through at least M9. Real-money settlement stays gated (spec section 4).
