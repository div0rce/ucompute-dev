#!/usr/bin/env python3
"""WP-04 P1: one model split between this Mac (H, the head: a dev daemon built from this commit) and the Linux
stage host (R, sentinel), with llama.cpp RPC carried only inside UCompute's authenticated QUIC connection
(plan docs/design/wp04-implementation-plan-2026-10-23.md section 3, ADR-0005). Configuration P only, at the lab's
split points. A daemon API run: no app window is driven.

    python3 -I split_two_machine.py --a <H's API> --b <R's API, through the SSH tunnel> --out <run dir> \
        --r-sh "scripts/mac-host.sh run" [--r-ip <R's IPv4 LAN address>] [--r-tree DIR] [--h-home DIR] \
        [--h-log FILE] [--r-home DIR] [--r-log FILE] [--link wifi|wired] [--models R1a,R1b] [--prereg FILE] \
        [--steps core,memory,capture,config_c,failures] [--r-start CMD --h-start CMD --h-kill CMD]

--r-sh is required (no default host). Every R-side command goes through it; an R command that exits non-zero (ssh
itself exits 255 when it cannot reach R) FAILs the step that needed it, never reads as a clean result.

Steps: preflight (P1.0 and every item of the lab's PREFLIGHT.md; a failed preflight sends no request), the
default-deny cases (P1.1: unpaired, remote-only groups, pooling allowed on H only, no acknowledgement), pairing, and
per model: two loads (P1.3), the session checks (P1.2), the three prompts with their counter checkpoints (P1.6), a
local request with the split loaded (P1.9) and the local references; then the audit join and the canary scan of
every log on both hosts (P1.10) and teardown (P1.11). That is `--steps core`, the default.

--steps also takes the steps below, in any order; they run after the core, in this order, the destructive one last,
and report in their own steps, never in the lab's lab-input.json. Without `core` the run is preflight, pairing and
groups, then these. They end with a canary scan and a teardown of their own (canary_scan_extra, teardown_extra).
Each uses the first model of --models unless it says otherwise.
  memory (P1.5, F-03-01, F-03-02, per model): `footprint -p` of llama-server, split head then a local full-model run
    with the same flags, on H; VmHWM of rpc-server and of a local full-model run on R. Passes when each host's peak
    is below its full-model peak by at least the bytes of the layers it does not host, -15 % (MEMORY_TOL), those
    bytes read here from the GGUF's tensor table (gguf_sizes, as `gguf::layer_bytes`). N/A on R when R lacks the
    model (OD-4).
  capture (P1.7): this script's own UDP relay on this Mac, on the real path. H is dialled through it with the
    existing API (POST /api/peers/<R>/disconnect, then connect with {"addr": "127.0.0.1:<relay>"}; /api/peers must
    then show that outbound connection, and R the relay's port), the weights and the three prompts cross it, every
    datagram goes to capture/datagrams.bin and is scanned for the canaries and a 64-byte weight pattern of R's range.
    Passes on zero hits, with the relay having carried at least the tunnel's bytes, and a positive control: the same
    relay, recording and scanner find every needle in a plaintext stream.
  config_c (P1.4): R1b with split_mode same_backend (H's CPU holds [0, k), R's CPU the rest and the output layer),
    3 x 64 tokens, against a local CPU run on H (POST /api/models/<id>/load {"device": "cpu"}) and on R. Tokens are
    reported as measured (OD-3), never a pass rule. The API returns no log-probabilities (C7, OD-2): N/A.
  failures (P1.8): the nine cases, each on a fresh session with a generation in flight. Passes on a typed error
    within 10 s, 5 s for cases 3-7, no request left, no ggml-rpc-server or listener on R and session_close with an
    outcome on each side whose daemon lives. Cases 8 and 9 kill a daemon, which writes no line, and case 9's client
    sees its socket end, not a typed error. They restart it with --r-start and --h-start, the runner's start commands.

What it exports follows the Core Lab's P1 contract (ucompute-core-lab d569c5b, p1-predictions/SCORING-INPUT.md
and PREFLIGHT.md): requests go to H's /v1/completions with stream, temperature 0, seed 42, stream_options
include_usage and the raw prompts.json text; per prompt, within one split session and with nothing in between, the
tunnel counters are read at `loaded`, after a max_tokens 1 request and after a max_tokens 65 request on the same
prompt, each 0.2 s after the stream's [DONE]; TTFT and TPOT are client-side times of the max_tokens 65 stream.
<out>/lab-input.json is the lab's input: python3 -I p1-predictions/score_p1.py <out>/lab-input.json.

Writes prereg.json (before P1.1), manifest.json, steps.json, raw.jsonl (every HTTP call and shell command), h/,
r/, tokens/, bytes/, capture/, failure/ and lab-input.json under --out. Exit 3 when a step FAILed. Stdlib only.
"""
import argparse
import bisect
import hashlib
import http.client
import ipaddress
import json
import os
import re
import selectors
import shlex
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
RESULTS = []
RAW = None
DRIVEN = "daemon API run (not app UI): pairing, groups, privacy and requests go through the two daemons' loopback APIs"
DECODE_STEPS = 64  # max_tokens 65 minus max_tokens 1: both requests prefill the same prompt
AUDIT_MAX = 1000  # /api/audit's largest n (mesh.rs audit_get)
SSH_PORT = 22  # R's sshd: the LAN probe's positive control, a port the run already reaches R on
RUN_START = None  # utc() before the run's first request; a full audit page must reach back past it
SESSIONS = set()  # the split sessions the driver saw open (H's /api/status at each checkpoint)
STEPS = ("core", "memory", "capture", "config_c", "failures")  # the order they run in, whatever --steps says
MEMORY_TOL = 0.15  # F-03-02's +/-15 %
FLIGHT_TOKENS = 900  # below a slot's n_ctx of 1024 (run-0820); at R1a's TPOT it outlasts every bound of P1.8
LOAD_WAIT = 300  # s until a split's first token (R1a loaded in 36 to 42 s in run-0820)
CAPTURE_KEEP = 32 << 20  # raw datagrams stay in the run folder up to this size (plan, evidence layout)
UNTYPED = (None, "transport", "unparsed")
# Shell commands by name. The R ones run through --r-sh (ssh); patterns carry [x] so pgrep never matches the shell
# that runs them.
R_CMDS = {
    "procs": "pgrep -af 'llama-serve[r]|ggml-rpc-serve[r]|local_soa[k]' || true",
    "rpc": "pgrep -af 'ggml-rpc-serve[r]' || true",
    "ss": "ss -ltnupH",
    "who": 'who; echo "ssh_client=${{SSH_CLIENT%% *}}"',
    # every logind session, `who` misses Wayland desktops (OD-10); fails when loginctl does
    "sessions": "l=$(loginctl list-sessions --no-legend) || exit 1; for s in $(echo \"$l\" | awk '{{print $1}}'); do "
                "loginctl show-session \"$s\" -p Id -p Name -p Class -p Type -p State -p Remote -p RemoteHost "
                "-p Service -p Seat; echo; done; "
                'echo "self=${{XDG_SESSION_ID:-}}"; echo "user=$(id -un)"; echo "ssh_client=${{SSH_CLIENT%% *}}"',
    "load": "cat /proc/loadavg",
    "top": "ps -eo pcpu,pid,comm --sort=-pcpu | head -6",
    "banner": "f=$(ls -t {home}/logs/rpc-server-*.log 2>/dev/null | head -1); "
              "[ -n \"$f\" ] && grep -h transport \"$f\" || true",
    "scan": "grep -rlF {pats} {home}/logs {home}/runs {log} 2>/dev/null || true",
    "proc_status": "cat /proc/{pid}/status",
    "argv": "ps -o command= -p {pid} || true",
    "kill_rpc": "pkill -9 -f 'ggml-rpc-serve[r]'",
    # SIGKILL of R's daemon, then the ms until no rpc-server is left, timed on R so ssh's latency is not in it
    "kill_daemon": "pkill -9 -f '^target/release/ucompute serve' || exit 1; t0=$(date +%s%N); "
                   "while pgrep -f 'ggml-rpc-serve[r]' >/dev/null; do "
                   "[ $(( ($(date +%s%N) - t0) / 1000000 )) -ge 3000 ] && break; sleep 0.02; done; "
                   "echo ms=$(( ($(date +%s%N) - t0) / 1000000 ))",
    "start": "{cmd}",
    # R's tree: the commit `scripts/mac-host.sh sync` recorded (or HEAD of a git checkout), then `path<TAB>blob hash`
    # of every file but build outputs and data homes, compared with H's HEAD
    "tree": "cd {tree} && {{ if [ -e .git ]; then git rev-parse HEAD; else cat .ucompute-sync-head; fi; }} && "
            "f=$(mktemp) && find . -type f -not -path './.git/*' -not -path './target/*' -not -path './.ucompute*' "
            "-not -path '*/node_modules/*' -not -path '*/__pycache__/*' -not -path './apps/desktop/src-tauri/binaries/*' "
            "-not -path './apps/desktop/src-tauri/gen/*' -not -path './apps/desktop/dist/*' | sed 's|^\\./||' "
            "| LC_ALL=C sort >\"$f\" && git hash-object --stdin-paths <\"$f\" | paste \"$f\" - && rm -f \"$f\"",
}
H_CMDS = {
    # Untracked files are left out: the runner creates its evidence folder inside the tree before preflight.
    "git": "git rev-parse HEAD && git status --porcelain --untracked-files=no",
    "tree": "git -c core.quotePath=false ls-tree -r HEAD",
    "procs": "pgrep -fl 'llama-serve[r]|ggml-rpc-serve[r]|cargo [b]uild|cargo [t]est|rust[c] |local_soa[k]"
             "|ucompute-deskto[p]' || true",
    "load": "sysctl -n vm.loadavg",
    "power": "pmset -g batt",
    "caffeinate": "pgrep -x caffeinate || true",
    "mem": "vm_stat",
    "top": "ps -Ao pcpu,pid,comm -r | head -6",
    "ping": "ping -c 5 -q {ip}",
    "nc": "nc -z -w 2 {ip} {port}; echo rc=$?",
    "footprint": "/usr/bin/footprint -p {pid}",
    "argv": "ps -o command= -p {pid} || true",
    "alive": "ps -o pid= -p {pid} || true",
    "run": "{cmd}",
}


def utc():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def log_raw(rec):
    RAW.write(json.dumps({"t": utc(), **rec}) + "\n")
    RAW.flush()


def call(base, method, path, body=None, timeout=120):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method, headers={"Content-Type": "application/json"})
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    except (urllib.error.URLError, OSError) as e:
        status, raw = 0, str(e).encode()
    ms = round((time.monotonic() - t0) * 1000, 1)
    try:
        val = json.loads(raw)
    except ValueError:
        val = raw.decode(errors="replace")
    log_raw({"base": base, "method": method, "path": path, "body": body, "status": status, "ms": ms, "response": val})
    return status, val, ms


def sse(base, body, timeout=900):
    """One streamed /v1/completions request, timed on this client as the lab's calibration does: ttft_ms from
    sending to the first chunk with non-empty text, done_ms to the [DONE] line."""
    req = urllib.request.Request(base + "/v1/completions", data=json.dumps(body).encode(), method="POST",
                                 headers={"Content-Type": "application/json"})
    out = {"status": 0, "text": "", "usage": None, "target": None, "placement": None, "error": None}
    first = done = None
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            out["status"] = r.status
            for raw in r:
                line = raw.decode().strip()
                if line == "data: [DONE]":
                    done = time.monotonic()
                    break
                if not line.startswith("data:"):
                    continue
                ev = json.loads(line[5:])
                out["error"] = ev.get("error") or out["error"]
                out["target"] = ev.get("target", out["target"])
                out["placement"] = ev.get("placement") or out["placement"]
                out["usage"] = ev.get("usage") or out["usage"]
                for ch in ev.get("choices") or []:
                    if ch.get("text"):
                        first = first or time.monotonic()
                        out["text"] += ch["text"]
    except urllib.error.HTTPError as e:
        out["status"] = e.code
        try:
            out["error"] = json.loads(e.read()).get("error")
        except ValueError:
            out["error"] = {"code": "unparsed"}
    except (urllib.error.URLError, OSError, ValueError) as e:
        out["error"] = {"code": "transport", "message": str(e)}
    out["ttft_ms"] = round((first - t0) * 1000, 1) if first else None
    out["done_ms"] = round((done - t0) * 1000, 1) if done else None
    log_raw({"base": base, "method": "POST", "path": "/v1/completions (stream)", "body": body, "response": out})
    return out


class Flight:
    """One streamed /v1/completions request on its own thread, so a failure can hit it mid-generation: the client's
    times of the first token, the error and the stream's end. A stream that stops without [DONE] and without an error
    event (its daemon died) ends in `transport`. cancel() ends the socket, as a client going away does."""

    def __init__(self, base, body, timeout=900):
        self.status, self.error, self.finish, self.tokens, self.sock, self.cancelled = 0, None, None, 0, None, False
        self.first_at = self.error_at = self.ended_at = None
        self.up, self.done = threading.Event(), threading.Event()
        threading.Thread(target=self.run, args=(base, body, timeout), daemon=True).start()

    def fail(self, error):
        self.error = self.error or error
        self.error_at = self.error_at or time.monotonic()

    def read(self, r):
        """The stream's events; True at [DONE]."""
        for raw in r:
            line = raw.decode().strip()
            if line == "data: [DONE]":
                return True
            if line.startswith("data:"):
                ev = json.loads(line[5:])
                if ev.get("error"):
                    self.fail(ev["error"])
                for ch in ev.get("choices") or []:
                    self.finish = ch.get("finish_reason") or self.finish
                    if ch.get("text"):
                        self.tokens += 1
                        self.first_at = self.first_at or time.monotonic()
                        self.up.set()
        return False

    def run(self, base, body, timeout):
        u = urllib.parse.urlsplit(base)
        conn = http.client.HTTPConnection(u.hostname, u.port, timeout=timeout)
        try:
            conn.connect()
            self.sock = conn.sock
            conn.request("POST", "/v1/completions", json.dumps(body), {"Content-Type": "application/json"})
            r = conn.getresponse()
            self.status = r.status
            if r.status != 200:
                self.fail(json.loads(r.read()).get("error") or {"code": "unparsed"})
            elif not self.read(r) and not self.cancelled:
                self.fail({"code": "transport", "message": "the stream ended without [DONE]"})
        except (OSError, ValueError, http.client.HTTPException) as e:
            if not self.cancelled:
                self.fail({"code": "transport", "message": str(e)})
        finally:
            conn.close()
            self.ended_at = time.monotonic()
            self.up.set()
            self.done.set()

    def wait_first(self, within):
        """True when a token arrived within `within` s (False: the stream ended first, or none yet)."""
        self.up.wait(within)
        return self.first_at is not None

    def wait_done(self, within):
        return self.done.wait(within)

    def cancel(self):
        self.cancelled = True
        try:
            self.sock.shutdown(socket.SHUT_RDWR)
        except (AttributeError, OSError):
            pass  # not connected yet, or already ended


class Relay:
    """P1.7's capture, this script's own UDP relay (as the test-only one in crates/ucompute-network/tests/support, which
    no release has): H dials 127.0.0.1:<port>, `back` forwards to `target` over the real path, and every datagram goes
    to `path` as it passes (direction, 0 to the target and 1 back; length; payload). What the relay never got reached
    neither side, so the file holds all the two sides exchanged."""
    # ponytail: a thread of this process, enough for run-0820's 5 MB/s; a process of its own if the upload ever stalls
    HEAD = struct.Struct("<BI")

    def __init__(self, target, path):
        self.target, self.client, self.stopping = target, None, False
        self.datagrams, self.bytes = [0, 0], [0, 0]
        self.file = open(path, "wb", 1 << 20)
        self.front, self.back = (socket.socket(socket.AF_INET, socket.SOCK_DGRAM) for _ in range(2))
        self.front.bind(("127.0.0.1", 0))
        self.back.bind(("0.0.0.0", 0))
        for s in (self.front, self.back):
            s.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 21)
        self.port, self.back_port = self.front.getsockname()[1], self.back.getsockname()[1]
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def record(self, direction, data):
        self.file.write(self.HEAD.pack(direction, len(data)) + data)
        self.datagrams[direction] += 1
        self.bytes[direction] += len(data)

    def forward(self, direction, data, sock, to):
        self.record(direction, data)
        try:
            sock.sendto(data, to)
        except OSError:
            pass  # a lost datagram, as on any network: QUIC resends

    def run(self):
        sel = selectors.DefaultSelector()
        sel.register(self.front, selectors.EVENT_READ, 0)
        sel.register(self.back, selectors.EVENT_READ, 1)
        while not self.stopping:
            for key, _ in sel.select(0.2):
                data, src = key.fileobj.recvfrom(65535)
                if key.data == 0:
                    self.client = src
                    self.forward(0, data, self.back, self.target)
                elif src == self.target and self.client:
                    self.forward(1, data, self.front, self.client)

    def stop(self):
        self.stopping = True
        self.thread.join(5)
        self.file.close()
        self.front.close()
        self.back.close()


class RShellError(Exception):
    """An R command did not complete: ssh could not reach R (exit 255), the command failed, or it timed out."""


def shell(where, key, **kw):
    """A named command on H (this Mac) or R (through --r-sh); its stdout. On R, a non-zero exit raises RShellError:
    every R command ends in `|| true` or cannot fail on its own, so it means the transport failed (privacy:F1)."""
    cmd = (R_CMDS if where == "r" else H_CMDS)[key].format(**kw)
    argv = [*shlex.split(ARGS.r_sh), cmd] if where == "r" else ["sh", "-c", cmd]
    try:
        p = subprocess.run(argv, capture_output=True, text=True, timeout=60)
        out, rc, err = p.stdout, p.returncode, p.stderr
    except (OSError, subprocess.TimeoutExpired) as e:
        out, rc, err = f"ERROR {e!r}", None, repr(e)
    log_raw({"shell": where, "cmd": cmd, "out": out, "rc": rc, "stderr": err[-2000:]})
    if where == "r" and rc != 0:
        raise RShellError(f"R command {key!r} exited {rc}: {err.strip()[-300:]}")
    return out


def step(name, ok, **facts):
    RESULTS.append({"step": name, "ok": bool(ok), **facts})
    print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(facts, default=str), flush=True)
    return bool(ok)


def step_na(name, why):
    RESULTS.append({"step": name, "ok": None, "na": why})
    print(f"N/A  {name} ({why})", flush=True)


def save(rel, data):
    path = os.path.join(ARGS.out, rel)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(data if isinstance(data, str) else json.dumps(data, indent=1, default=str) + "\n")


def wait(cond, within, poll=0.5):
    t0 = time.monotonic()
    while time.monotonic() - t0 < within:
        if cond():
            return round(time.monotonic() - t0, 3)
        time.sleep(poll)
    return None


# --- parsers (pure; the offline tests pin them)

def listeners(ss_text):
    """`ss -ltnupH` lines as (proto, local address, [process names]); names show only for this user's sockets."""
    out = []
    for line in ss_text.splitlines():
        f = line.split()
        if len(f) >= 5:
            out.append((f[0], f[4], re.findall(r'\("([^"]+)",pid=', line)))
    return out


def listeners_bad(ss_text, rpc_allowed=False):
    """The run user's listeners other than UCompute's: the daemon may listen on TCP only on loopback (its UDP peer
    endpoint and mDNS are expected), ggml-rpc-server only on TCP loopback and only during a session."""
    bad = []
    for proto, local, names in listeners(ss_text):
        for n in names:
            loop = local.startswith("127.0.0.1:")
            if n == "ucompute" and (proto != "tcp" or loop):
                continue
            if n == "ggml-rpc-server" and rpc_allowed and proto == "tcp" and loop:
                continue
            bad.append(f"{proto} {local} {n}")
    return bad


def rpc_ports(ss_text):
    return [int(local.rsplit(":", 1)[1]) for proto, local, names in listeners(ss_text) if "ggml-rpc-server" in names]


def who_others(text):
    """`who` lines other than the run's own SSH session (OD-10)."""
    lines = text.strip().splitlines()
    client = lines[-1].partition("=")[2].strip() if lines and lines[-1].startswith("ssh_client=") else ""
    return [x for x in lines[:-1] if x.strip() and not (client and f"({client})" in x)]


def sessions_others(text):
    """The `loginctl show-session` blocks (the "sessions" command) of active or online sessions other than the run's
    own SSH sessions (this command's, and the run's tunnel: sshd, remote, the run's user, from the run's client
    address), the user manager and the display manager's login screen (`Class=greeter`: nobody is logged in), one
    line each (OD-10: a desktop login on a seat is caught, `who` may not list it)."""
    blocks, meta = [], {}
    for chunk in text.strip().split("\n\n"):
        kv = dict(line.partition("=")[::2] for line in chunk.splitlines() if "=" in line)
        if "Id" in kv:
            blocks.append(kv)
        else:
            meta.update(kv)
    me, user, client = meta.get("self", ""), meta.get("user", ""), meta.get("ssh_client", "")

    def own(s):
        return (s["Id"] == me or s.get("Class", "").startswith("manager") or s.get("Class") == "greeter"
                or (s.get("Service") == "sshd" and s.get("Remote") == "yes" and bool(user) and s.get("Name") == user
                    and bool(client) and s.get("RemoteHost") == client))
    return [" ".join(f"{k}={s.get(k, '')}" for k in ("Id", "Name", "Class", "Type", "State", "Seat", "RemoteHost"))
            for s in blocks if s.get("State") in ("active", "online") and not own(s)]


def tree_diff(ls_tree, listing):
    """Paths that differ between HEAD (`git ls-tree -r HEAD`, regular files) and R's tree (`path<TAB>blob hash`
    lines): changed, missing on R or extra on R."""
    want = {}
    for line in ls_tree.splitlines():
        meta, _, path = line.partition("\t")
        f = meta.split()
        if len(f) == 3 and f[0] in ("100644", "100755"):
            want[path] = f[2]
    got = {p: h for p, _, h in (x.partition("\t") for x in listing.splitlines() if x)}
    return sorted(p for p in want.keys() | got.keys() if want.get(p) != got.get(p))


def free_bytes(vm_stat):
    """Free, inactive and speculative pages of `vm_stat`, in bytes."""
    page = int(re.search(r"page size of (\d+) bytes", vm_stat).group(1))
    pages = {k: int(v) for k, v in re.findall(r"Pages (free|inactive|speculative):\s+(\d+)", vm_stat)}
    return page * sum(pages.values())


def per_token(c):
    """The lab's decode-only method (SCORING-INPUT.md): ((after_n65 - after_n1) - (after_n1 - loaded)) / 64 per
    counter; turns = that difference for switches / 2. Both requests prefill, so the prefill cancels."""
    d = {k: ((c["after_n65"][k] - c["after_n1"][k]) - (c["after_n1"][k] - c["loaded"][k])) / DECODE_STEPS
         for k in ("to_r", "to_h", "switches")}
    return {"to_r": d["to_r"], "to_h": d["to_h"], "turns": d["switches"] / 2}


def timing(r):
    """Client-side TTFT and TPOT of one stream (SCORING-INPUT.md): tpot = ([DONE] - ttft) / (completion_tokens - 1)."""
    n = (r.get("usage") or {}).get("completion_tokens") or 0
    if r.get("ttft_ms") is None or r.get("done_ms") is None or n < 2:
        return None, None
    return r["ttft_ms"], round((r["done_ms"] - r["ttft_ms"]) / (n - 1), 3)


def size_bytes(text):
    """"45 MB", "2848 KB" or "1.2 GB" as `footprint` prints it, in bytes."""
    n, unit = text.split()
    return int(float(n) * {"B": 1, "KB": 1 << 10, "MB": 1 << 20, "GB": 1 << 30}[unit])


def footprint_peak(text):
    """`phys_footprint_peak` of `footprint -p`, in bytes; None when it is not there."""
    m = re.search(r"phys_footprint_peak:\s*([\d.]+ [KMG]?B)", text)
    return size_bytes(m[1]) if m else None


def footprint_totals(text):
    """The TOTAL line's dirty and clean bytes. `phys_footprint` leaves clean pages out, so weights an engine keeps
    mapped from the file show here and not in the peak."""
    m = re.search(r"^\s*([\d.]+ [KMG]?B)\s+([\d.]+ [KMG]?B)\s+[\d.]+ [KMG]?B\s+\d+\s+TOTAL\s*$", text, re.M)
    return {"dirty": size_bytes(m[1]), "clean": size_bytes(m[2])} if m else {}


def vm_hwm(text):
    """VmHWM of /proc/<pid>/status, in bytes; None when it is not there."""
    m = re.search(r"^VmHWM:\s*(\d+) kB", text, re.M)
    return int(m[1]) << 10 if m else None


def memory_verdict(full, split, absent):
    """F-03-02: `split` is below `full` by at least `absent`, the bytes of the layers this host does not host, -15 %."""
    if not full or not split or not absent:
        return {"met": False, "full_peak": full, "split_peak": split, "absent_bytes": absent}
    saved = full - split
    return {"met": saved >= absent * (1 - MEMORY_TOL), "full_peak": full, "split_peak": split, "absent_bytes": absent,
            "saved_bytes": saved, "saved_over_absent": round(saved / absent, 3)}


def gguf_tensors(path):
    """(tensor name, offset) pairs of a GGUF v2 or v3 file and where its data starts: the header only, little-endian,
    as crates/ucompute-models/src/gguf.rs reads it."""
    scalar = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}  # GGUF value type -> bytes
    unsigned = {0: "B", 2: "H", 4: "I", 10: "Q"}
    with open(path, "rb") as f:
        def num(fmt):
            return struct.unpack("<" + fmt, f.read(struct.calcsize("<" + fmt)))[0]

        def skip(kind):
            if kind == 8:  # string
                f.seek(num("Q"), 1)
            elif kind == 9:  # array
                inner, n = num("I"), num("Q")
                if inner in scalar:
                    f.seek(scalar[inner] * n, 1)
                else:
                    for _ in range(n):
                        skip(inner)
            else:
                f.seek(scalar[kind], 1)
        if f.read(4) != b"GGUF" or num("I") not in (2, 3):
            raise ValueError(f"{path} is not a GGUF v2 or v3 file")
        tensor_count, kv_count, align = num("Q"), num("Q"), 32
        for _ in range(kv_count):
            key, kind = f.read(num("Q")), num("I")
            if key == b"general.alignment" and kind in unsigned:
                align = num(unsigned[kind])
            else:
                skip(kind)
        tensors = []
        for _ in range(tensor_count):
            name = f.read(num("Q")).decode()
            f.seek(8 * num("I") + 4, 1)  # the dimensions and the tensor's type
            tensors.append((name, num("Q")))
        return tensors, (f.tell() + align - 1) // align * align


def gguf_sizes(path):
    """Each tensor's bytes, each tensor's offset and where the data starts. The size is the gap to the next offset
    (the last runs to the end of the data), as `GgufMetadata::layer_bytes`, so no type table is needed."""
    tensors, start = gguf_tensors(path)
    data = os.path.getsize(path) - start
    offsets = sorted({o for _, o in tensors})

    def end(o):
        i = bisect.bisect_right(offsets, o)
        return min(offsets[i] if i < len(offsets) else data, data)
    return {n: max(end(o) - o, 0) for n, o in tensors}, dict(tensors), start


def layer_bytes(sizes, n_layer):
    """`GgufMetadata::layer_bytes`: the blocks, the output layer, and the rest as input."""
    out = {"blocks": [0] * n_layer, "input": 0, "output": 0}
    for name, size in sizes.items():
        m = re.match(r"blk\.(\d+)\.", name)
        if m and int(m[1]) < n_layer:
            out["blocks"][int(m[1])] += size
        else:
            out["output" if name.startswith("output") else "input"] += size
    return out


def weight_pattern(path, block):
    """(tensor, file offset, 64 bytes) from the middle of block `block`'s largest tensor, as the lab's R0 control
    (rpc_dataflow.weight_pattern)."""
    sizes, offsets, start = gguf_sizes(path)
    name = max((n for n in sizes if n.startswith(f"blk.{block}.")), key=sizes.get)
    at = start + offsets[name] + sizes[name] // 2
    with open(path, "rb") as f:
        f.seek(at)
        return name, at, f.read(64)


def scan_capture(path, needles):
    """Relay.record's file against `needles` (label -> bytes), per direction as one stream, so a needle split between
    two datagrams is found too. Counts only: the datagrams, and each label found with how often."""
    keep = max(map(len, needles.values())) - 1
    tails, hits, n = [b"", b""], {}, 0
    with open(path, "rb") as f:
        while head := f.read(Relay.HEAD.size):
            direction, size = Relay.HEAD.unpack(head)
            buf = tails[direction] + f.read(size)
            n += 1
            for label, needle in needles.items():
                if needle in buf:
                    hits[label] = hits.get(label, 0) + 1
            tails[direction] = buf[-keep:]
    return {"datagrams": n, "hits": hits}


def failure_checks(done, fl, t0, limit, typed):
    """P1.8's bound for one case: the stream ended within `limit` s of the injection and, when `typed`, with a typed
    error by then. `done` is the wait's answer: a stream cancelled after the bound also has an end time."""
    checks = {"ended_in_time": bool(done) and fl.ended_at is not None and fl.ended_at - t0 <= limit}
    if typed:
        checks["typed_in_time"] = ((fl.error or {}).get("code") not in UNTYPED and fl.error_at is not None
                                   and fl.error_at - t0 <= limit)
    return checks


def prefix(x, y):
    """How many characters `x` and `y` share from the start."""
    return next((i for i, (p, q) in enumerate(zip(x, y)) if p != q), min(len(x), len(y)))


def lab_input(runs, link):
    """The lab's lab-input.json (SCORING-INPUT.md). UCompute's /v1/completions returns no token ids, so `ids` is
    left out (optional there)."""
    return {"link": link, "runs": runs}


# --- the run

def r_clean(tag, rpc_allowed=False):
    """R runs no rpc-server (unless allowed) and listens only where UCompute does; saves what it saw."""
    try:
        rpc, ss = shell("r", "rpc"), shell("r", "ss")
    except RShellError as e:
        return False, {"r_shell_error": str(e)}, ""
    save(f"r/pgrep-{tag}.txt", rpc)
    save(f"r/ss-ltnup-{tag}.txt", ss)
    bad = listeners_bad(ss, rpc_allowed)
    return (rpc_allowed or not rpc.strip()) and not bad, {"rpc_server": rpc.strip(), "bad_listeners": bad}, ss


def preflight(prereg, a, b):
    try:
        return preflight_checks(prereg, a, b)
    except RShellError as e:
        return step("preflight", False, r_shell_error=str(e))


def preflight_checks(prereg, a, b):
    pf, checks = prereg["preflight"], {}
    head, _, dirty = shell("h", "git").partition("\n")
    head = head.strip()
    dirty = "\n".join(l for l in dirty.splitlines() if not l.startswith("??"))
    r_head, _, r_files = shell("r", "tree", tree=ARGS.r_tree).partition("\n")
    differs = tree_diff(shell("h", "tree"), r_files)
    builds = {}
    for side, base in (("h", a), ("r", b)):
        v = call(base, "GET", "/health")[1]
        builds[side] = v.get("commit") if isinstance(v, dict) else None
    checks["same_commit"] = {"ok": bool(head) and not dirty.strip() and r_head.strip() == head and not differs
                             and all(c == head for c in builds.values()),
                             "head": head, "uncommitted": dirty.strip().splitlines()[:20], "r_tree": ARGS.r_tree,
                             "r_head": r_head.strip(), "r_tree_differs": differs[:20], "build_commit": builds,
                             "why": "H's tree is clean at HEAD, R's tree holds exactly HEAD's files, and both "
                                    "daemons were built from HEAD with a clean tree (/health commit)"}
    procs = shell("r", "procs")
    checks["r_soak_and_engines_stopped"] = {"ok": not procs.strip(), "found": procs.strip()}
    ss = shell("r", "ss")
    save("r/ss-ltnup-preflight.txt", ss)
    bad = listeners_bad(ss)
    checks["r_only_ucompute_listens"] = {"ok": not bad, "others": bad}
    logins, others = sessions_others(shell("r", "sessions")), who_others(shell("r", "who"))
    checks["r_no_other_sessions"] = {"ok": not logins and not others, "loginctl": logins, "who": others}
    hp = shell("h", "procs")
    checks["h_no_other_engines_or_builds"] = {"ok": not hp.strip(), "found": hp.strip()}
    checks["h_ac_power"] = {"ok": "AC Power" in shell("h", "power")}
    checks["h_caffeinate"] = {"ok": bool(shell("h", "caffeinate").strip()),
                              "why": "the runner wraps the run in caffeinate -dimsu"}
    try:
        free = free_bytes(shell("h", "mem"))
    except (AttributeError, ValueError):
        free = 0
    checks["h_free_memory"] = {"ok": free >= pf["h_free_min_bytes"], "bytes": free}
    # quiet: R's 1-minute load below r_load_max and H's below h_load_max at every sample for quiet_s
    t0 = start = time.monotonic()
    samples = []
    while True:
        r = shell("r", "load").split()
        h = shell("h", "load").strip("{} \n").split()
        rl, hl = float(r[0]) if r else 99.0, float(h[0]) if h else 99.0
        samples.append([round(time.monotonic() - t0, 1), rl, hl])
        if rl >= pf["r_load_max"] or hl >= pf["h_load_max"]:
            start = time.monotonic()
        if time.monotonic() - start >= pf["quiet_s"]:
            quiet = True
            break
        if time.monotonic() - t0 >= pf["quiet_wait_s"]:
            quiet = False
            break
        time.sleep(15)
    save("preflight-load.json", samples)
    checks["quiet"] = {"ok": quiet, "samples": len(samples), "last": samples[-1],
                       "rule": f"R below {pf['r_load_max']} and H below {pf['h_load_max']} for {pf['quiet_s']} s"}
    save("h/top-preflight.txt", shell("h", "top"))
    save("r/top-preflight.txt", shell("r", "top"))
    ok = all(c["ok"] for c in checks.values())
    step("preflight", ok, **{k: v for k, v in checks.items()})
    return ok


def deny(name, a, body, codes, r_must_be_clean=True, text=None):
    r = sse(a, body)
    code = (r["error"] or {}).get("code")
    ok = r["status"] in (403, 502) and code in codes and not r["text"]
    if text:
        ok = ok and text in (r["error"] or {}).get("message", "")
    clean, facts, _ = r_clean(name)
    step(name, ok and (clean or not r_must_be_clean), status=r["status"], code=code, want=list(codes), **facts)


def groups(base, other, pooled, remote=False):
    s, v, _ = call(base, "PUT", "/api/groups", {"name": "pool", "members": [other], "policy": {
        "allow_remote_inference": remote, "allow_pooled_inference": pooled}})
    return s == 200


def checkpoint(a, rid, tag):
    time.sleep(0.2)  # the tunnel settles after [DONE] (SCORING-INPUT.md)
    _, v, _ = call(a, "GET", "/api/status")
    sp = (v if isinstance(v, dict) else {}).get("split") or {}
    if sp.get("session"):
        SESSIONS.add(sp["session"])
    _, pv, _ = call(a, "GET", "/api/peers")
    peers = pv.get("peers", []) if isinstance(pv, dict) else []
    conn = next((p.get("connection") for p in peers if p["id"] == rid), None) or {}
    return {"tag": tag, "session": sp.get("session"), "counters": {k: sp.get(k) for k in ("to_r", "to_h", "switches")},
            "connection": conn, "t": utc()}


def run_model(a, b, rid, m, prereg, lab_runs, rows):
    label, model, k, n = m["label"], m["model"], m["split_at"], m["n_layer"]
    rq = prereg["requests"]

    def body(prompt, max_tokens, target=None):
        x = {"model": model, "prompt": prompt, "max_tokens": max_tokens, "stream": True, "temperature": 0,
             "seed": rq["seed"], "stream_options": {"include_usage": True}}
        if target is None:
            x.update(target=f"split:{rid}", split_at=k, split_mode=rq["split_mode"], acknowledged=True)
        else:
            x["target"] = target
        return x

    # P1.3: two loads; both upload R's weights (no cache), so neither is called cold or warm
    loads = []
    for i in (1, 2):
        r = sse(a, body(rq["load_request"]["prompt"], rq["load_request"]["max_tokens"]))
        loads.append({"status": r["status"], "ms": r["done_ms"], "error": r["error"],
                      "counters": checkpoint(a, rid, f"load{i}")["counters"]})
        if i == 1:
            call(a, "POST", "/api/unload")
            wait(lambda: not shell("r", "rpc").strip(), 15)
    want = [("RPC0", 0, k), ("MTL0", k, n)]
    got = [(s.get("device"), s.get("first_block"), s.get("end_block"))
           for s in (r["placement"] or {}).get("stages", [])]
    _, status, _ = call(a, "GET", "/api/status")
    save(f"h/status-{label}.json", status)
    clean, facts, ss = r_clean(f"session-{label}", rpc_allowed=True)
    ports = rpc_ports(ss)
    # R's rpc-server must not answer on R's LAN address (nc's exit code: 0 would mean it connected); it counts only
    # when R's sshd answers there in the same step (the positive control)
    control = shell("h", "nc", ip=ARGS.r_ip, port=SSH_PORT).strip().rpartition("rc=")[2]
    nc = [shell("h", "nc", ip=ARGS.r_ip, port=p).strip().rpartition("rc=")[2] for p in ports]
    banner = shell("r", "banner", home=ARGS.r_home)
    up = step(f"session_{label}", all(x["status"] == 200 for x in loads) and got == want and clean and len(ports) == 1
              and control == "0" and all(x not in ("", "0") for x in nc) and "TCP" in banner,
              loads=loads, placement=got, want=want, rpc_ports=ports, lan_control_rc=control, lan_probe_rc=nc,
              banner=banner.strip(), **facts)
    if not up:
        call(a, "POST", "/api/unload")
        return
    # P1.6: per prompt, three checkpoints in one session with nothing in between
    for p in prereg["prompts"]:
        # The first request on a new prompt also uploads that prompt's graphs (S1-M: about 191 kB and 8 switches
        # to R on R1a); a same-prompt warm-up before `loaded` keeps that out of the lab's decode-only subtraction.
        warm = sse(a, body(p["text"], 1))
        c0 = checkpoint(a, rid, "loaded")
        r1 = sse(a, body(p["text"], 1))
        c1 = checkpoint(a, rid, "after_n1")
        r65 = sse(a, body(p["text"], 65))
        c2 = checkpoint(a, rid, "after_n65")
        cps = {c["tag"]: c["counters"] for c in (c0, c1, c2)}
        sessions = {c["session"] for c in (c0, c1, c2)}
        ttft, tpot = timing(r65)
        usage = r65["usage"] or {}
        tok = None
        complete = all(v is not None for c in cps.values() for v in c.values())
        if complete:
            tok = per_token(cps)
        ok = (warm["status"] == r1["status"] == r65["status"] == 200 and not warm["error"] and not r1["error"]
              and not r65["error"]
              and len(sessions) == 1 and None not in sessions and complete and tok["to_r"] > 0 and tok["turns"] > 0
              and usage.get("completion_tokens") == 65 and tpot is not None)
        rows.append({"model": label, "prompt": p["id"], "session": next(iter(sessions)), "checkpoints": [c0, c1, c2],
                     "per_token": tok, "ratio_to_hidden_x4": tok and {d: round(tok[d] / (m["hidden"] * 4), 3)
                                                                    for d in ("to_r", "to_h")}})
        save(f"tokens/P-split-{label}-{p['id']}.json", {"text": r65["text"], "usage": usage, "ttft_ms": ttft,
                                                        "tpot_ms": tpot, "target": r65["target"]})
        step(f"split_{label}_p{p['id']}", ok, prompt_tokens=usage.get("prompt_tokens"), ttft_ms=ttft, tpot_ms=tpot,
             per_token=tok, sessions=len(sessions), errors=[r1["error"], r65["error"]])
        if ok:
            lab_runs.append({"model": label, "config": "P", "kind": "split", "prompt": p["id"],
                             "prompt_tokens": usage.get("prompt_tokens"), "ttft_ms": ttft, "tpot_ms": tpot,
                             "counters": cps})
    # P1.9: a local request with the split loaded reloads locally; nothing of it crosses to R
    before = checkpoint(a, rid, "before_local")
    first = prereg["prompts"][0]
    local1 = {first["id"]: sse(a, body(first["text"], 1, target="local"))}
    after = checkpoint(a, rid, "after_local")
    _, st, _ = call(a, "GET", "/api/status")
    loaded = (st or {}).get("loaded") or {}
    grew = {d: (after["counters"][d] or 0) - (before["counters"][d] or 0) for d in ("to_r", "to_h")}
    step(f"local_stays_local_{label}", local1[first["id"]]["status"] == 200 and local1[first["id"]]["target"] == "local"
         and not loaded.get("split") and all(v < 1024 for v in grew.values()),
         grew=grew, loaded_split=loaded.get("split"))
    # local references (Metal, same flags), timed the same way; tokens are compared as measured (ADR-0005 D10)
    compare, ok = [], True
    for p in prereg["prompts"]:
        if p["id"] not in local1:
            local1[p["id"]] = sse(a, body(p["text"], 1, target="local"))
        r = sse(a, body(p["text"], 65, target="local"))
        ttft, tpot = timing(r)
        ok &= r["status"] == 200 and local1[p["id"]]["status"] == 200
        save(f"tokens/local-mac-metal-{label}-{p['id']}.json", {"text": r["text"], "usage": r["usage"],
                                                                "ttft_ms": ttft, "tpot_ms": tpot})
        lab_runs.append({"model": label, "kind": "local", "prompt": p["id"], "ttft_ms": ttft, "tpot_ms": tpot})
        try:
            with open(os.path.join(ARGS.out, f"tokens/P-split-{label}-{p['id']}.json")) as f:
                split_text = json.load(f)["text"]
        except OSError:
            split_text = None
        same = None if split_text is None else next(
            (i for i, (x, y) in enumerate(zip(split_text, r["text"])) if x != y), min(len(split_text), len(r["text"])))
        compare.append({"model": label, "prompt": p["id"], "equal": split_text == r["text"],
                        "equal_prefix_chars": same})
    save(f"tokens/compare-{label}.json", compare)
    step(f"local_{label}", ok, compare=compare, note="as measured: Metal head with an x86 CPU stage (ADR-0005 D10)")
    stage_gone(a, label)


def stage_gone(a, label):
    call(a, "POST", "/api/unload")
    try:
        gone, err = wait(lambda: not shell("r", "rpc").strip(), 15), None
    except RShellError as e:
        gone, err = None, str(e)
    step(f"stage_gone_{label}", gone is not None, within_s=gone, r_shell_error=err)


def audit_join(a, b):
    """Both sides' audit (the largest page), joined on the split sessions. A full page may have cut the run's start
    off, so its oldest event must predate the run (privacy:F2); the sessions opened are exactly those the driver
    saw."""
    ev, oldest, whole = {}, {}, {}
    for side, base in (("h", a), ("r", b)):
        _, v, _ = call(base, "GET", f"/api/audit?n={AUDIT_MAX}")
        ev[side] = v.get("events", []) if isinstance(v, dict) else []
        save(f"{side}/audit.jsonl", "".join(json.dumps(e) + "\n" for e in ev[side]))
        oldest[side] = ev[side][0].get("ts_utc") if ev[side] else None
        whole[side] = len(ev[side]) < AUDIT_MAX or (oldest[side] or "") < RUN_START

    def sessions(side, kind, role):
        return {(e.get("fields") or {}).get("session") for e in ev[side]
                if e.get("kind") == kind and (e.get("fields") or {}).get("role") == role}
    opened = sessions("h", "session_open", "head")
    joined = opened <= sessions("r", "session_open", "stage")
    closed = opened <= sessions("h", "session_close", "head") and opened <= sessions("r", "session_close", "stage")
    return ev, step("audit_join", bool(opened) and joined and closed and opened == SESSIONS and all(whole.values()),
                    sessions=len(opened), driver_sessions=len(SESSIONS), joined=joined, closed_both_sides=closed,
                    run_start=RUN_START, oldest=oldest, whole_run=whole)


def canary_scan(prereg, ev, name="canary_scan", out="scan.json"):
    canaries = [p["canary"] for p in prereg["prompts"]]
    hits = {"h": [], "r": [], "audit": [c for c in canaries if c in json.dumps(ev)]}
    roots = [os.path.join(ARGS.h_home, "logs"), os.path.join(ARGS.h_home, "runs")]
    files = [os.path.join(d, f) for root in roots for d, _, fs in os.walk(root) for f in fs]
    files += [ARGS.h_log] if ARGS.h_log and os.path.isfile(ARGS.h_log) else []
    for f in files:
        with open(f, "rb") as fh:
            data = fh.read()
        if any(c.encode() in data for c in canaries):
            hits["h"].append(f)
    pats = " ".join("-e " + shlex.quote(c) for c in canaries)
    try:
        hits["r"], err = shell("r", "scan", pats=pats, home=ARGS.r_home, log=ARGS.r_log).split(), None
    except RShellError as e:
        err = str(e)
    save(out, {"canaries": canaries, "h_files_scanned": len(files), "hits": hits, "r_shell_error": err})
    step(name, not err and not any(hits.values()), h_files=len(files), hits=hits, r_shell_error=err)


def teardown(a, name="teardown", tag="teardown"):
    call(a, "POST", "/api/unload")
    ok, facts, _ = r_clean(tag)
    step(name, ok, **facts)


# --- the steps beyond the core run (--steps); `x` carries the run's two daemons, ids, prereg and models

def status(base):
    v = call(base, "GET", "/api/status")[1]
    return v if isinstance(v, dict) else {}


def peers(base):
    v = call(base, "GET", "/api/peers")[1]
    return v.get("peers", []) if isinstance(v, dict) else []


def connected(base, pid):
    return any(p["id"] == pid and p.get("connected") for p in peers(base))


def connection(base, pid):
    """`pid`'s open connection as `base` sees it, {} when there is none."""
    return next((p.get("connection") or {} for p in peers(base) if p["id"] == pid), {})


def fresh(x):
    """Nothing split left: H unloads, which ends the session, and R's rpc-server is gone."""
    call(x.a, "POST", "/api/unload")
    return wait(lambda: not shell("r", "rpc").strip(), 15) is not None


def local_body(x, m, prompt, n):
    return {"model": m["model"], "prompt": prompt, "max_tokens": n, "stream": True, "temperature": 0,
            "seed": x.prereg["requests"]["seed"], "stream_options": {"include_usage": True}, "target": "local"}


def split_body(x, m, prompt, n, mode="pooled"):
    return {**local_body(x, m, prompt, n), "target": f"split:{x.rid}", "split_at": m["split_at"], "split_mode": mode,
            "acknowledged": True}


def model_blob(model):
    """The GGUF of `model` in H's store."""
    with open(os.path.join(ARGS.h_home, "models", "manifests", model + ".json")) as f:
        return os.path.join(ARGS.h_home, "models", "blobs", "sha256-" + json.load(f)["sha256"])


def on_r(x, model):
    """Whether R's store holds `model`: a split does not need it, a local reference on R does (OD-4)."""
    v = call(x.b, "GET", "/api/models")[1]
    listed = v.get("models", []) if isinstance(v, dict) else []
    return any(m.get("id") == model and m.get("downloaded") for m in listed)


def h_peak(tag, pid):
    text = shell("h", "footprint", pid=pid)
    save(f"h/footprint-{tag}.txt", text)
    save(f"h/argv-{tag}.txt", shell("h", "argv", pid=pid))
    return footprint_peak(text), footprint_totals(text)


def r_peak(tag, pid):
    text = shell("r", "proc_status", pid=pid) if pid else ""
    save(f"r/proc-status-{tag}.txt", text)
    if pid:
        save(f"r/argv-{tag}.txt", shell("r", "argv", pid=pid))
    return vm_hwm(text)


def memory_model(x, m):
    label, model, k = m["label"], m["model"], m["split_at"]
    prompt = x.prereg["prompts"][0]["text"]
    lb = layer_bytes(gguf_sizes(model_blob(model))[0], m["n_layer"])
    # The same prompt and 64 decode steps everywhere; a peak is read while its process lives.
    fresh(x)
    split = sse(x.a, split_body(x, m, prompt, 65))
    head = status(x.a).get("loaded") or {}
    rpc = shell("r", "rpc").split()
    h_split, h_split_totals = h_peak(f"split-{label}", head.get("pid"))
    r_split = r_peak(f"rpc-{label}", rpc[0] if rpc else None)
    fresh(x)
    local = sse(x.a, local_body(x, m, prompt, 65))
    full = status(x.a).get("loaded") or {}
    h_full, h_full_totals = h_peak(f"local-{label}", full.get("pid"))
    call(x.a, "POST", "/api/unload")
    same = all(head.get(f) == full.get(f) for f in ("n_ctx", "parallel_slots", "threads"))
    ran = split["status"] == local["status"] == 200 and not split["error"] and not local["error"]
    h = memory_verdict(h_full, h_split, sum(lb["blocks"][:k]))
    record = {"model": model, "split_at": k, "layer_bytes": lb, "tolerance": MEMORY_TOL,
              "h": {**h, "same_flags": same, "totals_split": h_split_totals, "totals_local": h_full_totals}}
    step(f"memory_{label}_h", ran and same and h["met"], same_flags=same, totals_split=h_split_totals,
         totals_local=h_full_totals, errors=[split["error"], local["error"]], **h)
    if on_r(x, model):
        rlocal = sse(x.b, local_body(x, m, prompt, 65))
        r_full = r_peak(f"local-{label}", (status(x.b).get("loaded") or {}).get("pid"))
        call(x.b, "POST", "/api/unload")
        r = memory_verdict(r_full, r_split, sum(lb["blocks"][k:]) + lb["input"] + lb["output"])
        record["r"] = r
        step(f"memory_{label}_r", ran and rlocal["status"] == 200 and not rlocal["error"] and r["met"],
             error=rlocal["error"], **r)
    else:
        step_na(f"memory_{label}_r", f"{model} is not in R's store (OD-4): F-03-02's R side stays open")
    save(f"memory-{label}.json", record)


def memory(x):
    for m in x.models:
        memory_model(x, m)


def scanner_control(path, stream, needles):
    """The positive control of P1.7: plaintext `stream` through a Relay into its file and through scan_capture, the
    path of the real capture but for what travels."""
    sink, tx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM), socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sink.bind(("127.0.0.1", 0))
    sink.settimeout(5)
    relay = Relay(sink.getsockname(), path)
    try:
        for i in range(0, len(stream), 1200):
            tx.sendto(stream[i:i + 1200], ("127.0.0.1", relay.port))
            sink.recv(65535)  # one at a time: nothing is dropped on loopback
    finally:
        relay.stop()
        tx.close()
        sink.close()
    return scan_capture(path, needles)


def dial_at(x, addr):
    """H's one connection to R, dialled at `addr`, or None. Connect hands back an open connection as it is, so
    disconnect closes that first (a hold, which also keeps R's redial off until connect ends it); connect then dials
    `addr` first. R's redial can still win the race, so /api/peers has the word: connect's answer does not carry it."""
    for _ in range(5):
        call(x.a, "POST", f"/api/peers/{x.rid}/disconnect")
        call(x.a, "POST", f"/api/peers/{x.rid}/connect", {"addr": addr})
        c = connection(x.a, x.rid)
        if c.get("direction") == "outbound" and c.get("remote_addr") == addr:
            return c
        time.sleep(1)
    return None


def capture(x):
    m, prompts, a = x.models[0], x.prereg["prompts"], x.a
    k, blob = m["split_at"], model_blob(m["model"])
    needles = {f"canary {p['id']} {enc}": p["canary"].encode(enc) for p in prompts for enc in ("utf-8", "utf-16-le")}
    canaries = b"".join(needles.values())
    name, at, pattern = weight_pattern(blob, (k - 1) // 2)  # the middle of R's range, as the lab's R0 control
    out_name, _, out_pattern = weight_pattern(blob, k)  # a block R does not hold: not in the control's stream
    # ponytail: one pattern, as the plan and R0; every block's would close the blind spot of a pattern that QUIC framing
    # would have cut in a plaintext stream, if a capture ever needs that
    needles[f"weight {name}"] = pattern
    out = os.path.join(ARGS.out, "capture")
    os.makedirs(out, exist_ok=True)
    with open(blob, "rb") as f:  # real weights around the pattern, split between the first two datagrams
        f.seek(at - 1170)
        plain = f.read(4096) + canaries
    ctl = scanner_control(os.path.join(out, "control.bin"), plain, {**needles, f"out of range {out_name}": out_pattern})
    control = set(ctl["hits"]) == set(needles)
    step("capture_control", control, hits=sorted(ctl["hits"]), datagrams=ctl["datagrams"], plaintext_bytes=len(plain))

    fresh(x)
    path = os.path.join(out, "datagrams.bin")
    relay = Relay((ARGS.r_ip, x.r_port), path)
    via = f"127.0.0.1:{relay.port}"
    runs, c, r_conn = [], {}, {}
    try:
        dialled = dial_at(x, via)
        if dialled:
            runs = [sse(a, split_body(x, m, p["text"], 65)) for p in prompts]
            c = checkpoint(a, x.rid, "capture")
            r_conn = connection(x.b, x.hid)
            fresh(x)
    finally:
        back = dial_at(x, x.r_dial)  # H's pinned address goes back to R's own
        relay.stop()
    scan = scan_capture(path, needles)
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    size, kept = os.path.getsize(path), os.path.getsize(path) <= CAPTURE_KEEP
    if not kept:
        os.remove(path)
    tunnel = c.get("counters") or {}
    checks = {"dialled_through_relay": bool(dialled),
              "h_stayed_on_relay": (c.get("connection") or {}).get("remote_addr") == via,
              "r_saw_relay": (r_conn.get("remote_addr") or "").rpartition(":")[2] == str(relay.back_port),
              "requests_ok": len(runs) == 3 and all(r["status"] == 200 and not r["error"] for r in runs),
              "relay_carried_tunnel": relay.bytes[0] >= (tunnel.get("to_r") or 0) > 0
              and relay.bytes[1] >= (tunnel.get("to_h") or 0) > 0,
              "no_hits": not scan["hits"], "control": control, "dial_restored": bool(back)}
    both = ("to_r", "to_h")
    summary = {"via": via, "relay_port": relay.back_port, "target": f"{ARGS.r_ip}:{x.r_port}",
               "datagrams": dict(zip(both, relay.datagrams)), "bytes": dict(zip(both, relay.bytes)),
               "tunnel": tunnel, "h_connection": c.get("connection"), "r_connection": r_conn, "file_bytes": size,
               "sha256": digest.hexdigest(), "raw_kept": kept, "dial_restored_to": x.r_dial if back else None}
    save("capture/relay-summary.json", summary)
    save("capture/scan.json", {"model": m["model"], "needles": {k_: v.hex() for k_, v in needles.items()}, **scan,
                               "control": ctl})
    step("capture", all(checks.values()), failed=[k_ for k_, v in checks.items() if not v], hits=scan["hits"],
         datagrams=scan["datagrams"], relay_bytes=summary["bytes"], tunnel=tunnel)


def config_c(x):
    m = next(p for p in x.prereg["models"] if p["label"] == "R1b")
    label, model, k, n = m["label"], m["model"], m["split_at"], m["n_layer"]
    a, b, prompts = x.a, x.b, x.prereg["prompts"]
    step_na("config_c_logprobs", "the API returns logprobs null (C7 not built, OD-2): tokens only")

    def three(base, tag, body):
        """The three prompts, 64 decode tokens each; their texts and whether all ran."""
        texts, ok = {}, True
        for p in prompts:
            r = sse(base, body(p["text"]))
            ttft, tpot = timing(r)
            ok &= r["status"] == 200 and not r["error"] and (r["usage"] or {}).get("completion_tokens") == 65
            save(f"tokens/{tag}-{label}-{p['id']}.json", {"text": r["text"], "usage": r["usage"], "ttft_ms": ttft,
                                                          "tpot_ms": tpot, "target": r["target"]})
            texts[p["id"]] = r["text"]
        return texts, ok

    fresh(x)
    load = sse(a, split_body(x, m, prompts[0]["text"], 1, mode="same_backend"))
    want = [("CPU", 0, k), ("RPC0", k, n)]
    got = [(s.get("device"), s.get("first_block"), s.get("end_block"))
           for s in (load["placement"] or {}).get("stages", [])]
    st = status(a)
    sp = st.get("split") or {}
    device = sp.get("stage_device") or ""
    if not step("config_c_session", load["status"] == 200 and not load["error"] and got == want
                and device.startswith("CPU"), placement=got, want=want, stage_device=device, error=load["error"],
                load_ms=load["done_ms"], h_devices=(st.get("loaded") or {}).get("device_actual")):
        fresh(x)
        return
    split, ok = three(a, "C-split", lambda t: split_body(x, m, t, 65, mode="same_backend"))
    c = checkpoint(a, x.rid, "config_c")
    save("bytes/config-c.json", c)
    step("config_c_split", ok and c["session"] == sp.get("session"), counters=c["counters"], session=c["session"])

    fresh(x)
    s, info, _ = call(a, "POST", f"/api/models/{model}/load", {"device": "cpu"}, timeout=LOAD_WAIT)
    mac, ok = three(a, "local-mac-cpu", lambda t: local_body(x, m, t, 65))
    loaded = status(a).get("loaded") or {}  # still the CPU engine: a local request without a device reuses it
    call(a, "POST", "/api/unload")
    step("config_c_local_mac_cpu", s == 200 and ok and loaded.get("device_requested") == "cpu",
         status=s, device_requested=loaded.get("device_requested"), device_actual=loaded.get("device_actual"))

    sentinel = {}
    if on_r(x, model):
        sentinel, ok = three(b, "local-sentinel-cpu", lambda t: local_body(x, m, t, 65))
        call(b, "POST", "/api/unload")
        step("config_c_local_sentinel_cpu", ok)
    else:
        step_na("config_c_local_sentinel_cpu", f"{model} is not in R's store (OD-4)")
    pairs = (("C-split", split, "local-mac-cpu", mac), ("C-split", split, "local-sentinel-cpu", sentinel),
             ("local-mac-cpu", mac, "local-sentinel-cpu", sentinel))
    compare = [{"model": label, "prompt": p["id"], "a": x_, "b": y_, "equal": u[p["id"]] == v[p["id"]],
                "equal_prefix_chars": prefix(u[p["id"]], v[p["id"]])}
               for x_, u, y_, v in pairs for p in prompts if p["id"] in u and p["id"] in v]
    save(f"tokens/compare-C-{label}.json", compare)
    step_na("config_c_compare", f"as measured, never a pass rule (OD-3): {sum(r['equal'] for r in compare)} of "
                                f"{len(compare)} equal, tokens/compare-C-{label}.json")


def session_closes(sides, sid, within=15):
    """Each side's session_close outcome (`reason`) for `sid` in its audit. R writes its line after rpc-server stopped,
    so it is waited for."""
    role, out = {"h": "head", "r": "stage"}, {}

    def seen():
        for side, base in sides:
            v = call(base, "GET", "/api/audit?n=200")[1]
            for e in v.get("events", []) if isinstance(v, dict) else []:
                f = e.get("fields") or {}
                if e.get("kind") == "session_close" and f.get("session") == sid and f.get("role") == role[side]:
                    out[side] = f.get("reason")
        return all(out.get(s) for s, _ in sides)
    wait(seen, within)
    return out


def failure_case(x, n, name, limit, body, inject, recover=None, after=None, down=None, typed=True):
    """One case of P1.8 on a fresh session: a generation in flight, inject(flight, record) applies the failure, after()
    adds the case's own checks, recover() puts the hosts back. False when they did not come back."""
    label = f"failure_{n}_{name}"
    unloaded = fresh(x)
    fl = Flight(x.a, body)
    up = fl.wait_first(LOAD_WAIT)
    st = status(x.a)
    sid = (st.get("split") or {}).get("session")
    rec = {"case": n, "name": name, "limit_s": limit, "session": sid, "engine_pid": (st.get("loaded") or {}).get("pid")}
    if not up or not sid:
        fl.cancel()
        fl.wait_done(5)
        save(f"failure/{name}.json", {**rec, "error": fl.error, "status": fl.status})
        step(label, False, why="no generation in flight", status=fl.status, error=fl.error)
        return True
    checks = {"fresh_session": unloaded, "in_flight": not fl.wait_done(0)}
    t0 = time.monotonic()
    checks["injected"] = bool(inject(fl, rec))
    done = fl.wait_done(max(0, limit - (time.monotonic() - t0)))
    checks.update(failure_checks(done, fl, t0, limit, typed))
    if not done:  # the bound passed: it hangs, and the run goes on
        fl.cancel()
        fl.wait_done(5)
    rec.update(error=fl.error, finish=fl.finish, tokens=fl.tokens, status=fl.status,
               error_after_s=round(fl.error_at - t0, 3) if fl.error_at is not None else None,
               ended_after_s=round(fl.ended_at - t0, 3) if fl.ended_at is not None else None)
    if after:
        checks.update(after(rec, t0))
    gone = wait(lambda: not shell("r", "rpc").strip(), 15) is not None
    clean, facts, _ = r_clean(f"failure-{n}")
    sides = [(s, base) for s, base in (("h", x.a), ("r", x.b)) if s != down]
    closes = session_closes(sides, sid)
    checks["rpc_gone"] = gone and clean
    checks["session_close_with_outcome"] = all(closes.get(s) for s, _ in sides)
    back = recover() if recover else True
    checks["recovered"] = bool(back)
    active = []

    def idle():
        s, v, _ = call(x.a, "GET", "/api/jobs")
        active[:] = (v.get("active") or []) if isinstance(v, dict) else ["no answer"]
        return s == 200 and not active
    checks["no_job_left"] = wait(idle, limit) is not None
    rec.update(checks=checks, session_close=closes, jobs_active=active, **facts)
    save(f"failure/{name}.json", rec)
    step(label, all(checks.values()), failed=[k for k, v in checks.items() if not v], limit_s=limit,
         code=(fl.error or {}).get("code"), error_after_s=rec["error_after_s"], ended_after_s=rec["ended_after_s"],
         session_close=closes)
    return bool(back)


def failures(x):
    a, b, rid, hid = x.a, x.b, x.rid, x.hid
    m, prompts = x.models[0], x.prereg["prompts"]

    def done(r):
        return r[0] in (200, 204)

    def reconnect():
        call(a, "POST", f"/api/peers/{rid}/connect", {})
        return wait(lambda: connected(a, rid), 30) is not None

    def cancel(fl, rec):
        fl.cancel()
        return True

    def cancel_after(rec, t0):
        r = sse(a, split_body(x, m, prompts[0]["text"], 8))
        now = (status(a).get("split") or {}).get("session")
        rec["next_request"] = {"status": r["status"], "error": r["error"], "session": now}
        fresh(x)
        return {"next_request_ok": r["status"] == 200 and not r["error"], "same_session": now == rec["session"]}

    def kill_rpc(fl, rec):
        shell("r", "kill_rpc")
        return True

    def pair_again():
        s2, up = pair_up(a, b, rid)
        return s2 == 200 and up is not None and groups(b, hid, True)

    def network_on():
        return done(call(a, "PUT", "/api/network", {"enabled": True, "serve_peers": False})) and wait(
            lambda: connected(a, rid), 30) is not None

    def kill_r(fl, rec):
        found = re.search(r"ms=(\d+)", shell("r", "kill_daemon"))
        rec["rpc_gone_after_ms"] = int(found[1]) if found else None
        return True

    def r_after(rec, t0):
        return {"pdeathsig_within_1s": rec["rpc_gone_after_ms"] is not None and rec["rpc_gone_after_ms"] <= 1000}

    def r_back():
        shell("r", "start", cmd=ARGS.r_start)
        return wait(lambda: call(b, "GET", "/health")[0] == 200, 30) is not None and reconnect()

    def kill_h(fl, rec):
        shell("h", "run", cmd=ARGS.h_kill)
        return True

    def h_after(rec, t0):
        gone = wait(lambda: not shell("r", "rpc").strip(), max(0, 5 - (time.monotonic() - t0)), poll=0.2)
        rec["r_teardown_seen_after_s"] = round(time.monotonic() - t0, 3)
        pid = rec["engine_pid"]
        rec["engine_alive_after_kill"] = bool(shell("h", "alive", pid=pid).strip())
        shell("h", "run", cmd=ARGS.h_start)
        up = wait(lambda: call(a, "GET", "/health")[0] == 200, 30) is not None
        reaped = wait(lambda: not shell("h", "alive", pid=pid).strip(), 10) is not None
        return {"r_teardown_within_5s": gone is not None, "h_restarted": up, "engine_reaped": reaped}

    cases = [
        ("client_cancel", 10, cancel, {"after": cancel_after, "typed": False}),
        ("kill_rpc_server", 10, kill_rpc, {}),
        ("revoke_on_r", 5, lambda fl, rec: done(call(b, "POST", f"/api/peers/{hid}/revoke")), {"recover": pair_again}),
        ("hold_on_h", 5, lambda fl, rec: done(call(a, "POST", f"/api/peers/{rid}/disconnect")), {"recover": reconnect}),
        ("pooling_off_on_r", 5, lambda fl, rec: groups(b, hid, False), {"recover": lambda: groups(b, hid, True)}),
        ("local_only_on_h", 5, lambda fl, rec: done(call(a, "PUT", "/api/privacy", {"mode": "local_only"})),
         {"recover": lambda: done(call(a, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True}))}),
        ("networking_off_on_h", 5, lambda fl, rec: done(call(a, "PUT", "/api/network", {"enabled": False,
                                                                                         "serve_peers": False})),
         {"recover": network_on}),
        ("sigkill_r_daemon", 10, kill_r, {"after": r_after, "recover": r_back, "down": "r"}),
        ("sigkill_h_daemon", 10, kill_h, {"after": h_after, "recover": reconnect, "down": "h", "typed": False}),
    ]
    for i, (name, limit, inject, kw) in enumerate(cases, 1):
        body = split_body(x, m, prompts[i % len(prompts)]["text"], FLIGHT_TOKENS)  # the canaries reach the failure logs
        if not failure_case(x, i, name, limit, body, inject, **kw):
            for j, later in enumerate(cases[i:], i + 1):
                step_na(f"failure_{j}_{later[0]}", f"not run: case {i} did not recover")
            return


def extras(x):
    """The steps --steps named beyond the core, then their own scan and teardown."""
    run = {"memory": memory, "capture": capture, "config_c": config_c, "failures": failures}
    todo = [s for s in STEPS if s in run and s in ARGS.steps]
    for name in todo:
        try:
            run[name](x)
        except (RShellError, OSError, ValueError) as e:
            step(f"{name}_stopped", False, why=repr(e))
            call(x.a, "POST", "/api/unload")
    if todo:
        ev = {}
        for side, base in (("h", x.a), ("r", x.b)):
            v = call(base, "GET", f"/api/audit?n={AUDIT_MAX}")[1]
            ev[side] = v.get("events", []) if isinstance(v, dict) else []
            save(f"{side}/audit-extra.jsonl", "".join(json.dumps(e) + "\n" for e in ev[side]))
        canary_scan(x.prereg, ev, "canary_scan_extra", "scan-extra.json")
        teardown(x.a, "teardown_extra", "teardown-extra")


def pair_up(a, b, rid):
    """R shows a code, H joins, and the two connect: (join status, seconds until connected or None)."""
    s, start, _ = call(b, "POST", "/api/pair/start", {})
    s2, _, _ = call(a, "POST", "/api/pair/join", {"addr": start.get("pair_addr"), "code": start.get("code")}) \
        if s == 200 else (s, start, 0)
    return s2, wait(lambda: any(p["id"] == rid and p.get("connected")
                                for p in call(a, "GET", "/api/peers")[1].get("peers", [])), 30)


def p1(args, prereg, lab_runs, rows):
    global RUN_START
    a, b = args.a, args.b
    RUN_START = utc()
    if not preflight(prereg, a, b):
        return
    _, ha, _ = call(a, "GET", "/api/network")
    _, rb, _ = call(b, "GET", "/api/network")
    hid, rid = ha["peer_id"], rb["peer_id"]
    args.r_ip = args.r_ip or (rb.get("dial_addr") or "").rpartition(":")[0]
    try:  # the LAN probe needs R's IPv4 address (an empty or bracketed IPv6 one fails nc on its own)
        ipaddress.IPv4Address(args.r_ip)
    except ValueError:
        step("r_ip", False, r_ip=args.r_ip, why="R's LAN address must be IPv4: pass --r-ip")
        return
    save("prereg.json", {**prereg, "run": {"utc": utc(), "link": args.link, "head": hid, "stage": rid,
                                           "r_ip": args.r_ip, "driven": DRIVEN}})
    manifest = {"utc": utc(), "commit": shell("h", "git").split("\n")[0], "ping": shell("h", "ping", ip=args.r_ip)}
    for side, base in (("h", a), ("r", b)):
        manifest[side] = {p: call(base, "GET", p)[1] for p in ("/health", "/api/status", "/api/network")}
    save("manifest.json", manifest)
    # setup: networking on both, R serves peers, H in trusted_group (acknowledged)
    call(a, "PUT", "/api/network", {"enabled": True, "serve_peers": False})
    call(b, "PUT", "/api/network", {"enabled": True, "serve_peers": True})
    call(a, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})
    first = next(m for m in prereg["models"] if m["label"] in args.models.split(","))
    probe = {"model": first["model"], "prompt": "Hello", "max_tokens": 1, "stream": True, "temperature": 0,
             "target": f"split:{rid}", "split_at": first["split_at"], "split_mode": "pooled", "acknowledged": True}
    core = "core" in args.steps
    _, pv, _ = call(a, "GET", "/api/peers")
    if not any(p["id"] == rid for p in pv.get("peers", [])):
        if core:
            deny("deny_unpaired", a, probe, ("peer_unauthorized",))  # P1.1 (a)
        s2, up = pair_up(a, b, rid)
        if not step("pair", s2 == 200 and up is not None, status=s2):
            return
    elif core:
        step_na("deny_unpaired", "H and R arrived paired")
    if core:
        groups(a, rid, False, remote=True)
        groups(b, hid, False, remote=True)
        # (b) and (c): a group without pooling reads peer_unauthorized on either side (ucompute-policy evaluate,
        # may_serve)
        nogroup = "no trusted group allows pooled inference"
        deny("deny_remote_only", a, probe, ("peer_unauthorized",), text=nogroup)  # (b)
        groups(a, rid, True)
        deny("deny_stage_refuses", a, probe, ("peer_unauthorized",), text=nogroup)  # (c)
        groups(b, hid, True)
        deny("deny_unacknowledged", a, {**probe, "acknowledged": False}, ("policy_denied",),
             text="Pooling splits one model")  # (d)
        for m in prereg["models"]:
            if m["label"] in args.models.split(","):
                try:
                    run_model(a, b, rid, m, prereg, lab_runs, rows)
                except RShellError as e:
                    step(f"model_{m['label']}", False, r_shell_error=str(e))
                    call(a, "POST", "/api/unload")
        ev, _ = audit_join(a, b)
        canary_scan(prereg, ev)
        teardown(a)
    else:
        groups(a, rid, True)
        groups(b, hid, True)
    r_port = int((rb.get("dial_addr") or ":8193").rpartition(":")[2])
    extras(argparse.Namespace(a=a, b=b, hid=hid, rid=rid, prereg=prereg, r_port=r_port, r_dial=f"{args.r_ip}:{r_port}",
                              models=[m for m in prereg["models"] if m["label"] in args.models.split(",")]))


def steps_arg(text):
    names = tuple(s for s in text.split(",") if s)
    if not names or set(names) - set(STEPS):
        raise argparse.ArgumentTypeError(f"--steps takes {', '.join(STEPS)}")
    return names


def main():
    global RAW, ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True, help="H's API (this Mac's dev daemon)")
    ap.add_argument("--b", required=True, help="R's API (the stage host, through the SSH tunnel)")
    ap.add_argument("--out", required=True)
    ap.add_argument("--r-sh", required=True, help="prefix that runs one shell command on R (no default host)")
    ap.add_argument("--r-ip", help="R's IPv4 LAN address (default: from R's dial address)")
    ap.add_argument("--r-tree", default="$HOME/src/ucompute", help="R's source tree, synced from HEAD and built")
    ap.add_argument("--h-home", default=os.environ.get("UCOMPUTE_HOME", ".ucompute-split"))
    ap.add_argument("--h-log", help="H's daemon log")
    ap.add_argument("--r-home", default="$HOME/.ucompute-acceptance")
    ap.add_argument("--r-log", default="$HOME/.ucompute-acceptance.log")
    ap.add_argument("--link", choices=("wifi", "wired"), default="wifi")
    ap.add_argument("--models", default="R1a,R1b")
    ap.add_argument("--prereg", default=os.path.join(HERE, "split_prereg.json"))
    ap.add_argument("--steps", type=steps_arg, default=("core",), help=f"any of {','.join(STEPS)} (default core)")
    ap.add_argument("--r-start", help="R's daemon start command, run through --r-sh (failures: case 8)")
    ap.add_argument("--h-start", help="H's daemon start command, run here (failures: case 9)")
    ap.add_argument("--h-kill", help="SIGKILL of H's daemon, run here (failures: case 9)")
    ARGS = args = ap.parse_args()
    if "failures" in args.steps and not (args.r_start and args.h_start and args.h_kill):
        ap.error("--steps failures needs --r-start, --h-start and --h-kill: cases 8 and 9 restart the daemons")
    os.makedirs(args.out, exist_ok=True)
    RAW = open(os.path.join(args.out, "raw.jsonl"), "a")
    with open(args.prereg) as f:
        prereg = json.load(f)
    lab_runs, rows = [], []
    try:
        p1(args, prereg, lab_runs, rows)
    finally:
        save("bytes/checkpoints.json", rows)
        save("bytes/per-token.json", [{k: r[k] for k in ("model", "prompt", "per_token", "ratio_to_hidden_x4")}
                                      for r in rows])
        save("lab-input.json", lab_input(lab_runs, args.link))
        save("steps.json", {"driven": DRIVEN, "steps": RESULTS})
        failed = [r["step"] for r in RESULTS if r["ok"] is False]
        print(f"{len(RESULTS)} steps, {len(failed)} FAIL" + (f": {', '.join(failed)}" if failed else ""), flush=True)
    sys.exit(3 if any(r["ok"] is False for r in RESULTS) else 0)


ARGS = None

if __name__ == "__main__":
    main()
