# Second 24 h local soak on Linux (sentinel), 2026-10-29/30 — SINGLE-HOST: RUNNING, interference documented

Same build and setup as `linux-soak-2026-10-24.md` (`target/release/common` at 0b0c1a4 in `~/src/common`, home
`~/.common-soak`, port 18500, seed 20261023, a kill every 50 cycles, 40-request bursts), with the driver recording every
untyped result's detail (c26c0a1). Started 07:42:38 UTC 2026-10-29; ends about 07:43 UTC 2026-10-30. Bounds unchanged
(`tests/soak/local_soak.py` `verdict`): zero untyped errors; every golden completion golden; every engine kill golden
again within 30 s; no orphaned engine; RSS growth after the first hour <= 64 MiB; open-files growth <= 16.

## Interference (recorded 2026-10-29, before any verdict)

The Core Lab flagged it (read-only check at 17:11 UTC); the times below come from the workflow agents' own command
logs, sentinel's file times and the soak's one-minute rows.

| Window (UTC) | What ran on sentinel besides the soak |
|---|---|
| 16:56:50–17:04:27 | A rename workflow superseded by D-028 (`wf_891f06f6-84e`, believed stopped, still running) built and ran the daemon crates' tests (`cargo test -j 8`, with test engines) in `~/src/ucompute-rename-step1`; the D-028 run killed it at 17:04:27. The superseded run was stopped at about 17:16 |
| 17:06:49–17:13:39 | The D-028 rename run's step-1 Linux tests (`cargo test -j 6`, with test engines) in the same folder; at nice 19 and idle I/O from 17:12:00 |
| 17:12– (until the soak ends) | `~/soak/interference-watch.sh`: every 5 s it puts any `ucompute-rename`/`ucompute-fix` process tree at nice 19 and idle I/O and logs `/proc/loadavg` and the count of such processes to `~/soak/interference.log`, so later windows are measured |
| Whole run | A desktop session with Firefox, present since before the soak started (constant baseline) |

What the soak's own rows show (569 rows, 07:42–17:14): before 16:58, golden p95 median 16.3 ms and every kill
recovered within 1.03 s; a stretch from 10:40 to 12:38 at 77–79 cycles a minute (median 87) with p95 up to 18.7 ms,
cause unknown, no rename work then. In the two windows: p95 25.8–43.2 ms (the first soak's maximum without known
interference was 39.1 ms), cycles 60–82 a minute, kill recoveries 9.70 s (16:58) and 5.63 s (17:00), both under the
30 s bound (the targeted overload run's slowest was 8.2 s). Untyped errors 0, orphans 0, RSS 38.0 MiB, open files 11.

## Reading rule (fixed now, before the verdict)

Per bound:
- orphans, RSS growth, open-files growth and golden correctness are scoped to the soak's own daemon and home, so
  the windows do not affect them;
- kill recovery within 30 s held under extra load, which is at least as strong as the registered condition but is
  not that condition: reported as "held, with documented interference";
- zero untyped errors (the bound milestone C turns on):
  - any untyped error inside a documented window or within 5 minutes of one makes the run **inconclusive** for
    milestone C: rerun, or a targeted reproduction;
  - an untyped error outside every window counts as a FAIL, as registered;
  - zero untyped errors overall reads **"PASS with documented interference"**, never a clean PASS, with each bound
    annotated as above.

Further rules: nothing new is started on sentinel until the soak ends (the WP-04 fix pass does not use it); the D-028
rename run's final check may still run its Linux tests there, niced by the watcher, and any such window is added to
the table above from `~/soak/interference.log` at collection; future
soaks record `/proc/loadavg` in every sample; P1's preflight requires `who` to be empty, so the desktop session must
end (or the check be decided) before P1.
