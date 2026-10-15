# Common desktop

The desktop app is a thin shell around the Common daemon. A Rust daemon (`common serve`) owns models,
hardware probing and inference and exposes a local HTTP API on `http://127.0.0.1:8192`. This Tauri 2 +
React 19 app only renders that API; the Tauri Rust side does two things: it starts the bundled daemon
when none is running (and stops it on exit), and it answers `daemon_info()` with the base URL.

All API access lives in `src/api/client.ts`. If the daemon contract changes, that is the one file to edit.
Plain-language error texts live in `src/api/errors.ts` and are checked against
`crates/common-core/src/error.rs` by `src/api/errors.test.ts`.

## Devices and privacy (M3)

Two views talk to the peer-networking API from ADR-0003; both default to the safest state and say so.

- **Peers** (`#/peers`, F-10-05): the networking switch (off by default; `GET/PUT /api/network`), this device's
  id as 8 groups of 8 hex plus its `xxxx-xxxx-xxxx` label, "Pair this device" (shows the six-digit code large,
  the QR payload as text, a countdown, and polls `GET /api/pair/status` until `result` holds `{peer}` or
  `{error}`), "Join a device" (address + code, or the pasted `common-pair:v1:...` text alone; a wrong code shows
  the `pairing_failed` text, a lockout shows `pairing_locked`), the paired list with connection state (the
  daemon's `connected`, or `health` when a daemon reports heartbeats), groups, revoke and remove (both confirmed
  inline), household groups and their two policy switches (`GET /api/groups`; `PUT /api/groups` upserts one
  group and returns the list), and devices seen on the network but not trusted, whose only action is "Pair"
  (it prefills the join form with the host and `pair_port` of the advertised pairing window). No QR image is
  drawn; the payload is shown as text.
- **Privacy** (`#/privacy`, F-10-10): local only / trusted devices / public (disabled, "coming later"). Leaving
  local-only shows the disclosure returned by `GET /api/privacy` and requires the acknowledge checkbox before
  `PUT /api/privacy {mode, acknowledged}`. The current mode is the pill under the app name on every screen.
- **Run on**: Chat and Home show a device selector only when the mode is not local-only; a chat request then
  carries `"target": "peer:<id>"` and the reply footer says "Ran on <device>". In local-only mode the field is
  never sent, so the wire shape of a local request is unchanged.
- **Diagnostics** shows the audit tail (`GET /api/audit`, last 100 events, displayed most recent first: ids,
  sizes, reasons, never prompts) and "Copy diagnostics" copies the daemon's redacted bundle from
  `GET /api/diagnostics` when that endpoint exists, falling back to status + hardware when the daemon answers
  404 (the current one has no such route).

The fake daemon in `src/test/mocks.ts` serves every one of these endpoints with the response shapes of
`crates/common-api/src/mesh.rs` (a peer is a stored `PeerRecord` plus `short_label`, `connected` and `groups`;
a discovered device has `id`, `addr`, `pairing`, `pair_port`, `proto`, `trusted`, `seen_utc` and no label), so
the flows are tested end to end in jsdom (`src/test/peers.test.tsx`, `src/test/privacy.test.tsx`). Everything
here runs on one machine against a fake; no test claims two physical devices paired.

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
pnpm test         # vitest: API client, error-table sync with error.rs, axe scan, keyboard-only flows, pairing, privacy
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
src/state/             tiny stores: settings (localStorage), daemon status, chat history, session runs, privacy mode, run target
src/views/             Home, Models, Chat, Hardware, Performance, Settings, Diagnostics, Peers, Privacy
src/components/        FitBar (memory fit), ErrorNotice, RunOn (device selector), Groups, Later (placeholder for M3/M7 views)
src/test/              fake daemon (fetch mock), axe scan, keyboard-only flows, pairing and privacy flows
src-tauri/src/lib.rs   sidecar spawn, health probe, daemon_info command
scripts/prepare-sidecar.sh
```
