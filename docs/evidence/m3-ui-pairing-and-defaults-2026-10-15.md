# M3 desktop pairing through the UI, and a default-settings daemon rerun (2026-10-15)

> Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

**Everything here is SINGLE-HOST:** two daemons on the owner's one Mac (Apple M2, 8 GB, macOS 26.2). It is not two-machine evidence (B-002).

## 1. Two daemons, default settings (commit 2434bde, debug build)

Driver: `M3_DIR=<dir> python3 -I tests/distributed/single_host/m3_evidence.py step1 … step7 stop`, the same eight steps as `m3-single-host-2026-10-15.md`, rerun after the peer-admission change that keeps one engine slot for local work. Daemons used the shipped defaults (`--max-concurrent 4`, so 3 slots may serve peers), started with `--network --serve-peers`.

| Step | Result | Numbers |
|---|---|---|
| 1 discovery | PASS | each lists the other 0.98 s after both were healthy |
| 2 pairing | PASS | join 17.0 ms; both pinned; connected 0.86 s later; 6 wrong codes → `pairing_locked` |
| 3 group + privacy | PASS | non-local mode refused without `acknowledged` (403 `policy_denied`) |
| 4 remote execution | PASS | golden text on all three runs; warm engine TTFT 4.3 ms, client first chunk 4.8 ms, ~439 tok/s; 0 prompt hits over 19 files |
| 5 local_only | PASS | 403 `policy_denied` in 0.6 ms; server `remote_request` count unchanged (3 → 3) |
| 6 revocation | PASS | 403 `peer_revoked`; peer's connection closed 0.005 s after the revoke; still disconnected 6 s later |
| 7 disconnect | PASS | SIGKILL seen by the other daemon after 4.879 s; restart reconnected 3.8 s after spawn; remote request after reconnect golden |

## 2. Pairing through the desktop UI alone (commit 68e73f0)

Setup: two `ucompute serve` daemons with networking **off** (`--port 18311 --listen-port 18411` and `--port 18312 --listen-port 18412`, fresh UCOMPUTE_HOMEs), the desktop UI from `pnpm dev` (Vite, port 1420) in two tabs of the app's built-in browser, `?daemon=http://127.0.0.1:18311` and `?daemon=http://127.0.0.1:18312` (dev-only parameter, loopback only). Every step below was a click or keystroke in the UI; no API call was made by hand.

1. Device A: Peers → "Let this Mac talk to paired devices" on, then "Run models for paired devices" on. **Defect found here:** the second switch failed with "Address already in use (os error 48)" and turned networking off. Root cause and fix in 68e73f0 (the endpoint restart raced the old socket's release on the fixed port); regression test `network_settings_change_while_enabled_rebinds_the_fixed_port_single_host`. After the fix both switches stay on and A listens on 0.0.0.0:18411.
2. Device B: networking on; name "Mac B".
3. Device A: "Pair this device" → code `851 328`, "Expires in 118 s. Address 192.168.1.176:50301".
4. Device B: Join a device with that address and the wrong code `851 329` → "Pairing did not complete. Check the code and that both devices are on the same network, then try again." (`pairing_failed`).
5. Device B: the correct code → "Paired with Mac A. Add it to a group to let it run work for this Mac." Paired devices: Mac A, Health **Connected**.
6. Device A shows "Paired with Mac B." and lists Mac B as Connected.
7. API cross-check afterwards: each daemon's `GET /api/peers` lists the other pinned with `health: connected`; A's audit: `network_changed`, `network_changed`, `pairing_started`, `pairing_failed`, `pairing_succeeded`, `peer_connected`.

Limits: this is the web build in browser tabs, not two packaged desktop app installs on two machines.
