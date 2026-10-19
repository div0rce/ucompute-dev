#!/usr/bin/env python3
"""Two-machine M3 run: two `common serve --network --serve-peers` daemons on two physical machines.

    python3 -I mesh_two_machine.py --a http://127.0.0.1:18301 --b http://127.0.0.1:18302 \
        --out <dir> [--kill-b "<cmd>"] [--start-b "<cmd>"]

A and B are the daemons' loopback APIs (B usually through an SSH tunnel; the API never leaves loopback).
The peer traffic between the two machines is Common's own QUIC transport. Steps: discovery, pairing (wrong
code, then the right one), group + privacy, remote execution in both directions (JSON and streaming),
local_only refusal, audit check, revocation and re-pairing, and (with --kill-b/--start-b) loss detection and
reconnection. Every HTTP call goes to <out>/raw.jsonl. Stdlib only.
"""
import argparse
import json
import subprocess
import time
import urllib.error
import urllib.request

MODEL = "stories15M-q4_0"
PROMPT = "Once upon a time, a little rabbit"
GOLDEN = (" named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it."
          " But the radish was too big")
SECRETS = [PROMPT, "little rabbit", "Benny", "radish"]
RESULTS = []
RAW = None


def utc():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def call(base, method, path, body=None, timeout=120):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method=method, headers={"Content-Type": "application/json"})
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    ms = round((time.monotonic() - t0) * 1000, 1)
    try:
        val = json.loads(raw)
    except ValueError:
        val = raw.decode(errors="replace")
    RAW.write(json.dumps({"t": utc(), "base": base, "method": method, "path": path, "body": body,
                          "status": status, "ms": ms, "response": val}) + "\n")
    RAW.flush()
    return status, val, ms


def stream(base, body, timeout=120):
    req = urllib.request.Request(base + "/v1/completions", data=json.dumps({**body, "stream": True}).encode(),
                                 method="POST", headers={"Content-Type": "application/json"})
    t0, first, text, chunks, target = time.monotonic(), None, "", 0, None
    with urllib.request.urlopen(req, timeout=timeout) as r:
        for line in r:
            line = line.decode().strip()
            if not line.startswith("data:") or line == "data: [DONE]":
                continue
            ev = json.loads(line[5:])
            chunks += 1
            target = ev.get("target", target)
            for ch in ev.get("choices", []):
                if ch.get("text"):
                    first = first or time.monotonic()
                    text += ch["text"]
    return {"text": text, "golden": text == GOLDEN, "chunks": chunks, "target": target,
            "client_first_text_ms": round((first - t0) * 1000, 1) if first else None,
            "total_ms": round((time.monotonic() - t0) * 1000, 1)}


def step(name, ok, **facts):
    RESULTS.append({"step": name, "ok": bool(ok), **facts})
    print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(facts, default=str), flush=True)


def wait(cond, within, poll=0.05):
    t0 = time.monotonic()
    while time.monotonic() - t0 < within:
        if cond():
            return round(time.monotonic() - t0, 3)
        time.sleep(poll)
    return None


def peer(base, pid):
    _, v, _ = call(base, "GET", "/api/peers")
    return next((p for p in v.get("peers", []) if p["id"] == pid), None)


def connected(base, pid):
    p = peer(base, pid)
    return bool(p and p.get("connected"))


def remote(src, dst_id, tag):
    body = {"model": MODEL, "prompt": PROMPT, "max_tokens": 32, "temperature": 0, "target": f"peer:{dst_id}"}
    runs = {}
    for run in ("cold", "warm"):
        status, v, ms = call(src, "POST", "/v1/completions", body, timeout=300)
        text = v["choices"][0]["text"] if status == 200 else None
        runs[run] = {"status": status, "ms": ms, "golden": text == GOLDEN, "target": v.get("target") if status == 200 else v,
                     "engine_ttft_ms": (v.get("timings") or {}).get("time_to_first_token_ms") if status == 200 else None,
                     "decode_tok_s": round((v.get("timings") or {}).get("decode_tokens_per_second", 0), 1) if status == 200 else None}
    runs["stream"] = stream(src, body)
    ok = all(r.get("golden") for r in runs.values()) and all(
        (r.get("target") or "").startswith("peer:") for r in runs.values())
    step(f"remote execution {tag}", ok, **runs)


def audit_clean(base):
    _, v, _ = call(base, "GET", "/api/audit?n=1000")
    text = json.dumps(v)
    return [s for s in SECRETS if s in text], [e["kind"] for e in v.get("events", [])]


def main():
    global RAW
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True)
    ap.add_argument("--b", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--kill-b")
    ap.add_argument("--start-b")
    a = ap.parse_args()
    import os
    os.makedirs(a.out, exist_ok=True)
    RAW = open(os.path.join(a.out, "raw.jsonl"), "a")
    A, B = a.a, a.b
    na, nb = call(A, "GET", "/api/network")[1], call(B, "GET", "/api/network")[1]
    ida, idb = na["peer_id"], nb["peer_id"]
    print(f"A {na['short_label']} listen {na['listen_addr']}  |  B {nb['short_label']} listen {nb['listen_addr']}", flush=True)

    # 1. discovery (mDNS; needs one L2 segment)
    seen = wait(lambda: any(d["id"] == idb for d in call(A, "GET", "/api/peers")[1].get("discovered", []))
                and any(d["id"] == ida for d in call(B, "GET", "/api/peers")[1].get("discovered", [])), 15, 1.0)
    step("discovery: each daemon lists the other (mDNS)", seen is not None, seconds=seen,
         a_discovered=call(A, "GET", "/api/peers")[1].get("discovered"),
         b_discovered=call(B, "GET", "/api/peers")[1].get("discovered"))

    # 2. pairing: A shows a code, B joins with a wrong code once, then the right one
    s, start, _ = call(A, "POST", "/api/pair/start", {"name": "mac"})
    code, addr = start["code"], start.get("pair_addr")
    wrong = str((int(code) + 1) % 1_000_000).zfill(6)
    s1, v1, ms1 = call(B, "POST", "/api/pair/join", {"addr": addr, "code": wrong, "name": "hp"})
    step("pairing: wrong code refused", s1 >= 400 and (v1.get("error") or {}).get("code") == "pairing_failed",
         status=s1, code=(v1.get("error") or {}).get("code"), ms=ms1)
    s2, v2, ms2 = call(B, "POST", "/api/pair/join", {"addr": addr, "code": code, "name": "hp"})
    both = wait(lambda: connected(A, idb) and connected(B, ida), 15)
    step("pairing: right code pins both sides and they connect", s2 == 200 and v2.get("id") == ida and both is not None,
         pair_addr=addr, join_ms=ms2, connected_after_s=both)

    # 3. group + privacy on both sides
    for base, other in ((A, idb), (B, ida)):
        call(base, "PUT", "/api/groups", {"name": "two-machine", "members": [other],
                                           "policy": {"allow_remote_inference": True}})
        call(base, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})
    step("group and privacy set on both", True)

    # 4. remote execution both ways
    for base in (A, B):
        call(base, "POST", f"/api/models/{MODEL}/load", {"device": "auto"}, timeout=300)
    remote(A, idb, "A -> B (B serves)")
    remote(B, ida, "B -> A (A serves)")

    # 5. local_only refuses before anything leaves A
    call(A, "PUT", "/api/privacy", {"mode": "local_only"})
    s, v, ms = call(A, "POST", "/v1/completions", {"model": MODEL, "prompt": PROMPT, "max_tokens": 8, "target": f"peer:{idb}"})
    step("local_only: peer request refused on the sender", s == 403 and (v.get("error") or {}).get("code") == "policy_denied",
         status=s, ms=ms)
    call(A, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})

    # 6. audit carries no prompt text
    for name, base in (("A", A), ("B", B)):
        hits, kinds = audit_clean(base)
        step(f"audit on {name} has no prompt text", not hits, hits=hits, remote_entries=kinds.count("remote_request"))

    # 7. revocation: A revokes B; B's connection closes; B's requests to A are refused; then re-pair
    s, v, _ = call(A, "POST", f"/api/peers/{idb}/revoke")
    closed = wait(lambda: not connected(B, ida), 10)
    s2, v2, _ = call(B, "POST", "/v1/completions", {"model": MODEL, "prompt": PROMPT, "max_tokens": 8, "target": f"peer:{ida}"})
    step("revocation: B disconnected and refused", s == 200 and closed is not None and s2 >= 400,
         b_disconnected_after_s=closed, b_request_status=s2, b_error=(v2.get("error") or {}).get("code") if isinstance(v2, dict) else v2)
    _, start, _ = call(A, "POST", "/api/pair/start", {"name": "mac"})
    s3, _, _ = call(B, "POST", "/api/pair/join", {"addr": start.get("pair_addr"), "code": start["code"], "name": "hp"})
    call(A, "PUT", "/api/groups", {"name": "two-machine", "members": [idb], "policy": {"allow_remote_inference": True}})
    again = wait(lambda: connected(A, idb) and connected(B, ida), 15)
    step("re-pair after revocation", s3 == 200 and again is not None, connected_after_s=again)

    # 8. loss detection and reconnection
    if a.kill_b and a.start_b:
        subprocess.run(a.kill_b, shell=True, capture_output=True)
        t_kill = time.monotonic()
        lost = wait(lambda: not connected(A, idb), 15)
        step("B killed: A notices the loss", lost is not None and lost <= 7.0, seconds=lost)
        subprocess.Popen(a.start_b, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        back = wait(lambda: connected(A, idb), 60, 0.5)
        step("B restarted: the two reconnect", back is not None, seconds_after_restart=back,
             seconds_after_kill=round(time.monotonic() - t_kill, 1))
        if back is not None:
            wait(lambda: call(B, "GET", "/health")[0] == 200, 30, 0.5)
            call(B, "POST", f"/api/models/{MODEL}/load", {"device": "auto"}, timeout=300)
            remote(A, idb, "A -> B after B restarted")

    with open(os.path.join(a.out, "results.json"), "w") as f:
        json.dump({"a": na, "b": nb, "steps": RESULTS}, f, indent=2, default=str)
    failed = [r["step"] for r in RESULTS if not r["ok"]]
    print("FAILED: " + ", ".join(failed) if failed else "ALL PASS")


if __name__ == "__main__":
    main()
