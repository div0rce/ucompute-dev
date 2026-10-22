# Mac and Windows laptop on eduroam: connect at address and remote run (2026-10-22)

**Two-machine evidence.** This Mac and the Windows laptop on the Rutgers eduroam network, which drops multicast between clients, so mDNS discovery cannot work there (`docs/evidence/m3-two-machine-2026-10-18.md`). This is the case the connect-at-address fallback exists for (D3, requirement 5).

| | Mac | Windows laptop |
|---|---|---|
| Machine | MacBook Air, Apple M2, 8 GB, macOS 26.2, 10.75.79.236 | HP, Core Ultra 7 258V, Arc 140V, Windows 11, 10.75.120.9, network profile Private |
| App | Desktop UI 2.0 packaged app from d9d8067 (daemon `623876ae…`, verified ALL PASS, `desktop-ui2-packaged-mac-m2.json`) | older installed app built from caf1497 (does not have the connect route; only the dialling side needs it) |

The laptop's SSH host key at the new address was compared with the one already known for it (`SHA256:Tpf9ki…`, identical) before the address was trusted.

| Check | Result |
|---|---|
| Before: the Mac's pinned address for the laptop was its old hotspot address | `last_error`: "no answer from 172.20.10.6:8193 within 4 s"; 0 nearby devices (no mDNS on this network) |
| `POST /api/peers/<laptop>/connect {"addr":"10.75.120.9:8193"}` on the Mac | **200 in 0.166 s**, connected; the pinned address became `10.75.120.9:8193` only after the laptop presented the key pinned at pairing (the transport's pinned-key check; an address is a hint) |
| Remote run, Mac → laptop, `stories15M-q4_0`, 32 tokens, temperature 0 | **golden completion**, `target: peer:1a54602a…`; first token 2.5 s (the laptop loaded the model on demand), 509 tok/s decode on the laptop (fixture: a path check, not performance); the Mac's new jobs list shows the request `done` with the peer target and 32 tokens |

Not covered here (follows in the acceptance run with the new Windows build): pairing in both directions, the Windows firewall consent step, revocation, privacy, disconnect/hold, quit/crash cleanup, network change, Windows UI.

## Windows firewall consent step and pairing started on Windows (2026-10-22, 21:28-21:29 UTC)

The Windows installer built natively on the laptop from 593d4ea (`Common_0.1.0_x64-setup.exe`, sha256 `c8af273d467c38254fc96037d27dcc6cfdc115adf98f1e248fc4ab61c0ae05c0`, 4,608,282 bytes) was installed silently into `C:\Users\jessg\CommonVerify-593d4ea\app` by `apps/desktop/scripts/verify_windows_app.ps1`, with a fresh data folder (new device identity `b79212f0…`). While its window was open in the laptop's desktop session, the owner's side used both apps by hand (these steps were not automated):

- **Firewall consent step:** "Allow on private networks" was pressed and the Windows elevation prompt approved at the laptop. The resulting rule, read with `netsh advfirewall firewall show rule name="Common (paired devices)" verbose`: Enabled Yes, Direction In, Profiles **Private**, Protocol **UDP**, Program **`C:\Users\jessg\CommonVerify-593d4ea\app\common.exe`**, Action Allow. Exactly the scope the step promises; nothing broader.
- **Pairing started on Windows** (the direction Windows Firewall dropped on 2026-10-19, pktmon `receive inspection`): the laptop's audit log records `pairing_started` (port 50969) at 21:28:41 and `pairing_succeeded` with this Mac (`89839a1e…`, role initiator, addr 10.75.79.236:8193) at 21:29:09, then `peer_connected` outbound and, after Sharing was turned on, inbound. **The direction that failed is fixed on real hardware.**
- **Device name:** this Mac lists the laptop as "JESS-HP" (its computer name), not "This Mac".

The automated verifier itself stopped at its first launch check, because the app was being used by hand at the same time (its measurements need an untouched app); it is re-run on the next build. Networking was on in that fresh install only because it was turned on by hand (audit `network_changed` at 21:28:15); the default stays off.
