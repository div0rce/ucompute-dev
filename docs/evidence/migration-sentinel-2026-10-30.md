# Migration of sentinel (Linux) to UCompute (D-028, D-029, D-030) — 2026-10-30

Procedure: `docs/release/migration-to-ucompute.md`, adapted for a development host with no installed app: sentinel runs
daemons built from a synced source tree (`~/src/ucompute`), with the acceptance home `~/.ucompute-acceptance` and the
test home `~/.ucompute-test`. Host: Ubuntu 26.04 x86_64, i9-9900K, CPU-only engine pin (D-024). Done after the second
24 h soak ended (`docs/evidence/linux-soak-2026-10-29.md`), by the engineering session over SSH, every long step detached
(`setsid nohup`) with a log in `~/ucompute-migration-2026-10-29/`.

## Backup (condition 1: nothing deleted)

`~/ucompute-migration-2026-10-29/sentinel-migrate.sh backup` (kept beside the Mac's migration files): every home item
carrying the former name was SHA-256-inventoried, then moved (never deleted) into `~/ucompute-migration-2026-10-29/former-data/`,
then re-hashed in place. The script refuses to start while any process runs from those paths (none did: the soak's daemon
and driver were stopped by the collect step, the interference watcher exits with the soak).

| What | Moved | Size |
|---|---|---|
| The acceptance home (identity, pairings, four models, engine b11609, runs, logs) and its daemon log | `.<former>-acceptance`, `.<former>-acceptance.log` | 2.3 GB |
| The soak home (the 24 h soak's daemon home: models, engine, 2,650 engine logs) | `.<former>-soak` | 15 GB |
| The test home (fixtures) | `.<former>-test` | 81 MB |
| The two source trees (with build output) | `src/<former>`, `src/<former>-runtime-cli-wt` | 9.7 GB |
| Scripts and logs from earlier sessions | `<former>-build.log`, `<former>-linux-tests.sh`, `<former>-pull-qwen.sh`, `<former>-pull.log`, `<former>-status-during-load.sh`, `<former>-test-linux.log` | small |
| The soak driver's folder (driver, pid files, raw rows, watcher) | `soak` | 1.4 MB |
| The old Linux tarball and its checksum (missed by the pattern, moved by hand into `former-data/pkg-out/`) | `pkg-out/<former>-0.1.0-linux-x86_64.tar.gz`, `.sha256` | 4.2 MB |

Inventory: **23,722 files, 31 GB** (`inventory-sentinel.tsv`, `inventory-sentinel.sha256`), `backup.log`.

Re-check after the move: **2 of 23,722 files did not match** their inventory hash, both llama-server stdout logs of the
soak's engines (`former-data/.<former>-soak/logs/engine-0d217551-….log` and `engine-9962ba9d-….log`, 7.5 MB each, last
written 2026-10-29). Re-hashed 20 minutes later with no process holding them and their sizes unchanged over 4 s: the first
matches its inventory hash again (`cfe7534f…`), the second still differs (inventory `71b2263d…`, now `dc12a150…`).
A full second pass over the inventory (`recheck2.log`, `nice 19`, idle I/O, after the build): **23,721 of 23,722 match**; only `engine-9962ba9d-….log` differs, again `dc12a150…`, so its content is stable and the inventory value for that one file cannot be reproduced. Cause unknown; the two
files are the test engine's own stdout logs, not device data (identity, pairings, models, settings), all of which verified.

Afterwards no file or folder with the former name remains in the home outside the backup, `~/.cargo` and `~/.rustup`
(third-party crate sources) and `~/.cache`.

## Install and carry-over

- Source: `scripts/mac-host.sh sync` (HEAD 749fe93, tracked files only) into `~/src/ucompute`, `.ucompute-sync-head`
  written; `cargo build --release -p ucompute-cli` 1 m 50 s (`build-749fe93.log`); `target/release/ucompute --version`
  0.1.1; `/health` on a temporary port reports commit `749fe93…`, no dirty marker.
- Models (`sentinel-migrate.sh carry`): imported into `~/.ucompute-acceptance` only where the blob's SHA-256 and size
  match its manifest; all four did: `qwen2.5-0.5b-instruct-q4_k_m` (491,400,032 B), `qwen2.5-0.5b-instruct-q8_0`
  (675,710,816 B), `qwen2.5-1.5b-instruct-q4_k_m` (1,117,320,736 B), `stories15M-q4_0` (19,077,344 B).
- Engine: not copied; provisioned from the pinned catalog at first use (test home: `scripts/fetch-fixtures.sh`, hash-verified
  `llama-b11609-bin-ubuntu-x64.tar.gz`).
- Settings re-applied from the old `config.json`: privacy `local_only`, networking on, serving peers on, peer quota
  120 requests and 200,000 tokens per minute, 4 concurrent. `local_priority` absent, so the D-026 default `refuse_new` applies.
- Device identity: **intentional reset (recorded)**. Old peer id `edb3c91c…` (kept in the backup); the new one is created
  by the first daemon start. Every pairing (this Mac, the second Mac, the laptop) must be redone once with renamed builds.
- Test home `~/.ucompute-test`: fixtures (`stories260K`, `stories15M-q4_0`) and the engine from `fetch-fixtures.sh`, plus
  `qwen2.5-0.5b-instruct-q4_k_m` hard-linked from the acceptance home (no extra disk), 549 MB.
- Keep-awake: the user service `ucompute-keep-awake.service` was already installed and active (2026-10-29), untouched.

## Acceptance (condition 2), sentinel

| # | Check | Result |
|---|---|---|
| 1 | Fresh install | Pending: the renamed Linux tarball and `verify_linux_install.py` (milestone C) |
| 2 | Replacement of the previous development install | **PASS**: the old daemons stopped, no orphaned process (`pgrep` for the engine, the daemon and the old tree: none); the renamed build runs on a test port and reports its commit |
| 3 | Startup after a reboot | Not tested |
| 4 | Inference: local / remote / split | Pending: the Linux suite (local, real engine), then P1 (split) with this Mac |
| 5 | Mesh with updated Mac and Windows devices | Pending: P1's pairing with this Mac; the laptop later |
| 6 | An outdated build fails cleanly | Pending (needs a pre-rename peer on the network) |
| 7 | Recovery from a failed installation | Not tested |
