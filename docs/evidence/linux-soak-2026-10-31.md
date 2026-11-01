# Third 24 h local soak on Linux (sentinel), 2026-10-31/11-01 — SINGLE-HOST: PASS with documented interference (d3b178b, 960 s client timeout, 20:13:41 UTC 2026-10-31 to 20:19 UTC 2026-11-01)

Registered before the start; nothing below changes once samples exist.

**Purpose.** Milestone C's 24 h soak on the build that fixes the frequent burst failure
(`burst-failures-linux-2026-10-31.md`: the engine client no longer reuses idle connections; the first-token clock runs
from submit).

**Same as the second soak** (`linux-soak-2026-10-29.md`): `tests/soak/run_soak_linux.sh start 24`, the daemon built in
`~/src/ucompute` (now the renamed tree, migrated per D-029), home `~/.ucompute-soak` linking the acceptance home's
engine and models, port 18500, no peer endpoint, seed 20261023, an engine SIGKILL every 50 cycles, 40-request bursts,
one-minute samples with `/proc/loadavg`. Bounds unchanged (`tests/soak/local_soak.py` `verdict`): zero untyped errors;
every golden completion golden; every engine kill golden again within 30 s; no orphaned engine; RSS growth after the
first hour ≤ 64 MiB; open-files growth ≤ 16.

**One change, the client's patience.** Every request now waits up to 180 s (`--request-timeout`, default 180; it was
120 s). That is above the daemon's first-token timeout (120 s) plus its admission queue (30 s), so a daemon that gives
up answers with a typed `timeout` before the client gives up. An `untyped:no_response` in this soak is therefore the
daemon's, not a race between two equal clocks (the ambiguity of the second soak's two no-responses). The kill-recovery
poll keeps its own 30 s per request, and the cached pull its 60 s.

**P1 has priority.** WP-04 P1 needs sentinel quiet. If this Mac becomes ready for P1 during the soak (AC power, memory,
load; the watcher's `p1-ready` flag), the soak is stopped and collected at once, and this record reads **INCOMPLETE
(preempted by P1)**, with no verdict on the bounds; a new 24 h soak follows P1.

**Reading rule.** As registered for the second soak: an untyped error outside every documented interference window is
a FAIL; any foreign work on sentinel during the run is listed with its window before the verdict.

**Start (20:02 UTC 2026-10-31).** `run_soak_linux.sh start 24`: daemon pid 231422 at d3b178b (`/health` commit
`d3b178b2…`, version 0.1.1), the driver with `REQUEST_TIMEOUT = 180`. Just before it, `split_single_host` passed on
sentinel at d3b178b (9 passed, 1 ignored, no SKIP), which also covers P1's split path through the changed engine
client (the Core Lab's pre-P1 condition). A watcher on this Mac collects the soak as soon as P1's `p1-ready` flag appears.

**Amendment before the restart (20:13 UTC, from the Core Lab's read-only reading; no verdict existed).** 180 s does
not clear every daemon bound on the non-streaming steps: queue 30 s + first token 120 s + one inter-token gap 30 s is
exactly 180 s, and a request that arrives during a reload after an engine kill waits for the load, bounded only by
the 300 s load timeout. A no-response at 180 s would again be ambiguous. The client timeout is therefore **960 s**
(queue 30 + load 300 + total 600 + 30 s margin; `REQUEST_TIMEOUT`, the driver's new default), and an untyped
no-response in this soak means the daemon exceeded every bound it applies to a request. Bounds, seed and everything
else above unchanged. The 20:02 start was stopped and collected after 10 minutes (602 s, 974 cycles, every step ok,
0 `backend_failed` against the second soak's 4–98 a minute, 0 untyped, 2 kills recovered in 0.69 s; raw files in
`linux-soak-2026-10-31/aborted-start-180s/`); it is not part of the verdict.

**Interference, recorded before any verdict (13:45 UTC 2026-11-01, at 17.5 h).** From **05:21:32 UTC 2026-11-01** a local
desktop login on sentinel's seat0 (logind session 329, user `div0rce`, Wayland, Firefox and the release-upgrade check
running; the owner's own use of the machine), constant from then on. The soak drives sentinel's 1-minute load to about
7–8 by itself (median 7.71, max 14.98 so far). At 17.5 h, before any reading: 98,622 cycles, 0 untyped, 0
`backend_failed`, 1,972 kills all recovered (slowest 0.99 s), RSS 34.5 → 43.7 MiB after the first hour, open files 11,
no orphan. Under the reading rule an untyped error inside or within 5 minutes of this window would make the run
inconclusive for milestone C, not a FAIL.

## Result (collected 20:20 UTC 2026-11-01, `run_soak_linux.sh collect`)

Driver verdict (`soak.out`): **PASS**, no failed bound; 130,362 cycles in 24.0 h, seed 20261023. Raw rows
`linux-soak-2026-10-31/soak.jsonl.gz` (1,432 one-minute samples, 20:14:41Z to 20:13:42Z), the daemon's log
`daemon.log.gz` (2,607 warnings, one "engine exited unexpectedly" per kill; no error line), `version.txt` (0.1.1),
`cleanup.txt` (no soak engine left).

| Bound | Result |
|---|---|
| Zero untyped errors | **0** (client timeout 960 s, above every daemon bound, so a hang would have read untyped) |
| Every golden completion golden | 130,362 / 130,362 |
| Every engine kill golden again within 30 s | 2,607 kills, slowest 0.99 s, none above 2 s |
| No orphaned engine | 0 in every sample; at most 1 engine |
| RSS growth after the first hour ≤ 64 MiB | 10.9 MiB, as the driver measures it (`verdict`: the first sample at or after 3,600 s, 33.3 MiB at 3,615 s, to the last, 44.2 MiB, which is also the maximum) |
| Open-files growth ≤ 16 | 0 (11 throughout) |

Typed results: 4,693,032 `ok` and 521,448 `overloaded` (4.00 per burst of 40, the admission bound) in the overload
step, **0 `backend_failed` in 4,693,032 admitted burst requests** (the second soak: 24,562 in 4,771,369, before the
engine-client fix of 578999f); every `golden`, `stream`, `cancel`, `chat` and `pull` step ok (130,362 each). Golden
p95 per minute: median 14.9 ms, max 20.4 ms.

**Reading** (the rule registered for the second soak): zero untyped errors overall, with one documented interference
window (the owner's desktop login from 05:21:32 UTC, 887 of the 1,432 samples; golden p95 median 14.1 ms before it,
16.6 ms during it; 1-minute load median 7.59 before, 8.18 during), reads **PASS with documented interference**. The
interference added load and never removed any; every bound is scoped to the soak's own daemon and home. This is the
24 h soak half of milestone C; the renamed Linux tarball's install check is the other half.
