#!/usr/bin/env python3
"""Two-machine M3 run: two `common serve --network --serve-peers` daemons on two physical machines.

    python3 -I mesh_two_machine.py --a http://127.0.0.1:18301 --b http://127.0.0.1:18302 \
        --out <dir> [--kill-b "<cmd>"] [--start-b "<cmd>"] [--no-mdns]

A and B are the daemons' loopback APIs (B usually through an SSH tunnel; the API never leaves loopback).
The peer traffic between the two machines is Common's own QUIC transport. Steps: A and B are unpaired first
(unpinned, not revoked) when they arrive paired, discovery, an unpaired peer gets no work, pairing (wrong
code, then the right one), group + privacy, remote execution in both directions (JSON and streaming),
local_only refusal, audit check, revocation and re-pairing, and (with --kill-b/--start-b) loss detection and
reconnection. Pairing again keeps the names the two had for each other. Everything the run changes is saved
first and put back at the end, also when a step fails or the run is aborted (state.py; it reports a lost
pairing instead of pairing again). Every HTTP call goes to <out>/raw.jsonl. Exit 3 when a step FAILed, 1 when
the run stopped (A and B left unpaired or revoked, or a crash). Stdlib only.
"""
import argparse
import json
import os
import subprocess
import sys
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
DRIVEN = "daemon API run (not app UI): pairing, groups and requests go through the two daemons' loopback APIs"
STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "state.py")


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
    except (urllib.error.URLError, OSError) as e:
        status, raw = 0, str(e).encode()
    ms = round((time.monotonic() - t0) * 1000, 1)
    try:
        val = json.loads(raw)
    except ValueError:
        val = raw.decode(errors="replace")
    RAW.write(json.dumps({"t": utc(), "base": base, "method": method, "path": path, "body": body,
                          "status": status, "ms": ms, "response": val}) + "\n")
    RAW.flush()
    return status, val, ms


def state(*args):
    """state.py save/restore, as a subprocess (under -I this script cannot import it); its exit code."""
    return subprocess.run([sys.executable, "-I", STATE, *args]).returncode


def stream(base, body, timeout=120):
    req = urllib.request.Request(base + "/v1/completions", data=json.dumps({**body, "stream": True}).encode(),
                                 method="POST", headers={"Content-Type": "application/json"})
    t0, first, text, chunks, target = time.monotonic(), None, "", 0, None
    try:
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
    except (urllib.error.URLError, OSError) as e:  # a refused stream is this step's FAIL, not the run's end
        return {"error": str(e), "golden": False, "target": None}
    return {"text": text, "golden": text == GOLDEN, "chunks": chunks, "target": target,
            "client_first_text_ms": round((first - t0) * 1000, 1) if first else None,
            "total_ms": round((time.monotonic() - t0) * 1000, 1)}


def step(name, ok, **facts):
    RESULTS.append({"step": name, "ok": bool(ok), **facts})
    print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(facts, default=str), flush=True)
    return bool(ok)


def step_na(name, why="the network carries no mDNS (multicast dropped), so nothing can be seen nearby"):
    RESULTS.append({"step": name, "ok": None, "na": why})
    print(f"N/A  {name} ({why})", flush=True)


def wait(cond, within, poll=0.05):
    t0 = time.monotonic()
    while time.monotonic() - t0 < within:
        if cond():
            return round(time.monotonic() - t0, 3)
        time.sleep(poll)
    return None


def peers(base):
    s, v, _ = call(base, "GET", "/api/peers")
    return v if s == 200 and isinstance(v, dict) else {"peers": [], "discovered": []}


def peer(base, pid):
    return next((p for p in peers(base)["peers"] if p["id"] == pid), None)


def connected(base, pid):
    p = peer(base, pid)
    return bool(p and p.get("connected"))


def code(v):
    return (v.get("error") or {}).get("code") if isinstance(v, dict) else None


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
    return step(f"remote execution {tag}", ok, **runs)


def audit_clean(base):
    """(status, prompt text found, event kinds) of the last 1000 audit events; a failed read has no kinds."""
    s, v, _ = call(base, "GET", "/api/audit?n=1000")
    kinds = [e["kind"] for e in v.get("events", [])] if s == 200 and isinstance(v, dict) else []
    return s, [x for x in SECRETS if x in json.dumps(v)], kinds


def refused_as_revoked(base, addrs, since):
    """`base`'s own verdict on a dial from the host of one of `addrs`: refused because the key is revoked. The
    dialler cannot tell (a refusal before the device proved its key reads as peer_unauthorized): this is the proof."""
    s, v, _ = call(base, "GET", "/api/audit?n=1000")
    hosts = {x.rsplit(":", 1)[0] for x in addrs if x}
    return s == 200 and isinstance(v, dict) and any(
        e["kind"] == "peer_rejected" and e["ts_utc"] >= since and (e.get("fields") or {}).get("reason") == "peer_revoked"
        and (e.get("fields") or {}).get("addr", "").rsplit(":", 1)[0] in hosts for e in v.get("events", []))


def steps(A, B, ida, idb, a):
    """Steps 0-8; returns why the run must stop (A and B left unpaired or revoked), else None."""
    names = ((peer(A, idb) or {}).get("name"), (peer(B, ida) or {}).get("name"))  # A's for B, B's for A: kept
    # 0. start unpaired: the runner arrives paired (discovery driver); DELETE unpins without revoking, step 2 re-pairs
    dels = [call(base, "DELETE", f"/api/peers/{other}")[0] for base, other in ((A, idb), (B, ida)) if peer(base, other)]
    apart = wait(lambda: peer(A, idb) is None and peer(B, ida) is None, 10)
    step("setup: A and B start unpaired (unpinned, not revoked)", apart is not None and all(x == 204 for x in dels),
         delete_statuses=dels)
    lost = "A and B were unpaired for this run and pairing them again failed: pair the two again" if dels else None

    # 1. discovery (mDNS; needs one L2 segment that carries multicast); an unpaired peer gets no work (no mDNS needed)
    listed = "discovery: an unpaired peer is listed as untrusted"
    if a.no_mdns:
        step_na("discovery: each daemon lists the other (mDNS)")
        step_na(listed)
    else:
        seen = wait(lambda: any(d["id"] == idb for d in peers(A)["discovered"])
                    and any(d["id"] == ida for d in peers(B)["discovered"]), 15, 0.25)
        a_disc, b_disc = peers(A)["discovered"], peers(B)["discovered"]
        step("discovery: each daemon lists the other (mDNS)", seen is not None, seconds=seen,
             a_discovered=a_disc, b_discovered=b_disc)
        if seen is None:
            step_na(listed, "neither daemon listed the other")
        else:
            a_entry = next(d for d in a_disc if d["id"] == idb)
            b_entry = next(d for d in b_disc if d["id"] == ida)
            step(listed, apart is not None and not a_entry["trusted"] and not b_entry["trusted"],
                 a_sees=a_entry, b_sees=b_entry)
    sp = call(A, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})[0]
    s, v, ms = call(A, "POST", "/v1/completions", {"model": MODEL, "prompt": PROMPT, "max_tokens": 8, "target": f"peer:{idb}"})
    step("an unpaired peer cannot receive work", apart is not None and sp == 200 and s == 403
         and code(v) in ("peer_unauthorized", "policy_denied"), privacy_status=sp, status=s, code=code(v), ms=ms)
    call(A, "PUT", "/api/privacy", {"mode": "local_only"})

    # 2. pairing: A shows a code, B joins with a wrong code once, then the right one
    s, start, _ = call(A, "POST", "/api/pair/start", {"name": names[1]})
    if s != 200:
        step("pairing: A opens a pairing window", False, status=s, response=start)
        return lost
    c, addr = start["code"], start.get("pair_addr")
    wrong = str((int(c) + 1) % 1_000_000).zfill(6)
    s1, v1, ms1 = call(B, "POST", "/api/pair/join", {"addr": addr, "code": wrong})
    step("pairing: wrong code refused", s1 >= 400 and code(v1) == "pairing_failed", status=s1, code=code(v1), ms=ms1)
    s2, v2, ms2 = call(B, "POST", "/api/pair/join", {"addr": addr, "code": c, "name": names[0]})
    both = wait(lambda: connected(A, idb) and connected(B, ida), 15) if s2 == 200 else None
    paired_ok = step("pairing: right code pins both sides and they connect", apart is not None and s2 == 200
                     and v2.get("id") == ida and both is not None, status=s2, pair_addr=addr, join_ms=ms2,
                     connected_after_s=both)
    if s2 != 200:
        return lost

    # 3. group + privacy on both sides
    setup = {}
    for n, base, other in (("A", A, idb), ("B", B, ida)):
        sg, g, _ = call(base, "PUT", "/api/groups", {"name": "two-machine", "members": [other],
                                                     "policy": {"allow_remote_inference": True}})
        sp = call(base, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})[0]
        grp = next((x for x in (g.get("groups", []) if isinstance(g, dict) else []) if x.get("name") == "two-machine"), {})
        setup[n] = {"groups_status": sg, "privacy_status": sp, "group": grp,
                    "ok": sg == 200 and sp == 200 and other in grp.get("members", [])
                    and (grp.get("policy") or {}).get("allow_remote_inference") is True}
    step("group and privacy set on both", all(x["ok"] for x in setup.values()), **setup)

    # 4. remote execution both ways
    loads = []
    for base in (A, B):
        loads.append(call(base, "POST", f"/api/models/{MODEL}/pull", timeout=600)[0])
        loads.append(call(base, "POST", f"/api/models/{MODEL}/load", {"device": "auto"}, timeout=300)[0])
    step(f"{MODEL} pulled and loaded on both", all(x == 200 for x in loads), statuses=loads)
    remote_ok = [remote(A, idb, "A -> B (B serves)"), remote(B, ida, "B -> A (A serves)")]

    # 5. local_only refuses before anything leaves A
    sp = call(A, "PUT", "/api/privacy", {"mode": "local_only"})[0]
    s, v, ms = call(A, "POST", "/v1/completions", {"model": MODEL, "prompt": PROMPT, "max_tokens": 8, "target": f"peer:{idb}"})
    sb = call(A, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})[0]
    step("local_only: peer request refused on the sender", sp == 200 and s == 403 and code(v) == "policy_denied"
         and sb == 200, privacy_statuses=[sp, sb], status=s, ms=ms)

    # 6. audit carries no prompt text (and holds this run's remote requests, so it was really read)
    for n, base in (("A", A), ("B", B)):
        s, hits, kinds = audit_clean(base)
        k = kinds.count("remote_request")
        step(f"audit on {n} has no prompt text", s == 200 and k > 0 and not hits, status=s, hits=hits, remote_entries=k)

    # 7. revocation: A revokes B; B's connection closes; B's requests to A are refused; then re-pair. Only from a
    # sound state: a failed re-pair leaves B revoked on the owner's device.
    if not (paired_ok and all(remote_ok)):
        step("revocation: skipped (pairing or remote execution failed; B left paired)", False)
    else:
        t7, b_addrs = utc(), [(peer(A, idb) or {}).get("last_addr"), call(B, "GET", "/api/network")[1].get("dial_addr")]
        s, v, _ = call(A, "POST", f"/api/peers/{idb}/revoke")
        closed = wait(lambda: not connected(B, ida), 10)
        s2, v2, _ = call(B, "POST", "/v1/completions", {"model": MODEL, "prompt": PROMPT, "max_tokens": 8, "target": f"peer:{ida}"})
        as_revoked = refused_as_revoked(A, b_addrs, t7)
        step("revocation: B disconnected and refused as revoked", s == 200 and closed is not None and s2 == 403
             and code(v2) in ("peer_revoked", "peer_unauthorized") and as_revoked, b_disconnected_after_s=closed,
             b_request_status=s2, b_error=code(v2) if isinstance(v2, dict) else v2, a_refused_as_revoked=as_revoked)
        s, start, _ = call(A, "POST", "/api/pair/start", {"name": names[1]})
        s3 = call(B, "POST", "/api/pair/join", {"addr": start.get("pair_addr"), "code": start.get("code"),
                                                "name": names[0]})[0] if s == 200 else None
        sg = call(A, "PUT", "/api/groups", {"name": "two-machine", "members": [idb], "policy": {"allow_remote_inference": True}})[0]
        again = wait(lambda: connected(A, idb) and connected(B, ida), 15) if s3 == 200 else None
        step("re-pair after revocation", s3 == 200 and sg == 200 and again is not None, start_status=s, join_status=s3,
             group_status=sg, connected_after_s=again)
        if s3 != 200:
            return f"A revoked B and the re-pair failed (pair/start {s}, join {s3}): pair the two again"

    # 8. loss detection and reconnection
    if a.kill_b and a.start_b:
        subprocess.run(a.kill_b, shell=True, capture_output=True)
        t_kill = time.monotonic()
        try:
            lost_s = wait(lambda: not connected(A, idb), 15)
            step("B killed: A notices the loss", lost_s is not None and lost_s <= 7.0, seconds=lost_s)
        finally:  # B's app starts again even when the run is aborted here
            subprocess.Popen(a.start_b, shell=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        back = wait(lambda: connected(A, idb), 60, 0.5)
        step("B restarted: the two reconnect", back is not None, seconds_after_restart=back,
             seconds_after_kill=round(time.monotonic() - t_kill, 1))
        if back is not None:
            wait(lambda: call(B, "GET", "/health")[0] == 200, 30, 0.5)
            call(B, "POST", f"/api/models/{MODEL}/load", {"device": "auto"}, timeout=300)
            remote(A, idb, "A -> B after B restarted")
    return None


def main():
    global RAW
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True)
    ap.add_argument("--b", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--kill-b")
    ap.add_argument("--start-b")
    ap.add_argument("--no-mdns", action="store_true", help="the network drops multicast (e.g. eduroam): discovery is N/A")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    RAW = open(os.path.join(a.out, "raw.jsonl"), "a")
    print(DRIVEN, flush=True)
    A, B = a.a, a.b
    na, nb = call(A, "GET", "/api/network")[1], call(B, "GET", "/api/network")[1]
    ida, idb = na["peer_id"], nb["peer_id"]
    print(f"A {na['short_label']} listen {na['listen_addr']}  |  B {nb['short_label']} listen {nb['listen_addr']}", flush=True)
    snap = os.path.join(a.out, "state-before.json")
    if state("save", snap, A, B):
        raise SystemExit("state.py save failed; nothing was changed")
    stranded = None
    try:
        stranded = steps(A, B, ida, idb, a)
    finally:  # also on a crash or Ctrl-C: groups (revoking drops B from A's), privacy, Sharing, model, names
        rc = state("restore", snap)
        step("restore: both daemons put back as saved (state.py)", rc == 0, exit_code=rc)
        with open(os.path.join(a.out, "results.json"), "w") as f:
            json.dump({"driven": DRIVEN, "a": na, "b": nb, "steps": RESULTS}, f, indent=2, default=str)
    failed = [r["step"] for r in RESULTS if r["ok"] is False]
    n_a = sum(r["ok"] is None for r in RESULTS)
    print("FAILED: " + ", ".join(failed) if failed else "ALL PASS" + (f" ({n_a} N/A)" if n_a else ""))
    if stranded:
        raise SystemExit(stranded)  # 1: the runner stops; its restore reports the lost pairing
    sys.exit(3 if failed else 0)  # 3: a step FAILed (N/A is not a failure)


if __name__ == "__main__":
    main()
