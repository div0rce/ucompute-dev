#!/usr/bin/env bash
# Copy this import kit into an existing UCompute repository without overwriting anything.
#   - files absent in the repo are copied as-is
#   - files that exist and differ are written beside the original as <name>.claude-import
#   - identical files are skipped
# Nothing is committed, deleted, or pushed. Prints a summary and the list of conflicts.
#
# Usage: scripts/import-into-repo.sh /path/to/ucompute-repo
set -euo pipefail
KIT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEST="${1:?usage: $0 /path/to/repo}"
[ -d "$DEST" ] || { echo "not a directory: $DEST" >&2; exit 2; }
DEST="$(cd "$DEST" && pwd)"
[ "$DEST" != "$KIT" ] || { echo "destination is the kit itself" >&2; exit 2; }

copied=0; skipped=0; conflicts=()
while IFS= read -r -d '' src; do
  rel="${src#"$KIT"/}"
  case "$rel" in IMPORT.md|*/__pycache__/*|*.pyc) continue ;; esac
  dst="$DEST/$rel"
  mkdir -p "$(dirname "$dst")"
  if [ ! -e "$dst" ]; then
    cp -p "$src" "$dst"; copied=$((copied+1))
  elif cmp -s "$src" "$dst"; then
    skipped=$((skipped+1))
  else
    cp -p "$src" "$dst.claude-import"; conflicts+=("$rel")
  fi
done < <(find "$KIT" -type f -print0)

echo "copied: $copied  identical/skipped: $skipped  conflicts: ${#conflicts[@]}"
for c in "${conflicts[@]+"${conflicts[@]}"}"; do echo "  merge needed: $c  (new version at $c.claude-import)"; done
echo "next: cd \"$DEST\" && python3 scripts/validate_registry.py && scripts/inspect-env.sh"
