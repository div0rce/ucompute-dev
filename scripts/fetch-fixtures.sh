#!/usr/bin/env bash
# Fetch and verify the pinned llama.cpp b11609 (macos-arm64) archive and the two TinyStories test
# fixtures into the COMMON_HOME layout that crates/common-artifacts expects:
#   backends/llama.cpp/<archive>, backends/llama.cpp/b11609/, models/blobs/sha256-<hex>, models/manifests/<id>.json
# Idempotent: existing files are re-hashed and kept; a hash mismatch deletes the file and exits 1.
# Pins below must match catalog/backends.json and catalog/models.json.
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
COMMON_HOME="${COMMON_HOME:-$REPO/.common}"

ARCHIVE_NAME="llama-b11609-bin-macos-arm64.tar.gz"
ARCHIVE_URL="https://github.com/ggml-org/llama.cpp/releases/download/b11609/$ARCHIVE_NAME"
ARCHIVE_SHA="d03f28b5a5baa9cdfbb01db5ed22c695126c11c193326f14cdc51d001be10f4e"
ARCHIVE_DIR="llama-b11609"
RELEASE="b11609"

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

# manifest ID NAME FILE SHA BYTES URL LICENSE_URL SOURCE — write models/manifests/<id>.json if missing.
manifest() {
  local id="$1" name="$2" file="$3" sha="$4" bytes="$5" url="$6" license_url="$7"
  local out="$COMMON_HOME/models/manifests/$id.json"
  [ -f "$out" ] && return 0
  mkdir -p "$(dirname "$out")"
  cat > "$out" <<EOF
{
  "schema_version": 1,
  "id": "$id",
  "name": "$name",
  "file_name": "$file",
  "sha256": "$sha",
  "bytes": $bytes,
  "license": "unknown",
  "license_url": "$license_url",
  "source_url": "$url",
  "redistribution": "unknown",
  "imported_at_utc": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "chat": false,
  "tags": ["fixture"]
}
EOF
  echo "wrote manifest $id"
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

# --- model fixtures ---
S15_SHA="66967fbece6dbe97886593fdbb73589584927e29119ec31f08090732d1861739"
S15_URL="https://huggingface.co/ggml-org/models-moved/resolve/499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories15M-q4_0.gguf"
fetch "$S15_URL" "$S15_SHA" "$COMMON_HOME/models/blobs/sha256-$S15_SHA"
manifest stories15M-q4_0 "TinyStories 15M (Q4_0) — test fixture" stories15M-q4_0.gguf "$S15_SHA" 19077344 "$S15_URL" "https://huggingface.co/ggml-org/models-moved"

S260_SHA="270cba1bd5109f42d03350f60406024560464db173c0e387d91f0426d3bd256d"
S260_URL="https://huggingface.co/ggml-org/models/resolve/499bc8821c6b12b4e53c5bffcb21ec206f212d81/tinyllamas/stories260K.gguf"
fetch "$S260_URL" "$S260_SHA" "$COMMON_HOME/models/blobs/sha256-$S260_SHA"
manifest stories260K "TinyStories 260K (F32) — split-test fixture" stories260K.gguf "$S260_SHA" 1185376 "$S260_URL" "https://huggingface.co/ggml-org/models"

echo "fixtures ready in $COMMON_HOME"
