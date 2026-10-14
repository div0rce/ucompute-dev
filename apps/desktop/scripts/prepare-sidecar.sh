#!/bin/sh
# Copies the Common daemon binary to where Tauri expects a sidecar:
#   src-tauri/binaries/common-<rust target triple>   (e.g. common-aarch64-apple-darwin)
# Usage: scripts/prepare-sidecar.sh [target-triple]     (COMMON_BIN overrides the source path)
set -eu
cd "$(dirname "$0")/.."
TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
SRC="${COMMON_BIN:-../../target/release/common}"
if [ ! -x "$SRC" ]; then
  echo "No daemon binary at $SRC." >&2
  echo "Build it first from the repo root:  cargo build --release -p common-cli" >&2
  exit 1
fi
mkdir -p src-tauri/binaries
cp "$SRC" "src-tauri/binaries/common-$TRIPLE"
chmod +x "src-tauri/binaries/common-$TRIPLE"
echo "Sidecar ready: src-tauri/binaries/common-$TRIPLE"
