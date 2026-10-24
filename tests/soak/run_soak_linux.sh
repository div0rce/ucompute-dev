#!/bin/bash
# Start, check or collect the local soak (tests/soak/local_soak.py) on the Linux host, detached from SSH: the
# daemon and the soak run as transient systemd user units, so a dropped session or this Mac sleeping does not end
# them. The daemon is `target/release/common` in ~/src/common (sync and build it first), with its own home
# ~/.common-soak (engine and models linked from ~/.common-acceptance), port 18500 and no peer endpoint.
#
#   LINUX_HOST=<ip> LINUX_USER=<user> tests/soak/run_soak_linux.sh start [HOURS]   # default 24
#   LINUX_HOST=<ip> LINUX_USER=<user> tests/soak/run_soak_linux.sh status
#   LINUX_HOST=<ip> LINUX_USER=<user> tests/soak/run_soak_linux.sh collect <local dir>   # also stops both units
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${LINUX_HOST:?set LINUX_HOST}" "${LINUX_USER:?set LINUX_USER}"
export MAC_HOST="$LINUX_HOST" MAC_USER="$LINUX_USER" MAC_KEY="${LINUX_KEY:-$HOME/.ssh/id_ed25519}"
H=scripts/mac-host.sh
case "${1:-}" in
  start)
    hours="${2:-24}"
    $H put tests/soak/local_soak.py soak/local_soak.py
    # shellcheck disable=SC2016  # expanded on the Linux host
    $H run 'set -e; S=$HOME/.common-soak; cd ~/src/common; test -x target/release/common
      systemctl --user is-active -q common-soak-daemon && { echo "a soak is already running"; exit 1; }
      rm -rf "$S"; mkdir -p "$S/models" ~/soak
      ln -s ~/.common-acceptance/backends "$S/backends"
      ln -s ~/.common-acceptance/models/blobs "$S/models/blobs"
      ln -s ~/.common-acceptance/models/manifests "$S/models/manifests"
      target/release/common --version > ~/soak/version.txt
      systemd-run --user --unit common-soak-daemon --collect -E COMMON_HOME="$S" -E COMMON_MDNS=0 \
        -p StandardOutput=file:$HOME/soak/daemon.log -p StandardError=file:$HOME/soak/daemon.log \
        "$PWD/target/release/common" serve --port 18500
      for _ in $(seq 40); do curl -s -m1 127.0.0.1:18500/health >/dev/null && break; sleep 0.5; done
      pid=$(systemctl --user show -p MainPID --value common-soak-daemon)
      rm -f ~/soak/soak.jsonl
      systemd-run --user --unit common-soak --collect -p StandardOutput=file:$HOME/soak/soak.out \
        -p StandardError=file:$HOME/soak/soak.out /usr/bin/python3 -I "$HOME/soak/local_soak.py" --pid "$pid" \
        --home "$S" --log "$HOME/soak/daemon.log" --hours '"$hours"' --out "$HOME/soak/soak.jsonl"
      echo "soak started: daemon pid $pid, $(cat ~/soak/version.txt), '"$hours"' h"'
    ;;
  status)
    # shellcheck disable=SC2016
    $H run 'systemctl --user is-active common-soak-daemon common-soak || true; uptime
      tail -1 ~/soak/soak.jsonl 2>/dev/null | cut -c1-400'
    ;;
  collect)
    out="${2:?usage: run_soak_linux.sh collect <local dir>}"
    mkdir -p "$out"
    $H run 'systemctl --user stop common-soak 2>/dev/null; systemctl --user stop common-soak-daemon 2>/dev/null
      sleep 1; pgrep -af "[l]lama-server" | grep -F .common-soak/backends || echo "no soak engine left"; rm -rf ~/.common-soak' |
      tee "$out/cleanup.txt"
    for f in soak.jsonl soak.out daemon.log version.txt; do $H get "soak/$f" "$out/$f"; done
    tail -1 "$out/soak.jsonl"
    ;;
  *)
    sed -n '2,10p' "$0"
    exit 2
    ;;
esac
