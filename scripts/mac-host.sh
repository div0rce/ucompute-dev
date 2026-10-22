#!/bin/sh
# Drive a second Mac test host from this Mac over SSH (Remote Login on, this Mac's key in its
# ~/.ssh/authorized_keys; the host's owner grants and revokes that).
#
#   scripts/mac-host.sh run '<shell command>'      run a command (login shell PATH plus ~/.cargo/bin and Node)
#   scripts/mac-host.sh put <local file> <remote>   copy a file over the SSH channel (no sftp needed)
#   scripts/mac-host.sh get <remote> <local file>   copy a file back
#   scripts/mac-host.sh sync                        copy HEAD (tracked files only) to ~/src/common
#
# MAC_HOST (default 192.168.1.115), MAC_USER (default kylehebron) and MAC_KEY (default
# ~/.ssh/common_kyle_ed25519) select the host. Long jobs: wrap them in `caffeinate -i` and nohup so a
# sleeping lid or a dropped SSH session cannot cut them off.
set -eu
HOST="${MAC_HOST:-192.168.1.115}"
USER_="${MAC_USER:-kylehebron}"
KEY="${MAC_KEY:-$HOME/.ssh/common_kyle_ed25519}"
SSH_OPTS="-i $KEY -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15"
ENV='export PATH="$HOME/.cargo/bin:$HOME/.local/common-tools/node/bin:$PATH";'

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
    git archive --format=tar HEAD | ssh $SSH_OPTS "$USER_@$HOST" "mkdir -p src/common && tar -xf - -C src/common"
    echo "synced $(git rev-parse --short HEAD) to $HOST:~/src/common (uncommitted changes are not copied)"
    ;;
  *)
    sed -n '2,13p' "$0"
    exit 2
    ;;
esac
