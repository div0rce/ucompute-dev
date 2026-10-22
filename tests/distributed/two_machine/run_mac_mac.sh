#!/bin/sh
# Mac (A, this machine's installed app) <-> second Mac (B, its installed app) acceptance run.
#
#   OUT=<evidence dir> [MAC_HOST=...] [NO_MDNS=1] [STEPS=...] tests/distributed/two_machine/run_mac_mac.sh
#
# B's API (127.0.0.1:8192 there) is reached through an SSH tunnel on 18303. B is quit with SIGTERM to
# its app (the Cmd+Q path), crashed with SIGKILL and started with `open -a`, all through
# scripts/mac-host.sh. Long runs keep B awake with `caffeinate -i` for their duration (a closed lid still
# sleeps a MacBook on battery).
set -eu
cd "$(dirname "$0")/../../.."
: "${OUT:?set OUT}"
HOST="${MAC_HOST:-192.168.1.115}"
USER_="${MAC_USER:-kylehebron}"
KEY="${MAC_KEY:-$HOME/.ssh/common_kyle_ed25519}"
H=scripts/mac-host.sh
mkdir -p "$OUT"

pkill -f "1830[3]:127.0.0.1:8192" 2>/dev/null || true
ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ServerAliveInterval=15 -f -N -L 18303:127.0.0.1:8192 "$USER_@$HOST"
sleep 1
curl -s -m 5 127.0.0.1:18303/health >/dev/null || { echo "B's API is not answering through the tunnel"; exit 1; }
# Keep B awake for the whole run (ends when this script's ssh session ends).
ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -f "$USER_@$HOST" "caffeinate -i -t 7200 >/dev/null 2>&1 &"

QUIT="$H run 'pkill -TERM -x common-desktop'"
CRASH="$H run 'pkill -KILL -x common-desktop'"
START="$H run 'open -a /Applications/Common.app'"
PROCS="$H run 'pgrep -f \"Common.app/Contents/MacOS/common serve|llama-server\" | wc -l | tr -d \" \"'"
MDNS_FLAG=""
[ "${NO_MDNS:-}" = 1 ] && MDNS_FLAG="--no-mdns"

python3 -I tests/distributed/two_machine/discovery_two_machine.py --a http://127.0.0.1:8192 --b http://127.0.0.1:18303 \
  --out "$OUT/discovery" --steps "${STEPS:-state,manual,pair,quit,crash}" $MDNS_FLAG \
  --bogus-addr "${BOGUS_ADDR:-192.0.2.1:8193}" --watch "${WATCH:-120}" \
  --quit-b "$QUIT" --crash-b "$CRASH" --start-b "$START" --procs-b "$PROCS" | tee "$OUT/discovery-console.txt"
