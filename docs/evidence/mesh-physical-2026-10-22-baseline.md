# Mac and Windows laptop on eduroam: pre-fix baseline acceptance run (2026-10-22, 21:48-22:00 UTC)

**Two-machine evidence, daemon API run (not an app UI run).** The two-machine drivers drive the two installed apps'
daemon APIs (loopback; the laptop's through an SSH tunnel), not their windows. The network is Rutgers eduroam, which
drops multicast between clients, so every mDNS check is recorded N/A (`ok: null`), never PASS. This run used the builds
**before** the three fix rounds (3df9438, 75a0a89, 18c4cf3) and drivers that a later review found could pass some steps
without exercising them (section 3), so it is a baseline: it shows what worked before the fixes and which product
defect the fixes target. It does not certify the fixed builds; the post-fix run is pending the laptop (its battery
died at 22:05 UTC).

| | A | B |
|---|---|---|
| Machine | MacBook Air, Apple M2, 8 GB, macOS 26.2, 10.75.79.236 | HP, Core Ultra 7 258V, Arc 140V, Windows 11, 10.75.120.9 (network profile Private) |
| App | Desktop UI 2.0 packaged app from d9d8067, installed (daemon `623876ae…`) | native NSIS build from 593d4ea (`c8af273d…`), installed per user at `CommonVerify-593d4ea\app` with the approved firewall rule (40f78bd) |

Command (from this Mac, at 95592b0 plus the then-uncommitted mesh and hold drivers):
`WIN_HOST=10.75.120.9 OUT=<dir> NO_MDNS=1 bash tests/distributed/two_machine/run_mac_windows.sh`.
The Mac was compiling at the same time, so its timings are under load. Raw HTTP calls: `mesh-physical-2026-10-22-baseline/{discovery,mesh,hold}/raw.jsonl.gz`;
step results `results.json` and `console.txt`; runner log `runner.log`; both sides' state before the run `state-before.json`
(restored at the end: groups, privacy, Sharing).

## 1. Results

**Discovery and connection state** (`discovery/`): host names reported by each daemon; "last seen" advances while
connected; pairing in both directions (A shows the code and B joins, then B shows and A joins), a wrong code refused
(`pairing_failed`), the right code pairs, each side names the other by its host name ("JESS-HP", "mous").
Graceful quit of B's app (CommonTestQuit): A sees B gone in 1.04 s with `closed_by_peer` (B's daemon closed the
connection itself, not the kill fallback), B's daemon and engine end in 3.5 s, A shows why B is unreachable, a dial
at a dead hint fails in 8.1 s and does not replace the pinned address, B back 7.6 s after restart. Crash of B's app
(Stop-Process -Force): A sees the loss in 4.6 s, B's processes end in 1.8 s, B back 8.2 s after restart. N/A: the five
nearby/mDNS checks. Three steps FAILed because of driver defects, not the product (section 3).

**Mesh** (`mesh/`), ALL PASS (1 N/A): wrong code then right code; group and privacy set on both; remote execution
A → B and B → A, JSON and streaming, golden output each time (stories15M fixture; tok/s figures are path checks, not
performance); `local_only` on A refuses a peer request on the sender (403 `policy_denied`, 3 ms, nothing sent); neither
audit log contains prompt text; A revokes B → B disconnected in 0.05 s and refused `peer_revoked`, pairing again
connects in 0.42 s; B's app killed → A notices in 4.0 s, the two reconnect 8.1 s after B restarts, golden again.

**Disconnect (hold, E5)** (`hold/`): A disconnects B → both sides show it within 20 ms; A's own request to B refused on
A before anything is sent (403 `policy_denied`, 1.8 ms); B's request to A refused `peer_held`; no reconnection for 30 s
and one audit line on A for B's attempts; Connect ends the hold and a remote run is golden again; revoking a held
device works and the two pair again unheld. **FAIL, product defect:** turning Sharing off and on on A ended the hold
(review finding trust-1; fixed in 3df9438, SINGLE-HOST evidence in `docs/VERIFICATION.md`). FAIL, driver timing: B's
reason was checked 16 ms after the disconnect, before B's own redial (B's `last_error` is its latest failed dial).

## 2. Accessibility trees of the same builds (not tests with a screen reader)

`a11y/windows-uia-593d4ea-chat.txt`: the Windows UI Automation tree (what NVDA reads) of the 593d4ea app, Chat view,
from `scripts/windows-uia-dump.ps1` in the laptop's desktop session. Two keyboard-focusable status-bar items had no
name (fixed in 75a0a89). `a11y/macos-ax-d9d8067-{dashboard,devices}.txt`: the macOS accessibility tree (what VoiceOver
reads) of the installed d9d8067 app, from `scripts/macos-ax-dump.swift` (this process was already trusted for
Accessibility; nothing was granted). No unnamed interactive element; the window title is "UCompute"; the Devices table
showed the internal label "needs N2" (fixed in 75a0a89).

## 3. Driver defects this run exposed (fixed in 18c4cf3; none is a product defect)

The audit API lists events oldest first and the drivers assumed newest first, so some audit checks could see nothing;
an unpaired-id check compared whole peer records, whose "last seen" changes; the E4 count included the driver's own
dial at a dead hint. A later review (44 confirmed findings) also showed that the manual connect steps returned the
open connection without dialling, and that several steps did not check their preconditions or statuses. The results
above are reported as the steps' names claim only where the raw calls show the behaviour was exercised (for example
the dead-hint dial in the quit step took 8.1 s); the 0.3-0.5 ms manual connect steps are not counted as dials.

## 4. Windows key-file permissions (B-017 evidence, not a fix)

`windows-acl.txt`: `Get-Acl` on the test install's identity files (`device.key`, `device.pub`, `peers.json`,
`peers.json.lock`), `config.json` and `serve.json`: SYSTEM, BUILTIN\Administrators and the user only, all inherited
from the user's profile folder. Common sets no explicit DACL on Windows, so a `COMMON_HOME` outside the profile would
inherit that folder's ACL. Not tested from a second account (creating one is the owner's decision). B-017 stays open.
