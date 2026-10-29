# 24 h local soak on Linux (sentinel), 2026-10-24/25 — SINGLE-HOST: FAIL (1 untyped error in about 556,000 burst requests)

`tests/soak/run_soak_linux.sh start 24` on sentinel (Ubuntu 26.04 x86_64, i9-9900K) with `target/release/common`
built at 0b0c1a4 (the round 6 and 7 runtime), its own home `~/.common-soak` (engine and models linked from the
acceptance home), port 18500, no peer endpoint; `tests/soak/local_soak.py` with seed 20261023, an engine SIGKILL
every 50 cycles and 40-request overload bursts. 08:38 UTC 2026-10-24 to 08:37 UTC 2026-10-25, collected on 2026-10-29
(sentinel was off the network in between). Raw samples: `linux-soak-2026-10-24/soak.jsonl.gz` (1,431 one-minute rows).

| Bound (fixed before the run) | Result |
|---|---|
| Zero untyped errors | **FAIL: 1.** One overload-burst request got no HTTP response at all (`overload:untyped:no_response`) in the minute before 12:58:57 UTC on 2026-10-24. No engine kill fell in that minute, and the daemon log holds nothing but the regular kill warnings around it. The driver did not keep the exception text, so the cause is **not determined** |
| Every golden completion golden | PASS (132,032 cycles) |
| Engine killed: golden again within 30 s | PASS: 2,640 kills, all recovered, slowest 1.14 s |
| No orphaned engine at any sample | PASS (none in 1,431 samples; none left at collection) |
| RSS growth after the first hour <= 64 MiB | PASS: 4.8 MiB (36.6 to 41.4 MiB) |
| Open files growth after the first hour <= 16 | PASS: 11 to 11 (12 at most) |

Other counts: 528,128 burst requests refused `overloaded` (429) and 24,036 `backend_failed` while an engine was
being killed, both typed; golden-completion p95 median 14.4 ms, at most 39.1 ms.

Follow-up (this commit): the driver now records the time, step, status and exception text of every untyped result,
counts a burst request whose thread ends without a result, and catches `http.client.HTTPException` (it escaped
before); `--only overload` runs bursts alone. A targeted overload run on sentinel is next, to find the cause.
Milestone C stays open until a soak passes.
