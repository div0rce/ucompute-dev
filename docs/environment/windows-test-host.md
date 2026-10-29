# Windows 11 test host (HP laptop): one-time setup

The second test machine is the owner's HP laptop running Windows 11. UCompute is cross-compiled on the Mac and
run on the laptop; the Mac drives it over SSH. These steps are run **by the owner on the laptop**, in
**PowerShell as Administrator** (Start menu, type PowerShell, right-click, Run as administrator).

## 1. Turn on the built-in OpenSSH server

```powershell
Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0
Start-Service sshd
Set-Service -Name sshd -StartupType Automatic
```

## 2. Allow the Mac's key (and only that key)

```powershell
$key = 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOfEuX7P8emUWyL2Al4vbDBRyv6h5Sc6LX6w9AKtkeJR ucompute-dev-mac'
Add-Content -Path "$env:ProgramData\ssh\administrators_authorized_keys" -Value $key
icacls.exe "$env:ProgramData\ssh\administrators_authorized_keys" /inheritance:r /grant "Administrators:F" /grant "SYSTEM:F"
```

If your Windows account is not an administrator, use your own key file instead:

```powershell
New-Item -ItemType Directory -Force "$env:USERPROFILE\.ssh" | Out-Null
Add-Content -Path "$env:USERPROFILE\.ssh\authorized_keys" -Value $key
```

## 3. Let UCompute's peers reach the laptop on your home network only

```powershell
Get-NetConnectionProfile          # shows the Wi-Fi name and whether it is Public or Private
Set-NetConnectionProfile -InterfaceAlias 'Wi-Fi' -NetworkCategory Private
New-NetFirewallRule -DisplayName 'UCompute peer endpoint (QUIC)' -Direction Inbound -Protocol UDP -LocalPort 8193 -Profile Private -Action Allow
New-NetFirewallRule -DisplayName 'UCompute LAN discovery (mDNS)' -Direction Inbound -Protocol UDP -LocalPort 5353 -Profile Private -Action Allow
```

If `Get-NetConnectionProfile` shows a different interface name than `Wi-Fi`, use that name.

## 4. Send these back

```powershell
whoami
(Get-NetIPAddress -AddressFamily IPv4 -InterfaceAlias 'Wi-Fi').IPAddress
```

## Undo everything later

```powershell
Stop-Service sshd; Set-Service -Name sshd -StartupType Disabled
Remove-NetFirewallRule -DisplayName 'UCompute peer endpoint (QUIC)','UCompute LAN discovery (mDNS)'
# and delete the ucompute-dev-mac line from the authorized_keys file used in step 2
# (set up before D-028, the line ends common-dev-mac and the rules are named 'Common peer endpoint (QUIC)' and 'Common LAN discovery (mDNS)')
```

What this grants: anyone holding the private key `~/.ssh/ucompute_hp_ed25519` on the Mac can open a shell on the
laptop as your account while it is on the same network. Nothing else is opened to the network except UDP 8193
and 5353 on Private networks.

## Driving the laptop from the Mac

`scripts/windows-host.sh` wraps the SSH key and the laptop's name (`WIN_HOST`, default `jess-hp.local`):

```sh
scripts/windows-host.sh run 'ucompute.exe --version'                  # cmd.exe command
scripts/windows-host.sh ps  'Get-NetConnectionProfile'              # PowerShell, no quoting trouble
scripts/windows-host.sh cp  target/x86_64-pc-windows-gnu/release/ucompute.exe C:/Users/jessg/UCompute/ucompute.exe
scripts/windows-host.sh sync                                         # HEAD -> C:/Users/jessg/src/ucompute
```

On a network the laptop has not seen before, Windows marks it Public, and the firewall rules above (Private
only) block SSH and UCompute until the owner sets that network to Private (Settings > Network & internet > Wi-Fi
> the network > Network profile type). eduroam drops mDNS between clients; a phone hotspot carries it.

## Build tools installed on the laptop (2026-10-19, with the owner's go-ahead)

Installed with `winget` over SSH: Visual Studio 2022 Build Tools 17.14 with the "Desktop development with C++"
workload (Microsoft's license accepted on the owner's instruction; the installer asked for a restart, not yet done), Rustup (the repo's `rust-toolchain.toml` pins 1.98.0, fetched on first build), Node.js
LTS 24 and pnpm 10.34.4. The WebView2 runtime ships with Windows 11. The desktop app builds natively there:
`scripts/windows-host.sh sync`, then `cd apps\desktop && pnpm install --frozen-lockfile && pnpm tauri build`.

Long builds over SSH: Windows ends a session's processes when the SSH connection drops (a Wi-Fi switch is
enough), so start them detached and poll a log, for example
`scripts/windows-host.sh ps 'Invoke-CimMethod Win32_Process -MethodName Create -Arguments @{CommandLine="cmd /c cd /d C:\Users\jessg\src\ucompute\apps\desktop && pnpm tauri build --bundles nsis > C:\Users\jessg\build.log 2>&1"}'`.

## Long jobs and sleep (2026-10-22)

- Jobs started from an SSH session end when the session closes (the OpenSSH server kills its children), so builds and long tests run from a scheduled task in the signed-in session (`UComputeTestBuild`, `UComputeTestLaunch`, `UComputeTestQuit`, `UComputeTestWifi`). The tasks were created by hand on the laptop, as `CommonTest*` before D-028, and are re-created under these names when the laptop migrates (`docs/release/migration-to-ucompute.md`); `UComputeTestWifi`'s action (disconnect Wi-Fi, rejoin the same profile) is not in this repository yet (planned as `scripts/windows-wifi-cycle.ps1`, which needs the laptop to repoint the task).
- A sleeping laptop pauses them and drops off the network (SSH and UCompute both lose it). Wrap long jobs in `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-keep-awake.ps1 -Command "<cmd.exe command line>"` (the bare script name does not run from cmd.exe or over SSH), which holds `ES_SYSTEM_REQUIRED` while the command runs, like `caffeinate -i` on macOS; no power setting changes. The command runs as `cmd /d /s /c "<cmd.exe command line>"`, so one with several quoted paths works, and the wrapper waits for that command only, not for anything it leaves running. A closed lid on battery may still sleep.
- `scripts/windows-host.sh sync` extracts HEAD over `C:\Users\<user>\src\ucompute` and then, through `scripts/windows-sync-clean.ps1`, deletes **every file in that tree that HEAD does not track**, whether it was tracked once or never: logs, run outputs, hand-made fixtures and `.env` files included. Only `target/`, `.ucompute/`, every `node_modules/`, `apps/desktop/src-tauri/binaries/`, `gen/` and `target/`, and `apps/desktop/dist/` are kept. So keep nothing else in that tree: write logs and run outputs outside it (for example `C:\Users\<user>\UComputeTest\`). The keep-list is HEAD's tree (`git -c core.quotePath=false ls-tree -r --name-only HEAD`), matched ignoring case. The script refuses to delete anything when fewer than 95% of tracked files are found at their paths, when the list has fewer than 50 files or when more than 200 files would go (`SYNC_DRY_RUN=1` lists only). `scripts/mac-host.sh sync` does the same on a second Mac (`~/src/ucompute`; it keeps `target/`, `.ucompute/`, `node_modules/`, `apps/desktop/src-tauri/binaries/` and `gen/`, and `apps/desktop/dist/`, with the same 50 and 200 limits but no 95% check).

## Two-machine runs from the Mac (2026-10-22)

`tests/distributed/two_machine/run_mac_windows.sh` drives the Mac's installed app daemon (A, 127.0.0.1:8192) and the laptop's (B, reached through an SSH tunnel on 18302). It is a **daemon API run, not an app UI run**: pairing included, every step goes through the two loopback APIs and no window is driven. Pairing started in either app, a wrong code typed by hand, the firewall consent step and the Mac-side group and run steps each need their own manual evidence through the apps' windows.

```sh
WIN_HOST=<laptop ip or name> OUT=<evidence dir> [NO_MDNS=1] [STEPS=state,manual,pair,quit,crash] \
  [MESH=0] [HOLD=0] [CROSS_HOLD=1 [BIN=<ucompute binary>]] [WATCH=120] [BOGUS_ADDR=192.0.2.1:8193] \
  [WIN_KEY=~/.ssh/ucompute_hp_ed25519] [WIN_USER=jessg] tests/distributed/two_machine/run_mac_windows.sh
```

- Order: the discovery driver (`STEPS`, default `state,manual,pair,quit,crash`), then the mesh driver (`MESH=0` skips it), then the hold driver (`HOLD=0` skips it). `NO_MDNS=1` (a network that drops multicast, such as eduroam) records the nearby checks N/A. Every driver records an unmet precondition (a flag not given, B never listed nearby, nothing cached after the crash, no second address, no `--c`/`--d`) as N/A, never PASS.
- Exit codes: a driver exits 3 when a step FAILed and 1 when it stopped. The runner goes on after a 3 and stops after a 1; it exits 0 when every step passed and 1 after a FAIL, a stop or an incomplete restore. An incomplete restore prints the `state.py restore` command to rerun once B answers.
- Tunnels: 18302 (B's API) is opened at the start, reopened before the mesh and hold drivers and closed at exit. The 19222 forward for `cdp.mjs` (the app's WebView2 debugging port) is no longer opened by the runner: open it by hand with `ssh -f -N -L 19222:127.0.0.1:9222 <user>@<laptop>` and close it after use.
- B is quit with the scheduled task `UComputeTestQuit` (taskkill without /f, like closing the window), crashed with `Stop-Process -Force` over SSH and started with the scheduled task `UComputeTestLaunch`. The quit step passes only when A saw B's daemon close the connection itself (`peer_lost` "closed_by_peer").
- `wifi` is not in the default `STEPS`. Run it last among the steps that talk to B (only `offline` may follow): B's address after the reconnect goes to `<out>/discovery/b_host`, which the runner's tunnel follows (a new DHCP lease can move B), and the run stops when B does not come back. At its end, when the wifi step ran and one `GET /health` to B's API gets no answer within 5 s, the discovery driver records its own restore N/A instead of restoring through the old tunnel and leaves it to the runner, which reopens the tunnel at `b_host` and restores both daemons (ffe1a47; `test_a_new_lease_leaves_the_restore_to_the_runner`). Run alone it then exits 0 with ALL PASS and neither daemon restored, and prints `python3 -I state.py restore <snap>` with a bare `state.py`: from the repo root run `python3 -I tests/distributed/two_machine/state.py restore <snap>` once B answers. "Freshly signed (D1)" is N/A: the API does not show the record's signed time.
- `CROSS_HOLD=1` starts two throwaway daemons on this Mac (`BIN`, default `/Applications/UCompute.app/Contents/MacOS/ucompute`; fresh `UCOMPUTE_HOME`s that link this checkout's `.ucompute` engines and models; APIs on 18331/18332, peer endpoints on 18431/18432, mDNS off) and passes them to the hold driver as `--c`/`--d`. The laptop must reach 18431/18432, so the macOS firewall has to allow that binary. Their logs go to `$OUT/xhold-c.log` and `$OUT/xhold-d.log`; they are stopped and their homes deleted at exit. Their revocation records stay in A's and B's stores (no API removes one; they name keys nobody keeps).
- Save and restore: every driver writes `<out>/state-before.json` first and restores it at its end, also after a failed step or an abort; the runner saves `$OUT/state-before.json` and restores it last, after stopping a driver still running, reopening the tunnel and starting B's app when it is down. `tests/distributed/two_machine/state.py` (`save <file> <api-url>...`, `restore <file>`) puts back networking and Sharing, holds (it ends only holds the run created, with Connect), pairings (a device paired during the run is unpaired, not revoked), groups (restored as saved; a group the run created is emptied, since the API has no DELETE for groups), the privacy mode, the loaded model (reloaded or unloaded) and the names the two devices give each other (it pairs once more with the saved names when they changed). It cannot restore `paired_at_utc` or the superseded-pairings list, it never pairs a lost device again (it reports the lost pairing and exits 1), and it tries every daemon even when one is unreachable. Its name restore has run only against the synthetic fake.
- The mesh driver unpins A and B first (DELETE, not revoke) when they arrive paired, so its discovery and "they connect" checks are real, then pairs them again with the owner's names. If that re-pairing fails, the run stops and `state.py` reports the lost pairing (exit 1); pairing again is then a manual step.
- The hold driver (`hold_two_machine.py --a … --b … --out <dir> [--c URL --d URL] [--watch 30]`) checks Desktop UI 2.0 E5: a baseline remote run, Disconnect seen on both sides, B's redial refused as `peer_held` with exactly one `held` audit line on A, both sides' requests refused while held, "a proven refusal ends B's dial" (PASS only when B's `last_error` is `peer_held` at an address with a later one in B's dial order, mDNS addresses then the pinned one with IPv6 left out on an IPv4 endpoint, and A has its own held line for B; N/A when B knows one address or the refusal came at the last address it dials; ffe1a47; `test_a_proven_refusal_ends_the_dial`, `test_a_refusal_at_the_last_address_b_dials_proves_nothing`, `test_an_address_b_cannot_dial_is_not_a_later_one` in `test_mesh_hold_state.py`), a Sharing off/on that keeps the hold, Connect ending it, revoking a held device and pairing again, and with `--c`/`--d` the cross-hold step: C and D pair with A and B in a shared group, A holds B, A revokes C and B revokes D, B's connect is refused `peer_held` and still carries both revocations (B drops C, A drops D) while the hold stays. Limits: the proven-refusal step reads B's dial order after the dials, and its PASS branch has run only against the synthetic fake (no mDNS in the SINGLE-HOST runs); the cross-hold step has run only SINGLE-HOST. Against either app built before protocol minor 1 (the laptop's 593d4ea, and the Mac's d9d8067 holder until it was replaced by an ffe1a47 build on 2026-10-23) the `peer_held` steps cannot pass: a minor-1 dialler reads that holder's close as `peer_unauthorized`, and a minor-0 dialler gets no exchange.
- Revocation steps accept `peer_unauthorized` as well as `peer_revoked` for B's refused request (since round 3 a refusal before the device proved its key reads `peer_unauthorized`) and need two facts from A, recorded in the step output (ffe1a47): A's own signed revocation of B in `GET /api/peers` revocations (`peer_id` B, `revoked_by` A; `a_holds_its_revocation_of_b`), which ties the refused key to B, and A's audit line refusing B as revoked (`peer_rejected`, reason `peer_revoked`) from B's exact endpoint, a known host of B at the port B listens on (`a_refused_as_revoked`; a daemon dials from its listen socket). Inbound TLS refusals carry no peer id, so the line is matched by that endpoint, not the host alone. Offline tests: `test_revocation_needs_a_refusal_as_revoked` (mesh) and `test_revoke_needs_a_refusal_as_revoked` (hold) in `test_mesh_hold_state.py`; none yet pins the `revoked_by` check or the listen-port substitution. Revoke steps run only when the earlier steps passed and networking is on.
- Step names changed on 2026-10-22, so later evidence will not match earlier names one to one: "manual: a hint with no UCompute device falls through to a known address and is not saved" (it said Common device until D-028), "pair: connected after re-pairing, named by host name", "{how}: one audit line per address for B while it is away, not one per redial (E4)", "{how}: B restarted, the two reconnect", "wifi: B's Wi-Fi drops, A loses B, then a new connection forms", "wifi: A hears B's record again after the reconnect" (plus "freshly signed (D1)", N/A), "revocation: B disconnected and refused as revoked", and new restore, setup and cross-hold steps.
- `run_mac_mac.sh` (second Mac, tunnel 18303) is unchanged; it gets the save, restore and non-zero exit on FAIL from the discovery driver itself.
- Offline checks of the drivers (synthetic; no host or daemon contacted): `python3 -m unittest discover -s tests/distributed/two_machine -v` (fakes in `fake_mesh.py`, classes named Synthetic, canned completion text) and `sh tests/distributed/test_runner_flow.sh` (the runner's control flow with stub drivers and no-op `ssh`/`curl`). As of 2026-10-22 `run_mac_windows.sh` has not run against the real pair since these changes.

## Accessibility checks on the laptop (2026-10-22, not yet run there)

Both scripts must run in the signed-in desktop session (a scheduled task with the interactive token, like `UComputeTestLaunch`), with the app running. Neither has run on the laptop yet, and the Mac has no PowerShell, so they are untested.

- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-uia-dump.ps1 -Out <file> [-Depth 40]` writes the UI Automation tree of the `ucompute-desktop` window: control type, name, automation id, help text, item status, focusable, FOCUSED, disabled, offscreen and live-region setting, the names and states NVDA and Narrator read, ending with `UIA_DONE <n> elements`. An element removed mid-walk (a React re-render) is written as `<element gone: …>` and the rest of the dump continues. A screen-reader-level check without a screen reader, not a test with NVDA itself.
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-nvda-run.ps1 start|stop -Nvda <portable NVDA dir> -Work <dir> [-MinIdle 300] [-Window UCompute]` is a test with NVDA itself, silent. It uses NVDA 2026.2 portable (`nvda_2026.2.exe`, SHA256 `f3f8d29974a88d687b3c4809be192219ec579c5bdabcda5aaf53635288bca824`, as published in NV Access's release post; check it with `Get-FileHash` before creating the portable copy). `start` refuses (exit 3) unless the signed-in user has been idle for `-MinIdle` seconds and no NVDA is running, writes a seeded config (silence synthesizer, no sounds, no browse mode on page load, no typed-character echo, update and add-on store checks pointed at a dead local address, Remote off, configuration not saved on exit), starts NVDA `--minimal --disable-addons` logging at level IO to a raw log, stops with exit 4 unless the log shows NVDA initialized with the silence synthesizer, and brings the app window to the front. `stop` quits NVDA, confirms it exited, copies only the "Speaking" entries with their times to `<Work>\nvda-speech.txt`, deletes the raw logs and writes a status line to `<Work>\nvda-status.txt`.
- Keystroke caveat: logging level IO also records every key typed anywhere in the session, unmasked, passwords included. The raw log never leaves the laptop and is deleted at `stop`; the idle gate keeps runs to an unattended laptop. Do not type anything on the laptop between `start` and `stop`, and check `nvda-status.txt` reports `raw_deleted=True`.
- On the Mac side, `swift scripts/macos-ax-dump.swift [process] [depth] [link]` dumps the tree VoiceOver reads (see `apps/desktop/README.md`, "Checks").
