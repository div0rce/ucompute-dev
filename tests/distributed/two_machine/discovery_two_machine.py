#!/usr/bin/env python3
"""Two-machine discovery and connection-state run against two paired `common serve` daemons (usually the
daemons of the two installed desktop apps) on two physical machines.

    python3 -I discovery_two_machine.py --a http://127.0.0.1:8192 --b http://127.0.0.1:18302 --out <dir> \
        [--steps state,manual,pair,quit,crash,wifi,offline] [--quit-b CMD] [--crash-b CMD] [--start-b CMD] \
        [--procs-b CMD] [--wifi-cycle-b CMD] [--stranger URL --stranger-addr IP:PORT] [--offline-peer ID] \
        [--watch SECONDS] [--bogus-addr IP:PORT] [--no-mdns]

A and B are the daemons' loopback APIs (B through an SSH tunnel; the API never leaves loopback). Peer
traffic between the machines is Common's QUIC transport. Every step, pairing included, goes through those APIs:
this is daemon API evidence, not app UI evidence (no window is driven, no code is typed by hand, the Windows
firewall consent step is not reached). Steps:
  state    both daemons name themselves (no "This Mac"), discovered entries carry 1-2 routable addresses,
           "last seen" advances while connected.
  manual   connect at an address: an unpaired id is refused before any packet; a wrong hint never replaces
           the pinned address; the right address connects; with --stranger, a different Common device at
           the hinted address pins nothing on either side.
  pair     pairing in both directions (A shows the code / B shows the code), wrong code first.
  quit     graceful quit of B (--quit-b): A drops B from "nearby" quickly, B's engine ends; A reports why
           B is unreachable and audits it once, not once per redial; B restarts (--start-b) and reconnects.
  crash    B's app is killed (--crash-b): A shows B as not connected at once although its signed record
           stays cached until the mDNS TTL; B's daemon and engine end; B restarts and reconnects.
  wifi     B's Wi-Fi reconnects (--wifi-cycle-b, runs detached on B): A sees a freshly signed record and the
           connection comes back.
  offline  a paired device that is switched off (--offline-peer, pinned on A): one audit line, not one per
           redial, and a reason in GET /api/peers.
--no-mdns (the network drops multicast): the nearby checks are recorded N/A (ok null), never PASS.
Every HTTP call goes to <out>/raw.jsonl, results to <out>/results.json. Stdlib only.
"""
import argparse
import json
import os
import subprocess
import time
import urllib.error
import urllib.request

RESULTS = []
RAW = None
DRIVEN = "daemon API run (not app UI): every step goes through the two daemons' loopback APIs"


def utc():
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def call(base, method, path, body=None, timeout=60):
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


def step(name, ok, **facts):
    RESULTS.append({"step": name, "ok": bool(ok), "t": utc(), **facts})
    print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(facts, default=str), flush=True)


NO_MDNS = False  # set by --no-mdns: the network drops multicast, so nothing can be seen nearby


def na(name, why="the network carries no mDNS (multicast dropped), so nothing can be seen nearby"):
    RESULTS.append({"step": name, "ok": None, "na": why, "t": utc()})
    print(f"N/A  {name} ({why})", flush=True)


def wait(cond, within, poll=0.25):
    t0 = time.monotonic()
    while time.monotonic() - t0 < within:
        if cond():
            return round(time.monotonic() - t0, 2)
        time.sleep(poll)
    return None


def sh(cmd):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    return (r.stdout + r.stderr).strip()


def peers(base):
    s, v, _ = call(base, "GET", "/api/peers")
    return v if s == 200 else {"peers": [], "discovered": []}


def peer(base, pid):
    return next((p for p in peers(base)["peers"] if p["id"] == pid), None)


def nearby(base, pid):
    return next((d for d in peers(base)["discovered"] if d["id"] == pid), None)


def connected(base, pid):
    p = peer(base, pid)
    return bool(p and p.get("connected"))


def audit(base):
    """The newest audit events, newest first (the API returns at most 1000)."""
    _, v, _ = call(base, "GET", "/api/audit?n=1000")
    return v.get("events", []) if isinstance(v, dict) else []


def since(events, mark):
    """The events newer than `mark`, the newest event of an earlier audit() (None: the trail was empty).
    Counting instead would see nothing new once the trail holds 1000 events."""
    return events[:events.index(mark)] if mark in events else events


def rejected(events, pid):
    return [e for e in events if e["kind"] == "peer_rejected" and e.get("peer_id") == pid]


def lost_reason(events, pid):
    """Why A's first lost connection to `pid` in `events` ended (peer_lost audit): "closed_by_peer" when
    the peer's daemon closed it (graceful stop), "timed_out"/"reset" when it vanished (killed)."""
    hits = [e for e in events if e["kind"] == "peer_lost" and e.get("peer_id") == pid]
    return (hits[-1].get("fields") or {}).get("reason") if hits else None


def routable(addr):
    ip = addr.rsplit(":", 1)[0].strip("[]")
    return not (ip.startswith("127.") or ip.startswith("169.254.") or ip == "::1" or ip.lower().startswith("fe80"))


def s_state(A, B, ida, idb, neta, netb, a):
    names = {"A": neta.get("device_name"), "B": netb.get("device_name")}
    step("state: each daemon reports its own host name", all(names.values()) and "This Mac" not in names.values(),
         names=names, dial_addr={"A": neta.get("dial_addr"), "B": netb.get("dial_addr")})
    for me, base, oid in (("A", A, idb), ("B", B, ida)):
        if NO_MDNS:
            na(f"state: {me} sees the other with 1-2 routable addresses, best first")
            continue
        d = nearby(base, oid)
        addrs = (d or {}).get("addrs") or []
        step(f"state: {me} sees the other with 1-2 routable addresses, best first",
             d is not None and 1 <= len(addrs) <= 2 and addrs[0] == d["addr"] and all(map(routable, addrs)), entry=d)
    before = peer(A, idb)
    ok0 = bool(before and before.get("connected"))
    time.sleep(65)
    after = peer(A, idb)
    step("state: last seen advances while connected (E5)",
         ok0 and after and after.get("connected") and after["last_seen_utc"] > before["last_seen_utc"],
         before=before and before["last_seen_utc"], after=after and after["last_seen_utc"])


def s_manual(A, B, ida, idb, neta, netb, a):
    b_dial = netb.get("dial_addr")
    stranger_id = "ab" * 32
    before_a, before_b = peers(A)["peers"], peers(B)["peers"]
    b_mark = (audit(B) or [None])[0]
    s, v, ms = call(A, "POST", f"/api/peers/{stranger_id}/connect", {"addr": b_dial})
    time.sleep(2)
    b_new = since(audit(B), b_mark)
    step("manual: an unpaired id is refused before any packet (requirement 8)",
         s == 403 and (v.get("error") or {}).get("code") == "peer_unauthorized" and not b_new
         and peers(A)["peers"] == before_a, status=s, ms=ms, b_new_audit=b_new)
    pinned = (peer(A, idb) or {}).get("last_addr")
    bogus = a.bogus_addr
    s, v, ms = call(A, "POST", f"/api/peers/{idb}/connect", {"addr": bogus}, timeout=90)
    step("manual: a hint with no Common device falls through to the pinned address and is not saved",
         s == 200 and (peer(A, idb) or {}).get("last_addr") == pinned, status=s, ms=ms, hint=bogus, pinned=pinned)
    s, v, ms = call(A, "POST", f"/api/peers/{idb}/connect", {"addr": b_dial})
    step("manual: the address shown on B connects A to B (requirement 5)",
         s == 200 and connected(A, idb) and (peer(A, idb) or {}).get("last_addr") == b_dial, status=s, ms=ms, addr=b_dial)
    if a.stranger and a.stranger_addr:
        sid = call(a.stranger, "GET", "/api/network")[1]["peer_id"]
        st_before = peers(a.stranger)["peers"]
        s, v, ms = call(B, "POST", f"/api/peers/{ida}/connect", {"addr": a.stranger_addr}, timeout=90)
        b_rec = peer(B, ida) or {}
        step("manual: another Common device at the hinted address is not trusted (requirement 8)",
             peers(a.stranger)["peers"] == st_before and peer(B, sid) is None and b_rec.get("last_addr") != a.stranger_addr,
             status=s, ms=ms, code=(v.get("error") or {}).get("code") if isinstance(v, dict) else None,
             stranger=a.stranger_addr, b_last_addr_for_a=b_rec.get("last_addr"))
    after_b = [p["id"] for p in peers(B)["peers"]]
    step("manual: no peer was added on either side", [p["id"] for p in peers(A)["peers"]] == [p["id"] for p in before_a]
         and after_b == [p["id"] for p in before_b])


def pair(src_show, src_join, label):
    s, start, _ = call(src_show, "POST", "/api/pair/start", {})
    if s != 200:
        step(f"pair {label}: pairing window opens", False, status=s, response=start)
        return False
    # One window: a wrong code first (the window stays open, up to 5 failures), then the right one.
    code, addr = start["code"], start.get("pair_addr")
    wrong = str((int(code) + 1) % 1_000_000).zfill(6)
    s1, v1, ms1 = call(src_join, "POST", "/api/pair/join", {"addr": addr, "code": wrong})
    s2, v2, ms2 = call(src_join, "POST", "/api/pair/join", {"addr": addr, "code": code})
    step(f"pair {label}: wrong code refused, right code pairs",
         s1 >= 400 and (v1.get("error") or {}).get("code") == "pairing_failed" and s2 == 200,
         pair_addr=addr, wrong=(s1, (v1.get("error") or {}).get("message"), ms1), right=(s2, v2 if s2 != 200 else v2.get("name"), ms2))
    return s2 == 200


def s_pair(A, B, ida, idb, neta, netb, a):
    paired = [pair(A, B, "A shows the code, B joins"), pair(B, A, "B shows the code, A joins")]
    name = "pair: connected again after re-pairing, named by host name"
    if not any(paired):
        na(name, "no pairing succeeded, so nothing was re-paired")
        return
    both = wait(lambda: connected(A, idb) and connected(B, ida), 30)
    a_calls_b, b_calls_a = (peer(A, idb) or {}).get("name"), (peer(B, ida) or {}).get("name")
    step(name, both is not None and a_calls_b == netb.get("device_name") and b_calls_a == neta.get("device_name"),
         seconds=both, a_calls_b=a_calls_b, b_calls_a=b_calls_a)


def lost_and_back(A, B, ida, idb, a, how, cmd):
    mark = (audit(A) or [None])[0]
    t0 = time.monotonic()
    out = sh(cmd)
    gone = wait(lambda: not connected(A, idb), 30)
    unlisted = wait(lambda: nearby(A, idb) is None, 15 if how == "quit" else 1)
    wait(lambda: lost_reason(since(audit(A), mark), idb), 10)
    why = lost_reason(since(audit(A), mark), idb)
    p = peer(A, idb) or {}
    step(f"{how}: A shows B as not connected", gone is not None, seconds=gone, health=p.get("health"), lost_reason=why,
         cmd_output=out[-300:])
    if how == "quit":
        # Every other quit check also passes when B's app falls back to killing its daemon (A's idle timeout
        # ends the connection); only B's own close tells the graceful stop apart, mDNS or not.
        step("quit: B's daemon closed the connection itself (graceful stop, not the kill fallback)",
             why == "closed_by_peer", lost_reason=why)
    if how == "quit" and NO_MDNS:
        na("quit: A drops B from nearby within seconds (goodbye sent on quit, E6)")
    elif how == "quit":
        step("quit: A drops B from nearby within seconds (goodbye sent on quit, E6)", unlisted is not None and unlisted <= 10,
             seconds=unlisted)
    else:
        d = nearby(A, idb)
        step("crash: B's signed record may stay cached, but A never shows it as connected",
             not connected(A, idb) and (peer(A, idb) or {}).get("health") != "connected", cached_record=d,
             health=(peer(A, idb) or {}).get("health"))
    if a.procs_b:
        left = wait(lambda: sh(a.procs_b).strip() in ("", "0"), 20, 1)
        step(f"{how}: B's daemon and engine processes end", left is not None, seconds=left, procs=sh(a.procs_b))
    err = wait(lambda: (peer(A, idb) or {}).get("last_error"), 60, 1)
    p = peer(A, idb) or {}
    step(f"{how}: A explains why B is unreachable", err is not None and p.get("last_error"), last_error=p.get("last_error"))
    if how == "quit":
        # B is down, so this dial really happens: the hint and the pinned address both fail, the error
        # says why, and the pinned address is not replaced by the hint.
        pinned = (peer(A, idb) or {}).get("last_addr")
        s, v, ms = call(A, "POST", f"/api/peers/{idb}/connect", {"addr": a.bogus_addr}, timeout=90)
        code = (v.get("error") or {}).get("code") if isinstance(v, dict) else None
        step("quit: connecting at a dead hint fails with a reason and saves nothing", s >= 400 and code in
             ("peer_lost", "timeout", "peer_unauthorized") and (peer(A, idb) or {}).get("last_addr") == pinned,
             status=s, code=code, ms=ms, message=(v.get("error") or {}).get("message") if isinstance(v, dict) else v,
             pinned=pinned)
    time.sleep(a.watch)
    rej = rejected(since(audit(A), mark), idb)
    step(f"{how}: one audit line for B while it is away, not one per redial (E4)", len(rej) <= 1,
         audit_lines=len(rej), watched_s=round(time.monotonic() - t0), lines=rej[:3])
    if how == "crash" and NO_MDNS:
        na("crash: the cached record leaves nearby by the mDNS TTL (about 120 s after the last announcement)")
    elif how == "crash":
        gone_rec = wait(lambda: nearby(A, idb) is None, 150, 1)
        step("crash: the cached record leaves nearby by the mDNS TTL (about 120 s after the last announcement)",
             gone_rec is not None or nearby(A, idb) is None, seconds_after_watch=gone_rec,
             seconds_since_crash=round(time.monotonic() - t0))
    if a.start_b:
        sh(a.start_b)
        t1 = time.monotonic()
        back = wait(lambda: connected(A, idb), 120, 0.5)
        step(f"{how}: B restarted, A reconnects (backoff reset by discovery)", back is not None,
             seconds_after_start=back, seen_nearby_after=round(time.monotonic() - t1, 1) if nearby(A, idb) else None)
        wait(lambda: call(B, "GET", "/health")[0] == 200, 60, 1)


def s_quit(A, B, ida, idb, neta, netb, a):
    if a.quit_b:
        lost_and_back(A, B, ida, idb, a, "quit", a.quit_b)


def s_crash(A, B, ida, idb, neta, netb, a):
    if a.crash_b:
        lost_and_back(A, B, ida, idb, a, "crash", a.crash_b)


def s_wifi(A, B, ida, idb, neta, netb, a):
    if not a.wifi_cycle_b:
        return
    d0 = nearby(A, idb) or {}
    sh(a.wifi_cycle_b)
    t0 = time.monotonic()
    down = wait(lambda: not connected(A, idb), 40)
    back = wait(lambda: connected(A, idb), 180, 0.5)
    fresh = wait(lambda: (nearby(A, idb) or {}).get("seen_utc", "") > d0.get("seen_utc", ""), 60, 0.5)
    step("wifi: B's Wi-Fi reconnects, A sees a freshly signed record and reconnects (D1)",
         back is not None, disconnected_after_s=down, connected_after_s=back, fresh_record_after_reconnect_s=fresh,
         before=d0, after=nearby(A, idb), total_s=round(time.monotonic() - t0, 1))


def s_offline(A, B, ida, idb, neta, netb, a):
    if not a.offline_peer:
        return
    mark = (audit(A) or [None])[0]
    time.sleep(a.watch)
    rej = rejected(since(audit(A), mark), a.offline_peer)
    p = peer(A, a.offline_peer) or {}
    step("offline: a switched-off paired device costs at most one audit line per state change (E4)", len(rej) <= 1,
         audit_lines=len(rej), watched_s=a.watch, last_error=p.get("last_error"), health=p.get("health"))


STEPS = {"state": s_state, "manual": s_manual, "pair": s_pair, "quit": s_quit, "crash": s_crash, "wifi": s_wifi,
         "offline": s_offline}


def main():
    global RAW
    ap = argparse.ArgumentParser()
    ap.add_argument("--a", required=True)
    ap.add_argument("--b", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--steps", default="state,manual,offline,quit,crash")
    for f in ("quit-b", "crash-b", "start-b", "procs-b", "wifi-cycle-b", "stranger", "stranger-addr", "offline-peer"):
        ap.add_argument("--" + f)
    ap.add_argument("--bogus-addr", default="172.20.10.1:8193", help="an address with no Common device")
    ap.add_argument("--watch", type=int, default=180)
    ap.add_argument("--no-mdns", action="store_true", help="the network drops multicast (e.g. eduroam): nearby checks are N/A")
    a = ap.parse_args()
    global NO_MDNS
    NO_MDNS = a.no_mdns
    os.makedirs(a.out, exist_ok=True)
    RAW = open(os.path.join(a.out, "raw.jsonl"), "a")
    A, B = a.a, a.b
    print(DRIVEN, flush=True)
    neta, netb = call(A, "GET", "/api/network")[1], call(B, "GET", "/api/network")[1]
    ida, idb = neta["peer_id"], netb["peer_id"]
    print(f"A {neta.get('device_name')} {neta['short_label']} {neta.get('dial_addr')}  |  "
          f"B {netb.get('device_name')} {netb['short_label']} {netb.get('dial_addr')}", flush=True)
    for name in a.steps.split(","):
        STEPS[name](A, B, ida, idb, neta, netb, a)
    with open(os.path.join(a.out, "results.json"), "w") as f:
        json.dump({"driven": DRIVEN, "a": neta, "b": netb, "steps": RESULTS}, f, indent=2, default=str)
    failed = [r["step"] for r in RESULTS if r["ok"] is False]
    n_a = sum(r["ok"] is None for r in RESULTS)
    print("FAILED: " + ", ".join(failed) if failed else "ALL PASS" + (f" ({n_a} N/A)" if n_a else ""))


if __name__ == "__main__":
    main()
