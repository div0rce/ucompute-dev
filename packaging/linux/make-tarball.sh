#!/bin/bash
# Build the Linux user-level tarball: the `ucompute` binary (built --locked), LICENSE, THIRD_PARTY_NOTICES (which also
# refuses a licence outside deny.toml), the systemd user unit and the install notes, plus SHA256SUMS. Run on the
# Linux host it targets:  packaging/linux/make-tarball.sh <out dir>
# Not a release: no signature; distribution needs the owner (spec section 4, M9).
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT="${1:?usage: make-tarball.sh <out dir>}"
[ "$(uname -s)" = Linux ] || { echo "build this on Linux"; exit 1; }
cargo build --release --locked -p ucompute-cli
ver=$(target/release/ucompute --version | awk '{print $2}')
name="ucompute-$ver-linux-$(uname -m)"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name"
cp target/release/ucompute LICENSE packaging/linux/ucompute.service packaging/linux/README.md "$stage/$name/"
python3 scripts/gen_notices.py --out "$stage/$name/THIRD_PARTY_NOTICES.txt"
(cd "$stage/$name" && sha256sum ucompute LICENSE THIRD_PARTY_NOTICES.txt ucompute.service README.md >SHA256SUMS)
mkdir -p "$OUT"
tar -C "$stage" -czf "$OUT/$name.tar.gz" "$name"
(cd "$OUT" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
echo "$OUT/$name.tar.gz"
