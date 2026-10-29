# UCompute desktop app

The desktop app is a thin shell around the daemon. A Rust daemon (`ucompute serve`) owns models,
hardware probing and inference and exposes a local HTTP API on `http://127.0.0.1:8192`. This Tauri 2 +
React 19 app only renders that API; the Tauri Rust side does four things: it starts the bundled daemon
when none is running (restarts it if it dies, and stops it on exit), it keeps the daemon's output in a capped log
file, it answers `daemon_info()` with the base URL and the restart state, and on Windows it reads and, with the
user's consent, adds the daemon's firewall rule (`firewall_status`, `allow_through_firewall`).

All API access lives in `src/api/client.ts`. If the daemon contract changes, that is the one file to edit.
Plain-language error texts live in `src/api/errors.ts` and are checked against
`crates/ucompute-core/src/error.rs` by `src/api/errors.test.ts`.

## The app (UI 2.0, 1860196)

A computing-network control center (`docs/design/desktop-ui-2-plan.md`): a rail with seven sections
(Mod+1 … Mod+7), contextual toolbars, sortable dense tables (APG grid with cell focus and a column chooser), a
resizable inspector with tabs, and a status bar fed only by real API fields. Window and rail say "UCompute",
the product's name, as do the bundle, installers and data folders (`UCompute.app`, `org.ucompute.desktop`, `UCompute`
data folders; D-028, no compatibility with the former name). Routes are `#/<section>[/<id>][?params]`; the old
routes stay as aliases: `#/home` → Dashboard, `#/peers` → Devices, `#/hardware` → this device's Hardware tab,
`#/performance` → Jobs, `#/diagnostics` → Activity, `#/privacy` → Settings › Privacy.

Data honesty, everywhere: a store keeps its last good data and shows "Not updated since …" instead of
presenting it as live (tables, inspectors, privacy mode); a slow service reads "Busy (not answering)", not "Not
running"; a value never read reads "Not known" or "—", never a definite empty; a section whose stores were never read
while the service is busy shows "Loading…", not its empty state (and no busy banner); a store whose last read
failed is read again within 5 s if it is always read or already polled; inspector and Run panel live values
(Models State and Locations, Chat's loaded model, Devices Performance) read "not refreshed" while the service is
busy or stale; a failed privacy read counts as not known (Devices Permissions, readiness and "Can run there";
Settings › Privacy keeps its controls mounted but inert and reads "Current mode: not known while the background
service is not running."); a cached mDNS record never makes a device connected; every performance figure says
what was measured, where and over which window. No credits,
Swarm or Cloud controls exist (D-019).

- **Dashboard** (`#/dashboard`): Needs attention (including the Windows firewall notice in short form), Get
  started, Computers, Running now and Recent activity, each linking to its section.
- **Devices** (`#/devices[/<id>]`, F-10-05): one table of this computer, paired, nearby and revoked devices
  (scopes All, Connected, Not connected, Nearby, Revoked; a filter), the **Networking** switch (off by default;
  `GET/PUT /api/network`) and **Pair a device…**, which opens the Pair dialog: **Show a code** (`POST
  /api/pair/start`: the six-digit code, the `pair_addr` to type on the other device, the QR payload as text and a
  countdown; polls `GET /api/pair/status`) or **Enter a code** (address and code, or the pasted
  `ucompute-pair:v1:…` text alone; a wrong code shows the `pairing_failed` text, a lockout `pairing_locked`; spaces
  in the code are ignored). A nearby device's only action is Pair with it, which prefills the address. This
  device's own id is never listed as nearby.
  - Selection actions: **Connect** (paired, networking on, not connected; `POST /api/peers/{id}/connect` `{}`
    or `{addr}`), **Disconnect** (connected; `POST /api/peers/{id}/disconnect`, absent on daemons without
    `held`), Remove (unpair, confirmed). Revoke is in the Overview tab's Danger section (confirmed).
  - Inspector tabs: Overview (state, "Can run there", "Can use this computer", ids, groups, software from the
    signed capabilities), Hardware (`ucompute hardware --json` here; the device's signed report for a peer),
    Models, Performance, Connection (state, `remote_addr`, direction, open since, RTT, bytes and rates, lost
    packets, known addresses with where each came from, `last_error`, the Connect form, Disconnect, the
    firewall notice), Permissions (group membership), Activity (this device's audit lines).
  - Device state comes from `GET /api/peers` only: Connected / Not responding (`connected`, `health`),
    Connecting (this view's own dial), Unreachable (`last_error`, with the daemon's message and hint), Reconnecting,
    Offline (mDNS runs here and does not see it), Disconnected by you (`held`), Disconnected by <device>
    (`last_error.code` `peer_held`; the holder must be a protocol minor 1 build, see below), else Not connected. A device this computer disconnected reads "Can use
    this computer: No · disconnected by you", until you connect again, turn networking off and on, or the
    service restarts.
  - Connect: the address is only a hint. Whatever answers must present the key pinned at pairing, so another
    device at that address is refused (`peer_unauthorized`: "The device that answered did not prove it is
    {name}, or {name} no longer trusts this computer. Nothing was trusted or saved.") and nothing changes.
    A refusal reached before the device has proved its key on the control stream reads the same way, because it
    cannot be told from one forged on the path: a device that revoked this computer, or one held by a minor-0
    build (every build before 3df9438), shows `peer_unauthorized` on a new dial, not `peer_revoked` or
    "Disconnected by <device>". So does a claimed protocol mismatch before that proof, with "update UCompute on
    both devices" in its hint (780c2b9).
    Unpaired devices never get this action. With nothing nearby the view says why: networking off, discovery off
    on this computer, or a network that stops devices finding each other (many campus and office Wi-Fi
    networks), with pairing by address as the way on.
- **Models** (`#/models[/<id>]`): the catalog and store with memory fit (`GET /api/models`), download with
  the licence shown first, load, unload, delete.
- **Jobs** (`#/jobs[/<id>]`): `GET /api/jobs`, requests running here, sent to a paired device or served for
  one, with live token counts and timings; never prompt or reply text. An unknown state is kept with the
  active jobs, never filed as finished.
- **Chat** (`#/chat`): the **Destination** select (This computer, or a paired device that can run prompts;
  shown only when the privacy mode is not local-only) and the **Model** select. A run on a paired device needs no
  model loaded here: every model `GET /api/models` lists is offered and the device loads it on demand; a
  missing model reads "<device> does not have <model>". In local-only mode `target` is never sent, so a local
  request's wire shape is unchanged. A paired device receives only the trailing run of messages already
  exchanged with that same device, plus the new prompt: nothing written under Local only, for this computer or
  for another device is sent to it (`contextFor` in `src/sections/chat/session.ts`); this computer gets the
  whole stored conversation. The chat says so ("{device} gets only the messages exchanged with it since you
  last used another destination; earlier messages are not sent with this prompt."), and that hint is the
  composer's accessible description. The conversation is stored only in this app (`localStorage` `ucompute.chat`).
- **Activity** (`#/activity[/<key>]`): the audit tail (`GET /api/audit`: ids, sizes, reasons, never
  prompts) and **Copy diagnostics** (the daemon's redacted bundle from `GET /api/diagnostics` when that route
  exists, else status and hardware).
- **Settings** (`#/settings[/<page>]`): General, Execution, Network (Networking, the name other devices see,
  discovery, listening and dial addresses, the firewall notice), Sharing & permissions (run requests for
  paired devices, quotas, groups: `GET /api/groups`, `PUT /api/groups` upserts one), Privacy, Security,
  Storage. Privacy (F-10-10): Local only / Trusted devices; leaving local-only shows the disclosure from `GET
  /api/privacy` and needs "I understand" before `PUT /api/privacy {mode, acknowledged}`; there is no Public
  control (no backend exists). It also says what is kept where and offers "Delete chat history on this
  computer".
- **Status bar**: service state, engine and model (`GET /api/status` `engine`), devices, transfer rate,
  privacy mode and free disk, each from an API field and marked when stale. Service and Network rate are
  named groups ("Service: Running", "Network rate"); items do not remount when a tip appears; the engine's
  loading seconds are hidden from screen readers and given in the tip. While a pairing window is open it shows an Attention item named "Pairing open"; its countdown is
  visible but hidden from screen readers.

The name other devices see defaults to the daemon's `device_name` (macOS Computer Name, else hostname /
`COMPUTERNAME`); the Settings field only overrides it, and `pair/start` / `pair/join` send `name` only when one
is set. A stored "This Mac" (the old hard-coded default) counts as unset.

**Windows firewall notice** (`src/ui/FirewallNotice.tsx`; Tauri commands `firewall_status` and
`allow_through_firewall`, `src-tauri/src/firewall.rs`). Pairing started on Windows listens on a random UDP
port, which Windows Firewall drops without a rule. With networking on and no matching rule found, Devices, the
Pair dialog, the Connection tab, Settings › Network and the Dashboard show "Allow on private networks". The
click runs, through the Windows elevation prompt, `netsh advfirewall firewall delete rule name="UCompute (paired
devices)" program=<ucompute.exe next to the app>` and then `… add rule name="UCompute (paired devices)" dir=in
action=allow program=<that path> protocol=UDP profile=private enable=yes`: only this program's own rule is
replaced, so other Windows accounts' rules of the same name stay. Nothing is added without that prompt, and
never for the Public or Domain profile. The status is one read-only PowerShell query (`Get-NetFirewallRule
-DisplayName` with its port and application filters, `Get-NetConnectionProfile`; non-localized values, UTF-8):
a read that fails or stops early shows nothing, never "no rule". Connected networks Windows treats as Public
are named (set them to Private in Windows Settings if you trust them), and so are domain networks (their
administrator decides). Both reads keep their errors (`-ErrorAction SilentlyContinue -ErrorVariable`) and the script throws any error
that is not a not-found (`CmdletizationQuery_NotFound*`) before its `end` marker, so a failed read never reads as
"no rule"; only the script text is unit-tested, it has not yet run under PowerShell on Windows. The app counts
the daemon as its own only once its own child has printed `listening on http://127.0.0.1:8192` (stdout or
stderr) and while it runs; while it is still starting the firewall commands wait up to 20 s (ffe1a47). When the
app adopted a daemon it did not start, another daemon holds 8192 while its own is still starting, or the daemon
it started has exited, it neither checks nor adds a rule (the adopted message; Allow refuses). After Allow
succeeds, focus moves to the outcome; while Windows asks, Allow is `aria-disabled` (not `disabled`), so after a
cancel or failure focus stays on it and it is described by its `role=alert` outcome (ffe1a47).
A join where nothing answered shows the daemon's hint (a firewall on the other device) instead of "check the
code".

The fake daemon in `src/test/mocks.ts` serves every endpoint the app uses with the response shapes of the
daemon (contract fixtures for the UI 2.0 additions in `src/test/fixtures/api/`, checked against the real
daemon by `crates/ucompute-api/tests/ui_fixtures.rs`); `/api/peers/{id}/connect` dials hint, discovered and
pinned addresses against `answersAt` and `impostors`. The flows are tested end to end in jsdom per section
(`src/sections/*/*.test.tsx`) and across sections (`src/test/`). Everything here runs on one machine against
a fake; no test claims two physical devices paired.

## Development loop

Terminal 1, from the repo root, start the daemon yourself:

```sh
cargo run -p ucompute-cli -- serve --host 127.0.0.1 --port 8192
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
the sidecar binary is not needed for `tauri dev`. Every build of the shell (`tauri dev`, `cargo build`, `clippy` or
`test -p ucompute-desktop`) needs `src-tauri/gen/THIRD_PARTY_NOTICES.txt`, because tauri-build copies the bundle
resources: `pnpm build-sidecar` writes it, or, without building the daemon, from the repo root
`python3 scripts/gen_notices.py --out apps/desktop/src-tauri/gen/THIRD_PARTY_NOTICES.txt --crate ucompute-cli --crate
ucompute-desktop --js`. The file is git-ignored.

## Two daemons on one Mac (SINGLE-HOST UI testing)

Everything here is **SINGLE-HOST**: two UCompute daemons on one machine. It exercises the UI, pairing and policy
flows; it never stands in for two physical devices (`docs/BLOCKERS.md` B-002).

Start two daemons from the repo root, each with its own `UCOMPUTE_HOME` (the identity key lives there; two daemons
sharing one home would be the same device) and its own API and peer ports:

```sh
mkdir -p /tmp/ucompute-a /tmp/ucompute-b
UCOMPUTE_HOME=/tmp/ucompute-a cargo run -p ucompute-cli -- serve --port 18301 --listen-port 18401 --listen-addr 127.0.0.1
UCOMPUTE_HOME=/tmp/ucompute-b cargo run -p ucompute-cli -- serve --port 18302 --listen-port 18402 --listen-addr 127.0.0.1
```

`--listen-addr 127.0.0.1` keeps the peer endpoint on loopback, so the pairing address shown is `127.0.0.1:<port>`.
Without it the endpoint binds every interface (the product default) and the address shown is the Mac's LAN IP.
For remote inference the serving home needs a model; link or copy one from `.ucompute/models` and symlink
`.ucompute/backends` rather than downloading again.

Then `pnpm dev` and open two tabs on the dev server, one per daemon:

- `http://localhost:1420/?daemon=http://127.0.0.1:18301`
- `http://localhost:1420/?daemon=http://127.0.0.1:18302`

Use `localhost:1420`, not `127.0.0.1:1420`: the daemon only accepts browser requests from the origins in its
`cors_origins` list. `?daemon=` accepts only `http://127.0.0.1:<port>` or `http://localhost:<port>`; anything
else is ignored and the tab uses the default daemon. Settings › General shows which daemon a tab talks to (marked development only). Production
builds ignore the parameter entirely (`import.meta.env.DEV` is false and the code is compiled out).

To pair: turn Networking on in both tabs (Devices toolbar), press "Pair a device…" › "Show a code" in tab A,
then in tab B open "Pair a device…" › "Enter a code" and type the address shown under A's code and the six
digits (or select A in B's Nearby scope and press "Pair with" when mDNS sees it). Both tabs share one origin, so they share `localStorage` (device name, chat history): each tab
keeps what it loaded, and a reload picks up the last write from either, so set the device name in each tab after
loading it.

## Checks

```sh
pnpm test         # vitest: API client, error-table sync with error.rs, axe scan, keyboard-only flows, pairing, privacy, ?daemon=
pnpm build        # tsc + vite build -> dist/
cargo check -p ucompute-desktop   # from the repo root; the Tauri shell
```

Screen-reader-level checks of a running app (what a screen reader would read; none of them is a check with
the screen reader's own announcements except the NVDA run):

```sh
swift scripts/macos-ax-dump.swift [process, default ucompute-desktop] [max depth, default 40] [link title] > ax.txt
```

Prints the macOS accessibility tree VoiceOver reads (role, subrole, title, label, value, help, role
description, FOCUSED, disabled) and ends with `AX_DONE <n> elements`. With a link title (a rail section such as
`Devices`) it first presses that link (the AXPress VoiceOver's VO-Space sends) and waits a second. The calling
process must already be trusted for Accessibility (System Settings › Privacy & Security › Accessibility); the
script never asks and exits 2 when it is not. On Windows, `scripts/windows-uia-dump.ps1` prints the UI
Automation tree NVDA and Narrator read, and `scripts/windows-nvda-run.ps1` captures NVDA 2026.2's own speech
silently; both are described, with their limits, in `docs/environment/windows-test-host.md`. Neither Windows
script has run on the laptop yet.

## Packaging

```sh
cd apps/desktop && pnpm tauri build --bundles app,dmg     # macOS: UCompute.app + UCompute_<version>_aarch64.dmg
```

`pnpm tauri build` first runs `scripts/build-sidecar.mjs` (`build.beforeBuildCommand`; Node, so it runs on Windows too). It builds `ucompute`
from this checkout (`cargo build --release --locked -p ucompute-cli`, with `--target` when Tauri builds for
another triple), copies it to `src-tauri/binaries/ucompute-<triple>`, checks the copy is the binary just built,
and checks that it accepts the exact arguments the app starts it with. A bundle can therefore no longer
carry a daemon from an older commit: that happened once, and the packaged daemon exited with
"unexpected argument '--exit-with-parent' found" on every launch. `src-tauri/binaries/` is git-ignored.

Before that it writes `src-tauri/gen/THIRD_PARTY_NOTICES.txt` for the same target (`scripts/gen_notices.py --crate
ucompute-cli --crate ucompute-desktop --js --target <triple>`, run from the repo root with `python3`, else `python` or
`py -3`): every Rust crate the daemon and the app link on that target and the app's production npm packages, with
their licence texts. A licence that `deny.toml` does not allow, or no Python, fails the build before anything is
built. `bundle.resources` ships it as `Contents/Resources/THIRD_PARTY_NOTICES.txt` in the macOS app and next to the
exe in the Windows install. The file is git-ignored and regenerated by every build.

The app's daemon arguments live in one file, `src-tauri/daemon-args.txt`
(`serve --host 127.0.0.1 --port 8192 --exit-with-parent`). The shell reads it at compile time;
`crates/ucompute-cli/tests/desktop_sidecar.rs` and `build-sidecar.mjs` read the same file, so a CLI change that
the app's arguments no longer parse fails a test and the build.

Daemon lifecycle:
- At launch the shell probes `GET /health` on 127.0.0.1:8192. If nothing answers it spawns the bundled
  `ucompute` with those arguments through `tauri-plugin-shell`. If the daemon exits before it is healthy, the
  shell says so at once (exit code 2 means it rejected its command line) instead of waiting 20 s. The daemon
  is the app's own only once that child prints `listening on http://127.0.0.1:8192` (the CLI prints it right
  after the bind; the shell reads stdout and stderr) and until it exits (ffe1a47): a daemon that answers
  `/health` before then may be another one holding the port.
- Quit (Cmd+Q, closing the window, or SIGTERM/SIGINT/SIGHUP to the app) runs `RunEvent::Exit`: the shell asks
  the daemon to stop (SIGTERM to its own child on macOS and Linux; on Windows, which has no SIGTERM,
  `POST /api/shutdown` only when its own child has bound the port, otherwise it kills the child, so it never
  asks another daemon to stop), so it unloads the model, stops the llama.cpp engine and sends the mDNS
  goodbye, and waits up to 10 s before killing it. (A plain kill used to leave the engine running orphaned, and on Windows left the device listed as nearby
  on other computers for about two minutes.)
- If the app dies without running that (force quit, crash), `--exit-with-parent` makes the daemon notice
  within about a second and shut down the same way.
- If the daemon the app started dies while the app runs (killed, crashed, any exit other than 0, which is a
  requested stop, or 2, a command line it rejects), the shell restarts it after 1, 2, then 4 s. A fourth exit
  within a minute of the first restart is not restarted: the UI then says the service stopped repeatedly and
  asks to quit and reopen the app. A restart never starts a second daemon: if a daemon already answers
  `/health` on 8192 the shell uses that one instead (not its own, as at launch), and Quit during the wait
  starts nothing. A daemon the app found already serving at launch is never restarted, and neither is one it
  found serving when it went to restart its own (it is not the app's; if it stops, the status bar says the
  service is not running until it answers again or the app is reopened). A restart that cannot start the
  sidecar (binary missing, fork failing) counts as one more exit: it is tried again within the same bounds,
  then given up with the same text (the log then says it "could not start the daemon in 3 restart attempts", not
  that it exited again). A sidecar that cannot be started at launch is not tried again: the status bar says "Service
  could not be started. Quit and reopen the app." (`daemon_info` `start_failed`), not "Retrying automatically".
  (A daemon that exits with 0 or 2, which is not restarted, still reads "Service not running. Retrying
  automatically.": only the UI's polling retries then.) While the shell waits
  the status bar says "Service stopped unexpectedly. Restarting it."; once the service answers again the
  announcer says "Service restarted" and a dismissable entry stays in the status bar
  (`daemon_info` reports `restarts`, `restarting`, `gave_up`, `start_failed`).
- The daemon's stdout and stderr, and the shell's own lines about it (started, exited with which code or
  signal, restarting), go to `daemon.log` in the app's log folder: `~/Library/Logs/org.ucompute.desktop/`
  on macOS, `%LOCALAPPDATA%\org.ucompute.desktop\logs\` on Windows and
  `~/.local/share/org.ucompute.desktop/logs/` on Linux (owner-only on macOS and Linux). Once the next line
  would take it past 5 MiB it becomes `daemon.log.1`, replacing the older one, so the log never takes more than
  about 10 MiB. Settings › General shows the path; Copy diagnostics names it after copying, but the copied JSON
  holds only the restart count (the path contains the account name). This file is the app's, separate from
  `UCOMPUTE_HOME/logs/` that the daemon's own diagnostics bundle reads. What uninstall keeps of it is not yet
  checked (WP-01 retention table).

Verify a build on the exact packaged binaries (needs network and a free port 8192):

```sh
python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/UCompute_0.1.0_aarch64.dmg \
  --work "$(mktemp -d)" --out verify.json
```

It installs the app from the mounted .dmg into an empty HOME with a minimal PATH, then checks that the app ships
`Contents/Resources/THIRD_PARTY_NOTICES.txt` with entries for `rustls` and `react`, the daemon
arguments, a first launch that downloads the pinned engine, real inference, quit, restart, the daemon killed
(SIGKILL to the daemon only: the app restarts one daemon, `/health` within 10 s of the kill, the killed daemon's
engine stopped, inference again with one engine, and `daemon.log` holding the killed and the restarted run), a
crash of the app and restart again, and that nothing is left running. The log parsing, the stage-6 condition, the
notices check and the notices step of `build-sidecar.mjs` (stub tools) have offline tests:
`python3 -m unittest discover -s apps/desktop/scripts -p 'test_*.py'`. `verify_windows_app.ps1` checks the same
notices next to the installed exe (not yet run on Windows). The webview CSP only allows `connect-src` to `'self'` and
`http://127.0.0.1:8192`.

## Layout

```
src/api/client.ts      typed client for every endpoint, SSE parser, bearer-token support
src/api/errors.ts      error code -> plain-language title + next step (mirrors error.rs)
src/router.ts          the seven sections, `#/` routes and the old routes as aliases
src/shell/             rail, status bar, attention items, menu, shortcuts, announcer, dev-only axe hook
src/sections/          one folder per section: dashboard, devices (table, inspector tabs, Pair dialog), models, jobs, chat, activity, settings
src/ui/                design system: tokens, DataTable, Inspector, Dialog, SectionFrame, FirewallNotice, Groups, icons (Lucide subset, ISC)
src/state/             polled live stores (status, network, peers, jobs, privacy …), settings and chat history (localStorage), device rows
src/components/        FitBar (memory fit)
src/test/              fake daemon (fetch mock), contract fixtures, axe scan, keyboard-only flows, pairing and privacy flows
src-tauri/src/lib.rs   sidecar spawn, restart policy and graceful stop, daemon log, health probe, quit on SIGTERM, daemon_info and firewall commands
src-tauri/src/firewall.rs   Windows Firewall status (PowerShell read) and the consent-gated rule
src-tauri/daemon-args.txt   the daemon's command line (single source of truth)
scripts/build-sidecar.mjs   write THIRD_PARTY_NOTICES, build + stage + check the sidecar (run by `tauri build`)
scripts/verify_packaged_app.py   end-to-end check of a built .app/.dmg
scripts/verify_windows_app.ps1   the same for the Windows NSIS installer
```
