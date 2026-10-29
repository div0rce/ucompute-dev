#!/bin/bash
# Mac (A, this machine's installed app) <-> Windows laptop (B, its installed app): discovery, connection-state
# and mesh checks against the two apps' daemons.
# A daemon API run, not an app UI run: every step, pairing included, goes through the two daemons' loopback
# APIs and no window is driven. Not covered here (each needs its own manual UI evidence): pairing started in
# either app, a wrong code typed by hand, the Windows firewall consent step, the Mac-side group and run steps.
#
#   WIN_HOST=<laptop ip or name> OUT=<evidence dir> [NO_MDNS=1] [STEPS=...] [MESH=0] [HOLD=0] \
#     [CROSS_HOLD=1 [BIN=<ucompute binary>]] tests/distributed/two_machine/run_mac_windows.sh
#
# B's API (127.0.0.1:8192 on the laptop) is reached through an SSH tunnel on 18302, closed when the run ends;
# nothing on the laptop listens beyond loopback except UCompute's own authenticated peer transport. cdp.mjs
# needs its own tunnel to the app's WebView2 debugging port, opened by hand and closed after use:
# ssh -f -N -L 19222:127.0.0.1:9222 <user>@<laptop>.
# B is quit, crashed and started through the scheduled tasks UComputeTestQuit (taskkill without /f, like
# closing the window), Stop-Process -Force and UComputeTestLaunch, which run in the signed-in desktop session
# (setup: docs/environment/windows-test-host.md). The quit step passes only when A saw B's daemon close the
# connection itself (peer_lost "closed_by_peer"), not the app's kill fallback.
# CROSS_HOLD=1 starts two throwaway daemons on this Mac (BIN, default the installed app's `ucompute`; fresh
# UCOMPUTE_HOMEs that link this checkout's .ucompute engines and models; peer endpoints on 18431/18432, which B
# must reach) for the hold driver's cross-hold step; they are stopped and their homes deleted at the end.
# A driver whose step FAILed (exit 3) does not stop the run; the run then exits 1. A driver that stops (exit 1)
# ends the run. Whatever happens, both sides are put back as saved (state.py), with B's app started and the
# tunnel reopened first when they are down; an incomplete restore exits 1 and says how to finish it.
set -euo pipefail  # a crashed driver must stop the run, not hide behind tee
cd "$(dirname "$0")/../../.."
: "${WIN_HOST:?set WIN_HOST}" "${OUT:?set OUT}"
export WIN_HOST
KEY="${WIN_KEY:-$HOME/.ssh/ucompute_hp_ed25519}"
USER_="${WIN_USER:-jessg}"
H=scripts/windows-host.sh
mkdir -p "$OUT"
rm -f "$OUT/discovery/b_host"

tunnel() {  # (re)open the tunnel to B's API, at the address the wifi step last saw B at (b_host)
  if [ -s "$OUT/discovery/b_host" ]; then WIN_HOST=$(cat "$OUT/discovery/b_host"); fi
  pkill -f "1830[2]:127.0.0.1:8192" 2>/dev/null || true
  ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 \
    -o ExitOnForwardFailure=yes -f -N -L 18302:127.0.0.1:8192 "$USER_@$WIN_HOST" || return 1
  sleep 1
  curl -s -m 5 127.0.0.1:18302/health >/dev/null || { echo "B's API is not answering through the tunnel"; return 1; }
}
tunnel || exit 1

QUIT="$H run 'schtasks /run /tn UComputeTestQuit'"
CRASH="$H ps 'Stop-Process -Name ucompute-desktop -Force'"
START="$H run 'schtasks /run /tn UComputeTestLaunch'"
PROCS="$H ps '@(Get-Process ucompute,llama-server -ErrorAction SilentlyContinue).Count'"
WIFI="$H run 'schtasks /run /tn UComputeTestWifi'"  # disconnects B's Wi-Fi and rejoins the same profile (detached on B)
MDNS_FLAG=""
[ "${NO_MDNS:-}" = 1 ] && MDNS_FLAG="--no-mdns"

# shellcheck disable=SC2329  # run by the EXIT trap
cleanup() {  # on every exit: stop a driver still running (SIGTERM reaches bash only), then put both sides back
  rc=$?
  set +e
  pkill -P $$ 2>/dev/null
  while pgrep -P $$ >/dev/null; do sleep 0.2; done
  curl -s -m 5 127.0.0.1:18302/health >/dev/null || tunnel
  if ! curl -s -m 5 127.0.0.1:18302/health >/dev/null; then  # B's app stopped (an abort while it was away)
    eval "$START"
    for _ in $(seq 30); do sleep 2; tunnel >/dev/null 2>&1 && break; done
  fi
  python3 -I tests/distributed/two_machine/state.py restore "$OUT/state-before.json" || {
    echo "RESTORE INCOMPLETE: once B answers, rerun python3 -I tests/distributed/two_machine/state.py restore $OUT/state-before.json" >&2
    rc=1
  }
  pkill -f "1830[2]:127.0.0.1:8192" 2>/dev/null  # B's API stays reachable from this Mac no longer than the run
  [ -n "${XH:-}" ] && rm -rf "$XH"
  exit "$rc"
}
# Put back what the runs change (networking, Sharing, holds, pairings, groups, privacy, models, names).
python3 -I tests/distributed/two_machine/state.py save "$OUT/state-before.json" http://127.0.0.1:8192 http://127.0.0.1:18302
trap cleanup EXIT

XHOLD=""
if [ "${CROSS_HOLD:-}" = 1 ]; then
  BIN="${BIN:-/Applications/UCompute.app/Contents/MacOS/ucompute}"
  XH=$(mktemp -d "${TMPDIR:-/tmp}/ucompute-xhold.XXXXXX")
  xdaemon() {  # name api-port peer-port: a throwaway daemon (a child of this script, so cleanup stops it)
    mkdir -p "$XH/$1/models"
    ln -s "$PWD/.ucompute/backends" "$XH/$1/backends"
    ln -s "$PWD/.ucompute/models/blobs" "$XH/$1/models/blobs"
    ln -s "$PWD/.ucompute/models/manifests" "$XH/$1/models/manifests"
    UCOMPUTE_HOME="$XH/$1" "$BIN" serve --port "$2" --network --serve-peers --listen-port "$3" --no-mdns \
      >"$OUT/xhold-$1.log" 2>&1 &
    for _ in $(seq 60); do curl -s -m 1 "127.0.0.1:$2/health" >/dev/null && return 0; sleep 0.5; done
    echo "throwaway daemon $1 did not answer: $OUT/xhold-$1.log"
    return 1
  }
  xdaemon c 18331 18431 || exit 1
  xdaemon d 18332 18432 || exit 1
  XHOLD="--c http://127.0.0.1:18331 --d http://127.0.0.1:18332"
fi

FAILS=0
driver() {  # name args...: a FAILed step (exit 3) is counted and the run goes on; any other failure stops it
  python3 -I "tests/distributed/two_machine/$1_two_machine.py" --a http://127.0.0.1:8192 --b http://127.0.0.1:18302 \
    --out "$OUT/$1" "${@:2}" | tee "$OUT/$1-console.txt" || { r=$?; [ "$r" = 3 ] || exit "$r"; FAILS=1; }
}

# shellcheck disable=SC2086 # MDNS_FLAG and XHOLD are empty or whole flags
driver discovery --steps "${STEPS:-state,manual,pair,quit,crash}" $MDNS_FLAG \
  --bogus-addr "${BOGUS_ADDR:-192.0.2.1:8193}" --watch "${WATCH:-120}" \
  --quit-b "$QUIT" --crash-b "$CRASH" --start-b "$START" --procs-b "$PROCS" --wifi-cycle-b "$WIFI"

if [ "${MESH:-1}" = 1 ]; then
  tunnel || exit 1
  # shellcheck disable=SC2086
  driver mesh --kill-b "$CRASH" --start-b "$START" $MDNS_FLAG
fi

if [ "${HOLD:-1}" = 1 ]; then
  tunnel || exit 1
  # shellcheck disable=SC2086
  driver hold $XHOLD
fi
exit "$FAILS"
