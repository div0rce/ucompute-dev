# Packaged desktop app: daemon failed to start (fixed 2026-10-15)

## Report

On another Apple Silicon Mac the packaged `Common.app` never got a working daemon:

```
unexpected argument '--exit-with-parent' found
Usage: common serve --host <HOST> --port <PORT>
exited with Some(2)
daemon did not become healthy within 20s
```

## Diagnosis (on this Mac, before any change)

- The app shell (`apps/desktop/src-tauri/src/lib.rs`) has passed `--exit-with-parent` since a146ba1 (2026-10-14 13:06).
- `pnpm tauri build` never rebuilt or restaged the daemon: it bundled whatever file sat in `src-tauri/binaries/`. That file, and `target/release/common`, were built at 2026-10-14 12:50, before the flag existed.
- The `Common.app` built at 2026-10-15 16:32 and installed in `/Applications` carries such a daemon: `Contents/MacOS/common serve --exit-with-parent --help` exits **2** ("unexpected argument"), and nothing answered on 127.0.0.1:8192 while it ran. The CLI source was correct; the bundle was not built from it.

Found while verifying the fix:

1. **Shutdown orphaned the engine.** On quit the shell used `CommandChild::kill()`, which is SIGKILL, so the daemon could not stop its llama.cpp engine. An orphaned `llama-server` from the app's data directory had been observed earlier.
2. **A crashed app orphaned the engine too.** When the app died, the daemon's stdout/stderr pipe lost its reader. The daemon resets SIGPIPE to the default for CLI piping, so its next write ("parent process exited; shutting down") killed it before it stopped the engine. Reproduced by `desktop_daemon_starts_infers_exits_with_its_parent_and_restarts_single_host` once its daemon got a pipe like the app's.
3. **The bundle was not signed as a bundle.** Without a signing identity Tauri left only the linker's ad-hoc signature on the main executable: `codesign --verify --deep --strict` failed with "code has no resources but signature indicates they must be present". Apple Silicon requires downloaded code to be signed.

## Fix

| Change | Where |
|---|---|
| The daemon's arguments live in one file, `src-tauri/daemon-args.txt` (`serve --host 127.0.0.1 --port 8192 --exit-with-parent`); the shell compiles it in, the CLI test and the build script read it | `apps/desktop/src-tauri/daemon-args.txt`, `src/lib.rs` |
| `tauri build` runs `scripts/build-sidecar.sh` first: `cargo build --release --locked -p common-cli` (with `--target` for cross builds), stage, check the staged file is the one just built, check it accepts the app's arguments | `scripts/build-sidecar.sh`, `tauri.conf.json` `beforeBuildCommand` |
| Quit sends the daemon SIGTERM and waits up to 10 s (it unloads the model and stops the engine) before killing it; SIGTERM/SIGINT/SIGHUP to the app take the same path as Cmd+Q | `src/lib.rs` |
| A daemon that exits before it is healthy is reported at once, and exit code 2 is named as an app/daemon build mismatch, instead of a 20 s timeout | `src/lib.rs` |
| `common serve` ignores SIGPIPE; when its parent exits it points stdout/stderr at /dev/null before writing; failed log writes no longer `eprintln!` | `crates/common-cli/src/main.rs` |
| Ad-hoc bundle signing: `bundle.macOS.signingIdentity = "-"` | `tauri.conf.json` |

`--exit-with-parent` is kept: it is what stops the daemon when the app is force-quit or crashes.

## Regression tests

- `crates/common-cli/tests/desktop_sidecar.rs` (workspace suite):
  - `desktop_daemon_args_are_accepted_by_the_cli`: the CLI built from this checkout parses the exact arguments in `daemon-args.txt` (test 1).
  - `an_unknown_daemon_argument_is_a_usage_error`: exit 2, the code the shell now reports.
  - `desktop_daemon_starts_infers_exits_with_its_parent_and_restarts_single_host`: the daemon started with the app's arguments, with stdout/stderr on a pipe that closes with its parent, becomes healthy (test 2), runs the golden completion on the real engine, stops itself and its engine within 0.75 s of its parent being SIGKILLed (test 3), frees the port, restarts on the same port with one daemon and one engine, and stops cleanly on SIGTERM (test 4). Failed before the stdio fix ("engine stops with the daemon not within 5s"), passes after (3 runs).
- `apps/desktop/scripts/build-sidecar.sh` fails the build when the staged daemon rejects the app's arguments. The stale daemon fails that check with exit 2.
- `apps/desktop/scripts/verify_packaged_app.py`: end to end on the shipped .dmg (tests 1–5); results below.

## Packaged build verified

Built with `cd apps/desktop && pnpm tauri build --bundles app,dmg` on the owner's Mac (Apple M2, 8 GB, macOS 26.2), from the source committed as 5f0c580 (no source file changed after the build).

| Artifact | sha256 |
|---|---|
| `Common_0.1.0_aarch64.dmg` (6,002,227 bytes) | `7cc481d1c821625eadf00e0dcc3fbca50ab9e59041114b819efed2250ef934c2` |
| `Common.app/Contents/MacOS/common` (in the .dmg and in the .app) | `3168e521de50cc514d926a5f4839112c1fa1f04a122103592a1f4b160558286d` |
| `Common.app/Contents/MacOS/common-desktop` (in the .dmg and in the .app) | `c66c8c3570d7b855a577c09ca429a2b3314829c6565d211f1ef4bde6c965ea01` |

`python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/Common_0.1.0_aarch64.dmg --built target/release --work <empty dir>`: **ALL PASS** (15 checks).

| Check | Result |
|---|---|
| Both executables arm64, linking only `/usr/lib` and `/System/Library` | PASS |
| Packaged binaries are the binaries this build produced (Tauri re-signs them; identical once the signature and the `__LINKEDIT` vmsize that codesign enlarges are set aside; a one-byte change is detected) | PASS |
| `codesign --verify --deep --strict` on the app (ad-hoc, hardened runtime) | PASS |
| Bundled daemon accepts the app's exact arguments | PASS |
| Fresh install: app copied out of the mounted .dmg, empty HOME, PATH `/usr/bin:/bin:/usr/sbin:/sbin`, no COMMON_HOME; daemon healthy 2.4 s after launch, started by the app process with the exact arguments; pinned llama.cpp b11609 downloaded into the new HOME | PASS |
| Real inference through the app's daemon: fixture pulled and hash-verified (19,077,344 bytes), loaded on `MTL0 (Apple M2)`, golden completion, 418 tok/s | PASS |
| Quit (SIGTERM to the app): app exits in 0.1 s, daemon and engine stop, port 8192 free, app log shows "daemon stopped" | PASS |
| Restart: one daemon (a new pid, child of the new app), no port conflict, golden completion again (438 tok/s), one engine | PASS |
| Crash (SIGKILL to the app): daemon and engine stop 0.52 s later, port free | PASS |
| Restart after the crash, quit: nothing from the install left running, port free | PASS |

## Limits

- SINGLE-HOST: one Mac. The fresh install is a fresh HOME and minimal environment on this Mac, not a second Mac or a fresh macOS user account.
- `spctl -a -t exec` rejects the app: it is ad-hoc signed, not Developer ID signed or notarized (B-008). On another Mac a downloaded copy must be allowed once in System Settings > Privacy & Security ("Open Anyway").
- The quit test sends SIGTERM to the app, which runs the same `RunEvent::Exit` code as Cmd+Q; Cmd+Q itself was not clicked.
- The copy in `/Applications/Common.app` on this Mac is still the broken 16:32 build until it is replaced with the new one.
