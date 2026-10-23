#!/usr/bin/env python3
"""Two-machine Disconnect run (the hold, E5): A disconnects B and connects again; a Sharing change keeps
the hold; revoking a held device still works and the two pair again; with --c/--d, revocations cross a hold.

    python3 -I hold_two_machine.py --a http://127.0.0.1:8192 --b http://127.0.0.1:18302 --out <dir> \
        [--c URL --d URL]

A and B are the two daemons' loopback APIs (B usually through an SSH tunnel); this drives the daemon APIs,
not the app windows. A and B must already be paired. --c and --d are two throwaway daemons (APIs reachable
from this script, peer endpoints reachable from A and B, e.g. on A's machine): they pair with A and B, and
their revocation records stay in A's and B's stores (no API removes one; they name keys nobody keeps).
Everything the run changes is saved first and put back at the end, also when a step fails or the run is
aborted (state.py: networking, Sharing, holds, pairings, groups, privacy, loaded model, names; it reports a
lost pairing instead of pairing again). Every HTTP call goes to <out>/raw.jsonl. Exit 3 when a step FAILed,
1 when the run stopped (B left revoked, or a crash). Stdlib only.
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
RESULTS = []
RAW = None
DRIVEN = "daemon API run (not app UI): disconnect, connect, Sharing, revoke and pairing go through the two daemons' loopback APIs"
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


def step(name, ok, **facts):
    RESULTS.append({"step": name, "ok": bool(ok), **facts})
    print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(facts, default=str), flush=True)


def step_na(name, why):
    RESULTS.append({"step": name, "ok": None, "na": why})
    print(f"N/A  {name} ({why})", flush=True)


def wait(cond, within, poll=0.25):
    t0 = time.monotonic()
    while time.monotonic() - t0 < within:
        if cond():
            return round(time.monotonic() - t0, 3)
        time.sleep(poll)
    return None


def peers(base):
    s, v, _ = call(base, "GET", "/api/peers")
    return v if s == 200 and isinstance(v, dict) else {"peers": [], "discovered": [], "revocations": []}


def peer(base, pid):
    return next((p for p in peers(base)["peers"] if p["id"] == pid), None)


def connected(base, pid):
    return bool((peer(base, pid) or {}).get("connected"))


def code(v):
    return (v.get("error") or {}).get("code") if isinstance(v, dict) else None


def run(src, dst_id, max_tokens=32):
    body = {"model": MODEL, "prompt": PROMPT, "max_tokens": max_tokens, "temperature": 0, "target": f"peer:{dst_id}"}
    s, v, ms = call(src, "POST", "/v1/completions", body, timeout=300)
    text = v["choices"][0]["text"] if s == 200 else None
    return s, v, ms, text


def events(base, since):
    """Audit events at or after `since` (second resolution); None when the audit could not be read, which is
    never the same as no lines."""
    s, v, _ = call(base, "GET", "/api/audit?n=1000")
    if s != 200 or not isinstance(v, dict) or "events" not in v:
        return None
    return [e for e in v["events"] if e["ts_utc"] >= since]


def held_rejections(base, pid, since):
    ev = events(base, since)
    return None if ev is None else [e for e in ev if e["kind"] == "peer_rejected" and e.get("peer_id") == pid
                                    and (e.get("fields") or {}).get("reason") == "held"]


def refused_as_revoked(base, me, pid, net, addrs, since):
    """`base`'s own verdict on `pid`'s dials since `since`: the dialler cannot tell (a refusal before the device
    proved its key reads as peer_unauthorized), and an address alone proves little (on one host every daemon shares
    it). So (1) `base` holds a revocation of `pid`'s key that `base` (`me`) signed, and (2) it refused a revoked key
    from `pid`'s own endpoint: a known host of `pid` (`addrs`, its dial_addr) at the port `pid` listens on (`net`
    is `pid`'s GET /api/network; a daemon dials from its listen socket, which no other process there holds).
    The refusal line names no key: (1) ties the key to `pid`, (2) the socket. Returns both."""
    signed = any(r.get("peer_id") == pid and r.get("revoked_by") == me for r in peers(base).get("revocations") or [])
    port = (net.get("listen_addr") or "").rpartition(":")[2]
    ends = {x.rpartition(":")[0] + ":" + port for x in [*addrs, net.get("dial_addr")] if x and port}
    refused = any(e["kind"] == "peer_rejected" and (e.get("fields") or {}).get("reason") == "peer_revoked"
                  and (e.get("fields") or {}).get("addr") in ends for e in events(base, since) or [])
    return signed, refused


def pair_with(show, join):
    """`join` pairs with `show` through a real pairing window; returns join's status."""
    s, start, _ = call(show, "POST", "/api/pair/start", {})
    if s != 200:
        return s
    s, _, _ = call(join, "POST", "/api/pair/join", {"addr": start.get("pair_addr"), "code": start.get("code")})
    return s


def cross_hold(A, B, C, D, ida, idb):
    """ADR-0003 D7 across a hold (review finding trust-5): A holds B; A revokes C and B revokes D; B's next dial
    to A is refused (peer_held) but carries both revocations, so B drops C and A drops D, and A still holds B."""
    idc = call(C, "GET", "/api/network")[1]["peer_id"]
    idd = call(D, "GET", "/api/network")[1]["peer_id"]
    paired = [pair_with(x, y) for x in (C, D) for y in (A, B)]
    for base, others in ((A, [idb, idc, idd]), (B, [ida, idc, idd])):
        s, _, _ = call(base, "PUT", "/api/groups", {"name": "two-machine", "members": others,
                                                    "policy": {"allow_remote_inference": True}})
        paired.append(s)
    pins = all(peer(A, x) for x in (idc, idd)) and all(peer(B, x) for x in (idc, idd))
    step("cross-hold setup: C and D paired with both A and B, in a shared group", paired == [200] * 6 and pins,
         statuses=paired)
    s1, _, _ = call(A, "POST", f"/api/peers/{idb}/disconnect")
    held = wait(lambda: (peer(A, idb) or {}).get("held") is True and not connected(B, ida), 10)
    s2, _, _ = call(A, "POST", f"/api/peers/{idc}/revoke")
    s3, _, _ = call(B, "POST", f"/api/peers/{idd}/revoke")
    s4, v4, ms4 = call(B, "POST", f"/api/peers/{ida}/connect", {}, timeout=90)
    b_drops_c = wait(lambda: peer(B, idc) is None, 20, 0.5)
    a_drops_d = wait(lambda: peer(A, idd) is None, 20, 0.5)
    still = (peer(A, idb) or {}).get("held") is True and not connected(B, ida)
    step("cross-hold: revocations cross a hold both ways; the hold stays (trust-5, ADR-0003 D7)",
         held is not None and (s1, s2, s3) == (200, 200, 200) and s4 == 403 and code(v4) == "peer_held"
         and b_drops_c is not None and a_drops_d is not None and still,
         connect_status=s4, connect_code=code(v4), connect_ms=ms4, b_dropped_c_after_s=b_drops_c,
         a_dropped_d_after_s=a_drops_d, still_held=still,
         note="C's and D's revocation records stay in A's and B's stores (no API removes them)")
    call(A, "POST", f"/api/peers/{idb}/connect", {}, timeout=90)  # end the hold (state.py restore would too)
    return wait(lambda: connected(A, idb) and connected(B, ida), 15)


def steps(A, B, ida, idb, a, a_serve):
    """Steps 0-6; returns why the run must stop (B left revoked), else None."""
    # 0. both connected, a remote run works (the baseline the hold takes away)
    if not connected(A, idb):
        call(A, "POST", f"/api/peers/{idb}/connect", {}, timeout=90)
    setup = []
    for base, other in ((A, idb), (B, ida)):
        setup.append(call(base, "PUT", "/api/groups", {"name": "two-machine", "members": [other],
                                                       "policy": {"allow_remote_inference": True}})[0])
        setup.append(call(base, "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})[0])
    setup.append(call(B, "POST", f"/api/models/{MODEL}/pull", timeout=600)[0])
    setup.append(call(B, "POST", f"/api/models/{MODEL}/load", {"device": "auto"}, timeout=300)[0])
    s, v, ms, text = run(A, idb)
    step("baseline: connected, A -> B gives the golden completion", all(x == 200 for x in setup) and s == 200
         and text == GOLDEN, setup_statuses=setup, status=s, ms=ms, target=v.get("target") if s == 200 else v)

    # 1. A disconnects B: both sides show it, and B is told why
    t_hold = utc()  # before the hold: its one held line comes from B's first redial, during `told`
    s, v, ms = call(A, "POST", f"/api/peers/{idb}/disconnect")
    a_down = wait(lambda: (lambda p: p and not p.get("connected") and p.get("held"))(peer(A, idb)), 5)
    b_down = wait(lambda: not connected(B, ida), 10)
    step("disconnect: A shows B held and not connected, B shows A not connected", s == 200 and a_down is not None
         and b_down is not None, status=s, ms=ms, a_after_s=a_down, b_after_s=b_down)
    # B's `last_error` is its latest failed dial: B's own redial, refused by A, says why (the app then
    # shows "Disconnected by <A>"), with nothing done on either side.
    told = wait(lambda: ((peer(B, ida) or {}).get("last_error") or {}).get("code") == "peer_held", 15, 0.5)
    step("disconnect: B's own redial is refused and B shows why (peer_held)", told is not None,
         b_after_s=told, b_last_error=(peer(B, ida) or {}).get("last_error"))

    # 2. while held, neither side's requests reach the other
    s, v, ms, _ = run(A, idb, 8)
    step("held: A's own request to B is refused on A, before anything is sent", s == 403 and code(v) == "policy_denied"
         and ms < 1000, status=s, code=code(v), ms=ms)
    s, v, ms, _ = run(B, ida, 8)
    step("held: B's request to A is refused (peer_held)", s == 403 and code(v) == "peer_held", status=s, code=code(v), ms=ms)
    e0 = (peer(B, ida) or {}).get("last_error") or {}
    back = wait(lambda: connected(A, idb) or connected(B, ida), a.watch, 1.0)
    e1 = (peer(B, ida) or {}).get("last_error") or {}
    rej = held_rejections(A, idb, t_hold)  # before step 3: the Sharing restart starts a fresh once-per-hold line
    step(f"held: no reconnection for {a.watch} s and exactly one audit line on A for B's attempts",
         back is None and rej is not None and len(rej) == 1, reconnected_after_s=back,
         held_rejections=None if rej is None else len(rej), b_redialled_during_watch=e1.get("at_utc", "") > e0.get("at_utc", ""))
    # B's last_error is its latest failed dial. Each walk dials A's mDNS addresses, then the pinned one (mesh.rs
    # dial_order; no IPv6 address from an IPv4 endpoint); a walk that went on past the refusal would leave the
    # failure at a later address as the latest. So only peer_held at an address B's walk has a later one for proves
    # the refusal ended it; A's own held line shows the refusal was A's (the pinned key's, already proven to B).
    # ponytail: the order is read after the dials; an mDNS change in between can misplace an address (rare in a watch).
    b_rec, nearby = peer(B, ida) or {}, next((d for d in peers(B)["discovered"] if d["id"] == ida), None) or {}
    v4 = not (call(B, "GET", "/api/network")[1].get("listen_addr") or "").startswith("[")
    order = [x for x in dict.fromkeys([nearby.get("addr"), *(nearby.get("addrs") or []), b_rec.get("last_addr")])
             if x and not (v4 and x.startswith("["))]
    at = e1.get("addr")
    later = order[order.index(at) + 1:] if at in order else []
    name = "held: a proven refusal ends B's dial (no later address replaces peer_held)"
    if len(order) < 2:
        step_na(name, "B knows no address for A but the pinned one (no mDNS record), so nothing follows the refusal")
    elif e1.get("code") == "peer_held" and not later:
        step_na(name, f"A refused B at {at}, with no address after it in B's dial order {order} (mDNS candidates, "
                      "then the pinned one), so nothing followed the refusal that could have replaced it")
    else:
        step(name, back is None and e1.get("code") == "peer_held" and bool(rej), dial_order=order,
             refused_at=at, after_it_in_the_walk=later, b_last_error=e1, a_held_lines=None if rej is None else len(rej))

    # 3. a Sharing change restarts A's endpoint but keeps the hold
    t3, puts = utc(), []
    for want in (not a_serve, a_serve):
        s, v, ms = call(A, "PUT", "/api/network", {"serve_peers": want})
        puts.append({"want": want, "status": s, "ms": ms, "serve_peers": v.get("serve_peers") if isinstance(v, dict) else v,
                     "enabled": v.get("enabled") if isinstance(v, dict) else None})
    applied = all(x["status"] == 200 and x["serve_peers"] == x["want"] and x["enabled"] is True for x in puts)
    back = wait(lambda: connected(A, idb), 10, 0.5)
    p = peer(A, idb) or {}
    restarts = [e for e in events(A, t3) or [] if e["kind"] == "network_changed"]  # a fact, not a gate
    step("held: turning Sharing off and on keeps the hold", applied and back is None and p.get("held") is True,
         puts=puts, reconnected_after_s=back, held=p.get("held"), network_changed_audits=len(restarts))

    # 4. Connect ends the hold (only a hold in place right before counts)
    pre = peer(A, idb) or {}
    was_held = pre.get("held") is True and not pre.get("connected")
    s, v, ms = call(A, "POST", f"/api/peers/{idb}/connect", {}, timeout=90)
    both = wait(lambda: connected(A, idb) and connected(B, ida), 15)
    p = peer(A, idb) or {}
    step("connect: ends the hold, both sides connected", was_held and s == 200 and both is not None
         and p.get("held") is False, held_before=pre.get("held"), connected_before=pre.get("connected"),
         status=s, ms=ms, both_after_s=both, held=p.get("held"))
    sl = call(B, "POST", f"/api/models/{MODEL}/load", {"device": "auto"}, timeout=300)[0]
    s, v, ms, text = run(A, idb)
    step("connect: A -> B gives the golden completion again", sl == 200 and s == 200 and text == GOLDEN,
         load_status=sl, status=s, ms=ms)

    # 5. revoking a held device works, and the two can pair again (unheld). Only from a sound state: a failed
    # re-pair leaves B revoked on the owner's device.
    s, v, _ = call(A, "GET", "/api/network")
    net_on = s == 200 and isinstance(v, dict) and v.get("enabled") is True
    if not (net_on and all(r["ok"] for r in RESULTS[-2:])):
        step("revoke while held: skipped (connect, run or networking failed; B left paired)", False, network_enabled=net_on)
        return None
    names = (pre.get("name"), (peer(B, ida) or {}).get("name"))  # what A calls B, what B calls A: kept on re-pairing
    s0, _, _ = call(A, "POST", f"/api/peers/{idb}/disconnect")
    rec = peer(A, idb) or {}
    held = rec.get("held") is True
    wait(lambda: not connected(B, ida), 10)
    t5 = utc()
    s, v, ms = call(A, "POST", f"/api/peers/{idb}/revoke")
    s2, v2, ms2, _ = run(B, ida, 8)
    signed, as_revoked = refused_as_revoked(A, ida, idb, call(B, "GET", "/api/network")[1], [rec.get("last_addr")], t5)
    step("revoke while held: A unpins B, B's request to A is refused as revoked", s0 == 200 and held and s == 200
         and peer(A, idb) is None and s2 == 403 and code(v2) in ("peer_revoked", "peer_unauthorized") and signed
         and as_revoked, disconnect_status=s0, held=held, revoke_status=s, b_status=s2, b_code=code(v2),
         a_holds_its_revocation_of_b=signed, a_refused_as_revoked=as_revoked)
    s, start, _ = call(A, "POST", "/api/pair/start", {"name": names[1]})
    s3 = call(B, "POST", "/api/pair/join", {"addr": start.get("pair_addr"), "code": start.get("code"),
                                            "name": names[0]})[0] if s == 200 else None
    again = wait(lambda: connected(A, idb) and connected(B, ida), 15) if s3 == 200 else None
    p = peer(A, idb) or {}
    step("re-pair after revoking a held device: connected, not held", s3 == 200 and again is not None
         and p.get("held") is False, start_status=s, join_status=s3, connected_after_s=again, held=p.get("held"))
    if s3 != 200:
        return f"A revoked B and the re-pair failed (pair/start {s}, join {s3}): pair the two again"

    # 6. revocations cross a hold (needs two more daemons)
    if a.c and a.d:
        cross_hold(A, B, a.c, a.d, ida, idb)
    else:
        step_na("cross-hold: revocations cross a hold both ways; the hold stays (trust-5, ADR-0003 D7)",
                "--c and --d not given (two throwaway daemons that A and B can reach)")
    return None


def main():
    global RAW
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True)
    ap.add_argument("--b", required=True)
    ap.add_argument("--c", help="a throwaway daemon's API URL (with --d: the cross-hold step)")
    ap.add_argument("--d", help="a second throwaway daemon's API URL")
    ap.add_argument("--out", required=True)
    ap.add_argument("--watch", type=int, default=30, help="seconds to watch B being refused while held")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    RAW = open(os.path.join(a.out, "raw.jsonl"), "a")
    print(DRIVEN, flush=True)
    A, B = a.a, a.b
    na, nb = call(A, "GET", "/api/network")[1], call(B, "GET", "/api/network")[1]
    ida, idb = na["peer_id"], nb["peer_id"]
    print(f"A {na['device_name']} {na['short_label']}  |  B {nb['device_name']} {nb['short_label']}", flush=True)
    if peer(A, idb) is None or peer(B, ida) is None:
        raise SystemExit("A and B must be paired first")
    snap = os.path.join(a.out, "state-before.json")
    if state("save", snap, A, B):
        raise SystemExit("state.py save failed; nothing was changed")
    stranded = None
    try:
        stranded = steps(A, B, ida, idb, a, na["serve_peers"])
    finally:  # also on a crash or Ctrl-C: holds, groups, privacy, Sharing and the model go back
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
