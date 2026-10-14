# Common desktop

The desktop app is a thin shell around the Common daemon. A Rust daemon (`common serve`) owns models,
hardware probing and inference and exposes a local HTTP API on `http://127.0.0.1:8192`. This Tauri 2 +
React 19 app only renders that API; the Tauri Rust side does two things: it starts the bundled daemon
when none is running (and stops it on exit), and it answers `daemon_info()` with the base URL.

All API access lives in `src/api/client.ts`. If the daemon contract changes, that is the one file to edit.
Plain-language error texts live in `src/api/errors.ts` and are checked against
`crates/common-core/src/error.rs` by `src/api/errors.test.ts`.

## Development loop

Terminal 1, from the repo root, start the daemon yourself:

```sh
cargo run -p common-cli -- serve --host 127.0.0.1 --port 8192
```

Terminal 2, in `apps/desktop`:

```sh
pnpm install
pnpm dev          # Vite on http://localhost:1420; open it in a browser
```

In a plain browser the app assumes the daemon is at `http://127.0.0.1:8192`. When it runs inside Tauri it asks
the shell (`daemon_info`) instead. If the daemon is not reachable the UI says so and every view degrades
honestly; nothing is fabricated.

To run the actual desktop window in development (needs the Rust toolchain and a few minutes for the first
compile) use `pnpm tauri dev`. With a daemon already serving on 8192 the shell does not spawn a sidecar, so
the sidecar binary is not needed for `tauri dev`.

## Checks

```sh
pnpm test         # vitest: API client, error-table sync with error.rs, axe scan, keyboard-only flow
pnpm build        # tsc + vite build -> dist/
cargo check -p common-desktop   # from the repo root; the Tauri shell
```

## Packaging the sidecar

Tauri bundles external binaries named `<name>-<target triple>`. Build the daemon, then copy it into place:

```sh
cargo build --release -p common-cli          # repo root; produces target/release/common
cd apps/desktop && pnpm prepare-sidecar       # -> src-tauri/binaries/common-aarch64-apple-darwin
pnpm tauri build
```

`scripts/prepare-sidecar.sh [triple]` reads the host triple from `rustc -vV`; set `COMMON_BIN` to copy a
binary from elsewhere. `src-tauri/binaries/` is git-ignored.

At launch the shell probes `GET /health` on 127.0.0.1:8192. If nothing answers it spawns
`common serve --host 127.0.0.1 --port 8192` through `tauri-plugin-shell`, waits for `/health` in the
background, and kills the child on exit. The webview CSP only allows `connect-src` to `'self'` and
`http://127.0.0.1:8192`.

## Layout

```
src/api/client.ts      typed client for every endpoint, SSE parser, bearer-token support
src/api/errors.ts      error code -> plain-language title + next step (mirrors error.rs)
src/state/             tiny stores: settings (localStorage), daemon status, chat history, session runs
src/views/             Home, Models, Chat, Hardware, Performance, Settings, Diagnostics
src/components/        FitBar (memory fit), ErrorNotice, Later (honest placeholder for M3/M7 views)
src/test/              fake daemon (fetch mock), axe scan, keyboard-only flow
src-tauri/src/lib.rs   sidecar spawn, health probe, daemon_info command
scripts/prepare-sidecar.sh
```
