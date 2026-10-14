# Verification

How evidence is recorded, and the current state of each quality gate (spec section 11).

## Evidence entry format
```
### <REQ-ID or GATE> — <short title>
- Date / commit: YYYY-MM-DD / <git sha>
- Host: <hostname> (<chip>, <memory>, <OS>)   label: REAL | SINGLE-HOST | SIMULATED
- Command: <exact command>
- Result: PASS | FAIL (+ numbers)
- Artifacts: <paths under benchmarks/results/ or tests/ output>
```
A row in `docs/REQUIREMENTS.csv` may be marked `VERIFIED` only when its `observed_evidence` points to an entry here covering every acceptance criterion.

## Quality gates
| Gate | Scope | State | Evidence |
|---|---|---|---|
| A — Build | clean build, lint, fmt, dependency/license scan, secrets check | Not assessed | — |
| B — Inference | real load, generation, streaming, cancellation, cleanup | Not assessed in this ledger (existing Mac repo reports partial; B-009) | — |
| C — Network | discovery, auth, revocation, disconnects, malformed messages | Not started | — |
| D — Pooling | genuine two-node partitioned execution, correctness, memory, failure semantics | Not started; real verification blocked (B-002) | — |
| E — Product | install, select, run, pair, pool, diagnose without developer | Not started | — |
| F — Public swarm | adversarial review, accounting, privacy, abuse, churn | Not started; disabled by default | — |
| G — Release | signed/checksummed, reproducible, compat matrix, CI, docs, benchmarks | Not started (B-007, B-008) | — |

## Evidence log

### REGISTRY — requirements ledger integrity
- Date: 2026-10-14 · commit: n/a (import kit, not yet in repo)
- Host: Claude cloud workspace (Linux x86_64) · label: REAL (tooling only; no product code)
- Command: `python3 scripts/validate_registry.py && python3 -m unittest discover -s tests/registry -v`
- Result: PASS — 148 P0 rows; NOT_STARTED=143, IN_PROGRESS=3, IMPLEMENTED_UNVERIFIED=2, others 0. Negative tests confirm the validator rejects unevidenced VERIFIED, missing rows, unknown statuses, cycles and stale generated files.
- Scope: proves ledger structure only. Proves nothing about the product.

### PHASE-1 — environment inspector
- Date: 2026-10-14 · label: REAL (macOS branch executed on the owner's Mac this session; Linux branch earlier on a cloud workspace)
- Host: owner's MacBook Air (Apple M2, 8 GB unified, 10-core GPU, Metal 4, macOS 26.2 build 25C5048a, arm64)
- Command: `scripts/inspect-env.sh`
- Result: PASS — report in `docs/environment/ENVIRONMENT-*.md`. Toolchain: rustc/cargo 1.98.0, Python 3.14.8, Node 26.10, uv, Homebrew 7.0.8, cmake, ninja, Xcode clang. No preinstalled llama.cpp, Ollama, MLX. Disk 9.5 GiB free at start (→17 GiB after owner-authorized cache cleanup).

### RECOVERY-CODEX-M1 — recovered Python inference slice re-verified
- Date / commit: 2026-10-14 / Codex history `efdec54` (imported as `refs/recovered/codex-m1/feat/m1-macos-local-inference`)
- Host: owner's MacBook Air (above) · label: REAL
- Command: in `.recovery/codex-m1/` with the pinned runtime + model copied from the original checkout: `python3 -m unittest discover -s tests -v` then `COMMON_REAL_INFERENCE=1 python3 -m unittest discover -s tests -v`
- Result: PASS — 17/17 (13 fixture tests + 4 real: CPU generation, one-token generation, context-overflow rejection, Metal generation with 7/7 layers offloaded). Greedy continuation of "Once upon a time, a little rabbit" = " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big" on both CPU and Metal. Metal decode 455 tok/s on this tiny model (CPU is faster for it; not a benchmark).
- Scope: proves the recovered slice works on one physical Mac; it does not by itself satisfy any registry row (acceptance criteria require golden-token tests, streaming, fuzzing, fallback tests — being implemented in Rust, see M1 entries below).

### ENGINE-SMOKE — pinned `llama-server` b11609 on loopback (basis for ADR-0002)
- Date: 2026-10-14 · label: REAL
- Command: `llama-server -m stories15M-q4_0.gguf --host 127.0.0.1 --port 18081 -c 128 -ngl 99 --no-webui --metrics --offline -t 4`; `curl /health`, `/v1/models`, `/v1/completions` (temperature 0, seed 42, max_tokens 32), streaming variant, `/metrics`, `/props`
- Result: PASS — `/health` = `{"status":"ok"}`; completion text identical to the Codex golden output; usage prompt_tokens 10 / completion_tokens 32; timings prompt 868 tok/s, decode 442 tok/s; 18 SSE `data:` chunks for a 16-token stream; `/metrics` serves Prometheus text (`llamacpp:*`).

### F-09-01 (JS half) — official OpenAI JS client against `common serve`
- Date / commit: 2026-10-14 / working tree after wave 2 (API `common-api`, CLI `common-cli`)
- Host: owner's MacBook Air (Apple M2, 8 GB, macOS 26.2) · label: REAL
- Command: `common serve --port 18192` then `cd examples/clients/openai-js && COMMON_BASE_URL=http://127.0.0.1:18192/v1 COMMON_MODEL=stories15M-q4_0 node smoke.mjs` (openai npm 6.49.0, unmodified; only `baseURL` set)
- Result: PASS — `models.list` 8 models; `completions.create` (temperature 0, seed 42) returns the golden prefix " named Benny was hopping in a field. He "; `chat.completions.create` usage {prompt 16, completion 16}; streaming delivered 16 content chunks and a finish chunk; unknown model → HTTP 404 with code `model_not_found`.
- Python half: `crates/common-api/tests/openai_client_smoke.py` (openai 3.31.0) runs inside `cargo test -p common-api` (real_engine.rs) and printed "ALL CHECKS PASSED".

### F-09-03 — Prometheus exposition validated with promtool
- Date: 2026-10-14 · label: REAL
- Command: `curl -s http://127.0.0.1:18192/metrics > metrics.txt && promtool check metrics < metrics.txt` (promtool from Homebrew `prometheus`)
- Result: PASS — exit 0, no lint output; 42 lines incl. `common_requests_total{route,status}`, `common_tokens_generated_total`, `common_ttft_seconds` histogram, `common_queue_depth`, `common_inflight`; the scrape taken right after chat requests contains no prompt text (grep for the prompt string: 0 matches).

### DESKTOP-E2E — desktop UI against the real daemon (dev loop, browser)
- Date: 2026-10-14 · label: REAL (Vite dev server + `common serve`; not the packaged Tauri app)
- Host: owner's MacBook Air (Apple M2, 8 GB, macOS 26.2)
- Command: `COMMON_HOME=.common common serve` then `pnpm --dir apps/desktop dev`; opened `http://localhost:1420` in the Claude desktop built-in browser
- Result: PASS — Home shows "Common is running… Apple M2, 8.0 GB memory, metal" and the recommended model with its memory fit bar; Models lists all 8 catalog models with size, license (never blank) and fits / "Needs X, you have Y" from `/api/models`; Hardware view values equal `common hardware --json`; Chat: selected `smollm2-360m-instruct-q8_0`, Load model → "Talking to smollm2-360m-instruct-q8_0 on MTL0 (Apple M2), 4,096 token context", a question streamed a reply from the real engine (Metal). Observed defect: the reply footer showed "tokens unknown" (usage/timings not surfaced in the streaming path) — fixed in the follow-up commit referenced below.
- Note: `available` memory during this run was ~1.0 GB because several build agents and test engines were running; the 0.5B models were therefore reported as not fitting. Calibration of the estimate and the available-memory formula is tracked under F-01-03.
