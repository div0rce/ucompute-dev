#!/usr/bin/env bash
# Fetch and verify this platform's pinned llama.cpp archive (read from catalog/backends.json: macos-arm64,
# linux-x64) and the test models (catalog/models.json tags "fixture", plus "test-suite" under COMMON_REQUIRE_FIXTURES=1)
# into the COMMON_HOME layout that crates/common-artifacts expects:
#   backends/llama.cpp/<archive>, backends/llama.cpp/<release>/, models/blobs/sha256-<hex>, models/manifests/<id>.json
# Idempotent: existing files are re-hashed and kept; a hash mismatch deletes the file and exits 1.
# Windows pins are .zip releases this script does not extract: there, run `common setup --fixtures`.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
COMMON_HOME="${COMMON_HOME:-$REPO/.common}"

# This platform as crates/common-artifacts' current_platform() names it.
case "$(uname -s)" in
  Darwin) os=macos ;;
  Linux) os=linux ;;
  MINGW* | MSYS* | CYGWIN*) os=win ;;
  *) os="$(uname -s)" ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) arch=arm64 ;;
  x86_64 | amd64) arch=x64 ;;
  *) arch="$(uname -m)" ;;
esac
PLATFORM="$os-$arch"

# Its llama.cpp pin, one field per line (directory may be empty).
pin="$(python3 -I -c '
import json, sys
pins = [b for b in json.load(open(sys.argv[1]))["backends"] if b["name"] == "llama.cpp" and b["platform"] == sys.argv[2]]
if pins:
    print("\n".join(pins[0][k] for k in ("archive", "url", "sha256", "directory", "release")))
' "$REPO/catalog/backends.json" "$PLATFORM")"
if [ -z "$pin" ]; then
  echo "catalog/backends.json pins no llama.cpp build for $PLATFORM" >&2
  exit 1
fi
{ read -r ARCHIVE_NAME; read -r ARCHIVE_URL; read -r ARCHIVE_SHA; read -r ARCHIVE_DIR; read -r RELEASE; } <<EOF
$pin
EOF
case "$ARCHIVE_NAME" in
  *.tar.gz) ;;
  *)
    echo "$PLATFORM pins $ARCHIVE_NAME, which this script does not extract: run \`common setup --fixtures\` instead" >&2
    exit 1
    ;;
esac

sha256() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1; else sha256sum "$1" | cut -d' ' -f1; fi
}

# fetch URL SHA DEST — download (if missing) and verify; mismatch deletes and fails.
fetch() {
  local url="$1" sha="$2" dest="$3"
  mkdir -p "$(dirname "$dest")"
  if [ ! -f "$dest" ]; then
    echo "downloading $(basename "$dest")"
    curl -fL --retry 3 --retry-delay 2 -C - -o "$dest.part" "$url"
    mv "$dest.part" "$dest"
  fi
  local actual; actual="$(sha256 "$dest")"
  if [ "$actual" != "$sha" ]; then
    echo "sha256 mismatch for $dest: got $actual, expected $sha; deleted" >&2
    rm -f "$dest"
    exit 1
  fi
  echo "verified $(basename "$dest")"
}

# manifest ID — write models/manifests/<id>.json from its catalog entry if missing, as `common models pull` does.
manifest() {
  local out="$COMMON_HOME/models/manifests/$1.json"
  [ -f "$out" ] && return 0
  mkdir -p "$(dirname "$out")"
  python3 -I -c '
import json, sys
m = next(m for m in json.load(open(sys.argv[1]))["models"] if m["id"] == sys.argv[2])
print(json.dumps({"schema_version": 1, "id": m["id"], "name": m["name"], "file_name": m["file_name"],
                  "sha256": m["sha256"], "bytes": m["bytes"], "license": m["license"],
                  "license_url": m.get("license_url"), "source_url": m["url"],
                  "redistribution": m.get("redistribution", "unknown"), "imported_at_utc": sys.argv[3],
                  "chat": m.get("chat", False), "tags": m.get("tags", [])}, indent=2, ensure_ascii=False))
' "$REPO/catalog/models.json" "$1" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > "$out.tmp"
  mv "$out.tmp" "$out"
  echo "wrote manifest $1"
}

# --- backend archive + extraction ---
fetch "$ARCHIVE_URL" "$ARCHIVE_SHA" "$COMMON_HOME/backends/llama.cpp/$ARCHIVE_NAME"
if [ ! -d "$COMMON_HOME/backends/llama.cpp/$RELEASE" ]; then
  tmp="$(mktemp -d "$COMMON_HOME/backends/llama.cpp/.extract-XXXXXX")"
  tar -xzf "$COMMON_HOME/backends/llama.cpp/$ARCHIVE_NAME" -C "$tmp"
  mv "$tmp/$ARCHIVE_DIR" "$COMMON_HOME/backends/llama.cpp/$RELEASE"
  rmdir "$tmp"
  echo "extracted backends/llama.cpp/$RELEASE"
fi

# --- model fixtures: catalog/models.json entries tagged "fixture" (the two TinyStories models) and, under
# COMMON_REQUIRE_FIXTURES=1, also those tagged "test-suite" (about 3 GB), which the strict test suite loads ---
tags=fixture
if [ "${COMMON_REQUIRE_FIXTURES:-}" = 1 ]; then tags="fixture test-suite"; fi
models="$(python3 -I -c '
import json, sys
want = set(sys.argv[2].split())
for m in json.load(open(sys.argv[1]))["models"]:
    if want & set(m.get("tags", [])):
        print(m["id"], m["sha256"], m["url"])
' "$REPO/catalog/models.json" "$tags")"
if [ -z "$models" ]; then
  echo "catalog/models.json tags no model with: $tags" >&2
  exit 1
fi
while read -r id sha url; do
  fetch "$url" "$sha" "$COMMON_HOME/models/blobs/sha256-$sha"
  manifest "$id"
done <<EOF
$models
EOF

echo "fixtures ready in $COMMON_HOME"
