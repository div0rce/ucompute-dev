# Linux user-level install verified on sentinel (2026-10-24, 00:33 UTC) — SINGLE-HOST, EXPERIMENTAL

First check of the Linux install path of platform readiness plan M6: a tarball holding the `common` binary, LICENSE,
the systemd user unit `packaging/linux/common.service` and its install notes, verified from the tarball by
`packaging/linux/verify_linux_install.py` on one Linux machine. Not a release and not distributable yet: no
signature and no owner approval of the format (spec section 4, M9).

- Host: sentinel, Ubuntu 26.04 LTS x86_64, Intel Core i9-9900K, glibc 2.43, systemd user manager of a desktop
  session, lingering off (`docs/environment/linux-test-host.md`).
- Build: `packaging/linux/make-tarball.sh ~/pkg-out` in sentinel's `~/src/common` (the c811a90 tree, code equal to
  ce54da1, with this commit's `packaging/linux/` files copied in): `cargo build --release --locked -p common-cli`,
  `common 0.1.0`. Tarball `common-0.1.0-linux-x86_64.tar.gz`, sha256
  `ea682daee72bbecc5200468b99fec20a6428feb05a8321a7b96dbb2f6a044371`.
- Command: `python3 -I packaging/linux/verify_linux_install.py --tarball ~/pkg-out/common-0.1.0-linux-x86_64.tar.gz
  --out verify.json` (port 18600, a throwaway unit name, a fresh data folder; the shipped unit with only ExecStart's
  path and port changed and COMMON_HOME added).
- Result: **ALL PASS, 15 checks** (`linux-install-2026-10-24/verify-linux-install.json`): SHA256SUMS verifies; the
  unit starts and the daemon is healthy within 10 s; `common setup` provisions and verifies the pinned b11609 CPU
  engine and `ldd` resolves it; stories15M pulled, loaded on the CPU (6 of 6 layers) and completed to the golden text;
  after a SIGKILL of the daemon, systemd started a new one, healthy 2.67 s after the kill, and the killed daemon's
  engine was gone (the unit's default `KillMode=control-group`); golden again after the restart; `systemctl --user
  stop` freed the port and left no engine; removing the unit and binary kept the data folder.
- Rerun with notices (00:52 UTC): the tarball now also holds `THIRD_PARTY_NOTICES.txt` from
  `scripts/gen_notices.py` (191 Rust packages for x86_64-unknown-linux-gnu, every licence allowed by `deny.toml`;
  sha256 of the tarball `19b8ee61eef994886648589d778308a100227b6b002aeb80a39f0b65f493dd68`): ALL PASS again, 15
  checks (`linux-install-2026-10-24/verify-linux-install-notices.json`).
- With a lint step (01:10 UTC): `systemd-analyze --user verify` of the shipped unit reports nothing about it (a
  misspelt key in a copy was reported, so the check can fail); ALL PASS, 16 checks
  (`linux-install-2026-10-24/verify-linux-install-lint.json`), same tarball.
- **At 0b0c1a4 (about 04:40 local, 2026-10-24), with the round 6 and 7 runtime:** `mac-host.sh sync` of HEAD into sentinel's
  `~/src/common`, `make-tarball.sh` (191 packages in the notices), tarball sha256
  `a99a77f2bf2bda228acb2cf328e1d18f1b3c132c8119b9dad10654f1152b7d5d`: ALL PASS, 16 checks, the SIGKILL recovery
  in 2.67 s with the killed daemon's engine gone (`linux-install-2026-10-24/verify-linux-install-0b0c1a4.json`).
- Not covered: the documented install paths themselves (`~/.local/bin`, port 8192, the real unit name: the owner's
  runners use port 8192 on sentinel), lingering and boot start, other distributions, an older glibc, GPU engines,
  networking (Mesh) under the unit, and a daemon built from HEAD with round 6.

## Soak smoke, the same evening (pre-round-6 build)

`tests/soak/local_soak.py` (M5 step 1) ran for 5 minutes against `target/release/common serve --port 18500` built
from the c811a90 tree, with a fresh home and an engine kill every 10 cycles. Before the first kill: every step typed
and golden, golden p95 22 ms, 4 of 40 burst requests refused `overloaded`. After the first engine SIGKILL the daemon
never served again: every inference request returned a typed `backend_failed` and a reload did not help, and the
killed engine stayed a zombie (not reaped). Verdict FAIL ("1 engine kills not recovered within 30 s"). This is the
engine crash recovery round 6 adds (uncommitted at the time of this run); the soak is to be repeated on a build with it.
