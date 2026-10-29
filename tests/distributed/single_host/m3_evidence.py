#!/usr/bin/env python3
"""SINGLE-HOST M3 evidence driver: two `ucompute serve` daemons on ONE machine. Never two-machine evidence.

    M3_DIR=<dir holding a/ b/ (UCOMPUTE_HOMEs) bin/ucompute raw/> python3 m3_evidence.py <cmd>
    cmds: step1 (starts both) step2 step3 step4 step5 step6 step7 stop

Every HTTP call goes to <M3_DIR>/raw/<cmd>.jsonl with the UTC time, an equivalent curl line, the
status, the body and the elapsed ms; daemon stdout/stderr go to raw/daemon-<n>.log.
Stdlib only. Ports: API 18301/18302, peer endpoint 18401/18402 (UDP, default bind 0.0.0.0).
"""
import json
import os
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone

D = os.environ["M3_DIR"]
BIN = os.environ.get("UCOMPUTE_BIN", f"{D}/bin/ucompute")
NODES = {"a": (18301, 18401), "b": (18302, 18402)}
MODEL = "stories15M-q4_0"
PROMPT = "Once upon a time, a little rabbit"
GOLDEN = (" named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it."
          " But the radish was too big")
# Strings that must never appear in audit entries, logs or state files.
SECRETS = [PROMPT, "little rabbit", "Benny", "radish"]
STEP = sys.argv[1] if len(sys.argv) > 1 else "help"
STATE = f"{D}/state.json"
os.makedirs(f"{D}/raw", exist_ok=True)
LOG = open(f"{D}/raw/{STEP}.jsonl", "a")


def utc(t=None):
    return datetime.fromtimestamp(t or time.time(), timezone.utc).isoformat(timespec="milliseconds")


def rec(kind, **kv):
    kv = {"t": utc(), "kind": kind, **kv}
    LOG.write(json.dumps(kv) + "\n")
    LOG.flush()
    short = {k: v for k, v in kv.items() if k not in ("body", "response", "chunks", "events")}
    print(json.dumps(short)[:600])
    return kv


def load_state():
    try:
        return json.load(open(STATE))
    except FileNotFoundError:
        return {}


def save_state(s):
    json.dump(s, open(STATE, "w"), indent=1)


def http(n, method, path, body=None, timeout=120, quiet=False):
    """(status, json-or-text, ms). Errors come back as their JSON body, never raised."""
    url = f"http://127.0.0.1:{NODES[n][0]}{path}"
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"content-type": "application/json"})
    t0 = time.perf_counter()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
        status, raw = 0, str(e).encode()
    ms = round((time.perf_counter() - t0) * 1000, 1)
    try:
        resp = json.loads(raw) if raw else None
    except ValueError:
        resp = raw.decode(errors="replace")
    if not quiet:
        curl = f"curl -sS -X {method} {url}" + (f" -H 'content-type: application/json' -d '{json.dumps(body)}'" if body is not None else "")
        rec("http", node=n, curl=curl, status=status, ms=ms, body=body, response=resp)
    return status, resp, ms


def err_code(resp):
    return resp.get("error", {}).get("code") if isinstance(resp, dict) else None


def stream(n, body, timeout=120):
    """POST an SSE request; returns (status, [(ms since send, event json)], total ms)."""
    url = f"http://127.0.0.1:{NODES[n][0]}/v1/completions"
    req = urllib.request.Request(url, data=json.dumps(body).encode(), method="POST",
                                 headers={"content-type": "application/json"})
    t0 = time.perf_counter()
    chunks = []
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status = r.status
            for line in r:
                line = line.decode().strip()
                if line.startswith("data: ") and line != "data: [DONE]":
                    chunks.append((round((time.perf_counter() - t0) * 1000, 1), json.loads(line[6:])))
    except urllib.error.HTTPError as e:
        status = e.code
        chunks.append((0, json.loads(e.read() or b"null")))
    ms = round((time.perf_counter() - t0) * 1000, 1)
    rec("http_stream", node=n, curl=f"curl -sS -N -X POST {url} -H 'content-type: application/json' -d '{json.dumps(body)}'",
        status=status, ms=ms, body=body, chunks=chunks)
    return status, chunks, ms


def pid_of(n):
    try:
        return int(open(f"{D}/{n}.pid").read())
    except (FileNotFoundError, ValueError):
        return None


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except (ProcessLookupError, TypeError):
        return False


def start(n):
    api, net = NODES[n]
    args = [BIN, "serve", "--port", str(api), "--network", "--serve-peers", "--listen-port", str(net), "-v"]
    env = {**os.environ, "UCOMPUTE_HOME": f"{D}/{n}"}
    out = open(f"{D}/raw/daemon-{n}.log", "a")
    out.write(f"\n===== {utc()} start: UCOMPUTE_HOME={D}/{n} {' '.join(args)}\n")
    out.flush()
    spawn = time.time()
    p = subprocess.Popen(args, env=env, stdout=out, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                         start_new_session=True)
    open(f"{D}/{n}.pid", "w").write(str(p.pid))
    rec("spawn", node=n, pid=p.pid, cmd=f"UCOMPUTE_HOME={D}/{n} {' '.join(args)}", spawn_utc=utc(spawn))
    return p.pid, spawn


def wait_ready(n, spawn, limit=60):
    while time.time() - spawn < limit:
        s, _, _ = http(n, "GET", "/health", quiet=True)
        if s == 200:
            ready = time.time()
            rec("ready", node=n, ready_utc=utc(ready), ms_after_spawn=round((ready - spawn) * 1000))
            return ready
        time.sleep(0.05)
    raise SystemExit(f"FAIL: {n} not healthy within {limit}s (see raw/daemon-{n}.log)")


def ids():
    s = load_state()
    return s["a_id"], s["b_id"]


def peer_view(n, other):
    """(pinned, connected, discovered entry) of `other` as seen in n's GET /api/peers."""
    _, r, _ = http(n, "GET", "/api/peers", quiet=True)
    if not isinstance(r, dict):
        return False, False, None
    p = next((p for p in r.get("peers", []) if p["id"] == other), None)
    d = next((d for d in r.get("discovered", []) if d["id"] == other), None)
    return p is not None, bool(p and p.get("connected")), d


def poll(fn, limit, every=0.1):
    """Seconds until fn() is truthy (None on timeout)."""
    t0 = time.time()
    while time.time() - t0 < limit:
        if fn():
            return round(time.time() - t0, 3)
        time.sleep(every)
    return None


def rss(tag):
    rows = {}
    for n in NODES:
        pid = pid_of(n)
        if not alive(pid):
            continue
        out = subprocess.run(["ps", "-axo", "pid=,ppid=,rss=,comm="], capture_output=True, text=True).stdout
        for line in out.splitlines():
            f = line.split(None, 3)
            if len(f) == 4 and (int(f[0]) == pid or int(f[1]) == pid):
                rows[f"{n}:{f[0]}:{os.path.basename(f[3])}"] = round(int(f[2]) / 1024, 1)
    rec("rss_mb", tag=tag, rss=rows)
    return rows


def completion(stream_=False, target=None):
    b = {"model": MODEL, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "target": target}
    if stream_:
        b |= {"stream": True, "stream_options": {"include_usage": True}}
    return b


def leak_scan(where):
    """Every SECRET string in audit/log/state files under both homes and the daemon logs."""
    hits = []
    files = [f"{D}/raw/daemon-a.log", f"{D}/raw/daemon-b.log"]
    for n in NODES:
        for root, dirs, fs in os.walk(f"{D}/{n}"):
            dirs[:] = [x for x in dirs if x not in ("models", "backends")]
            files += [os.path.join(root, f) for f in fs]
    for f in files:
        try:
            text = open(f, errors="replace").read()
        except OSError:
            continue
        hits += [(f, s) for s in SECRETS if s in text]
    rec("leak_scan", where=where, files_scanned=len(files), hits=hits)
    return hits


# ------------------------------------------------------------------------------------------- steps

def step1():
    """F-02-01 discovery: start A and B, poll each /api/peers until the other is discovered."""
    st = {}
    for n in NODES:
        if alive(pid_of(n)):
            raise SystemExit(f"{n} already running (pid {pid_of(n)}); run stop first")
    _, sa = start("a")
    _, sb = start("b")
    ra, rb = wait_ready("a", sa), wait_ready("b", sb)
    for n in NODES:
        _, net, _ = http(n, "GET", "/api/network")
        st[f"{n}_id"] = net["peer_id"]
        st[f"{n}_spawn"], st[f"{n}_ready"] = (sa, ra) if n == "a" else (sb, rb)
    save_state(st)
    both_ready = max(ra, rb)
    res = {}
    for n, other in (("a", st["b_id"]), ("b", st["a_id"])):
        t = poll(lambda: peer_view(n, other)[2] is not None, 60)
        seen = time.time() if t is not None else None
        res[n] = {"sees_other": t is not None,
                  "s_after_own_spawn": seen and round(seen - st[f"{n}_spawn"], 3),
                  "s_after_both_ready": seen and round(seen - both_ready, 3),
                  "entry": peer_view(n, other)[2]}
    for n in NODES:
        http(n, "GET", "/api/peers")
    rec("result", step="1-discovery", **res, PASS=all(r["sees_other"] for r in res.values()))
    rss("after-start")


def step2():
    """Pairing: success, wrong code, lockout after 6 wrong codes."""
    a_id, b_id = ids()
    _, s, _ = http("a", "POST", "/api/pair/start", {"name": "node-a"})
    status, joined, ms = http("b", "POST", "/api/pair/join", {"addr": s["pair_addr"], "code": s["code"], "name": "node-b"})
    http("a", "GET", "/api/pair/status")
    t_conn = poll(lambda: peer_view("a", b_id)[1] and peer_view("b", a_id)[1], 30)
    pa, pb = peer_view("a", b_id), peer_view("b", a_id)
    http("a", "GET", "/api/peers")
    http("b", "GET", "/api/peers")
    rec("result", step="2a-pair", join_status=status, join_ms=ms, joined_id_is_a=isinstance(joined, dict) and joined.get("id") == a_id,
        a_pins_b=pa[0], b_pins_a=pb[0], s_until_both_connected=t_conn,
        PASS=status == 200 and pa[0] and pb[0])

    _, s, _ = http("a", "POST", "/api/pair/start", {"name": "node-a"})
    wrong = f"{(int(s['code']) + 1) % 1_000_000:06d}"
    codes = []
    for i in range(6):
        st_, r, ms = http("b", "POST", "/api/pair/join", {"addr": s["pair_addr"], "code": wrong, "name": "node-b"})
        codes.append({"attempt": i + 1, "status": st_, "code": err_code(r), "ms": ms})
    st_, r, ms = http("b", "POST", "/api/pair/join", {"addr": s["pair_addr"], "code": s["code"], "name": "node-b"}, timeout=30)
    after_lock = {"status": st_, "code": err_code(r), "ms": ms}
    _, ps, _ = http("a", "GET", "/api/pair/status")
    lock_code = ((ps.get("result") or {}).get("error") or {}).get("code")
    rec("result", step="2b-wrong-code", attempts=codes, correct_code_after_lock=after_lock, a_pair_status_code=lock_code,
        PASS=codes[0]["code"] == "pairing_failed"
        and [c["code"] for c in codes[:5]] == ["pairing_failed"] * 5
        and codes[5]["code"] == "pairing_locked" and lock_code == "pairing_locked" and after_lock["status"] != 200)
    for n in NODES:
        http(n, "GET", "/api/audit?n=50")


def step3():
    """Group allowing remote inference on both; B privacy trusted_group (needs acknowledged)."""
    a_id, b_id = ids()
    sa, _, _ = http("a", "PUT", "/api/groups", {"name": "evidence", "members": [b_id], "policy": {"allow_remote_inference": True}})
    sb, _, _ = http("b", "PUT", "/api/groups", {"name": "evidence", "members": [a_id], "policy": {"allow_remote_inference": True}})
    s_noack, r_noack, _ = http("b", "PUT", "/api/privacy", {"mode": "trusted_group"})
    s_ack, r_ack, _ = http("b", "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})
    for n in NODES:
        http(n, "GET", "/api/groups")
        http(n, "GET", "/api/privacy")
    rec("result", step="3-group-privacy", a_group=sa, b_group=sb, unacknowledged=(s_noack, err_code(r_noack)),
        acknowledged=(s_ack, r_ack.get("mode") if isinstance(r_ack, dict) else r_ack),
        PASS=sa == 200 and sb == 200 and s_noack == 403 and s_ack == 200)


def step4():
    """Mode B from B to A: golden text, target echoed, TTFT, tok/s, streaming, audit without prompt."""
    a_id, b_id = ids()
    target = f"peer:{a_id}"
    runs = {}
    for label in ("cold", "warm"):
        s, r, ms = http("b", "POST", "/v1/completions", completion(target=target))
        text = r["choices"][0]["text"] if s == 200 else None
        runs[label] = {"status": s, "ms": ms, "text_is_golden": text == GOLDEN, "text": text,
                       "target": r.get("target") if isinstance(r, dict) else None,
                       "usage": r.get("usage") if s == 200 else r, "timings": r.get("timings") if s == 200 else None}
    s, chunks, ms = stream("b", completion(True, target))
    texts = [(t, c["choices"][0].get("text", "")) for t, c in chunks if c.get("choices")]
    full = "".join(x for _, x in texts)
    with_text = [t for t, x in texts if x]
    usage = next((c.get("usage") for _, c in chunks if c.get("usage")), None)
    timings = next((c.get("timings") or (c.get("usage") or {}).get("timings") for _, c in chunks
                    if c.get("timings") or (c.get("usage") or {}).get("timings")), None)
    ntok = (usage or {}).get("completion_tokens")
    runs["stream"] = {"status": s, "ms": ms, "chunks": len(chunks), "text_chunks": len(with_text),
                      "text_is_golden": full == GOLDEN, "targets": sorted({c.get("target") for _, c in chunks}),
                      "client_ttft_ms": with_text[0] if with_text else None,
                      "client_decode_tok_s": round((ntok - 1) / ((with_text[-1] - with_text[0]) / 1000), 1)
                      if ntok and len(with_text) > 1 and with_text[-1] > with_text[0] else None,
                      "usage": usage, "engine_timings": timings}
    audits = {}
    for n in NODES:
        _, r, _ = http(n, "GET", "/api/audit?n=1000")
        ev = r["events"]
        blob = json.dumps(ev)
        audits[n] = {"remote_request": [e for e in ev if e.get("kind") == "remote_request"][-6:],
                     "prompt_or_output_in_audit": [x for x in SECRETS if x in blob]}
    leaks = leak_scan("step4")
    rec("result", step="4-remote", runs=runs, audit=audits, leaks=leaks,
        PASS=all(runs[k]["status"] == 200 and runs[k]["text_is_golden"] for k in runs)
        and runs["cold"]["target"] == target and runs["stream"]["targets"] == [target]
        and not any(a["prompt_or_output_in_audit"] for a in audits.values()) and not leaks)
    rss("after-remote-inference")


def a_remote_count():
    _, r, _ = http("a", "GET", "/api/audit?n=1000", quiet=True)
    return sum(1 for e in r["events"] if e.get("kind") == "remote_request")


def step5():
    """B local_only: the same request is refused with policy_denied and never reaches A."""
    a_id, _ = ids()
    before = a_remote_count()
    http("b", "PUT", "/api/privacy", {"mode": "local_only"})
    s, r, ms = http("b", "POST", "/v1/completions", completion(target=f"peer:{a_id}"))
    time.sleep(1)
    after = a_remote_count()
    rec("result", step="5-local-only", status=s, code=err_code(r), ms=ms, a_remote_request_before=before,
        a_remote_request_after=after, PASS=s == 403 and err_code(r) == "policy_denied" and before == after)
    # Back to trusted_group so step 6 tests revocation, not privacy.
    http("b", "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})


def step6():
    """Revoke A on B: B refuses peer:A with peer_revoked; A's connection to B closes within 5 s."""
    a_id, b_id = ids()
    t_up = poll(lambda: peer_view("a", b_id)[1] and peer_view("b", a_id)[1], 35)
    rec("precondition", both_connected=t_up is not None)
    t0 = time.time()
    s_rev, r_rev, _ = http("b", "POST", f"/api/peers/{a_id}/revoke")
    t_closed = poll(lambda: not peer_view("a", b_id)[1], 15, 0.05)
    closed_s = None if t_closed is None else round(time.time() - t0, 3)
    s, r, _ = http("b", "POST", "/v1/completions", completion(target=f"peer:{a_id}"))
    time.sleep(6)  # A's sync loop keeps dialling B; B must keep refusing in the handshake
    still_closed = not peer_view("a", b_id)[1]
    for n in NODES:
        http(n, "GET", "/api/peers")
        http(n, "GET", "/api/network")
        http(n, "GET", "/api/audit?n=30")
    rec("result", step="6-revoke", revoke_status=s_rev, request_status=s, request_code=err_code(r),
        a_conn_closed_s_after_revoke=closed_s, a_still_disconnected_after_6s=still_closed,
        PASS=s_rev == 200 and err_code(r) == "peer_revoked" and closed_s is not None and closed_s <= 5 and still_closed)


def step7():
    """Re-pair, SIGKILL A: B shows A disconnected within 6 s (+1 s); restart A: reconnects."""
    a_id, b_id = ids()
    _, s, _ = http("a", "POST", "/api/pair/start", {"name": "node-a"})
    s_join, _, _ = http("b", "POST", "/api/pair/join", {"addr": s["pair_addr"], "code": s["code"], "name": "node-b"})
    http("b", "PUT", "/api/groups", {"name": "evidence", "members": [a_id], "policy": {"allow_remote_inference": True}})
    t_up = poll(lambda: peer_view("a", b_id)[1] and peer_view("b", a_id)[1], 35)
    rec("precondition", repair_status=s_join, both_connected_s=t_up)
    rss("before-kill")
    pid = pid_of("a")
    t0 = time.time()
    os.kill(pid, signal.SIGKILL)
    rec("sigkill", node="a", pid=pid, at_utc=utc(t0))
    t_down = poll(lambda: not peer_view("b", a_id)[1], 20, 0.05)
    down_s = None if t_down is None else round(time.time() - t0, 3)
    rec("disconnect_seen", node="b", s_after_kill=down_s)
    time.sleep(2)
    _, spawn = start("a")
    ready = wait_ready("a", spawn)
    t_re = poll(lambda: peer_view("b", a_id)[1] and peer_view("a", b_id)[1], 60)
    re_s = None if t_re is None else round(time.time() - spawn, 3)
    sr, rr, msr = http("b", "POST", "/v1/completions", completion(target=f"peer:{a_id}"))
    after_text_ok = sr == 200 and rr["choices"][0]["text"] == GOLDEN
    for n in NODES:
        http(n, "GET", "/api/peers")
    rec("result", step="7-disconnect", b_sees_a_down_s=down_s, a_restart_ready_s=round(ready - spawn, 3),
        reconnected_s_after_spawn=re_s, remote_after_reconnect=(sr, after_text_ok, msr),
        PASS=down_s is not None and down_s <= 7 and re_s is not None)
    rss("end")
    leak_scan("step7")


def stop():
    for n in NODES:
        pid = pid_of(n)
        if not alive(pid):
            rec("stop", node=n, pid=pid, note="not running")
            continue
        os.kill(pid, signal.SIGTERM)
        t = poll(lambda: not alive(pid), 20)
        rec("stop", node=n, pid=pid, exited_s=t)
    ps = subprocess.run(["ps", "-axo", "pid=,ppid=,args="], capture_output=True, text=True).stdout
    left = [l for l in ps.splitlines() if D in l]
    rec("leftover_processes", processes=left)


if __name__ == "__main__":
    cmds = {**{f"step{i}": globals()[f"step{i}"] for i in range(1, 8)}, "stop": stop}
    if STEP not in cmds:
        raise SystemExit(__doc__)
    cmds[STEP]()
