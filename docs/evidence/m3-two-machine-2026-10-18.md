# M3 on two physical machines: Mac + Windows laptop (2026-10-18)

**Two-machine evidence.** Two different physical computers on the same campus Wi-Fi (eduroam), not one host:

| | A | B |
|---|---|---|
| Machine | MacBook Air, Apple M2, 8 GB, macOS 26.2 | HP laptop, Intel Core Ultra 7 258V, 32 GB, Intel Arc 140V, Windows 11 Home 26200 |
| Build | `common` debug build from this checkout | `common.exe` cross-built on the Mac (`x86_64-pc-windows-gnu`), release |
| Engine | llama.cpp b11609 macos-arm64 (Metal) | llama.cpp b11609 win-vulkan-x64 (Vulkan), downloaded and verified by `common setup` on the laptop |
| Peer endpoint | UDP 18401 | UDP 8193 (Windows firewall: allowed on Private networks only) |

Both daemons ran `serve --network --serve-peers`. The laptop was driven from the Mac over SSH (key-only, set up by the owner: `docs/environment/windows-test-host.md`); its loopback API reached the Mac through an SSH tunnel. **All peer traffic between the machines was Common's own QUIC + TLS 1.3 transport over the Wi-Fi.** Driver: `tests/distributed/two_machine/mesh_two_machine.py`. Run 1 used commit 4d91cf0; run 2 used the job-object fix (see below) on both sides.

## Results (run 1 / run 2)

| Step | Result | Numbers |
|---|---|---|
| Discovery (mDNS) | **FAIL (network)** | Neither daemon listed the other. Not a Common defect: on this network neither machine can resolve the other's own `.local` name with the OS's mDNS stack either (Mac `dns-sd` sees only its own advert; Windows `Resolve-DnsName mous-2.local` → "does not exist"). eduroam does not pass multicast between clients. Needs a network that does (home Wi-Fi, a phone hotspot). |
| Pairing, wrong code | PASS | 401 `pairing_failed` (113 / 68 ms) |
| Pairing, right code | PASS | Laptop joined the Mac's window at the Mac's LAN address; both pinned; join 69 / 79 ms; connected 0.41 / 0.01 s later |
| Remote execution Mac → laptop (laptop serves on Vulkan0, Intel Arc 140V) | PASS | Golden text JSON and SSE (33 chunks), `target` echoed. Warm request total 64 / 46 ms; engine TTFT 14 / 12 ms; decode 714 / 980 tok/s; first streamed text at the Mac after 14 / 11 ms |
| Remote execution laptop → Mac (Mac serves on Metal, M2) | PASS | Golden text JSON and SSE. Warm total 194 / 98 ms; engine TTFT 9 ms; decode 437 / 512 tok/s |
| `local_only` on the sender | PASS | 403 `policy_denied` in 6.5 / 2.1 ms; nothing sent |
| Audit logs | PASS | No prompt or completion text on either machine (9 / 24 and 9 / 21 `remote_request` entries) |
| Revocation (Mac revokes the laptop) | PASS | Laptop disconnected 0.012 s later; its next request to the Mac: 403 `peer_revoked`; re-pairing reconnects in 0.41 / 0.35 s |
| Laptop daemon killed (`taskkill /F`) | PASS | Mac saw the loss after 4.19 / 4.23 s (bound 6 s) |
| Laptop daemon restarted | PASS | Reconnected 4.6 / 5.1 s after the restart; Mac → laptop golden again |

## Windows defects found and fixed

- **Free disk space read 0** (`df` is POSIX) → `GetDiskFreeSpaceExW` (commit 4d91cf0).
- **GPU name cut at the first `)`**: `Vulkan0 (Intel(R)` → `Vulkan0 (Intel(R) Arc(TM) 140V GPU (16GB))` (4d91cf0).
- **A killed daemon left `llama-server.exe` running** with the model loaded (observed: a 99 MB orphan after run 1). The engine now goes into a Windows job object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, whose only handle the daemon holds. Checked on the laptop: daemon pid killed with `taskkill /F` (no `/T`), its engine was gone 1.5 s later; in run 2 the only engine left was the restarted daemon's own child.

## Limits

- One run of each, on one network. Discovery (F-02-01) is still unverified: it needs two machines on a network that carries mDNS.
- The Windows desktop app (Tauri) is not built yet; F-10-05 (two installs pairing through the UI) still needs it.
- No packet capture on the LAN (the Mac has no sudo for tcpdump); the in-process relay capture (SINGLE-HOST) remains the capture evidence.
- The Mac side ran a debug build.

## Run 3: phone hotspot (discovery), 2026-10-19 02:38 UTC

Same two machines, both joined to an iPhone personal hotspot (one subnet that carries mDNS; the laptop's profile set to Private by the owner). Fresh Mac data folder (new identity), so the two started unpaired. Commit 5bbd4b7-era binaries: the Mac debug build and the laptop's `common.exe` with the job-object fix.

| Step | Result | Numbers |
|---|---|---|
| Discovery (mDNS) | **PASS** | Each daemon listed the other at its hotspot address (laptop `:8193`, Mac `:18401`) by the first poll, within about 1 s of both daemons answering `/health` (bound 5 s) |
| Unpaired peer is untrusted | PASS | Both entries `trusted: false`, neither pinned |
| Unpaired peer cannot receive work | PASS | Mac → laptop request before pairing: 403 `peer_unauthorized` in 3.2 ms |
| Pairing (wrong, then right code) | PASS | 401 `pairing_failed`; join 198 ms, connected 0.6 s later |
| Remote execution both ways | PASS | Golden JSON and SSE; Mac → laptop warm 116 ms total, 768 tok/s; laptop → Mac warm 222 ms, 275 tok/s (hotspot latency) |
| local_only, audit, revocation, re-pair | PASS | 403 in 4.4 ms; no prompt text; laptop disconnected 0.061 s after the revoke, `peer_revoked`; re-paired in 1.1 s |
| Laptop daemon killed / restarted | PASS | Loss seen after 4.0 s; reconnected 5.6 s after the restart; golden again |

No `llama-server.exe` was left on the laptop after its daemon was killed at the end.

"mDNS can be disabled by config" (the third F-02-01 criterion) is covered by `disabled_config_is_inert` (crate test) and by the CLI's `--no-mdns` / `COMMON_MDNS=0`, which the CLI mesh test uses.
