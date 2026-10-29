#!/bin/bash
# Start, check or collect the local soak (tests/soak/local_soak.py) on the Linux host, detached from SSH: the
# daemon and the soak run as their own sessions (setsid nohup, pid files in ~/soak), so a dropped SSH session or this
# Mac sleeping does not end them. (Transient systemd user units did not survive: with lingering off and no desktop
# login, the user manager stops them when the last SSH session closes, 2026-10-29.) The daemon is `target/release/common` in ~/src/common (sync and build it first), with its own home
# ~/.common-soak (engine and models linked from ~/.common-acceptance), port 18500 and no peer endpoint.
#
#   LINUX_HOST=<ip> LINUX_USER=<user> tests/soak/run_soak_linux.sh start [HOURS [local_soak.py args...]]   # default 24
#   LINUX_HOST=<ip> LINUX_USER=<user> tests/soak/run_soak_linux.sh status
#   LINUX_HOST=<ip> LINUX_USER=<user> tests/soak/run_soak_linux.sh collect <local dir>   # also stops the soak and its daemon
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${LINUX_HOST:?set LINUX_HOST}" "${LINUX_USER:?set LINUX_USER}"
export MAC_HOST="$LINUX_HOST" MAC_USER="$LINUX_USER" MAC_KEY="${LINUX_KEY:-$HOME/.ssh/id_ed25519}"
H=scripts/mac-host.sh
case "${1:-}" in
  start)
    hours="${2:-24}"
    extra="${*:3}"  # more local_soak.py arguments, e.g. --only overload
    $H put tests/soak/local_soak.py soak/local_soak.py
    # shellcheck disable=SC2016  # expanded on the Linux host
    $H run 'set -e; S=$HOME/.common-soak; cd ~/src/common; test -x target/release/common
      alive() { test -s "$1" && kill -0 "$(cat "$1")" 2>/dev/null; }
      alive ~/soak/daemon.pid && { echo "a soak is already running"; exit 1; }
      rm -rf "$S"; mkdir -p "$S/models" ~/soak
      ln -s ~/.common-acceptance/backends "$S/backends"
      ln -s ~/.common-acceptance/models/blobs "$S/models/blobs"
      ln -s ~/.common-acceptance/models/manifests "$S/models/manifests"
      target/release/common --version > ~/soak/version.txt
      COMMON_HOME="$S" COMMON_MDNS=0 setsid nohup "$PWD/target/release/common" serve --port 18500 \
        >~/soak/daemon.log 2>&1 </dev/null & echo $! > ~/soak/daemon.pid
      for _ in $(seq 40); do curl -s -m1 127.0.0.1:18500/health >/dev/null && break; sleep 0.5; done
      pid=$(cat ~/soak/daemon.pid)
      rm -f ~/soak/soak.jsonl
      setsid nohup /usr/bin/python3 -I "$HOME/soak/local_soak.py" --pid "$pid" \
        --home "$S" --log "$HOME/soak/daemon.log" --hours '"$hours"' '"$extra"' --out "$HOME/soak/soak.jsonl" \
        >~/soak/soak.out 2>&1 </dev/null & echo $! > ~/soak/soak.pid
      echo "soak started: daemon pid $pid, $(cat ~/soak/version.txt), '"$hours"' h"'
    ;;
  status)
    # shellcheck disable=SC2016
    $H run 'for n in daemon soak; do f=~/soak/$n.pid; if test -s $f && kill -0 "$(cat $f)" 2>/dev/null; then echo "$n running"; else echo "$n not running"; fi; done; uptime
      tail -1 ~/soak/soak.jsonl 2>/dev/null | cut -c1-400'
    ;;
  collect)
    out="${2:?usage: run_soak_linux.sh collect <local dir>}"
    mkdir -p "$out"
    # shellcheck disable=SC2016  # expanded on the Linux host
    $H run 'for n in soak daemon; do f=~/soak/$n.pid; test -s $f && kill -TERM "$(cat $f)" 2>/dev/null; done
      sleep 3; pgrep -af "[l]lama-server" | grep -F .common-soak/backends || echo "no soak engine left"; rm -rf ~/.common-soak' |
      tee "$out/cleanup.txt"
    for f in soak.jsonl soak.out daemon.log version.txt; do $H get "soak/$f" "$out/$f"; done
    tail -1 "$out/soak.jsonl"
    ;;
  *)
    sed -n '2,10p' "$0"
    exit 2
    ;;
esac
