#!/bin/sh
# Offline check of scripts/mac-host.sh sync's keep-list (synthetic: a scratch repo, `ssh` replaced by a local
# `sh -c`, no host contacted). Files HEAD tracks must survive a staged rename, a non-ASCII path and a folder
# whose case changed on a case-insensitive disk; only the untracked file goes.
#   sh tests/distributed/test_sync_keep_list.sh
set -eu
src="$(cd "$(dirname "$0")/../.." && pwd)/scripts/mac-host.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q r && cd r
mkdir scripts UI
for i in $(seq 1 60); do echo "$i" > "f$i.txt"; done
echo x > UI/x.tsx; echo c > "café.md"; echo o > old.txt
# shellcheck disable=SC2016 # the ssh call is matched literally
sed -e 's|ssh \$SSH_OPTS "\$USER_@\$HOST"|sh -c|g' -e "s|/tmp/|$tmp/|g" "$src" > scripts/mac-host.sh
git add -A && git -c user.name=t -c user.email=t@t commit -qm init
git mv old.txt new.txt                       # staged, not committed: the index is not HEAD
mkdir -p src/common/ui                       # the folder's old case, already on disk
echo stale > src/common/stale.txt
sh scripts/mac-host.sh sync >/dev/null
for f in old.txt café.md f1.txt; do test -f "src/common/$f" || { echo "FAIL: deleted tracked $f"; exit 1; }; done
ls src/common/*/x.tsx >/dev/null 2>&1 || { echo "FAIL: deleted tracked UI/x.tsx"; exit 1; }
test ! -e src/common/stale.txt || { echo "FAIL: kept untracked stale.txt"; exit 1; }
echo "PASS sync keep-list"
