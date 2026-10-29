# Mac and Linux on home Wi-Fi: the fixed builds on two machines (2026-10-23, 18:31-18:53 UTC)

> Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

**Two-machine evidence, daemon API run (not an app UI run).** The first physical run of the code of fix rounds 1-5.
The drivers in `tests/distributed/two_machine/` drive the two daemons' loopback APIs (the Linux one through an SSH
tunnel); no window is driven. The owner's home Wi-Fi carries mDNS, so discovery is checked, not N/A.

| | A | B |
|---|---|---|
| Machine | `sentinel`: Ubuntu 26.04 LTS x86_64, Intel Core i9-9900K, 14 GiB, Wi-Fi, 192.168.1.167 (`docs/environment/linux-test-host.md`) | MacBook Air, Apple M2, 8 GB, macOS 26.2, Wi-Fi, 192.168.1.176 |
| Software | `ucompute serve --network --serve-peers` built on sentinel from ce54da1 (mesh code identical to ffe1a47; ce54da1 adds the Linux engine pin, D-024), `UCOMPUTE_HOME=~/.ucompute-acceptance` (its own identity `edb3c91c…`) | the installed packaged app from ffe1a47 (daemon `afe3ea22…`, verified ALL PASS that day, `docs/evidence/desktop-packaged-mac-m2-ffe1a47.json`) |
| Engine | llama.cpp b11609 Ubuntu CPU build, provisioned and hash-verified by the daemon on first start | llama.cpp b11609 macOS arm64 (Metal) |

Command: `LINUX_HOST=192.168.1.167 LINUX_USER=div0rce OUT=<dir> CROSS_HOLD=1 tests/distributed/two_machine/run_mac_linux.sh`.
CROSS_HOLD starts two throwaway daemons C and D on the Mac (the installed app's `ucompute` binary, fresh homes), so the
cross-hold step involves A on Linux and B, C and D on the one Mac. Both sides were saved before and put back after each
run (`state.py`, PASS every time). Raw calls, step results and runner logs: `mesh-physical-2026-10-23-linux/run-*/`.

**Linux local inference (first Linux evidence, B-004):** on sentinel, before the runs, `stories15M-q4_0` pulled,
loaded on `cpu` (6 of 6 layers on CPU, n_ctx 128) and completed with the golden text (32 tokens, temperature 0;
1684 tok/s decode: a path check on the fixture, not performance).

## Final run (run-1845), with the runner fixed

- **Discovery driver: ALL PASS (2 N/A).** Each daemon names itself (`sentinel`, `mous`); each lists the other with
  routable addresses (A after 2.42 s: a freshly started daemon; B at once); "last seen" advances while connected; an
  unpaired id is refused before any packet; a dial at a dead hint falls through to a known address and saves nothing;
  the address B shows connects A to B; pairing both ways with a wrong code refused first and host names stored;
  graceful quit of B's app (SIGTERM): A sees B leave in 0.46 s with B's own close (`closed_by_peer`), drops B from nearby
  in 1.05 s (goodbye), B's daemon and engine end in 0.2 s, A explains why, a dial at a dead hint fails in 8.0 s and saves
  nothing, one audit line while B is away, the two reconnect 2.9 s after B starts; crash of B's app (SIGKILL): A sees
  it in 0.43 s (B's daemon, told by `--exit-with-parent`, closes its connections itself), B's daemon and engine end in
  2.13 s, one audit line, reconnect 2.65 s after start. N/A: two cached-record checks (A held no record
  of B after the crash).
- **Hold driver: ALL PASS (1 N/A).** Disconnect seen on both sides in under 20 ms; B's own redial is refused and B shows
  `peer_held` after 2.0 s; A's request refused locally before anything is sent; B's refused `peer_held`; no
  reconnection for 30 s and exactly one audit line; **turning Sharing off and on keeps the hold** (review finding
  trust-1, fixed in 3df9438: first shown on two machines); **Connect ends a hold that was in place** (held before:
  true; not exercised on hardware before); golden output again; revoking a held device; pairing again unheld;
  **revocations cross a hold both ways while the hold stays** (trust-5, ADR-0003 D7; with C and D on the Mac).
  N/A: "a proven refusal ends B's dial" (B knew no address for A but the pinned one).
- **Mesh driver: all PASS except one.** FAIL: "each daemon lists the other (mDNS)": right after the driver unpaired the
  two, A listed B but B's (the Mac's) nearby list stayed empty for the 15 s allowed (finding 2). The rest passed:
  pairing with a wrong code first; groups and privacy; fixture pulled and loaded on both; golden remote execution
  A → B and **B → A (the Linux CPU serving the Mac)**, JSON and streaming; `local_only` refused on the sender; no prompt
  text in either audit log; revocation, B refused, pairing again; B killed, A notices in 0.52 s, reconnect 3.8 s after
  restart, golden again (`run-1845/mesh/results.json`, steps 15-16; run-1838 gave 0.49 s and 3.7 s).

## Earlier runs the same afternoon (runner defects, fixed in 6de3487)

- run-1831 and run-1838: **mesh ALL PASS** both times (B listed A in mDNS at once). The discovery driver's state and
  manual steps FAILed because the runner did not pair the two first, and in run-1831 its process count also matched
  the cross-hold daemons; the hold driver was skipped because the mesh driver leaves the two unpaired. None of these is
  a product result.
- run-1825 never started (the runner's `pgrep -f` matched its own ssh shell).

## Findings (open)

1. In run-1831 the freshly started Linux daemon's first record of the Mac carried only IPv6 link-local addresses
   (`fe80::1`, the Mac's loopback link-local, and its Wi-Fi link-local); with a wait (run-1845) the complete record with
   the Mac's IPv4 address arrived within 2.42 s. Product follow-up (round 6): rank only addresses the endpoint can dial
   and report a link-local-only peer as having no reachable address.
2. The Mac's nearby list lacked the Linux peer for more than 15 s once in three mesh runs. Entries leave the list when
   the signed record is older than 300 s and peers re-sign every 120 s, so missed multicast announcements age a live
   peer out. Product follow-up (round 6): re-query actively.

Not covered: Windows (the laptop was offline), Mac-to-Mac, mixed protocol minors on hardware, the apps' windows.
