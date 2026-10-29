# Linux test host "sentinel"

The owner's always-on Linux machine, made available on 2026-10-23 to the engineering session, the Core Lab and the
website session. It is the project's first Linux machine (B-004: Linux stays EXPERIMENTAL at most, spec §27.1).

| | |
|---|---|
| Host | `sentinel`, 192.168.1.167 on the owner's home network (Wi-Fi `wlo1`), always on |
| OS | Ubuntu 26.04 LTS, x86_64 |
| Hardware | Intel Core i9-9900K (8 cores, 16 threads), 14 GiB RAM, about 189 GB free, Intel UHD 630 (no GPU engine pinned) |
| Access | `ssh div0rce@192.168.1.167` with this Mac's `~/.ssh/id_ed25519` (key login only; the owner added the key). Host key ED25519 `SHA256:/h68X/E8lE3ZmoJV+AHqXITwaULbqGVXEoFmmZTnc7c` |
| Privileges | no passwordless sudo: nothing is installed system-wide; Rust (rustup, toolchain from `rust-toolchain.toml`) lives under `~/.cargo` |
| Network services | Avahi runs (mDNS); ufw is installed but disabled (owner's setting; not changed) |
| Engine | llama.cpp b11609 Ubuntu CPU build, pinned for `linux-x64` (D-024), provisioned and hash-verified by the daemon on first start |

## Driving it from this Mac

`scripts/mac-host.sh` drives it unchanged with `MAC_HOST=192.168.1.167 MAC_USER=div0rce MAC_KEY=~/.ssh/id_ed25519`:
`run '<command>'`, `put`, `get`, and `sync` (HEAD's tracked files into `~/src/ucompute`; stale tracked-file leftovers are
removed with the same guards as the Windows sync). Build: `scripts/mac-host.sh run 'cd ~/src/ucompute && cargo build
--release -p ucompute-cli'` (about 1.5 min).

Two-machine runs: `LINUX_HOST=192.168.1.167 LINUX_USER=div0rce OUT=<dir> [CROSS_HOLD=1]
tests/distributed/two_machine/run_mac_linux.sh`. The Linux daemon is A (`UCOMPUTE_HOME=~/.ucompute-acceptance`, its own
identity, started by the runner and stopped at the end); this Mac's installed app is B. A daemon API run, not an app
UI run. Both sides are saved before and restored after (`tests/distributed/two_machine/state.py`).

## Rules

- Engineering uses `~/src/ucompute`, `~/.ucompute-acceptance`, `~/.ucompute-acceptance.log`, `~/ucompute-build.log` and the
  daemon's ports (127.0.0.1:8192 API, UDP 8193 peer) during runs. The owner's own `~/repos` and other files are not
  touched. Other sessions use their own folders and ports.
- Nothing listens beyond loopback except UCompute's authenticated peer transport (UDP 8193) while a run is going; the
  runner stops the daemon at the end.
- No firewall, sudo or system-setting changes; those stay with the owner.
