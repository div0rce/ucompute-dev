# Migration of the owner's Mac to UCompute 0.1.1 (D-028, D-029, D-030) — 2026-10-29

Procedure: `docs/release/migration-to-ucompute.md`. This Mac: MacBook Air, Apple M2, 8 GB, macOS 26.2.

## Backup (condition 1: nothing deleted)

Everything carrying the former name was moved, not deleted, into `~/ucompute-migration-2026-10-29/former-data/`
with a SHA-256 inventory and verified after the move:

| Part | What | Files | Check |
|---|---|---|---|
| 1 (unused items, before the app swap) | the previous app copy, a development build's cache and WebKit data, early project downloads, crash-reporter records, candidate 1's release folder | 132 | inventory `inventory-mac-part1.tsv` |
| 2 (the running app, after quitting it with SIGTERM: app, daemon and engine gone, port 8192 free) | the installed app, its data folder (identity, pairings, models, engine, runs, logs, settings), its log, cache and WebKit folders | 79 | `inventory-mac-part2.tsv`, re-hashed after the move: **0 mismatches** |
| Build output | the candidate 1 disk image left in `target/release/bundle/dmg/` (SHA-256 `a3239588…dd6d`, identical to candidate 1) | 1 | `release-candidate-1/build-output-0.1.0.dmg` |

Afterwards no file or folder with the former name remains in `/Applications`, `~/Library/Application Support`,
`~/Library/Logs`, `~/Library/Caches`, `~/Library/WebKit` or `~/Downloads` (third-party Adobe preferences excepted).

## Install and carry-over

- Installed `/Applications/UCompute.app` 0.1.1 from `~/UCompute-release-candidates/0.1.1-7d4d194/UCompute_0.1.1_aarch64.dmg`
  (SHA-256 `c1166ba91cf07d0af7d57e3563a4f592b0de077fefc9e0725192aae38c42b937`, `SHA256SUMS` checked), signature verifies.
- Models: imported only where the blob's SHA-256 and size match its manifest; both did:
  `qwen2.5-0.5b-instruct-q4_k_m` (491,400,032 B) and `stories15M-q4_0` (19,077,344 B).
- Engine: not copied; the app downloaded and hash-checked the pinned b11609 Metal build at the first load.
- Settings re-applied from the old `config.json`: privacy `trusted_group`, networking on, serving peers on, peer quota
  120 requests and 200,000 tokens per minute, 4 concurrent. `/api/network` reads them back.
- Device identity: **intentional reset (recorded)**. Old peer id `89839a1e…` (kept in the backup), new `7954c98a…`.
  Every pairing (the second Mac, the laptop, sentinel) must be redone once with renamed builds.

## Acceptance (condition 2), this Mac

| # | Check | Result |
|---|---|---|
| 1 | Fresh install | **PASS**: `verify_packaged_app.py` ALL PASS 19/19 on the 0.1.1 disk image (empty HOME, engine download, golden Metal output, quit, restart, daemon SIGKILL, app crash, no leftovers): `desktop-packaged-mac-m2-7d4d194.json` |
| 2 | Replacement of the previous install | **PASS**: the old app, daemon and engine stopped; no orphaned process; port 8192 free; the new app runs one daemon (`/health` 0.1.1, commit 7d4d194) |
| 3 | Startup after a reboot | Not tested (needs a reboot of the owner's Mac) |
| 4 | Inference: local / remote / split | Local **PASS** (stories15M on Metal, golden text equal to the verifier's); remote (Mode B) and split: pending sentinel's renamed build (after its soak) |
| 5 | Mesh with updated Linux and Windows devices | Pending (sentinel after its soak; the laptop when it is back) |
| 6 | An outdated build fails cleanly | Pending (needs a pre-rename peer on the network) |
| 7 | Recovery from a failed installation | Not tested |

If macOS's application firewall is on, it may ask once whether UCompute may accept incoming connections (the
encrypted peer port 8193); that approval is the owner's. A Dock shortcut to the former app, if any, now shows a question mark.
