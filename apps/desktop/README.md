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
  - The name other devices see defaults to the daemon's `device_name` (macOS Computer Name, else hostname /
    `COMPUTERNAME`); the field only
    overrides it, and `pair/start` / `pair/join` send `name` only when one is set. A stored "This Mac" (the old
    hard-coded default) counts as unset.
  - This device shows `dial_addr`, the `ip:port` another device can be given when discovery does not work.
  - Each paired device shows where it is: the addresses mDNS sees now ("Seen on this network"), else its pinned
    `last_addr` ("Last known"), else "Not known"; and, while not connected, why the last dial failed (`last_error`:
    the daemon's message and hint, e.g. no answer within the idle timeout, a firewall on UDP 8193).
  - **Connect** (paired devices only, networking on, not connected) opens an inline form: empty retries the
    addresses the daemon knows, an address tries that first (`POST /api/peers/{id}/connect` `{}` or `{addr}`). The
    address is only a hint: whatever answers must present the key pinned at pairing, so another device at that
    address is refused (`peer_unauthorized`) and nothing changes. Unpaired devices never get this action.
  - With nothing nearby the view says why: networking off, discovery off on this computer, or a network that blocks
    devices from finding each other (many campus and office Wi-Fi networks), with pairing by address as the way on.
  - Paired-device state, from `GET /api/peers` only: Connected / Not responding (`connected`, `health`), Connecting
    (this view's own dial), Unreachable (`last_error`, which outranks the sticky `health: reconnecting`),
    Reconnecting, Offline (mDNS runs here and does not see it), else Not connected. A nearby record never makes a
    device connected or ready. "Can run prompts from this computer" needs an open connection, one of its groups
    allowing prompts and Privacy not local-only; otherwise the row says which of those is missing.
  - Windows only: the pairing window listens on a random UDP port, which Windows Firewall drops without a rule.
    With networking on and no rule, Peers (and the pairing code) shows "Allow on private networks"; the click runs
    `netsh advfirewall firewall add rule name="Common (paired devices)" dir=in action=allow program=<common.exe next
    to the app> protocol=UDP profile=private` through the Windows elevation prompt (Tauri commands
    `firewall_status` and `allow_through_firewall`, `src-tauri/src/firewall.rs`). Nothing is added without that
    prompt, and never for the Public or Domain profile. `firewall_status` also names connected networks Windows
    treats as Public (PowerShell `Get-NetConnectionProfile`, read-only): there the rule does not apply, so the notice
    stays, without a button, and says to set the network to Private in Windows Settings if the user trusts it. A join
    where nothing answered shows the daemon's hint (a firewall on the other device) instead of "check the code".
  - Connect: a `peer_unauthorized` refusal says "The device that answered did not prove it is {name}, or {name} no
    longer trusts this computer. Nothing was trusted or saved." The daemon reports an alert sent before the device
    showed its pinned key as that, never as `peer_revoked`.
- **Privacy** (`#/privacy`, F-10-10): local only / trusted devices / public (disabled, "coming later"). Leaving
  local-only shows the disclosure returned by `GET /api/privacy` and requires the acknowledge checkbox before
  `PUT /api/privacy {mode, acknowledged}`. The current mode is the pill under the app name on every screen.
- **Run on**: Chat and Home show a device selector only when the mode is not local-only; a chat request then
  carries `"target": "peer:<id>"` and the reply footer says "Ran on <device>". In local-only mode the field is
  never sent, so the wire shape of a local request is unchanged. A run on a paired device needs no model loaded
  here: the chat offers every model `GET /api/models` lists (the ids this daemon can name for a remote run; no
  route lists a peer's models) and the peer loads it on demand. When the peer does not have it, its
  `model_not_downloaded` arrives inside the stream; the client turns any in-stream error object into the typed
  error, and the chat says "<device> does not have <model>" instead of saving an empty reply.
- **Diagnostics** shows the audit tail (`GET /api/audit`, last 100 events, displayed most recent first: ids,
  sizes, reasons, never prompts) and "Copy diagnostics" copies the daemon's redacted bundle from
  `GET /api/diagnostics` when that endpoint exists, falling back to status + hardware when the daemon answers
  404 (the current one has no such route).

The fake daemon in `src/test/mocks.ts` serves every one of these endpoints with the response shapes of
`crates/common-api/src/mesh.rs` (a peer is a stored `PeerRecord` plus `short_label`, `connected` and `groups`;
a discovered device has `id`, `addr`, `addrs`, `pairing`, `pair_port`, `proto`, `trusted`, `seen_utc` and no label;
`/api/peers/{id}/connect` dials hint, discovered, pinned against `answersAt` and `impostors`), so
the flows are tested end to end in jsdom (`src/test/peers.test.tsx`, `src/test/privacy.test.tsx`,
`src/test/devices.test.tsx`). Everything
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

`pnpm tauri build` first runs `scripts/build-sidecar.mjs` (`build.beforeBuildCommand`; Node, so it runs on Windows too). It builds `common`
from this checkout (`cargo build --release --locked -p common-cli`, with `--target` when Tauri builds for
another triple), copies it to `src-tauri/binaries/common-<triple>`, checks the copy is the binary just built,
and checks that it accepts the exact arguments the app starts it with. A bundle can therefore no longer
carry a daemon from an older commit: that happened once, and the packaged daemon exited with
"unexpected argument '--exit-with-parent' found" on every launch. `src-tauri/binaries/` is git-ignored.

The app's daemon arguments live in one file, `src-tauri/daemon-args.txt`
(`serve --host 127.0.0.1 --port 8192 --exit-with-parent`). The shell reads it at compile time;
`crates/common-cli/tests/desktop_sidecar.rs` and `build-sidecar.mjs` read the same file, so a CLI change that
the app's arguments no longer parse fails a test and the build.

Daemon lifecycle:
- At launch the shell probes `GET /health` on 127.0.0.1:8192. If nothing answers it spawns the bundled
  `common` with those arguments through `tauri-plugin-shell`. If the daemon exits before it is healthy, the
  shell says so at once (exit code 2 means it rejected its command line) instead of waiting 20 s.
- Quit (Cmd+Q, closing the window, or SIGTERM/SIGINT/SIGHUP to the app) runs `RunEvent::Exit`: the shell asks
  the daemon to stop (SIGTERM on macOS and Linux; `POST /api/shutdown` on Windows, which has no SIGTERM), so it
  unloads the model, stops the llama.cpp engine and sends the mDNS goodbye, and waits up to 10 s before killing
  it. (A plain kill used to leave the engine running orphaned, and on Windows left the device listed as nearby
  on other computers for about two minutes.)
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
scripts/build-sidecar.mjs   build + stage + check the sidecar (run by `tauri build`)
scripts/verify_packaged_app.py   end-to-end check of a built .app/.dmg
```
