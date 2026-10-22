#!/bin/sh
# Mac (A, this machine's installed app) <-> Windows laptop (B, its installed app) acceptance run.
#
#   WIN_HOST=<laptop ip or name> OUT=<evidence dir> [NO_MDNS=1] [STEPS=...] tests/distributed/two_machine/run_mac_windows.sh
#
# B's API (127.0.0.1:8192 on the laptop) and the app's WebView2 debugging port (127.0.0.1:9222) are
# reached through SSH tunnels on 18302 and 19222; nothing on the laptop listens beyond loopback except
# Common's own authenticated peer transport. B is quit, crashed and started through the scheduled tasks
# CommonTestQuit (taskkill without /f, like closing the window), Stop-Process -Force and CommonTestLaunch,
# which run in the signed-in desktop session (setup: docs/environment/windows-test-host.md).
set -eu
cd "$(dirname "$0")/../../.."
: "${WIN_HOST:?set WIN_HOST}" "${OUT:?set OUT}"
export WIN_HOST
KEY="${WIN_KEY:-$HOME/.ssh/common_hp_ed25519}"
USER_="${WIN_USER:-jessg}"
H=scripts/windows-host.sh
mkdir -p "$OUT"

pkill -f "1830[2]:127.0.0.1:8192" 2>/dev/null || true
pkill -f "1922[2]:127.0.0.1:9222" 2>/dev/null || true
ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ServerAliveInterval=15 -f -N \
  -L 18302:127.0.0.1:8192 -L 19222:127.0.0.1:9222 "$USER_@$WIN_HOST"
sleep 1
curl -s -m 5 127.0.0.1:18302/health >/dev/null || { echo "B's API is not answering through the tunnel"; exit 1; }

QUIT="$H run 'schtasks /run /tn CommonTestQuit'"
CRASH="$H ps 'Stop-Process -Name common-desktop -Force'"
START="$H run 'schtasks /run /tn CommonTestLaunch'"
PROCS="$H ps '@(Get-Process common,llama-server -ErrorAction SilentlyContinue).Count'"
MDNS_FLAG=""
[ "${NO_MDNS:-}" = 1 ] && MDNS_FLAG="--no-mdns"

python3 -I tests/distributed/two_machine/discovery_two_machine.py --a http://127.0.0.1:8192 --b http://127.0.0.1:18302 \
  --out "$OUT/discovery" --steps "${STEPS:-state,manual,pair,quit,crash}" $MDNS_FLAG \
  --bogus-addr "${BOGUS_ADDR:-192.0.2.1:8193}" --watch "${WATCH:-120}" \
  --quit-b "$QUIT" --crash-b "$CRASH" --start-b "$START" --procs-b "$PROCS" | tee "$OUT/discovery-console.txt"

if [ "${MESH:-1}" = 1 ]; then
  python3 -I tests/distributed/two_machine/mesh_two_machine.py --a http://127.0.0.1:8192 --b http://127.0.0.1:18302 \
    --out "$OUT/mesh" --kill-b "$CRASH" --start-b "$START" | tee "$OUT/mesh-console.txt"
fi
