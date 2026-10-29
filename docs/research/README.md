# M0 Research Baseline — Provenance

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

**Status of this material: research baseline, not independently verified truth.**

| Field | Value |
|---|---|
| Origin | "UCompute — M0 Research Phase Report", produced 2026-10-13 by a Claude research agent in the claude.ai chat that received the handoff spec |
| Access date for all cited sources | 2026-10-13 |
| Imported into repo | 2026-10-14 |
| Re-verification performed on import | None. Content was transcribed and reorganized only. |

## Uncertainty markers (preserved verbatim from the report)

- ✅ confirmed on a primary page (repository, release page, PyPI, crates.io, arXiv) during the research pass
- ⚠️ secondary-source or partially confirmed
- `unknown/unverified` no primary evidence obtained

All benchmark figures are as published by their authors. **None were replicated by UCompute.** Nothing in these files claims UCompute is superior to any project (spec section 5).

## Files

| File | Report part |
|---|---|
| `COMPETITIVE_MATRIX.md` | Part 1 — competitive matrix (spec section 5) |
| `RESEARCH_GAPS.md` | Part 2 — gaps, falsifiable hypotheses H1–H5, partitioning physics |
| `FRONTIER_FEASIBILITY.md` | Part 3 — F-06-11, F-06-12, activation privacy, Sybil/reputation, settlement compliance |
| `TEST_MODELS.md` | Part 5 — tiny GGUF models for CI |
| `../decisions/ADR-0001-stack-and-license.md` | Part 4 — dependency/license verification and license recommendation |
| `SOURCES.md` | Source URL list from the report |

## Known gaps (from the report's caveats)

- Several licenses were not re-read from LICENSE files; verify before shipping a NOTICE file.
- Some release dates come from aggregators (releasealert.dev, releases.sh, mise), not GitHub directly.
- Several 2026 arXiv papers and news items are cited as found. Vendor-reported performance claims (Exo speedups and RDMA latency, iroh hole-punch rate, TOPLOC accuracy) are unreplicated.
- Not researched with primary sources: Bittensor, Gensyn, opML, ARM CCA, Apple PCC, US legal specifics.
- The report's source list was truncated in the research tool output; `SOURCES.md` lists what was returned.

## Rule for future agents

Before relying on any ⚠️ or `unknown/unverified` item for a decision, re-verify it against the primary source and record the result (date, URL, finding) in the relevant file. Do not silently upgrade a marker.

## Subsequent research

The dated 2026-10-18 sections in COMPETITIVE_MATRIX.md, SOURCES.md and RESEARCH_GAPS.md are a new primary-page refresh and strategic experiments, not part of the imported M0 report. Conflicting present-tense interpretations defer to that dated refresh; historical markers remain intact. None of the refreshed competitors was benchmarked. Audience, distribution-loop and revenue claims remain hypotheses H6–H9, linked to the execution program in [the existing specification](../SPEC_HANDOFF_V2.md), sections 26–33.
