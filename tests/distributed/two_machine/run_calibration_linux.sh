#!/bin/bash
# Run the Core Lab's local-versus-remote calibration (ucompute-core-lab calibration/RUNBOOK.md) with this Mac's
# installed app as host A and the Linux host's daemon as host B. This script only sets up and puts back what the
# driver needs (B's daemon and tunnel, a pairing, a group allowing remote inference on both sides); it runs the
# lab's driver unmodified and does not analyse its data.
#
#   LINUX_HOST=<ip> LINUX_USER=<user> LAB=<ucompute-core-lab checkout> NETWORK=<label> \
#     tests/distributed/two_machine/run_calibration_linux.sh
set -euo pipefail
cd "$(dirname "$0")/../../.."
: "${LINUX_HOST:?set LINUX_HOST}" "${LINUX_USER:?set LINUX_USER}" "${LAB:?set LAB}" "${NETWORK:?set NETWORK}"
KEY="${LINUX_KEY:-$HOME/.ssh/id_ed25519}"
export MAC_HOST="$LINUX_HOST" MAC_USER="$LINUX_USER" MAC_KEY="$KEY"
H=scripts/mac-host.sh
A=http://127.0.0.1:8192 B=http://127.0.0.1:18304
OUT=$(mktemp -d "${TMPDIR:-/tmp}/common-calibration.XXXXXX")
# shellcheck disable=SC2016  # expanded on the Linux host
B_START='cd ~/src/common && test -x target/release/common && { pgrep -f "^target/release/common serve" >/dev/null ||
  { COMMON_HOME=$HOME/.common-acceptance nohup target/release/common serve --network --serve-peers -v \
    >>$HOME/.common-acceptance.log 2>&1 </dev/null & sleep 1; }; } && pgrep -f "^target/release/common serve"'
$H run "$B_START" >/dev/null
pkill -f "1830[4]:127.0.0.1:8192" 2>/dev/null || true
ssh -i "$KEY" -o IdentitiesOnly=yes -o BatchMode=yes -o ExitOnForwardFailure=yes -f -N -L 18304:127.0.0.1:8192 "$LINUX_USER@$LINUX_HOST"
for _ in $(seq 40); do curl -s -m 1 "$B/health" >/dev/null && break; sleep 0.5; done
python3 -I tests/distributed/two_machine/state.py save "$OUT/state-before.json" "$A" "$B"
cleanup() {
  rc=$?
  set +e
  python3 -I tests/distributed/two_machine/state.py restore "$OUT/state-before.json" || rc=1
  $H run 'pkill -TERM -f "^target/release/common serve"' >/dev/null 2>&1
  pkill -f "1830[4]:127.0.0.1:8192" 2>/dev/null
  echo "state saved and restored from $OUT"
  exit "$rc"
}
trap cleanup EXIT
id_of() { curl -s "$1/api/network" | python3 -c 'import sys,json; print(json.load(sys.stdin)["peer_id"])'; }
ida=$(id_of "$A"); idb=$(id_of "$B")
connected() { curl -s "$A/api/peers" | python3 -c 'import sys,json; sys.exit(0 if any(p["id"] == sys.argv[1] and p.get("connected") for p in json.load(sys.stdin)["peers"]) else 1)' "$idb"; }
if ! connected; then
  start=$(curl -s -X POST "$A/api/pair/start" -H 'content-type: application/json' -d '{}')
  join=$(python3 -c 'import sys,json; s=json.loads(sys.argv[1]); print(json.dumps({"addr": s["pair_addr"], "code": s["code"]}))' "$start")
  curl -s -X POST "$B/api/pair/join" -H 'content-type: application/json' -d "$join" | grep -q '"id"' || { echo "pairing failed"; exit 1; }
  for _ in $(seq 30); do connected && break; sleep 0.5; done
  connected || { echo "paired but not connected"; exit 1; }
fi
for pair in "$A $idb" "$B $ida"; do
  read -r url peer <<<"$pair"
  curl -s -X PUT "$url/api/groups" -H 'content-type: application/json' \
    -d "{\"name\":\"calibration\",\"members\":[\"$peer\"],\"policy\":{\"allow_remote_inference\":true}}" | grep -q calibration
done
(cd "$LAB" && python3 -I tools/test_calibrate.py)
(cd "$LAB" && caffeinate -i python3 -I tools/calibrate.py run --network "$NETWORK" --a "$A" --b "$B" --set-trusted-group)
