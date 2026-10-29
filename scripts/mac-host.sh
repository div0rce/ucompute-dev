#!/bin/sh
# Drive a second Mac test host from this Mac over SSH (Remote Login on, this Mac's key in its
# ~/.ssh/authorized_keys; the host's owner grants and revokes that).
#
#   scripts/mac-host.sh run '<shell command>'      run a command (login shell PATH plus ~/.cargo/bin and Node)
#   scripts/mac-host.sh put <local file> <remote>   copy a file over the SSH channel (no sftp needed)
#   scripts/mac-host.sh get <remote> <local file>   copy a file back
#   scripts/mac-host.sh sync                        copy HEAD (tracked files only) to ~/src/ucompute and record
#                                                   its commit in ~/src/ucompute/.ucompute-sync-head (the build's
#                                                   /health commit there: no .git is copied)
#
# MAC_HOST (default 192.168.1.115), MAC_USER (default kylehebron) and MAC_KEY (default
# ~/.ssh/ucompute_kyle_ed25519) select the host. Long jobs: wrap them in `caffeinate -i` and nohup so a
# sleeping lid or a dropped SSH session cannot cut them off.
set -eu
HOST="${MAC_HOST:-192.168.1.115}"
USER_="${MAC_USER:-kylehebron}"
KEY="${MAC_KEY:-$HOME/.ssh/ucompute_kyle_ed25519}"
SSH_OPTS="-i $KEY -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15"
ENV='export PATH="$HOME/.cargo/bin:$HOME/.local/ucompute-tools/node/bin:$PATH";'

case "${1:-}" in
  run)
    # shellcheck disable=SC2086
    exec ssh $SSH_OPTS "$USER_@$HOST" "$ENV $2"
    ;;
  put)
    # shellcheck disable=SC2086
    exec ssh $SSH_OPTS "$USER_@$HOST" "mkdir -p \"\$(dirname '$3')\" && cat > '$3'" < "$2"
    ;;
  get)
    # shellcheck disable=SC2086
    exec ssh $SSH_OPTS "$USER_@$HOST" "cat '$2'" > "$3"
    ;;
  sync)
    cd "$(dirname "$0")/.."
    # shellcheck disable=SC2086
    git archive --format=tar HEAD | ssh $SSH_OPTS "$USER_@$HOST" "mkdir -p src/ucompute && tar -xf - -C src/ucompute"
    # Delete files HEAD does not track (a stale deleted source breaks the build); keep build outputs. The
    # keep-list is exactly the archive's file list (HEAD's tree, unquoted paths), compared ignoring case: on
    # case-insensitive APFS a file extracted into a folder whose case changed keeps the old folder's case.
    # shellcheck disable=SC2086
    git -c core.quotePath=false ls-tree -r --name-only HEAD | ssh $SSH_OPTS "$USER_@$HOST" 'cd src/ucompute && cat > /tmp/ucompute-sync-manifest &&
      find . -type f -not -path "./target/*" -not -path "./.ucompute/*" -not -path "./.ucompute-sync-head" -not -path "*/node_modules/*" \
        -not -path "./apps/desktop/src-tauri/binaries/*" -not -path "./apps/desktop/src-tauri/gen/*" -not -path "./apps/desktop/dist/*" |
      sed "s|^\./||" | awk "NR == FNR { keep[tolower(\$0)]; next } (tolower(\$0) in keep) == 0" /tmp/ucompute-sync-manifest - > /tmp/ucompute-sync-stale &&
      n=$(wc -l < /tmp/ucompute-sync-stale) && [ "$(wc -l < /tmp/ucompute-sync-manifest)" -ge 50 ] &&
      if [ "$n" -gt 200 ]; then echo "refusing to delete $n files"; exit 1; fi &&
      while IFS= read -r f; do rm -f -- "$f" && echo "removed stale $f"; done < /tmp/ucompute-sync-stale'
    # shellcheck disable=SC2086
    git rev-parse HEAD | ssh $SSH_OPTS "$USER_@$HOST" 'cat > src/ucompute/.ucompute-sync-head'
    echo "synced $(git rev-parse --short HEAD) to $HOST:~/src/ucompute (uncommitted changes are not copied)"
    ;;
  *)
    sed -n '2,14p' "$0"
    exit 2
    ;;
esac
