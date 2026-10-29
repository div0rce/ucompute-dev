#!/bin/bash
# Linux host (A, a `ucompute serve` daemon built from this checkout's HEAD) <-> this Mac (B, its installed app):
# discovery, connection-state, mesh and hold checks against the two daemons. A daemon API run, not an app UI run.
#
#   LINUX_HOST=<ip> LINUX_USER=<user> OUT=<evidence dir> [NO_MDNS=1] [STEPS=...] [MESH=0] [HOLD=0] \
#     [CROSS_HOLD=1 [BIN=<ucompute binary>]] tests/distributed/two_machine/run_mac_linux.sh
#
# Roles are swapped against run_mac_windows.sh: the Linux daemon is A, the Mac's app B. Since ce54da1 the catalog pins
# the llama.cpp CPU build for Linux (D-024), so both directions run (B -> A on the Linux CPU engine).
# A's API (127.0.0.1:8192 on the Linux host) is reached through an SSH tunnel on 18304, closed when the run ends.
# A's daemon runs as target/release/ucompute in ~/src/ucompute (matched by that start of its command line, never by
# the ssh shell whose command text contains it) with UCOMPUTE_HOME=~/.ucompute-acceptance (its own identity),
# started here when it is not running and stopped at the end. B (this Mac's app) is quit with SIGTERM to the app (the
# Cmd+Q path), crashed with SIGKILL and started with `open -a`. Exit codes and restore as in run_mac_windows.sh.
#
# SPLIT=1 runs WP-04 P1 instead (plan docs/design/wp04-implementation-plan-2026-10-23.md section 3; ADR-0005):
#
#   LINUX_HOST=192.168.1.167 LINUX_USER=div0rce SPLIT=1 OUT=docs/evidence/pool-two-machine-<date>/run-<HHMM> \
#     [LINK=wifi|wired] [MODELS=R1a,R1b] [LAB=<ucompute-core-lab checkout>] [H_PORT=18320] [H_LISTEN=18420] \
#     [R_IP=<the Linux host's IPv4 LAN address, default LINUX_HOST>] tests/distributed/two_machine/run_mac_linux.sh
#
# The head H is a dev daemon from this checkout (target/release/ucompute, built from HEAD with a clean tree; sync the
# same HEAD to the Linux host with scripts/mac-host.sh sync and build it there first: the driver's preflight compares
# ~/src/ucompute there and both daemons' /health commit with HEAD) with UCOMPUTE_HOME=.ucompute-split, hardlinked
# from .ucompute's models and backends (no extra disk). The Linux daemon A is the stage host R. The installed app is quit for the
# run and started again at the end; A's and H's state is saved and put back (state.py). The driver,
# split_two_machine.py, runs under caffeinate -dimsu and writes the run directory $OUT; with LAB set, the lab's
# scorer reads $OUT/lab-input.json into $OUT/lab-score.json.
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${LINUX_HOST:?set LINUX_HOST}" "${LINUX_USER:?set LINUX_USER}" "${OUT:?set OUT}"
KEY="${LINUX_KEY:-$HOME/.ssh/id_ed25519}"
mkdir -p "$OUT"
export MAC_HOST="$LINUX_HOST" MAC_USER="$LINUX_USER" MAC_KEY="$KEY"
H=scripts/mac-host.sh  # also drives a Linux host (ssh, a login shell's PATH plus ~/.cargo/bin)
A=http://127.0.0.1:18304 B=http://127.0.0.1:8192
# shellcheck disable=SC2016  # expanded on the Linux host
A_START='cd ~/src/ucompute && test -x target/release/ucompute && { pgrep -f "^target/release/ucompute serve" >/dev/null ||
  { UCOMPUTE_HOME=$HOME/.ucompute-acceptance nohup target/release/ucompute serve --network --serve-peers -v \
    >>$HOME/.ucompute-acceptance.log 2>&1 </dev/null & sleep 1; }; } && pgrep -f "^target/release/ucompute serve"'

tunnel() {
  pkill -f "1830[4]:127.0.0.1:8192" 2>/dev/null || true
  ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 \
    -o ExitOnForwardFailure=yes -f -N -L 18304:127.0.0.1:8192 "$LINUX_USER@$LINUX_HOST" || return 1
  for _ in $(seq 20); do curl -s -m 2 "$A/health" >/dev/null && return 0; sleep 0.5; done
  echo "A's API is not answering through the tunnel"; return 1
}
$H run "$A_START" >/dev/null || { echo "A's daemon did not start (build it: cargo build --release -p ucompute-cli)"; exit 1; }
tunnel || exit 1

if [ "${SPLIT:-}" = 1 ]; then
  HS="http://127.0.0.1:${H_PORT:-18320}" HOME_H="$PWD/.ucompute-split" APP_WAS_UP=0 HPID=""
  test -x target/release/ucompute || { echo "H is not built: cargo build --release -p ucompute-cli"; exit 1; }
  mkdir -p "$OUT/h" "$HOME_H"
  (cd .ucompute && pax -rwlk models backends "$HOME_H")  # hardlinks (B-010: the disk is tight)
  # shellcheck disable=SC2329  # run by the EXIT trap
  split_cleanup() {
    rc=$?
    set +e
    [ -f "$OUT/state-before.json" ] && { python3 -I tests/distributed/two_machine/state.py restore "$OUT/state-before.json" || rc=1; }
    [ -n "$HPID" ] && kill "$HPID" 2>/dev/null && wait "$HPID" 2>/dev/null
    $H run 'pkill -TERM -f "^target/release/ucompute serve"' >/dev/null 2>&1
    pkill -f "1830[4]:127.0.0.1:8192" 2>/dev/null
    [ "$APP_WAS_UP" = 1 ] && open -a /Applications/UCompute.app
    exit "$rc"
  }
  trap split_cleanup EXIT
  if pgrep -x ucompute-desktop >/dev/null; then
    APP_WAS_UP=1
    pkill -TERM -x ucompute-desktop
    for _ in $(seq 30); do curl -s -m 1 "$B/health" >/dev/null || break; sleep 1; done
  fi
  # shellcheck disable=SC2086
  UCOMPUTE_HOME="$HOME_H" target/release/ucompute serve --port "${H_PORT:-18320}" --network \
    --listen-port "${H_LISTEN:-18420}" $MDNS_FLAG -v >>"$OUT/h/daemon.log" 2>&1 </dev/null &
  HPID=$!
  for _ in $(seq 60); do curl -s -m 1 "$HS/health" >/dev/null && break; sleep 0.5; done
  curl -s -m 2 "$HS/health" >/dev/null || { echo "H's dev daemon did not answer: $OUT/h/daemon.log"; exit 1; }
  python3 -I tests/distributed/two_machine/state.py save "$OUT/state-before.json" "$A" "$HS"
  rc=0
  # shellcheck disable=SC2086
  caffeinate -dimsu python3 -I tests/distributed/two_machine/split_two_machine.py --a "$HS" --b "$A" --out "$OUT" \
    --r-sh "$H run" --r-ip "${R_IP:-$LINUX_HOST}" --h-home "$HOME_H" --h-log "$OUT/h/daemon.log" --link "${LINK:-wifi}" ${MODELS:+--models "$MODELS"} \
    | tee "$OUT/split-console.txt" || rc=$?
  if [ -n "${LAB:-}" ] && [ -s "$OUT/lab-input.json" ]; then
    python3 -I -B "$LAB/p1-predictions/score_p1.py" "$OUT/lab-input.json" >"$OUT/lab-score.json" || rc=1
  fi
  exit "$rc"
fi
curl -s -m 5 "$B/health" >/dev/null || { echo "B (this Mac's app) is not running"; exit 1; }

QUIT="pkill -TERM -x ucompute-desktop"
CRASH="pkill -KILL -x ucompute-desktop"
START="open -a /Applications/UCompute.app"
# B's own daemon (the app's exact arguments) and its engine only: the CROSS_HOLD daemons run the same binary.
PROCS="pgrep -f 'MacOS/ucompute serve --host 127.0.0.1 --port 8192|Application Support/UCompute/backends/.*llama-server' | wc -l | tr -d ' '"
MDNS_FLAG=""
[ "${NO_MDNS:-}" = 1 ] && MDNS_FLAG="--no-mdns"

# shellcheck disable=SC2329  # run by the EXIT trap
cleanup() {
  rc=$?
  set +e
  pkill -P $$ 2>/dev/null
  while pgrep -P $$ >/dev/null; do sleep 0.2; done
  curl -s -m 5 "$B/health" >/dev/null || { eval "$START"; for _ in $(seq 30); do sleep 1; curl -s -m 1 "$B/health" >/dev/null && break; done; }
  curl -s -m 5 "$A/health" >/dev/null || { $H run "$A_START" >/dev/null; tunnel; }
  python3 -I tests/distributed/two_machine/state.py restore "$OUT/state-before.json" || {
    echo "RESTORE INCOMPLETE: rerun python3 -I tests/distributed/two_machine/state.py restore $OUT/state-before.json" >&2
    rc=1
  }
  $H run 'pkill -TERM -f "^target/release/ucompute serve"' >/dev/null 2>&1  # the Linux host runs nothing of ours after the run
  pkill -f "1830[4]:127.0.0.1:8192" 2>/dev/null
  [ -n "${XH:-}" ] && rm -rf "$XH"
  exit "$rc"
}
python3 -I tests/distributed/two_machine/state.py save "$OUT/state-before.json" "$A" "$B"
trap cleanup EXIT
# The discovery and hold drivers need A and B paired and connected (the mesh driver unpairs them at its start and
# its own restore leaves them so); pairing made here is removed again by the final restore.
pair_ab() {
  local ida start join
  ida=$(curl -s "$A/api/network" | python3 -c 'import sys,json; print(json.load(sys.stdin)["peer_id"])')
  # Only B's `peers` count: a revocation of A from an earlier run also carries A's id.
  curl -s "$B/api/peers" | python3 -c 'import sys,json; sys.exit(0 if any(p["id"] == sys.argv[1] and p.get("connected") for p in json.load(sys.stdin)["peers"]) else 1)' "$ida" && return 0
  start=$(curl -s -X POST "$B/api/pair/start" -H 'content-type: application/json' -d '{}')
  join=$(python3 -c 'import sys,json; s=json.loads(sys.argv[1]); print(json.dumps({"addr": s["pair_addr"], "code": s["code"]}))' "$start")
  curl -s -X POST "$A/api/pair/join" -H 'content-type: application/json' -d "$join" | grep -q '"id"' || { echo "pairing A with B failed"; return 1; }
  for _ in $(seq 30); do
    curl -s "$B/api/peers" | python3 -c 'import sys,json; sys.exit(0 if any(p["id"] == sys.argv[1] and p.get("connected") for p in json.load(sys.stdin)["peers"]) else 1)' "$ida" && return 0
    sleep 0.5
  done
  echo "A and B paired but not connected"; return 1
}
pair_ab || exit 1

XHOLD=""
if [ "${CROSS_HOLD:-}" = 1 ]; then
  BIN="${BIN:-/Applications/UCompute.app/Contents/MacOS/ucompute}"
  XH=$(mktemp -d "${TMPDIR:-/tmp}/ucompute-xhold.XXXXXX")
  xdaemon() {
    mkdir -p "$XH/$1/models"
    ln -s "$PWD/.ucompute/backends" "$XH/$1/backends"
    ln -s "$PWD/.ucompute/models/blobs" "$XH/$1/models/blobs"
    ln -s "$PWD/.ucompute/models/manifests" "$XH/$1/models/manifests"
    UCOMPUTE_HOME="$XH/$1" "$BIN" serve --port "$2" --network --serve-peers --listen-port "$3" --no-mdns \
      >"$OUT/xhold-$1.log" 2>&1 &
    for _ in $(seq 60); do curl -s -m 1 "127.0.0.1:$2/health" >/dev/null && return 0; sleep 0.5; done
    echo "throwaway daemon $1 did not answer: $OUT/xhold-$1.log"; return 1
  }
  xdaemon c 18331 18431 || exit 1
  xdaemon d 18332 18432 || exit 1
  XHOLD="--c http://127.0.0.1:18331 --d http://127.0.0.1:18332"
fi

FAILS=0
driver() {
  python3 -I "tests/distributed/two_machine/$1_two_machine.py" --a "$A" --b "$B" \
    --out "$OUT/$1" "${@:2}" | tee "$OUT/$1-console.txt" || { r=$?; [ "$r" = 3 ] || exit "$r"; FAILS=1; }
}
# shellcheck disable=SC2086
driver discovery --steps "${STEPS:-state,manual,pair,quit,crash}" $MDNS_FLAG \
  --bogus-addr "${BOGUS_ADDR:-192.0.2.1:8193}" --watch "${WATCH:-120}" \
  --quit-b "$QUIT" --crash-b "$CRASH" --start-b "$START" --procs-b "$PROCS"
if [ "${MESH:-1}" = 1 ]; then
  tunnel || exit 1
  # shellcheck disable=SC2086
  driver mesh --kill-b "$CRASH" --start-b "$START" $MDNS_FLAG
fi
if [ "${HOLD:-1}" = 1 ]; then
  tunnel || exit 1
  pair_ab || exit 1
  # shellcheck disable=SC2086
  driver hold $XHOLD
fi
exit "$FAILS"
