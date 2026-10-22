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
