# Second 24 h local soak on Linux (sentinel), 2026-10-29/30 — SINGLE-HOST: FAIL as registered (2 untyped no-response in the overload step), interference documented

> Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

Same build and setup as `linux-soak-2026-10-24.md` (`target/release/ucompute` at 0b0c1a4 in `~/src/ucompute`, home
`~/.ucompute-soak`, port 18500, seed 20261023, a kill every 50 cycles, 40-request bursts), with the driver recording every
untyped result's detail (c26c0a1). Started 07:42:38 UTC 2026-10-29; the driver ended 07:43 UTC 2026-10-30 (24.0 h); collected and the daemon stopped later that morning. Bounds unchanged
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
| 17:12:23–17:14:35, 17:20:57–17:34:49, 17:58:36–18:05:15, 18:13:03 (measured by the watcher, up to 24 processes, all at nice 19 and idle I/O) | The rename run's Linux test reruns and its final check's Linux build and tests in fresh `~/src/ucompute-rename-*` folders. Nothing else ran on sentinel after 18:13 (watcher log, checked at 23:08) |
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

## Result (collected 2026-10-30, `run_soak_collect_keep.sh collect`, the soak's home moved into the D-029 backup instead of deleted)

Driver verdict (`soak.out`): **FAIL: 2 untyped errors**; 132,538 cycles in 24.0 h, seed 20261023. Raw rows `soak.jsonl.gz` (1429 one-minute samples), the daemon's log `daemon.log.gz`, the watcher's `interference.log.gz`.

| Bound | Result | Reading |
|---|---|---|
| Zero untyped errors | **2** `overload:untyped:no_response` (no answer within the driver's 120 s; a typed `timeout` at 120 s plus milliseconds would read the same, see below), both 2026-10-30, 7.7 h and 11.0 h after the last interference window ended (18:13:03Z); nothing of ours ran on sentinel then except the engineering session's liveness poll (`ssh … kill -0` and `tail -1`, every 120 s) | **FAIL as registered** (reading rule: an untyped error outside every window counts) |
| Every golden completion golden | 132,538/132,538 ok | held |
| Every engine kill golden again within 30 s | 2,650 kills, slowest recovery 9.70 s, 2 above 5 s (both in the 16:58–17:00 window, as recorded above); all after 18:13: at most 0.95 s | held, with documented interference |
| No orphaned engine | 0 in every sample; `cleanup.txt`: no soak engine left | held |
| RSS growth after the first hour <= 64 MiB | 32.6 MiB at 1 h, 37.9 MiB at the end (max 37.9 MiB) | held |
| Open-files growth <= 16 | 11 at 1 h, 11 at the end (max 13) | held |

Golden p95 per minute: median 14.4 ms, max 67.6 ms (in the documented windows); after 18:13 max 15.8 ms.

### The two untyped errors

Both are in the overload step (40 concurrent `/v1/completions` requests with the driver's default 120 s timeout, `call()` line 81; the 30 s timeout is the kill-recovery poll's): the client got no response at all within 120 s. The sample windows holding them are 140 s and 138 s long with 32 and 29 cycles (96 a minute otherwise), the stall of one burst waiting out that timeout.

| Recorded at | Detail | Timing against the engine kills | `backend_failed` in that sample minute | Kills recovered in that minute (s) | Golden p95 (ms) |
|---|---|---|---|---|---|
| 2026-10-30T01:52:26Z | `TimeoutError('timed out')` (the driver's 120 s request timeout) | started about 01:50:26Z; the engine was alive throughout: exits (SIGKILL by the soak) logged at 01:49:57.478Z before and 01:52:28.709Z after, none between | 5 | none in the sample minute | 14.6 |
| 2026-10-30T05:15:55Z | `TimeoutError('timed out')` (the driver's 120 s request timeout) | started about 05:13:55Z, 11 s after the kill logged at 05:13:44.411Z (recovered in 0.69 s); the engine was alive throughout: next exit 05:16:15.709Z | 4 | [0.69] | 14.6 |

The daemon log holds no error or warning for those minutes beyond the expected "engine exited unexpectedly" lines (the soak's SIGKILL every 50 cycles, 31.6 s apart at the median).

**What the data supports** (engineering's reading, checked by the Core Lab against the committed files and re-checked here): in both cases the engine process was alive for the whole 120 s and kept serving the other steps (golden p95 14.6 ms in those windows); no kill fell inside the request's window, and the next scheduled kill came 2 s and 20 s after the client gave up, after which the cycle rate returned to 96 a minute. The hung request had been **admitted**: both windows show exactly 36.00 admitted and 4.00 `overloaded` per burst, like every other minute, and admission waits are typed and bounded, so the wait was after admission (`ensure_loaded` with the engine already up, or the engine request itself, whose first-token bound is 120 s). The `backend_failed` results are **not kill-related**: the daemon logs `in_flight=0` at every one of the 2,650 kills (the driver kills between cycles, after all 40 burst threads have joined), and no sample minute has fewer than 4 of them; they are a steady failure of admitted requests on a live engine, 24,562 of 4,771,369 admitted burst requests (0.51 %), tracking host load (correlation −0.71 with cycles per minute, +0.57 with the golden p95; 21.1 a minute before 18:13Z, 14.2 after; the six worst minutes, 61–98, all in the 17:04–17:33Z interference window). That is a frequent, reproducible engine-under-burst failure of the same family as the two hangs: a reproduction that records the engine-side status behind each `backend_failed` sees hundreds in an hour, against 2 hangs in 5.3 M. The daemon's own first-token timeout is 120 s (`ucompute-core` `Timeouts::default`), equal to the driver's patience, and a non-streaming client reads nothing until the body is complete, so **this run cannot distinguish an untyped hang from a typed `timeout` returned at 120 s plus a few milliseconds**: the FAIL stands as registered, but "untyped" is not established by it (the 2026-10-24/25 failure has the same ambiguity). The next soak must give the client a timeout well above the daemon's first-token timeout, or lower that timeout for the run, so a hang reads as typed `timeout` and an engine not answering one admitted request becomes a separately measurable item. **Hypothesis, not a determination:** one request of a 40-burst never received a first token from llama-server (b11609, `-np 4 -c 128`) or never reached it, while the engine served everything else, and nothing bounded its wait below 120 s. The first 24 h soak (2026-10-24/25) failed on one untyped no-response of the same kind in about 556k burst requests; this run has two in 5,301,520. The 3 h overload-only rerun of 2026-10-29 (no kills) had none in 716k. Determining the cause needs a targeted reproduction, SINGLE-HOST is enough: bursts against a local daemon with a short first-token timeout and per-request engine-side timing, checking that every admitted request gets a response or a typed error within that timeout plus a margin; that is the next stable-runtime item for milestone C, run on the Mac while sentinel migrates.

Typed results over the run: 4,746,805 `ok`, 530,151 `overloaded` (4.0 per burst of 40, the admission bound working), 24,562 `backend_failed` (on a live engine, 0.51 % of admitted burst requests, load-correlated; see above), every `cancel`, `stream`, `pull`, `chat` and `golden` step ok (132,538 each).

Milestone C therefore stays open: the runtime is stable in memory, descriptors, orphans and recovery over 24 h on Linux, and fails its own zero-untyped bound twice in 5,301,520 adversarial requests.
