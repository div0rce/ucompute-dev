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
           the hinted address pins nothing on either side. Each connect follows a Disconnect, so it dials
           (an open connection is returned as is); the audit trail shows the dial.
  pair     pairing in both directions (A shows the code / B shows the code), wrong code first; the two stay
           connected and call each other by host name.
  quit     graceful quit of B (--quit-b): A drops B from "nearby" quickly, B's engine ends; A reports why
           B is unreachable and audits it once, not once per redial; B restarts (--start-b) and reconnects.
  crash    B's app is killed (--crash-b): A shows B as not connected at once although its signed record
           stays cached until the mDNS TTL; B's daemon and engine end; B restarts and reconnects.
  wifi     B's Wi-Fi drops and reconnects (--wifi-cycle-b, runs detached on B): A loses B, a new connection
           forms and A hears B's record again (that it is freshly signed is not visible through the API: N/A).
           B's address afterwards goes to <out>/b_host (a new lease can move it); the run stops when B does not
           come back. Run it last among the steps that talk to B.
  offline  a paired device that is switched off (--offline-peer, pinned on A): one audit line per address,
           not one per redial, and a reason in GET /api/peers.
--no-mdns (the network drops multicast): the nearby checks are recorded N/A (ok null), never PASS; so is a
step whose flag is missing or whose premise was not met. What the run changes is saved first and put back at
the end (state.py). Every HTTP call goes to <out>/raw.jsonl, results to <out>/results.json. Exit 3 when a step
FAILed, 1 when the run stopped. Stdlib only.
"""
import argparse
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

RESULTS = []
RAW = None
DRIVEN = "daemon API run (not app UI): every step goes through the two daemons' loopback APIs"
STATE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "state.py")


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


def state(*args):
    """state.py save/restore, as a subprocess (under -I this script cannot import it); its exit code."""
    return subprocess.run([sys.executable, "-I", STATE, *args]).returncode


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


def conn(base, pid):
    """The connection in use to `pid` (id, direction, remote_addr), {} when there is none."""
    return (peer(base, pid) or {}).get("connection") or {}


def audit(base):
    """The last 1000 audit events (the API's cap), oldest first (the API's order). A failed read is a FAIL
    (recorded once per daemon), never "no lines"."""
    s, v, _ = call(base, "GET", "/api/audit?n=1000")
    if s == 200 and isinstance(v, dict) and "events" in v:
        return v["events"]
    name = f"audit: the audit trail on {base} can be read"
    if not any(r["step"] == name for r in RESULTS):
        step(name, False, status=s, response=v)
    return []


def mark_of(events):
    """A place in the trail: its newest three events (identical events can repeat, three in a row hardly)."""
    return events[-3:]


def since(events, mark):
    """The events newer than `mark` (from mark_of on an earlier audit()). Counting instead would see nothing
    new once the trail holds 1000 events; a mark that slid out of the window means every event is newer.
    The first match wins: identical events in one second can only make an old event count (a FAIL to look
    at), never hide a new one."""
    n = len(mark)
    for i in range(0, len(events) - n + 1) if n else ():
        if events[i:i + n] == mark:
            return events[i + n:]
    return events


def ids(p):
    """The paired ids in a GET /api/peers answer (records also carry last-seen times, which move)."""
    return sorted(x["id"] for x in p["peers"])


def rejected(events, pid):
    return [e for e in events if e["kind"] == "peer_rejected" and e.get("peer_id") == pid]


def once_per_address(rej):
    """A failed dial is audited once per (address, reason) since the last connection, not once per redial."""
    keys = [((e.get("fields") or {}).get("addr"), (e.get("fields") or {}).get("reason")) for e in rej]
    return len(set(keys)) == len(keys)


def dialled(events, pid, addr, kind="peer_rejected"):
    """An outbound dial to `pid` at `addr` in `events`: refused there (peer_rejected) or connected (peer_connected)."""
    return any(e["kind"] == kind and e.get("peer_id") == pid and (e.get("fields") or {}).get("addr") == addr
               and (e.get("fields") or {}).get("direction") == "outbound" for e in events)


def fresh_dial(base, pid, hint, timeout=60):
    """Disconnect `pid` (a hold) and then connect at `hint`: an open connection would be returned as is, and
    connect ends the hold, so this one dials. Returns status, body, ms, the connection afterwards, whether it is
    a new one, and the audit lines written since the disconnect."""
    old = conn(base, pid).get("id")
    sd = call(base, "POST", f"/api/peers/{pid}/disconnect")[0]
    wait(lambda: not connected(base, pid), 10)
    mark = mark_of(audit(base))
    s, v, ms = call(base, "POST", f"/api/peers/{pid}/connect", {"addr": hint}, timeout=timeout)
    c = conn(base, pid)
    return s, v, ms, c, sd == 200 and c.get("id") not in (None, old), since(audit(base), mark)


def lost_reason(events, pid):
    """Why A's first lost connection to `pid` in `events` ended (peer_lost audit): "closed_by_peer" when
    the peer's daemon closed it (graceful stop), "timed_out"/"reset" when it vanished (killed)."""
    hits = [e for e in events if e["kind"] == "peer_lost" and e.get("peer_id") == pid]
    return (hits[0].get("fields") or {}).get("reason") if hits else None


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
    wait(lambda: connected(A, idb), 30)  # right after a pairing the connection can still be forming
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
    b_mark = mark_of(audit(B))
    s, v, ms = call(A, "POST", f"/api/peers/{stranger_id}/connect", {"addr": b_dial})
    time.sleep(2)
    b_new = since(audit(B), b_mark)
    step("manual: an unpaired id is refused before any packet (requirement 8)",
         s == 403 and (v.get("error") or {}).get("code") == "peer_unauthorized" and not b_new
         and ids(peers(A)) == ids({"peers": before_a}), status=s, ms=ms, b_new_audit=b_new)
    pinned = (peer(A, idb) or {}).get("last_addr")
    bogus = a.bogus_addr
    # Not the direction: B's own redial often connects during A's dial at the dead hint, and connect then
    # returns that live connection for the pinned address.
    s, v, ms, c, fresh, new = fresh_dial(A, idb, bogus, 90)
    # With mDNS the fall-through may reach B at the address mDNS shows first and pin that one: only the hint
    # must never be pinned.
    last = (peer(A, idb) or {}).get("last_addr")
    step("manual: a hint with no Common device falls through to a known address and is not saved",
         s == 200 and fresh and dialled(new, idb, bogus) and connected(A, idb) and last not in (None, bogus),
         status=s, ms=ms, hint=bogus, pinned_before=pinned, pinned_after=last, new_connection=fresh,
         dialled_hint=dialled(new, idb, bogus), connection=c)
    for attempt in (1, 2):  # a crossed dial (B's redial won the race) is rare: one retry
        s, v, ms, c, fresh, new = fresh_dial(A, idb, b_dial)
        if dialled(new, idb, b_dial, "peer_connected"):
            break
    step("manual: the address shown on B connects A to B (requirement 5)",
         s == 200 and fresh and dialled(new, idb, b_dial, "peer_connected") and c.get("direction") == "outbound"
         and (peer(A, idb) or {}).get("last_addr") == b_dial, status=s, ms=ms, addr=b_dial, connection=c, attempts=attempt)
    if a.stranger and a.stranger_addr:
        sid = call(a.stranger, "GET", "/api/network")[1]["peer_id"]
        st_before = ids(peers(a.stranger))
        s, v, ms, c, fresh, new = fresh_dial(B, ida, a.stranger_addr, 90)
        b_rec = peer(B, ida) or {}
        step("manual: another Common device at the hinted address is not trusted (requirement 8)",
             fresh and dialled(new, ida, a.stranger_addr) and ids(peers(a.stranger)) == st_before and peer(B, sid) is None
             and b_rec.get("last_addr") != a.stranger_addr, status=s, ms=ms,
             code=(v.get("error") or {}).get("code") if isinstance(v, dict) else None,
             dialled_stranger=dialled(new, ida, a.stranger_addr), stranger=a.stranger_addr,
             b_last_addr_for_a=b_rec.get("last_addr"))
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
    c0 = conn(A, idb).get("id")  # usually still open after re-pairing: this step is not a fresh connect
    paired = [pair(A, B, "A shows the code, B joins"), pair(B, A, "B shows the code, A joins")]
    name = "pair: connected after re-pairing, named by host name"
    if not any(paired):
        na(name, "no pairing succeeded, so nothing was re-paired")
        return
    both = wait(lambda: connected(A, idb) and connected(B, ida), 30)
    a_calls_b, b_calls_a = (peer(A, idb) or {}).get("name"), (peer(B, ida) or {}).get("name")
    step(name, both is not None and a_calls_b == netb.get("device_name") and b_calls_a == neta.get("device_name"),
         seconds=both, a_calls_b=a_calls_b, b_calls_a=b_calls_a, connection_id_before=c0,
         connection_id_after=conn(A, idb).get("id"))


def away(A, B, ida, idb, a, how, cmd, mark):
    """B's app quits or crashes (`cmd`): what A shows, and audits, while B is away."""
    listed = NO_MDNS or nearby(A, idb) is not None  # B must be nearby for its leaving nearby to mean anything
    t0 = time.monotonic()
    out = sh(cmd)
    gone = wait(lambda: not connected(A, idb), 30)
    unlisted = wait(lambda: nearby(A, idb) is None, 15 if how == "quit" else 1)
    wait(lambda: lost_reason(since(audit(A), mark), idb), 10)
    why = lost_reason(since(audit(A), mark), idb)
    p = peer(A, idb) or {}
    step(f"{how}: A shows B as not connected", gone is not None, seconds=gone, health=p.get("health"), lost_reason=why,
         cmd_output=out[-300:])
    cached = None
    e6 = "quit: A drops B from nearby within seconds (goodbye sent on quit, E6)"
    if how == "quit":
        # Every other quit check also passes when B's app falls back to killing its daemon (A's idle timeout
        # ends the connection); only B's own close tells the graceful stop apart, mDNS or not.
        step("quit: B's daemon closed the connection itself (graceful stop, not the kill fallback)",
             why == "closed_by_peer", lost_reason=why)
        if NO_MDNS:
            na(e6)
        elif not listed:
            na(e6, "B was not listed nearby on A before the quit, so there was nothing to drop")
        else:
            step(e6, unlisted is not None and unlisted <= 10, seconds=unlisted)
    else:
        cached = nearby(A, idb)
        name = "crash: B's signed record may stay cached, but A never shows it as connected"
        if NO_MDNS:
            na(name)
        elif cached is None:  # nothing cached: the premise is not met, so this is no PASS
            na(name, "A held no record of B after the crash, so there was no cached record to ignore")
        else:
            h = (peer(A, idb) or {}).get("health")
            step(name, not connected(A, idb) and h != "connected", cached_record=cached, health=h)
    if a.procs_b:
        left = wait(lambda: sh(a.procs_b).strip() == "0", 20, 1)  # a count of zero; no output is no answer
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
    # The connect at the dead hint above is a user action, audited on its own: only redials count here. A's
    # redials while B is away write at least one line; a cached record with two addresses can add a second.
    rej = [e for e in rejected(since(audit(A), mark), idb) if (e.get("fields") or {}).get("addr") != a.bogus_addr]
    step(f"{how}: one audit line per address for B while it is away, not one per redial (E4)",
         rej and once_per_address(rej), audit_lines=len(rej), watched_s=round(time.monotonic() - t0), lines=rej[:3])
    ttl = "crash: the cached record leaves nearby by the mDNS TTL (about 120 s after the last announcement)"
    if how == "crash" and NO_MDNS:
        na(ttl)
    elif how == "crash" and cached is None:
        na(ttl, "A held no record of B after the crash, so there was nothing to expire")
    elif how == "crash":
        gone_rec = wait(lambda: nearby(A, idb) is None, 150, 1)
        step(ttl, gone_rec is not None, seconds_after_watch=gone_rec, seconds_since_crash=round(time.monotonic() - t0))


def lost_and_back(A, B, ida, idb, a, how, cmd):
    mark = mark_of(audit(A))
    try:
        away(A, B, ida, idb, a, how, cmd, mark)
    finally:  # B's app starts again, also when the run is aborted while B is down
        if a.start_b:
            sh(a.start_b)
    if a.start_b:
        t1 = time.monotonic()
        back = wait(lambda: connected(A, idb), 120, 0.5)
        dirs = [(e.get("fields") or {}).get("direction") for e in since(audit(A), mark)
                if e["kind"] == "peer_connected" and e.get("peer_id") == idb]
        # ponytail: the restarted B dials A on its first tick, so whether discovery reset A's backoff is not
        # visible here (mesh.rs backoff_resets_on_connection_or_discovery_change covers it); direction is a fact.
        step(f"{how}: B restarted, the two reconnect", back is not None, seconds_after_start=back,
             a_connected_direction=dirs[-1] if dirs else None,
             seen_nearby_after=None if NO_MDNS else (round(time.monotonic() - t1, 1) if nearby(A, idb) else None))
        wait(lambda: call(B, "GET", "/health")[0] == 200, 60, 1)


def s_quit(A, B, ida, idb, neta, netb, a):
    if not a.quit_b:
        return na("quit", "--quit-b not given")
    lost_and_back(A, B, ida, idb, a, "quit", a.quit_b)


def s_crash(A, B, ida, idb, neta, netb, a):
    if not a.crash_b:
        return na("crash", "--crash-b not given")
    lost_and_back(A, B, ida, idb, a, "crash", a.crash_b)


def s_wifi(A, B, ida, idb, neta, netb, a):
    if not a.wifi_cycle_b:
        return na("wifi", "--wifi-cycle-b not given")
    c0 = conn(A, idb).get("id")
    d0 = nearby(A, idb) or {}
    out = sh(a.wifi_cycle_b)
    t0 = time.monotonic()
    down = wait(lambda: not connected(A, idb), 40)
    back = wait(lambda: connected(A, idb), 180, 0.5) if down is not None else None  # no drop: nothing to come back
    c = conn(A, idb)
    step("wifi: B's Wi-Fi drops, A loses B, then a new connection forms" + (" (no mDNS: by redial)" if NO_MDNS else ""),
         back is not None, disconnected_after_s=down, connected_after_s=back, connection_id_before=c0,
         connection_id_after=c.get("id"), b_addr_after=c.get("remote_addr"), cmd_output=out[-300:],
         total_s=round(time.monotonic() - t0, 1))
    host = (c.get("remote_addr") or "").rsplit(":", 1)[0].strip("[]")
    if host:  # a new lease can move B: the runner reopens its tunnel to this address
        with open(os.path.join(a.out, "b_host"), "w") as f:
            f.write(host + "\n")
    if down is not None and back is None:
        raise SystemExit("wifi: B did not come back; stopping before anything else talks to B")
    if NO_MDNS:
        return
    heard = wait(lambda: (nearby(A, idb) or {}).get("seen_utc", "") > d0.get("seen_utc", ""), 60, 0.5)
    step("wifi: A hears B's record again after the reconnect", heard is not None, record_heard_after_reconnect_s=heard,
         d0_listed=bool(d0), before=d0, after=nearby(A, idb))
    na("wifi: the record A hears is freshly signed (D1)",
       "GET /api/peers does not show the record's signed ts, and seen_utc also advances on a same-ts re-announcement")


def s_offline(A, B, ida, idb, neta, netb, a):
    if not a.offline_peer:
        return na("offline", "--offline-peer not given")
    p0 = peer(A, a.offline_peer)
    if p0 is None:
        return step("offline: the switched-off device is paired on A", False, offline_peer=a.offline_peer)
    mark = mark_of(audit(A))
    time.sleep(a.watch)
    rej = rejected(since(audit(A), mark), a.offline_peer)
    p = peer(A, a.offline_peer) or {}
    # No line at all is right here: its one line per address may predate the watch.
    step("offline: a switched-off paired device costs at most one audit line per address (E4)", once_per_address(rej),
         audit_lines=len(rej), watched_s=a.watch, last_error=p.get("last_error"), health=p.get("health"))
    redial = "offline: A kept redialling it during the watch (a newer last_error)"
    if a.watch >= 330:
        step(redial, ((p.get("last_error") or {}).get("at_utc") or "") > ((p0.get("last_error") or {}).get("at_utc") or ""),
             before=p0.get("last_error"), after=p.get("last_error"))
    else:
        na(redial, f"the watch ({a.watch} s) is shorter than the 300 s redial backoff cap plus a dial")


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
    snap = os.path.join(a.out, "state-before.json")
    if state("save", snap, A, B):
        raise SystemExit("state.py save failed; nothing was changed")
    try:
        for name in a.steps.split(","):
            STEPS[name](A, B, ida, idb, neta, netb, a)
    finally:  # also when B did not come back or the run is aborted: names (re-pairing), holds, B's model
        rc = state("restore", snap)
        step("restore: both daemons put back as saved (state.py)", rc == 0, exit_code=rc)
        with open(os.path.join(a.out, "results.json"), "w") as f:
            json.dump({"driven": DRIVEN, "a": neta, "b": netb, "steps": RESULTS}, f, indent=2, default=str)
    failed = [r["step"] for r in RESULTS if r["ok"] is False]
    n_a = sum(r["ok"] is None for r in RESULTS)
    print("FAILED: " + ", ".join(failed) if failed else "ALL PASS" + (f" ({n_a} N/A)" if n_a else ""))
    sys.exit(3 if failed else 0)  # 3: a step FAILed (N/A is not a failure); a crash or a stop is 1


if __name__ == "__main__":
    main()
