# Tiny GGUF Models for CI

> Imported from the M0 Research Phase Report (2026-10-13). Research baseline, not independently verified. Markers: ✅ primary-confirmed, ⚠️ secondary/partial, `unknown/unverified`.

| Model | Hosting | Size | Layers | Context | License |
|---|---|---|---|---|---|
| stories260K | huggingface.co/ggml-org/models (`tinyllamas/stories260K.gguf`) ✅ | ~1 MB ✅; used in llama.cpp CI ✅ | 5 (llama2.c README, recalled; not re-verified) | small | llama2.c MIT (unverified); TinyStories dataset license unverified |
| stories15M | ggml-org tinyllamas / `ggml-org/tiny-llamas` (Q4_0 available) ✅ | tens of MB (unverified) | 6 (recalled; not re-verified) | 128 tokens ✅ (`llama.context_length = 128`) | as above |
| stories42M / stories110M | llama2.c lineage | larger | 8 / 12 (recalled; not re-verified) | — | as above |

## Assessment
- **Layer partitioning:** ≥5 layers suffice to split 2+3 (or 3+3) across two processes and exercise boundary activation transfer.
- **Correctness:** stories260K in F32 suits bit-exact or tight-tolerance comparison between local and partitioned execution on the same backend. Across backends use tolerance-based logit comparison (max-abs and KL thresholds), not bitwise equality.
- **Context-length trap:** stories15M's 128-token context breaks any prompt longer than 128 tokens. One downstream project's smoke test "passed" on an error fallback for this reason. **CI must assert real inference occurred** (non-empty output, no error path, token count > 0).

## Recommendation
- Per-commit split-equivalence: stories260K F32.
- Quantized path: stories15M Q4_0.
- Nightly: one real small model (e.g., Qwen2.5-0.5B GGUF) for GQA and real tokenizers.

## To verify on import to the Mac
- [ ] Layer counts via GGUF metadata (`<arch>.block_count`) — record actual values here.
- [ ] SHA-256 of each fixture pinned in `tests/fixtures/MANIFEST`.
- [ ] Licenses of llama2.c weights and TinyStories dataset from primary sources.
