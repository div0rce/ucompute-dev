# Burst failures on Linux: cause and fix (2026-10-31) — SINGLE-HOST, sentinel

Follow-up to the second 24 h Linux soak (`linux-soak-2026-10-29.md`, FAIL as registered). That soak showed two things on
a live engine: 24,562 of 4,771,369 admitted burst requests (0.51 %) ending with the typed `backend_failed`, and two
admitted requests with no answer within the driver's 120 s. This record is an investigate-then-verify workflow
(`soak-burst-failures-linux`) plus engineering's own checks. Every run is SINGLE-HOST on sentinel (Ubuntu 26.04 x86_64,
i9-9900K, llama.cpp b11609 on the CPU, stories15M, `-c 128 -np 4`, the soak's request body with `max_tokens` 64,
bursts of 40 concurrent `POST /v1/completions`), in its own folders, away from P1's tree, homes and ports.

## The frequent failure: determined

**Cause:** the daemon's HTTP client to llama-server kept idle loopback connections and reused them; sometimes the
engine had already closed one, and the request sent on it failed with `client error (SendRequest): connection closed
before message completed`, which the adapter can only report as `backend_failed`. The cause text appeared once the
adapter printed the error's source chain (`chain()`, part of the change). Why llama-server closes those connections
(cpp-httplib keep-alive handling) is not determined; the fix avoids reuse instead of explaining it.

| Run | Build | Length | Admitted | `backend_failed` | Rate | Cause of each | Untyped / hangs | Slowest admitted |
|---|---|---|---|---|---|---|---|---|
| Investigator, before | 6cd9135 + diagnostics | 8 min, 689 bursts | 24,804 | 139 | 0.560 % | 139 × "connection closed before message completed" | 0 / 0 | 699 ms |
| Investigator, after | + the fix | 4 min, 479 bursts | 17,244 | 0 | 0 % | none | 0 / 0 | 558 ms |
| Verifier, without the fix (own copy, own driver `scripts/vburst.py`) | fixes reverted, diagnostics kept | 6 min, 703 bursts | 25,308 | 133 | 0.526 % | 133 × the same | 0 / 0 | 704 ms |
| Verifier, with the fix | the investigator's file | 6 min, 718 bursts | 25,848 | 0 | 0 % | none | 0 / 0 | 556 ms |

At the before rate about 97 and 136 failures were expected in the two "with the fix" runs; 0 is far outside chance.
Every `ok` completion was golden on both builds. llama-server alone (`direct.jsonl.gz`, started by hand with the
daemon's arguments and a fresh connection per request) answered 6,560 of 6,560 with 200: it queues the 36 requests
beyond its 4 slots itself and refuses none. The verifier sampled about 4,040 `TIME_WAIT` sockets on the engine port in
the run with reuse.

The soak recorded per-code counts only, no causes, so the link between this mechanism and the soak's 0.51 % rests on the
same code, rate and burst shape: inferred, not shown.

**Fix** (`crates/ucompute-runtime/src/llamacpp.rs`): the engine client is built with `pool_max_idle_per_host(0)`, so it
never reuses an idle connection; a fresh loopback connection costs well under 1 ms. Not measured: the extra connection
churn (`TIME_WAIT`, ephemeral ports) on macOS and Windows at soak rates; only Linux was run.

## The rare no-answer: not reproduced, one bound added

0 hangs in about 42,000 admitted requests in the investigator's runs and none in the verifier's. Reading the code found
one wait no timer covered: the adapter's request to llama-server (connect, write, response headers) ran before the
first-token timer was armed, and the client had no timeout of its own. The change bounds it: the first-token clock now
runs from submit (`tokio::time::timeout(req.timeouts.first_token, self.post(...))`, and the stream's first wait gets what
is left), so a request whose headers never arrive ends with the typed `timeout`. The verifier measured that b11609 sends
its headers 3–8 ms after submit, before prompt processing (`verify-hdr.out`), so before the change the first-token timer
already covered nearly the whole wait and this gap is narrow. The verifier's better-fitting reading of the two soak cases:
the driver's 120 s clock starts before admission (which may queue up to 30 s) and the daemon's first-token clock after
it, so the client always gives up before a typed 120 s `timeout` could arrive. Neither reading is established. The
next soak must give the client a timeout well above the daemon's first-token timeout, or lower that timeout for the run.

## Tests

- New `bursts_answer_within_the_first_token_bound_single_host` (`crates/ucompute-api/tests/real_engine.rs`, the real
  engine): 40 rounds of 40-request bursts against a daemon with a 2 s first-token timeout (the new `ApiConfig.timeouts`,
  serde default `Timeouts::default()`, so production defaults are unchanged): every request answered, every non-2xx
  typed, every admitted request within queue timeout + first token + 3 s, and no admitted `backend_failed`. With the
  fix on sentinel: 1,440 ok, 160 overloaded, slowest admitted 573 ms. Without the connection fix (engineering's mutation
  check, `mutation-check-engineering.out`, the file restored to the same hash afterwards): one run failed with 2
  `backend_failed`, one passed with 0; the verifier's run had 1. So the test guards the fix in about two runs of three
  at 40 rounds, as its doc comment says.
- `stalled_engine_is_a_typed_timeout_within_bound` (unit, FAKE engine) gained a case: a listener that accepts and never
  writes. Without the header bound it fails (`the wait for the engine's answer is bounded: Elapsed(())`, verifier); with
  it, a typed `timeout` within 1.5 s.
- On sentinel at the final tree: `cargo fmt --all --check`, `cargo clippy -p ucompute-runtime -p ucompute-api
  --all-targets -D warnings` clean; runtime lib 76/76; `llamacpp_real` 8/8 (engine-exit recovery included);
  `real_engine` 12/12; `api` 18/18 (after correcting engineering's own sync marker to a full commit hash; a short hash
  failed `status_ready_models_and_admin_validation`); `jobs` 3/3; api lib 62/62. D-026's `local_priority` tests 8/8
  (verifier). Not run: the F-06-04 flood timing tests (known to fail on single hosts), the split and mesh suites
  (untouched crates), any Mac or Windows run.

## Open

A new 24 h soak with this build and a client timeout above the daemon's first-token timeout is what milestone C now needs.
The other open items: `post_json` (tokenize, detokenize, embeddings) still has no timer; the `ensure_loaded` lock is
bounded only by whoever holds it (a load is capped at 300 s); connection churn on macOS and Windows.
