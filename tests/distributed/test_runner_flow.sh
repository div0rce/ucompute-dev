#!/bin/sh
# Offline check of tests/distributed/two_machine/run_mac_windows.sh's control flow (SYNTHETIC: a scratch tree whose
# drivers and state.py are stubs that log and exit with a chosen code, `ssh` and `curl` replaced by no-ops on PATH;
# no host or daemon is contacted). A FAILed driver (exit 3) does not stop the others but the run exits 1; a stopped
# driver (exit 1) ends the run; the restore runs last, also after SIGTERM, once the driver still running is gone;
# CROSS_HOLD's throwaway daemons get to the hold driver and are stopped and deleted at the end.
#   sh tests/distributed/test_runner_flow.sh
set -eu
src="$(cd "$(dirname "$0")/../.." && pwd)/tests/distributed/two_machine/run_mac_windows.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/r/tests/distributed/two_machine" "$tmp/r/scripts" "$tmp/bin"
cp "$src" "$tmp/r/tests/distributed/two_machine/"
for n in discovery mesh hold state; do
  f="$tmp/r/tests/distributed/two_machine/$n.py"
  [ "$n" = state ] || f="$tmp/r/tests/distributed/two_machine/${n}_two_machine.py"
  cat > "$f" <<EOF
import os, sys, time
open(os.environ["LOG"], "a").write("$n " + " ".join(sys.argv[1:2] if "$n" == "state" else sys.argv[1:]) + "\n")
time.sleep(float(os.environ.get("SLOW_$n", "0")))
open(os.environ["LOG"], "a").write("$n done\n")
sys.exit(int(os.environ.get("EXIT_$n", "0")))
EOF
done
printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/ssh"
printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/curl"
printf '#!/bin/sh\necho $$ >>"$LOG.pids"\nwhile :; do sleep 1; done\n' > "$tmp/bin/fake-ucompute"  # runs until stopped
chmod +x "$tmp/bin/ssh" "$tmp/bin/curl" "$tmp/bin/fake-ucompute"
run() {  # env assignments...: run the runner, print its exit code
  ( cd "$tmp" && env PATH="$tmp/bin:$PATH" LOG="$tmp/log" WIN_HOST=x OUT="$tmp/out" TMPDIR="$tmp" "$@" \
      bash r/tests/distributed/two_machine/run_mac_windows.sh >"$tmp/console" 2>&1 && echo 0 || echo $? )
}
fail() { echo "FAIL: $1"; cat "$tmp/log" "$tmp/console"; exit 1; }

: > "$tmp/log"
[ "$(run EXIT_discovery=3)" = 1 ] || fail "a FAILed step must make the run exit 1"
grep -q '^hold done' "$tmp/log" || fail "a FAILed step must not stop the later drivers"
[ "$(tail -1 "$tmp/log")" = "state done" ] && grep -q '^state restore' "$tmp/log" || fail "restore must run last"

: > "$tmp/log"
[ "$(run EXIT_mesh=1)" = 1 ] || fail "a stopped driver must end the run with its code"
! grep -q '^hold' "$tmp/log" || fail "a stopped driver must stop the run"
grep -q '^state restore' "$tmp/log" || fail "restore must run after a stop"

: > "$tmp/log"
[ "$(run)" = 0 ] || fail "a clean run exits 0"

: > "$tmp/log"
(cd "$tmp" && exec env PATH="$tmp/bin:$PATH" LOG="$tmp/log" WIN_HOST=x OUT="$tmp/out" SLOW_discovery=4 \
    bash r/tests/distributed/two_machine/run_mac_windows.sh >"$tmp/console" 2>&1) &
pid=$!  # the runner itself (exec), as a `kill` or a harness timeout would signal it
sleep 2.5  # the driver starts after the tunnel (sleep 1) and runs 4 s
kill -TERM "$pid"
wait "$pid" || true
sleep 5  # a driver left running would log "done" by now
! grep -q '^discovery done' "$tmp/log" || fail "SIGTERM must stop the running driver before the restore"
grep -q '^state restore' "$tmp/log" || fail "SIGTERM must still restore"

: > "$tmp/log"
[ "$(run CROSS_HOLD=1 BIN="$tmp/bin/fake-ucompute")" = 0 ] || { cat "$tmp/console"; fail "a CROSS_HOLD run exits 0"; }
grep -q '^hold .*--c http://127.0.0.1:18331 --d http://127.0.0.1:18332' "$tmp/log" || fail "hold must get --c and --d"
[ "$(wc -l <"$tmp/log.pids")" -eq 2 ] || fail "two throwaway daemons must start"
for p in $(cat "$tmp/log.pids"); do ! kill -0 "$p" 2>/dev/null || fail "the throwaway daemons must be stopped"; done
! ls -d "$tmp"/ucompute-xhold.* >/dev/null 2>&1 || fail "the throwaway homes must be deleted"
echo "PASS runner flow"
