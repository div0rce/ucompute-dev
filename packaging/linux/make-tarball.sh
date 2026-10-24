#!/bin/bash
# Build the Linux user-level tarball: the `common` binary (built --locked), LICENSE, the systemd user unit and the
# install notes, plus SHA256SUMS. Run on the Linux host it targets:  packaging/linux/make-tarball.sh <out dir>
# Not a release: no third-party notices yet and no signature; distribution needs the owner (spec section 4, M9).
set -euo pipefail
cd "$(dirname "$0")/../.."
OUT="${1:?usage: make-tarball.sh <out dir>}"
[ "$(uname -s)" = Linux ] || { echo "build this on Linux"; exit 1; }
cargo build --release --locked -p common-cli
ver=$(target/release/common --version | awk '{print $2}')
name="common-$ver-linux-$(uname -m)"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/$name"
cp target/release/common LICENSE packaging/linux/common.service packaging/linux/README.md "$stage/$name/"
(cd "$stage/$name" && sha256sum common LICENSE common.service README.md >SHA256SUMS)
mkdir -p "$OUT"
tar -C "$stage" -czf "$OUT/$name.tar.gz" "$name"
(cd "$OUT" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
echo "$OUT/$name.tar.gz"
