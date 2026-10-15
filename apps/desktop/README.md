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
  the `pair_addr` to type into the other device's Join form (just the port on daemons without it), the QR
  payload as text, a countdown, and polls `GET /api/pair/status` until `result` holds `{peer}` or
  `{error}`), "Join a device" (address + code, or the pasted `common-pair:v1:...` text alone; a wrong code shows
  the `pairing_failed` text, a lockout shows `pairing_locked`), the paired list with connection state (the
  daemon's `connected`, or `health` when a daemon reports heartbeats), groups, revoke and remove (both confirmed
  inline), household groups and their two policy switches (`GET /api/groups`; `PUT /api/groups` upserts one
  group and returns the list), and devices seen on the network but not trusted, whose only action is "Pair"
  (it prefills the join form with the host and `pair_port` of the advertised pairing window). This device's own
  id (`GET /api/network` `peer_id`) is never listed there, even if mDNS reports its own advert. The code field
  ignores spaces, so a pasted `482 913` works. No QR image is drawn; the payload is shown as text.
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

## Two daemons on one Mac (SINGLE-HOST UI testing)

Everything here is **SINGLE-HOST**: two Common daemons on one machine. It exercises the UI, pairing and policy
flows; it never stands in for two physical devices (`docs/BLOCKERS.md` B-002).

Start two daemons from the repo root, each with its own `COMMON_HOME` (the identity key lives there; two daemons
sharing one home would be the same device) and its own API and peer ports:

```sh
mkdir -p /tmp/common-a /tmp/common-b
COMMON_HOME=/tmp/common-a cargo run -p common-cli -- serve --port 18301 --listen-port 18401 --listen-addr 127.0.0.1
COMMON_HOME=/tmp/common-b cargo run -p common-cli -- serve --port 18302 --listen-port 18402 --listen-addr 127.0.0.1
```

`--listen-addr 127.0.0.1` keeps the peer endpoint on loopback, so the pairing address shown is `127.0.0.1:<port>`.
Without it the endpoint binds every interface (the product default) and the address shown is the Mac's LAN IP.
For remote inference the serving home needs a model; link or copy one from `.common/models` and symlink
`.common/backends` rather than downloading again.

Then `pnpm dev` and open two tabs on the dev server, one per daemon:

- `http://localhost:1420/?daemon=http://127.0.0.1:18301`
- `http://localhost:1420/?daemon=http://127.0.0.1:18302`

Use `localhost:1420`, not `127.0.0.1:1420`: the daemon only accepts browser requests from the origins in its
`cors_origins` list. `?daemon=` accepts only `http://127.0.0.1:<port>` or `http://localhost:<port>`; anything
else is ignored and the tab uses the default daemon. Diagnostics shows which daemon a tab talks to. Production
builds ignore the parameter entirely (`import.meta.env.DEV` is false and the code is compiled out).

To pair: turn networking on in both tabs, press "Pair this device" in tab A, then in tab B type the address shown
under A's code into Address and the six digits into Code (or press "Pair" next to A under "Nearby, not paired"
when mDNS sees it). Both tabs share one origin, so they share `localStorage` (device name, chat history): each tab
keeps what it loaded, and a reload picks up the last write from either, so set the device name in each tab after
loading it.

## Checks

```sh
pnpm test         # vitest: API client, error-table sync with error.rs, axe scan, keyboard-only flows, pairing, privacy, ?daemon=
pnpm build        # tsc + vite build -> dist/
cargo check -p common-desktop   # from the repo root; the Tauri shell
```

## Packaging

```sh
cd apps/desktop && pnpm tauri build --bundles app,dmg     # macOS: Common.app + Common_<version>_aarch64.dmg
```

`pnpm tauri build` first runs `scripts/build-sidecar.sh` (`build.beforeBuildCommand`). It builds `common`
from this checkout (`cargo build --release --locked -p common-cli`, with `--target` when Tauri builds for
another triple), copies it to `src-tauri/binaries/common-<triple>`, checks the copy is the binary just built,
and checks that it accepts the exact arguments the app starts it with. A bundle can therefore no longer
carry a daemon from an older commit: that happened once, and the packaged daemon exited with
"unexpected argument '--exit-with-parent' found" on every launch. `src-tauri/binaries/` is git-ignored;
`pnpm prepare-sidecar` (copy only) remains for unusual cases.

The app's daemon arguments live in one file, `src-tauri/daemon-args.txt`
(`serve --host 127.0.0.1 --port 8192 --exit-with-parent`). The shell reads it at compile time;
`crates/common-cli/tests/desktop_sidecar.rs` and `build-sidecar.sh` read the same file, so a CLI change that
the app's arguments no longer parse fails a test and the build.

Daemon lifecycle:
- At launch the shell probes `GET /health` on 127.0.0.1:8192. If nothing answers it spawns the bundled
  `common` with those arguments through `tauri-plugin-shell`. If the daemon exits before it is healthy, the
  shell says so at once (exit code 2 means it rejected its command line) instead of waiting 20 s.
- Quit (Cmd+Q, closing the window, or SIGTERM/SIGINT/SIGHUP to the app) runs `RunEvent::Exit`: the shell sends
  the daemon SIGTERM, which unloads the model and stops the llama.cpp engine, and waits up to 10 s before
  killing it. (A plain kill used to leave the engine running orphaned.)
- If the app dies without running that (force quit, crash), `--exit-with-parent` makes the daemon notice
  within about a second and shut down the same way.

Verify a build on the exact packaged binaries (needs network and a free port 8192):

```sh
python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/Common_0.1.0_aarch64.dmg \
  --work "$(mktemp -d)" --out verify.json
```

It installs the app from the mounted .dmg into an empty HOME with a minimal PATH, then checks the daemon
arguments, a first launch that downloads the pinned engine, real inference, quit, restart, crash and restart
again, and that nothing is left running. The webview CSP only allows `connect-src` to `'self'` and
`http://127.0.0.1:8192`.

## Layout

```
src/api/client.ts      typed client for every endpoint, SSE parser, bearer-token support
src/api/errors.ts      error code -> plain-language title + next step (mirrors error.rs)
src/state/             tiny stores: settings (localStorage), daemon status, chat history, session runs, privacy mode, run target
src/views/             Home, Models, Chat, Hardware, Performance, Settings, Diagnostics, Peers, Privacy
src/components/        FitBar (memory fit), ErrorNotice, RunOn (device selector), Groups, Later (placeholder for M3/M7 views)
src/test/              fake daemon (fetch mock), axe scan, keyboard-only flows, pairing and privacy flows
src-tauri/src/lib.rs   sidecar spawn and graceful stop, health probe, quit on SIGTERM, daemon_info command
src-tauri/daemon-args.txt   the daemon's command line (single source of truth)
scripts/build-sidecar.sh    build + stage + check the sidecar (run by `tauri build`)
scripts/verify_packaged_app.py   end-to-end check of a built .app/.dmg
scripts/prepare-sidecar.sh
```
