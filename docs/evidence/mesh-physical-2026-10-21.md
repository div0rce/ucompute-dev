# Mesh fixes on two physical Macs, packaged app on Kyle's MacBook Pro (2026-10-21)

**Two-machine evidence, partial.** Two installed desktop apps built from 6ab9f8b on two different Macs on one home Wi-Fi network (192.168.1.0/24). Driver: `tests/distributed/two_machine/discovery_two_machine.py` (A = this Mac's daemon API, B = Kyle's daemon API through an SSH tunnel to its loopback port; peer traffic is Common's own QUIC transport). Raw API logs and results are in `mesh-physical-2026-10-21/`. Kyle's MacBook went to sleep twice during the runs (SSH timed out, pings answered only by its sleep proxy); every check that ran while it was asleep is marked **invalid** below and must be repeated. The Windows laptop was not on this network, so nothing here covers Windows.

| | A: owner's Mac ("mous") | B: Kyle's MacBook Pro |
|---|---|---|
| Machine | MacBook Air, Apple M2, 8 GB, macOS 26.2, 192.168.1.176 | MacBook Pro, Apple M2 Pro, 16 GB, macOS 15.7.9, 192.168.1.115 |
| App | `/Applications/Common.app` from 6ab9f8b (daemon sha256 `0e87970e…`, shell `47eca2cf…`), same bundle as B | built on B from 6ab9f8b (`pnpm tauri build`), installed per-user |

## Packaged app (on B)

`python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/Common_0.1.0_aarch64.dmg --built target/release --work <empty>` on Kyle's Mac: **ALL PASS** (15 checks, `packaged-app-kyle-m2pro.json`). Binaries arm64 and identical to the build; ad-hoc signature verifies (no Developer ID, B-008); the bundled daemon accepts the app's exact arguments; fresh-home install healthy in 1.49 s with the pinned engine downloaded; golden completion on `MTL0 (Apple M2 Pro)` at 551 and 585 tok/s (stories15M fixture: a path check, not performance); quit, restart, crash (daemon gone 0.73 s) and restart leave nothing running.

The full Rust suite on B (quiet, load 1.1): `COMMON_HOME=$PWD/.common COMMON_REQUIRE_FIXTURES=1 COMMON_MDNS=0 cargo test --workspace --no-fail-fast` → **409 passed, 0 failed, 2 ignored**, including `peer_flood_throttled_through_daemon_single_host`, which fails only on the loaded owner's Mac (docs/VERIFICATION.md 2026-10-21).

## Results

| Check | Result |
|---|---|
| Each daemon reports its own host name; no "This Mac" (E1) | PASS: "mous", "Kyle’s MacBook Pro"; dial addresses 192.168.1.176:8193 and 192.168.1.115:8193 |
| Each side sees the other with 1-2 routable addresses, best first (D2) | PASS both ways (one address each on this network) |
| "Last seen" advances while connected (E5) | PASS: 22:55:25Z → 22:56:25Z |
| Connect for an unpaired id is refused before any packet (requirement 8) | PASS: 403 in 9 ms, no audit line on B |
| Connect at a hint while already connected | PASS but weak: returned the open connection in < 1 ms without dialling; replaced by the dead-hint check below |
| No peer added on either side by manual connects | PASS |
| An unreachable paired device (the Windows laptop, not on this network) | PASS: `last_error` peer_lost "no answer … within 4 s" with a firewall hint; **0 audit lines in 150 s** (the 5f0c580 build wrote one per ~35 s: 567 lines for one device) |
| Graceful quit of B (E6) | PASS: A shows B disconnected after 0.26 s; B leaves A's nearby list after **1.04 s** (goodbye); B's daemon and engine gone after 0.54 s; A shows "no answer from 192.168.1.115:8193 within 4 s" |
| Connect at a dead hint while B is down (D3, requirement 8) | PASS: 502 peer_lost after 8.0 s (hint, then pinned address), pinned address unchanged |
| One audit line while B is away, not one per redial (E4) | PASS: 0 lines in 164 s |
| Crash of B: cached record never shown as connected | PASS, but B had already been down (see invalid) |
| Pairing, A shows the code | **FAIL (test setup)**: 409, a pairing window from the driver's first, aborted run was still open on A; there is no route to cancel a pairing window early (gap) |
| Pairing, B shows the code, A joins | **FAIL, cause not yet established**: "nothing answered at 192.168.1.115:61578" (B's random pairing port), with the new firewall hint. Candidates: the macOS application firewall on B blocking the new, differently signed build, or B already going to sleep. To be repeated with B awake and its firewall state read |
| B restarted, A reconnects (quit and crash) | **invalid**: SSH to B timed out (asleep); the restart command never ran |
| Crash cleanup on B | **invalid**: the kill command never reached B |

Re-pairing ran on the existing pair: both sides stayed connected (0.02 s) and kept their earlier names ("Kyle", "Mous"), because the 409 and the failed join pinned nothing new.

## Not covered here

Windows (laptop off this network), pairing initiated on Windows and the firewall consent step, interface and address changes (D1), Mac-side UI flows (all driving was through the daemon APIs and the driver), and anything about pooling (none exists).
