# Third 24 h local soak on Linux (sentinel), 2026-10-31/11-01 — SINGLE-HOST: RUNNING (started 20:02 UTC 2026-10-31)

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
