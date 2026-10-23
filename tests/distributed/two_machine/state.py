#!/usr/bin/env python3
"""Save, then put back, what the two-machine runs change on each daemon: networking and Sharing, holds,
pairings, groups, privacy mode, the loaded model and the names two saved daemons give each other.

    python3 -I state.py save <file> <api-url>...
    python3 -I state.py restore <file>

The runs revoke and pair again, which drops the other device from this device's groups; restore puts each
group back as saved, leaving out a device that is no longer paired. Such a lost pairing is reported, never
paired again from here. A group the run created is emptied (the API has no DELETE for groups; an empty group
with the default deny-all policy grants nothing). A hold the run left is ended with Connect; a device paired
during the run is unpaired (not revoked). When the first two saved daemons still pair with each other but
under other names, they pair once more with the saved names; `paired_at_utc` and the list of superseded
pairings change with every pairing and cannot be put back. Restore tries every daemon, prints what it could
not put back and then exits 1. Stdlib only.
"""
import json
import sys
import urllib.error
import urllib.request


def call(base, method, path, body=None, timeout=30):
    req = urllib.request.Request(base + path, method=method, data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read() or b"null")


def save(bases):
    state = {}
    for b in bases:
        net, peers = call(b, "GET", "/api/network"), call(b, "GET", "/api/peers")["peers"]
        loaded = call(b, "GET", "/api/status")["loaded"]
        state[b] = {"peer_id": net["peer_id"], "network": {"enabled": net["enabled"], "serve_peers": net["serve_peers"]},
                    "names": {p["id"]: p["name"] for p in peers},  # the paired devices, by what this one calls them
                    "held": [p["id"] for p in peers if p.get("held")],  # holds the owner set stay
                    "groups": call(b, "GET", "/api/groups")["groups"], "privacy": call(b, "GET", "/api/privacy")["mode"],
                    "loaded": loaded and {"id": loaded["model_id"], "device": loaded.get("device_requested") or "auto"}}
    return state


def restore(b, s, left):
    """Put one daemon back as saved in `s`; what cannot be put back goes to `left`. Files saved before a field
    was recorded skip that field (networking then defaults to on)."""
    want = s.get("network") or {"enabled": True, "serve_peers": s["serve_peers"]}
    now = call(b, "GET", "/api/network")
    if (now["enabled"], now["serve_peers"]) != (want["enabled"], want["serve_peers"]):
        call(b, "PUT", "/api/network", want)  # enabled: true also starts an endpoint that failed to restart
    peers = call(b, "GET", "/api/peers")["peers"]
    paired = {p["id"] for p in peers}
    for p in peers:
        if p.get("held") and p["id"] not in s.get("held", []):
            try:
                call(b, "POST", f"/api/peers/{p['id']}/connect", {}, timeout=90)
            except urllib.error.HTTPError:
                pass  # connect ends the hold before it dials; a failed dial still ends it
    if "names" in s:
        for pid in sorted(paired - set(s["names"])):
            call(b, "DELETE", f"/api/peers/{pid}")
            print(f"{b}: unpaired {pid[:12]} (paired during the run)")
        lost = sorted(set(s["names"]) - paired)
        if lost:
            left.append(f"{b} lost its pairing with {', '.join(s['names'][x] + ' ' + x for x in lost)}: pair them "
                        "again by hand (restore never pairs a lost device), then rerun this restore for its groups")
        paired &= set(s["names"])
    kept = {g["name"] for g in s["groups"]}
    for g in call(b, "GET", "/api/groups")["groups"]:
        if g["name"] not in kept:  # ponytail: no DELETE /api/groups; an empty deny-all group is inert
            call(b, "PUT", "/api/groups", {"name": g["name"], "members": []})
    for g in s["groups"]:
        call(b, "PUT", "/api/groups", {**g, "members": [m for m in g["members"] if m in paired]})
    mode = s["privacy"]
    call(b, "PUT", "/api/privacy", {"mode": mode, **({"acknowledged": True} if mode != "local_only" else {})})
    if "loaded" in s:
        l, cur = s["loaded"], call(b, "GET", "/api/status")["loaded"]
        try:
            if l and (cur or {}).get("model_id") != l["id"]:
                call(b, "POST", f"/api/models/{l['id']}/load", {"device": l["device"]}, timeout=300)
            elif not l and cur:
                call(b, "POST", "/api/unload")
        except urllib.error.HTTPError as e:
            left.append(f"{b}: model {l['id'] + ' not loaded' if l else 'not unloaded'} ({e.code})")
    print(f"restored {b}: networking {want['enabled']}, sharing {want['serve_peers']}, {len(s['groups'])} groups, "
          f"privacy {mode}, model {(s.get('loaded') or {}).get('id', 'none') if 'loaded' in s else 'not saved'}")


def restore_names(st, left):
    """A and B (the first two saved) pair once more when they call each other by other names than saved."""
    (a, sa), (b, sb) = list(st.items())[:2]
    ida, idb = sa["peer_id"], sb["peer_id"]
    want_a, want_b = sa["names"].get(idb), sb["names"].get(ida)  # what A called B, what B called A
    cur_a = {p["id"]: p["name"] for p in call(a, "GET", "/api/peers")["peers"]}
    cur_b = {p["id"]: p["name"] for p in call(b, "GET", "/api/peers")["peers"]}
    if not (want_a and want_b and idb in cur_a and ida in cur_b) or (cur_a[idb], cur_b[ida]) == (want_a, want_b):
        return
    start = call(a, "POST", "/api/pair/start", {"name": want_b})  # the name B stores for A
    call(b, "POST", "/api/pair/join", {"addr": start["pair_addr"], "code": start["code"], "name": want_a})
    print(f"paired again with the saved names: A calls B {want_a!r}, B calls A {want_b!r}")


def main():
    cmd, path, *bases = sys.argv[1:]
    if cmd == "save":
        state = save(bases)
        json.dump(state, open(path, "w"), indent=2)
        print(f"saved {path}: " + json.dumps(state))
        return
    st, left = json.load(open(path)), []
    for b, s in st.items():
        try:
            restore(b, s, left)
        except Exception as e:  # ponytail: one unreachable daemon must not keep the others from being put back
            left.append(f"{b} not (fully) restored: {e!r}")
    if len(st) >= 2 and all("names" in s and "peer_id" in s for s in list(st.values())[:2]):
        try:
            restore_names(st, left)
        except Exception as e:
            left.append(f"names not put back: {e!r}")
    for x in left:
        print("NOT RESTORED: " + x, file=sys.stderr)
    sys.exit(1 if left else 0)


if __name__ == "__main__":
    main()
