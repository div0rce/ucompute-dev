#!/usr/bin/env python3
"""WP-04 P1: one model split between this Mac (H, the head: a dev daemon built from this commit) and the Linux
stage host (R, sentinel), with llama.cpp RPC carried only inside UCompute's authenticated QUIC connection
(plan docs/design/wp04-implementation-plan-2026-10-23.md section 3, ADR-0005). Configuration P only, at the lab's
split points. A daemon API run: no app window is driven.

    python3 -I split_two_machine.py --a <H's API> --b <R's API, through the SSH tunnel> --out <run dir> \
        --r-sh "scripts/mac-host.sh run" [--r-ip <R's IPv4 LAN address>] [--r-tree DIR] [--h-home DIR] \
        [--h-log FILE] [--r-home DIR] [--r-log FILE] [--link wifi|wired] [--models R1a,R1b] [--prereg FILE]

--r-sh is required (no default host). Every R-side command goes through it; an R command that exits non-zero (ssh
itself exits 255 when it cannot reach R) FAILs the step that needed it, never reads as a clean result.

Steps: preflight (P1.0 and every item of the lab's PREFLIGHT.md; a failed preflight sends no request), the
default-deny cases (P1.1: unpaired, remote-only groups, pooling allowed on H only, no acknowledgement), pairing, and
per model: two loads (P1.3), the session checks (P1.2), the three prompts with their counter checkpoints (P1.6), a
local request with the split loaded (P1.9) and the local references; then the audit join and the canary scan of
every log on both hosts (P1.10) and teardown (P1.11). Not automated here, each with its own evidence: memory (P1.5),
the capture (P1.7), the failure cases (P1.8) and configuration C (P1.4).

What it exports follows the Core Lab's P1 contract (ucompute-core-lab d569c5b, p1-predictions/SCORING-INPUT.md
and PREFLIGHT.md): requests go to H's /v1/completions with stream, temperature 0, seed 42, stream_options
include_usage and the raw prompts.json text; per prompt, within one split session and with nothing in between, the
tunnel counters are read at `loaded`, after a max_tokens 1 request and after a max_tokens 65 request on the same
prompt, each 0.2 s after the stream's [DONE]; TTFT and TPOT are client-side times of the max_tokens 65 stream.
<out>/lab-input.json is the lab's input: python3 -I p1-predictions/score_p1.py <out>/lab-input.json.

Writes prereg.json (before P1.1), manifest.json, steps.json, raw.jsonl (every HTTP call and shell command), h/,
r/, tokens/, bytes/ and lab-input.json under --out. Exit 3 when a step FAILed. Stdlib only.
"""
import argparse
import ipaddress
import json
import os
import re
import shlex
import subprocess
import sys
import time
import urllib.error
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
    address) and the user manager, one line each (OD-10: a desktop login on a seat is caught, `who` may not list it)."""
    blocks, meta = [], {}
    for chunk in text.strip().split("\n\n"):
        kv = dict(line.partition("=")[::2] for line in chunk.splitlines() if "=" in line)
        if "Id" in kv:
            blocks.append(kv)
        else:
            meta.update(kv)
    me, user, client = meta.get("self", ""), meta.get("user", ""), meta.get("ssh_client", "")

    def own(s):
        return (s["Id"] == me or s.get("Class", "").startswith("manager")
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
        ok = (r1["status"] == r65["status"] == 200 and not r1["error"] and not r65["error"]
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


def canary_scan(prereg, ev):
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
    save("scan.json", {"canaries": canaries, "h_files_scanned": len(files), "hits": hits, "r_shell_error": err})
    step("canary_scan", not err and not any(hits.values()), h_files=len(files), hits=hits, r_shell_error=err)


def teardown(a):
    call(a, "POST", "/api/unload")
    ok, facts, _ = r_clean("teardown")
    step("teardown", ok, **facts)


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
    _, pv, _ = call(a, "GET", "/api/peers")
    if not any(p["id"] == rid for p in pv.get("peers", [])):
        deny("deny_unpaired", a, probe, ("peer_unauthorized",))  # P1.1 (a)
        s, start, _ = call(b, "POST", "/api/pair/start", {})
        s2, _, _ = call(a, "POST", "/api/pair/join", {"addr": start.get("pair_addr"), "code": start.get("code")}) \
            if s == 200 else (s, start, 0)
        connected = wait(lambda: any(p["id"] == rid and p.get("connected")
                                     for p in call(a, "GET", "/api/peers")[1].get("peers", [])), 30)
        if not step("pair", s2 == 200 and connected is not None, status=s2):
            return
    else:
        step_na("deny_unpaired", "H and R arrived paired")
    groups(a, rid, False, remote=True)
    groups(b, hid, False, remote=True)
    # (b) and (c): a group without pooling reads peer_unauthorized on either side (ucompute-policy evaluate, may_serve)
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
    ARGS = args = ap.parse_args()
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
