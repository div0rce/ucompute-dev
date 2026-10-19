#!/bin/sh
# Drive the Windows test laptop from the Mac over SSH (setup: docs/environment/windows-test-host.md).
#
#   scripts/windows-host.sh run '<cmd.exe command>'          run a command (cmd.exe is the remote shell)
#   scripts/windows-host.sh ps  '<PowerShell script>'        run PowerShell without cmd.exe quoting
#   scripts/windows-host.sh cp  <local file> <C:/remote/path> copy a file to the laptop
#   scripts/windows-host.sh sync                              copy HEAD (tracked files only) to C:/Users/<user>/src/common
#
# WIN_HOST (default jess-hp.local), WIN_USER (default jessg) and WIN_KEY (default ~/.ssh/common_hp_ed25519)
# select the laptop. Nothing here changes the laptop's settings; it only runs what you pass.
set -eu
HOST="${WIN_HOST:-jess-hp.local}"
USER_="${WIN_USER:-jessg}"
KEY="${WIN_KEY:-$HOME/.ssh/common_hp_ed25519}"
SSH_OPTS="-i $KEY -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15"

case "${1:-}" in
  run)
    shift
    # shellcheck disable=SC2086
    exec ssh $SSH_OPTS "$USER_@$HOST" "$@"
    ;;
  ps)
    # Progress records arrive as CLIXML on stderr and drown the output; switch them off first.
    enc=$(printf '$ProgressPreference="SilentlyContinue"; %s' "$2" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\n')
    # shellcheck disable=SC2086
    exec ssh $SSH_OPTS "$USER_@$HOST" "powershell -NoProfile -NonInteractive -EncodedCommand $enc"
    ;;
  cp)
    # shellcheck disable=SC2086
    exec scp -q $SSH_OPTS "$2" "$USER_@$HOST:$3"
    ;;
  sync)
    cd "$(dirname "$0")/.."
    dest="C:/Users/$USER_/src/common"
    # Extract over the existing tree so target/ and node_modules/ survive (incremental builds); a file
    # deleted on the Mac stays on the laptop until the folder is removed by hand.
    # shellcheck disable=SC2086
    ssh $SSH_OPTS "$USER_@$HOST" "if not exist C:\\Users\\$USER_\\src\\common mkdir C:\\Users\\$USER_\\src\\common"
    # shellcheck disable=SC2086
    git archive --format=tar HEAD | ssh $SSH_OPTS "$USER_@$HOST" "tar -xf - -C $dest"
    echo "synced $(git rev-parse --short HEAD) to $HOST:$dest (uncommitted changes are not copied)"
    ;;
  *)
    sed -n '2,12p' "$0"
    exit 2
    ;;
esac
