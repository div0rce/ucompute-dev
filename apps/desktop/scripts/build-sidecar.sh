#!/bin/sh
# Builds the Common daemon from THIS checkout, stages it as the Tauri sidecar, and checks that it
# accepts the exact arguments the app starts it with (src-tauri/daemon-args.txt).
#
# `pnpm tauri build` runs this first (tauri.conf.json > build.beforeBuildCommand), so a bundle can no
# longer ship a daemon left over from an earlier commit. That happened: an app built after
# `--exit-with-parent` was added bundled a daemon built before it, which exited with a usage error.
#
# Tauri sets TAURI_ENV_TARGET_TRIPLE for the target being bundled; without it the host triple is used.
set -eu
cd "$(dirname "$0")/.."
HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
TRIPLE="${TAURI_ENV_TARGET_TRIPLE:-$HOST_TRIPLE}"
ROOT="$(cd ../.. && pwd)"
if [ "$TRIPLE" = "$HOST_TRIPLE" ]; then
  cargo build --release --locked -p common-cli --manifest-path "$ROOT/Cargo.toml"
  BIN="$ROOT/target/release/common"
else
  cargo build --release --locked -p common-cli --target "$TRIPLE" --manifest-path "$ROOT/Cargo.toml"
  BIN="$ROOT/target/$TRIPLE/release/common"
fi
COMMON_BIN="$BIN" sh scripts/prepare-sidecar.sh "$TRIPLE"
STAGED="src-tauri/binaries/common-$TRIPLE"
if ! cmp -s "$BIN" "$STAGED"; then
  echo "build-sidecar: $STAGED differs from the binary just built ($BIN)" >&2
  exit 1
fi
if [ "$TRIPLE" = "$HOST_TRIPLE" ]; then
  # clap rejects an unknown argument before it reaches --help, so this fails exactly when the app's
  # arguments would make the daemon exit with a usage error at launch.
  # shellcheck disable=SC2046
  if ! "$STAGED" $(cat src-tauri/daemon-args.txt) --help >/dev/null; then
    echo "build-sidecar: the staged daemon rejects the app's arguments: $(cat src-tauri/daemon-args.txt)" >&2
    exit 1
  fi
  echo "build-sidecar: staged daemon accepts: $(cat src-tauri/daemon-args.txt)"
else
  echo "build-sidecar: cross target $TRIPLE; argument check skipped (cannot run it here)"
fi
