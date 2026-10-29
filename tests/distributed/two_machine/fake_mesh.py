"""SYNTHETIC fake daemons for the offline tests of the two-machine drivers and state.py: a few dicts answer the
API routes the drivers call, with the behaviour they rely on (connect returns an open connection as is and ends a
hold, a failed dial is audited once per address, a held device reads peer_held and still exchanges revocations,
pairing, groups, privacy, Sharing restarts). The completion text is canned. No network, machine, daemon or
inference is involved, so nothing produced with these fakes is evidence of UCompute.

`bad` turns one behaviour off or wrong, so a test can show that the step claiming it FAILs (or is N/A)."""
import contextlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import time
from pathlib import Path
from unittest import mock

HERE = Path(__file__).parent
EPOCH = 1_790_000_000  # the fake clock's zero, so audit times and the drivers' utc() compare as strings
IDS = {k: k * 64 if k in "abcd" else "e" * 64 for k in "abcds"}


class Clock:
    """Replaces the drivers' `time`: sleeping moves the clock, nothing waits."""

    def __init__(self):
        self.t = 0.0

    def monotonic(self):
        return self.t

    def sleep(self, s):
        self.t += s

    def gmtime(self, secs=None):
        return time.gmtime(EPOCH + self.t if secs is None else secs)

    strftime = staticmethod(time.strftime)


class Daemon:
    def __init__(self, key, n):
        self.key, self.id, self.name = key, IDS[key], f"mac-{key}"
        self.addrs = [f"10.0.0.{n}:8193"] + (["10.0.0.22:8193"] if key == "b" else [])  # B also answers at an older address
        self.up, self.enabled, self.serve, self.privacy, self.off_net = True, True, True, "local_only", False
        self.pins, self.groups, self.audit, self.revs = {}, {}, [], []
        self.held, self.held_audited, self.revoked, self.failures, self.last_error = set(), set(), set(), set(), {}
        self.mdns = None  # what its mDNS record lists, when not the addresses it answers on (an old interface)
        self.window = self.loaded = self.crashed_at = self.quit_at = None
        self.pair_addr = f"10.0.9.{n}:5000"


class Mesh:
    """A and B paired (A has B pinned at B's older address) and connected; C, D and a stranger S unpaired."""

    def __init__(self, bad=(), nearby=False, race=0, audit_b=0, golden=""):
        self.clock, self.bad, self.nearby, self.race, self.golden = Clock(), set(bad), nearby, race, golden
        self.d = {f"http://{k}": Daemon(k, n) for n, k in enumerate("abcds", 1)}
        self.d["http://s"].addrs = ["10.0.0.9:8193"]
        self.ids = {x.id: x for x in self.d.values()}
        self.conns, self.next_conn, self.wifi_back_at, self.tunnel_dead = {}, 1, None, False
        A, B = self.d["http://a"], self.d["http://b"]
        A.pins[B.id] = {"name": B.name, "last_addr": B.addrs[1], "paired_at_utc": self.ts()}
        B.pins[A.id] = {"name": A.name, "last_addr": A.addrs[0], "paired_at_utc": self.ts()}
        self.link(A, B, A, B.addrs[1])
        B.audit += [{"ts_utc": "old", "kind": "remote_request", "n": i} for i in range(audit_b)]

    # --- plumbing
    def ts(self):
        return time.strftime("%Y-%m-%dT%H:%M:%SZ", self.clock.gmtime())

    def log(self, d, kind, peer=None, **fields):
        d.audit.append({"ts_utc": self.ts(), "kind": kind, "peer_id": peer, "fields": fields, "n": len(d.audit)})

    def conn(self, x, y):
        return self.conns.get(frozenset((x.id, y.id)))

    def link(self, src, dst, frm, addr):
        """A new connection, dialled by `frm` at `addr`."""
        to = dst if frm is src else src
        self.conns[frozenset((src.id, dst.id))] = {"id": self.next_conn, "from": frm.id, "addr": addr}
        self.next_conn += 1
        for x, y in ((src, dst), (dst, src)):
            x.failures = {f for f in x.failures if f[0] != y.id}
            x.last_error.pop(y.id, None)
        self.log(frm, "peer_connected", to.id, addr=addr, direction="outbound")
        self.log(to, "peer_connected", frm.id, addr=frm.addrs[0], direction="inbound")

    def unlink(self, x, y, reason=None):
        if self.conns.pop(frozenset((x.id, y.id)), None) and reason:
            self.log(x, "peer_lost", y.id, reason=reason)

    def record(self, me, pid):
        c, pin = self.conns.get(frozenset((me.id, pid))), me.pins[pid]
        if c:
            pin["seen"] = self.ts()
        mine = c and c["from"] == me.id
        return {"id": pid, "name": pin["name"], "last_addr": pin["last_addr"], "connected": bool(c),
                "held": pid in me.held, "health": "connected" if c else "disconnected", "last_seen_utc": pin.get("seen"),
                "last_error": me.last_error.get(pid), "paired_at_utc": pin["paired_at_utc"],
                "groups": [g for g, v in me.groups.items() if pid in v["members"]],
                "connection": c and {"id": c["id"], "direction": "outbound" if mine else "inbound",
                                     "remote_addr": c["addr"] if mine else self.ids[c["from"]].addrs[0]}}

    def discovered(self, me):
        t, out = self.clock.t, []
        for o in self.d.values():
            cached = (o.crashed_at is not None and t - o.crashed_at < 120 and "crash_forgets" not in self.bad) or (
                o.quit_at is not None and "no_goodbye" in self.bad)
            if self.nearby and o is not me and (o.up and not o.off_net or cached):
                adv = (o.mdns or o.addrs)[:2]
                out.append({"id": o.id, "addr": adv[0], "addrs": adv, "trusted": o.id in me.pins,
                            "pairing": False, "seen_utc": self.ts() if o.up else o.seen})
        return out

    def unpin(self, me, pid):
        me.pins.pop(pid, None)
        for g in me.groups.values():
            g["members"] = [m for m in g["members"] if m != pid]
        if pid in self.ids:
            self.unlink(me, self.ids[pid])

    def dial_failed(self, src, pid, addr, code):
        if (pid, addr, code) not in src.failures or "audit_each_redial" in self.bad:
            self.log(src, "peer_rejected", pid, addr=addr, direction="outbound", reason=code)
        src.failures.add((pid, addr, code))
        src.last_error[pid] = {"addr": addr, "code": code, "message": code, "at_utc": self.ts()}

    def exchange(self, x, y):
        """Revocations x holds that y applies (the held exchange, ADR-0003 D7)."""
        for r in list(x.revs):
            t = r["peer_id"]
            if t != y.id and t in y.pins and r not in y.revs:
                self.unpin(y, t)
                y.revoked.add(t)
                y.revs.append(r)

    def held_refused(self, holder, src):
        if "held_unaudited" in self.bad:
            pass
        elif src.id not in holder.held_audited or "audit_each_redial" in self.bad:
            self.log(holder, "peer_rejected", src.id, addr=src.addrs[0], reason="held")
            holder.held_audited.add(src.id)
        if "held_drops_revocations" not in self.bad:
            self.exchange(holder, src)
            self.exchange(src, holder)

    def dial(self, src, dst, hint):
        """src dials dst: the hint, the addresses mDNS shows, then the pinned address; a proven refusal ends the
        dial (`refusal_continues`: it does not)."""
        pinned = src.pins[dst.id]["last_addr"]
        seen = next((x["addrs"] for x in self.discovered(src) if x["id"] == dst.id), [])
        code = None
        for addr in dict.fromkeys([hint] * bool(hint) + seen + [pinned]):
            if addr == hint != pinned and "skip_hint" in self.bad:
                continue
            target = next((x for x in self.d.values() if addr in x.addrs and x.up and not x.off_net), None)
            if target is dst and src.id in dst.held and "held_before_revoked" in self.bad:
                code = "peer_held"
            elif target is dst and src.id in dst.revoked:  # refused inside TLS: unproven to the dialler
                code = "peer_lost" if "revoked_reads_lost" in self.bad else "peer_unauthorized"
                if "revoked_unaudited" not in self.bad:  # from the dialler's listen socket, no key named
                    host, port = src.addrs[0].rsplit(":", 1)
                    other = "revoked_line_from_neighbour" in self.bad  # another daemon on B's host
                    self.log(dst, "peer_rejected", addr=f"{host}:{int(port) + other}", reason="peer_revoked")
            elif target is dst and src.id in dst.held:
                code = "peer_held"
            elif target is dst and src.id in dst.pins:
                if addr == hint and self.race:  # dst's own redial wins the crossed dial: its connection is used
                    self.race -= 1
                    self.link(src, dst, dst, src.addrs[0])
                else:
                    self.link(src, dst, src, addr)
                    src.pins[dst.id]["last_addr"] = addr
                if "saves_hint" in self.bad and hint:
                    src.pins[dst.id]["last_addr"] = hint
                return 200, self.record(src, dst.id)
            else:
                code = "peer_unauthorized" if target else "peer_lost"
                self.clock.sleep(0 if target else 8)  # the idle timeout where nothing answers
            if code == "peer_held":
                self.held_refused(dst, src)
            self.dial_failed(src, dst.id, addr, code)
            if code in ("peer_held", "peer_revoked") and "refusal_continues" not in self.bad:
                break
        return (502 if code == "peer_lost" else 403), {"error": {"code": code, "message": code}}

    def tick(self):
        if self.wifi_back_at is not None and self.clock.t >= self.wifi_back_at:
            A, B = self.d["http://a"], self.d["http://b"]
            self.wifi_back_at, B.off_net = None, False
            if "new_lease" in self.bad:  # B comes back at another address; the tunnel to the old one is dead
                B.addrs, self.tunnel_dead = ["10.0.0.42:8193"], True
            self.dial(B, A, None)

    # --- the API
    def call(self, base, method, path, body=None, timeout=60):
        self.tick()
        me, body = self.d[base], body or {}
        if not me.up or (base == "http://b" and self.tunnel_dead):
            return 0, "connection refused", 1.0
        s, v = self.route(me, method, path.split("?")[0], body)
        return s, v, 1.0

    def route(self, me, method, p, body):
        def err(s, code):
            return s, {"error": {"code": code, "message": code}}
        if p == "/health":
            return 200, {}
        if p == "/api/network" and method == "GET":
            return 200, {"peer_id": me.id, "device_name": me.name, "short_label": me.key.upper(), "enabled": me.enabled,
                         "serve_peers": me.serve, "dial_addr": me.addrs[0] if me.enabled else None,
                         "listen_addr": "0.0.0.0:8193", "connections": sum(me.id in k for k in self.conns)}
        if p == "/api/network":
            if "network_put_refused" in self.bad:
                return err(400, "invalid")
            if "network_put_fails" in self.bad:
                me.enabled = False
                me.held.clear()
                for o in list(self.ids.values()):
                    self.unlink(me, o)
                return err(500, "internal")
            before = (me.enabled, me.serve)
            me.enabled, me.serve = body.get("enabled", me.enabled), body.get("serve_peers", me.serve)
            if (me.enabled, me.serve) != before:  # the endpoint restarts; a hold survives unless networking went off
                if not me.enabled or not before[0] or "sharing_drops_hold" in self.bad:
                    me.held.clear()
                for pid in me.pins:
                    o = self.ids[pid]
                    self.unlink(me, o)
                    if me.enabled and o.up and pid not in me.held and me.id not in o.held:
                        self.dial(me, o, None)
                self.log(me, "network_changed", enabled=me.enabled, serve_peers=me.serve)
            return self.route(me, "GET", p, {})
        if p == "/api/peers":
            return 200, {"peers": [self.record(me, pid) for pid in me.pins], "discovered": self.discovered(me),
                         "revocations": me.revs}
        if p.startswith("/api/peers/"):
            pid, _, action = p[len("/api/peers/"):].partition("/")
            if method == "DELETE":
                self.unpin(me, pid)
                me.held.discard(pid)
                return 204, None
            if action == "revoke":
                rev = {"seq": len(me.revs), "peer_id": pid, "revoked_by": me.id, "at_utc": self.ts()}
                self.unpin(me, pid)
                me.revoked.add(pid)
                if "revocation_unrecorded" not in self.bad:
                    me.revs.append(rev)
                if "held_before_revoked" not in self.bad:
                    me.held.discard(pid)
                return 200, rev
            if pid not in me.pins:
                if "leak" in self.bad:
                    self.log(self.d["http://b"], "peer_rejected")
                return err(403, "peer_unauthorized")
            if not me.enabled:
                return err(403, "policy_denied")
            o = self.ids[pid]
            if action == "disconnect":
                if "ignores_disconnect" not in self.bad:
                    me.held.add(pid)
                    me.held_audited.discard(pid)
                    self.unlink(me, o)
                    if o.up and not o.off_net and me.id in o.pins:
                        self.dial(o, me, None)  # o's own redial, refused: o reads peer_held
                return 200, self.record(me, pid)
            me.held.discard(pid)  # connect: ends the hold, returns an open connection as is, else dials
            return (200, self.record(me, pid)) if self.conn(me, o) else self.dial(me, o, body.get("addr"))
        if p == "/api/pair/start":
            if not me.enabled:
                return err(403, "policy_denied")
            if "pair_start_fails" in self.bad:
                return err(409, "overloaded")
            me.window = {"code": "123456", "pair_addr": me.pair_addr, "name": body.get("name") or me.name}
            return 200, {"code": "123456", "pair_addr": me.pair_addr, "port": 5000}
        if p == "/api/pair/join":
            shower = next((x for x in self.d.values() if x.window and x.window["pair_addr"] == body.get("addr")), None)
            if shower is None or "pair_join_fails" in self.bad:
                return err(502, "peer_lost")
            if body["code"] != shower.window["code"]:
                return err(401, "pairing_failed")
            w, shower.window = shower.window, None
            other = "Kyle" if "names_not_host" in self.bad else None
            me.pins[shower.id] = {"name": other or w["name"], "last_addr": shower.addrs[0], "paired_at_utc": self.ts()}
            shower.pins[me.id] = {"name": other or body.get("name") or me.name, "last_addr": me.addrs[0],
                                  "paired_at_utc": self.ts()}
            for x, y in ((me, shower), (shower, me)):
                x.revoked.discard(y.id)
                x.held.discard(y.id)
            if not self.conn(me, shower):
                self.link(me, shower, me, shower.addrs[0])
            return 200, self.record(me, shower.id)
        if p == "/api/groups":
            if method == "PUT":
                if "groups_put_fails" in self.bad:
                    return err(400, "invalid")
                me.groups[body["name"]] = {"name": body["name"], "members": list(body["members"]), "policy": {
                    "allow_remote_inference": False, "allow_pooled_inference": False, **(body.get("policy") or {})}}
            return 200, {"groups": [dict(g, members=list(g["members"])) for g in me.groups.values()]}
        if p == "/api/privacy":
            if method == "PUT":
                if body["mode"] == "trusted_group" and not body.get("acknowledged"):
                    return err(403, "policy_denied")
                me.privacy = body["mode"]
            return 200, {"mode": me.privacy}
        if p == "/api/audit":
            if f"audit_fails_{me.key}" in self.bad:
                return err(500, "internal")
            return 200, {"events": me.audit[-1000:]}
        if p == "/api/status":
            return 200, {"loaded": me.loaded and {"model_id": me.loaded, "device_requested": "auto"}}
        if p.startswith("/api/models/"):
            model, _, action = p[len("/api/models/"):].partition("/")
            if action == "load":
                me.loaded = model
            return 200, {"model_id": model} if action == "load" else 'data: {"done": true}'
        if p == "/api/unload":
            me.loaded = None
            return 204, None
        if p == "/v1/completions":
            return self.complete(me, body)
        raise AssertionError((method, p))

    def complete(self, me, body):
        def err(s, code):
            return s, {"error": {"code": code, "message": code}}
        pid = body.get("target", "")[5:]
        if me.privacy == "local_only" or pid in me.held:
            return err(403, "policy_denied")
        if pid not in me.pins:
            return err(403, "peer_unauthorized")
        o = self.ids[pid]
        if not o.up:
            return err(502, "peer_lost")
        if not self.conn(me, o):
            s, v = self.dial(me, o, None)
            if s != 200:
                return s, v
        if not (o.serve and any(me.id in g["members"] and g["policy"]["allow_remote_inference"] for g in o.groups.values())):
            return err(403, "policy_denied")
        if o.loaded != body["model"]:
            return err(503, "model_not_loaded")
        for x in (me, o):
            self.log(x, "remote_request", pid if x is me else me.id,
                     **({"prompt": body["prompt"]} if "leaks_prompt" in self.bad else {}))
        return 200, {"choices": [{"text": self.golden}], "target": body["target"], "timings": {}}

    def stream(self, base, body, timeout=120):  # the mesh driver's streaming request, as one chunk
        s, v, _ = self.call(base, "POST", "/v1/completions", body)
        text = v["choices"][0]["text"] if s == 200 else ""
        return {"text": text, "golden": s == 200 and text == self.golden, "chunks": 1, "target": v.get("target") if s == 200 else None}

    def sh(self, cmd):
        """The runner's B commands: quit, crash, start, procs, wifi."""
        A, B = self.d["http://a"], self.d["http://b"]
        if cmd in ("quit", "crash"):
            B.up = False
            B.seen = self.ts()
            graceful = cmd == "quit" and "quit_is_kill" not in self.bad
            self.unlink(A, B, "closed_by_peer" if graceful else "timed_out")
            if cmd == "crash":
                B.crashed_at = self.clock.t
            else:
                B.quit_at = self.clock.t
            if "no_redial_lines" not in self.bad:
                for _ in range(3):  # A redials while B is away
                    self.dial(A, B, None)
        elif cmd == "start":
            B.up, B.crashed_at, B.quit_at = True, None, None
            B.held.clear()
            self.dial(B, A, None)  # the restarted B dials A at once
        elif cmd == "procs":
            return "" if "procs_silent" in self.bad else ("1" if B.up else "0")
        elif cmd == "wifi":
            if "wifi_never_drops" not in self.bad:
                self.unlink(A, B, "timed_out")
                B.off_net = True
                self.wifi_back_at = None if "wifi_never_back" in self.bad else self.clock.t + 10
            return "SUCCESS: Attempted to run the scheduled task"
        return ""


class SplitMesh(Mesh):
    """SYNTHETIC head (A, this Mac's dev daemon) and stage host (B, sentinel) for split_two_machine.py, unpaired at
    the start like a fresh head home. A split request passes the gates in the order openai::run and rpc.rs check
    them; each request adds the lab's registered per-token traffic for its model (R1a 14,150 B up, 3,592 B down,
    one turn per token) plus a prefill, so the lab's scorer accepts the run. The texts, timings, `ss`, `who` and
    the other shell outputs are canned. Nothing here is P1 or evidence of UCompute.

    `bad`: dirty, busy_r, other_user, desktop_login, r_engine_left, low_memory, r_tree_modified, r_tree_head,
    r_stale_build, h_dirty_build (preflight); r_ip_v6; stage_spawns_on_refusal, stage_refusal_untyped (P1.1);
    wrong_placement, public_rpc, rpc_reachable, lan_down, rdma_banner (the session); session_changes (a prompt's
    checkpoints); local_uses_split; rpc_outlives_session; leaks_prompt (R's logs); no_close_audit, extra_session,
    audit_full_page."""
    HEAD = "abc123"
    BLOBS = {"Cargo.toml": "1" * 40, "crates/ucompute-api/src/lib.rs": "2" * 40, "scripts/mac-host.sh": "3" * 40}
    PER = {"R1a": (14150, 3592), "R1b": (21830, 6152)}
    TIMES = {"R1a": (230.0, 36.0), "R1b": (350.0, 54.0)}  # TTFT and TPOT (ms) inside the lab's Wi-Fi bands

    def __init__(self, bad=()):
        super().__init__(bad)
        self.H, self.R = self.d["http://a"], self.d["http://b"]
        self.H.pins.clear()
        self.R.pins.clear()
        self.conns.clear()
        prereg = json.loads((HERE / "split_prereg.json").read_text())
        self.models = {m["model"]: m for m in prereg["models"]}
        self.tokens = {p["text"]: p["tokens"] for p in prereg["prompts"]}
        self.session, self.sessions, self.loaded_split, self.leak, self.requests = None, 0, None, False, 0

    def pooled(self, me, pid):
        return any(pid in g["members"] and g["policy"].get("allow_pooled_inference") for g in me.groups.values())

    def close(self):
        if self.session:
            sid = self.session["id"]
            if "no_close_audit" not in self.bad:
                self.log(self.H, "session_close", self.R.id, role="head", session=sid)
                self.log(self.R, "session_close", self.H.id, role="stage", session=sid)
            self.leak |= "rpc_outlives_session" in self.bad
        self.session, self.loaded_split = None, None

    def open(self, model):
        self.close()
        self.sessions += 1
        sid = f"{self.sessions:032x}"
        self.session = {"id": sid, "model": model, "port": 40000 + self.sessions,
                        "c": {"to_r": 200_000_000, "to_h": 9_000, "switches": 40}}  # the upload
        self.log(self.H, "session_open", self.R.id, role="head", session=sid)
        self.log(self.R, "session_open", self.H.id, role="stage", session=sid)
        if "extra_session" in self.bad and self.sessions == 1:  # one the driver never sees in /api/status
            for d, role in ((self.H, "head"), (self.R, "stage")):
                for kind in ("session_open", "session_close"):
                    self.log(d, kind, None, role=role, session="f" * 32)

    def traffic(self, m, tokens, n):
        c, (up, down), act = self.session["c"], self.PER[m["label"]], m["hidden"] * 4
        c["to_r"] += 3 * act * tokens + 22_528 + up * (n - 1)  # a prefill, then n - 1 decode steps
        c["to_h"] += act * tokens + down * (n - 1)
        c["switches"] += 6 + 2 * (n - 1)

    def route(self, me, method, p, body):
        if p == "/health":
            stale = (me is self.R and "r_stale_build" in self.bad) or (me is self.H and "h_dirty_build" in self.bad)
            return 200, {"status": "ok", "version": "1.0.0",
                         "commit": "0ld000" if stale and me is self.R else self.HEAD + ("-dirty" if stale else "")}
        if p == "/api/network" and method == "GET" and me is self.R and "r_ip_v6" in self.bad:
            return 200, {**super().route(me, method, p, body)[1], "dial_addr": "[fe80::1]:8193"}
        if p == "/api/audit" and "audit_full_page" in self.bad:  # a page of 1000 that starts after the run did
            return 200, {"events": [me.audit[0]] * (1000 - len(me.audit)) + me.audit}
        if p == "/api/status":
            s = self.session if me is self.H else None
            split = self.loaded_split if me is self.H else None
            return 200, {"loaded": me.loaded and {"model_id": me.loaded, "split": split},
                         "split": s and {"session": s["id"], **s["c"]}}
        if p == "/api/unload":
            me.loaded = None
            if me is self.H:
                self.close()
            return 204, None
        return super().route(me, method, p, body)

    def sse(self, base, body, timeout=900):
        self.tick()
        me, tgt, m = self.d[base], body.get("target", "local"), self.models[body["model"]]
        n, tokens = body["max_tokens"], self.tokens.get(body["prompt"], 2)
        ttft, tpot = self.TIMES[m["label"]]

        def err(status, code, msg=None):
            return {"status": status, "text": "", "usage": None, "target": None, "placement": None,
                    "error": {"code": code, "message": msg or code}, "ttft_ms": None, "done_ms": None}

        def ok(placement=None):
            return {"status": 200, "text": " canned text" * n, "target": tgt, "placement": placement, "error": None,
                    "usage": {"prompt_tokens": tokens, "completion_tokens": n},
                    "ttft_ms": ttft, "done_ms": ttft + tpot * (n - 1)}
        if tgt == "local":
            if "local_uses_split" in self.bad and self.session:
                self.traffic(m, tokens, n)
            me.loaded, self.loaded_split = body["model"], None
            return ok()
        pid = tgt[len("split:"):]
        if me.privacy != "trusted_group" or not me.enabled:
            return err(403, "policy_denied")
        if pid not in me.pins:
            return err(403, "peer_unauthorized")
        if not self.pooled(me, pid):
            return err(403, "peer_unauthorized", "no trusted group allows pooled inference with device R")
        if not body.get("acknowledged"):
            return err(403, "policy_denied", "Pooling splits one model between this device and R. ...")
        if not (self.R.serve and self.pooled(self.R, me.id)):
            self.leak |= "stage_spawns_on_refusal" in self.bad
            if "stage_refusal_untyped" in self.bad:  # the right words under another code
                return err(403, "policy_denied", "no trusted group allows pooled inference with device H")
            return err(403, "peer_unauthorized", "no trusted group allows pooled inference with device H")
        self.requests += 1
        if self.session is None or self.session["model"] != body["model"] or (
                "session_changes" in self.bad and self.requests % 2 == 0):
            self.open(body["model"])
        self.traffic(m, tokens, n)
        k, nl = m["split_at"], m["n_layer"]
        devs = ("MTL0", "RPC0") if "wrong_placement" in self.bad else ("RPC0", "MTL0")
        self.loaded_split = [{"device": devs[0], "first_block": 0, "end_block": k},
                             {"device": devs[1], "first_block": k, "end_block": nl}]
        me.loaded = body["model"]
        return ok({"stages": self.loaded_split, "session": self.session["id"]})

    def ss(self, rpc):
        lines = ['tcp LISTEN 0 1024 127.0.0.1:8192 0.0.0.0:* users:(("ucompute",pid=100,fd=9))',
                 'udp UNCONN 0 0 0.0.0.0:8193 0.0.0.0:* users:(("ucompute",pid=100,fd=10))',
                 "tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:*"]  # sshd: another user's socket, no process shown
        if rpc:
            host = "0.0.0.0" if "public_rpc" in self.bad else "127.0.0.1"
            port = self.session["port"] if self.session else 39999
            lines.append(f'tcp LISTEN 0 1 {host}:{port} 0.0.0.0:* users:(("ggml-rpc-server",pid=5151,fd=3))')
        return "\n".join(lines) + "\n"

    def shell(self, where, key, **kw):
        bad, rpc = self.bad, self.session is not None or self.leak
        if where == "r":
            return {
                "procs": "4242 /home/x/llama-server -m m.gguf\n" if "r_engine_left" in bad else "",
                "rpc": "5151 ggml-rpc-server -H 127.0.0.1 -p 40001\n" if rpc else "",
                "ss": self.ss(rpc),
                "who": ("kyle pts/1 2026-10-29 10:00 (10.0.0.77)\n" if "other_user" in bad else "")
                + "div0rce pts/0 2026-10-29 09:00 (10.0.0.1)\nssh_client=10.0.0.1\n",
                "sessions": ("Id=2\nName=div0rce\nClass=user\nType=wayland\nState=active\nRemote=no\nRemoteHost=\n"
                             "Service=gdm-password\nSeat=seat0\n\n" if "desktop_login" in bad else "")
                + "Id=c7\nName=div0rce\nClass=user\nType=tty\nState=active\nRemote=yes\nRemoteHost=10.0.0.1\n"
                  "Service=sshd\nSeat=\n\nself=c7\nuser=div0rce\nssh_client=10.0.0.1\n",
                "tree": ("0ld000" if "r_tree_head" in bad else self.HEAD) + "\n" + "".join(
                    f"{p}\t{'9' * 40 if 'r_tree_modified' in bad and p == 'Cargo.toml' else h}\n"
                    for p, h in sorted(self.BLOBS.items())),
                "load": ("1.50" if "busy_r" in bad else "0.12") + " 0.20 0.30 1/300 4242\n",
                "top": "%CPU PID COMMAND\n 1.0 1 systemd\n",
                "banner": "transport : "
                + ("RDMA" if "rdma_banner" in bad else "TCP (RDMA auto-negotiate enabled)") + "\n",
                "scan": "/home/x/.ucompute-acceptance/logs/engine-1.log\n" if "leaks_prompt" in bad else "",
            }[key]
        return {
            "git": self.HEAD + "\n" + (" M crates/x.rs\n" if "dirty" in bad else "")
                   + ("?? docs/evidence/pool-two-machine-x/\n" if "untracked_out" in bad else ""),
            "tree": "".join(f"100644 blob {h}\t{p}\n" for p, h in sorted(self.BLOBS.items())),
            "procs": "",
            "load": "{ 0.50 0.60 0.70 }\n",
            "power": "Now drawing from 'AC Power'\n",
            "caffeinate": "999\n",
            "mem": "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: "
                   + ("20000" if "low_memory" in bad else "200000") + ".\n"
                   "Pages active: 9.\nPages inactive: 50000.\nPages speculative: 1000.\n",
            "top": "%CPU PID COMMAND\n 2.0 1 launchd\n",
            "ping": "round-trip min/avg/max/stddev = 5.1/12.5/24.0/3.0 ms\n",
            "nc": "rc=1\n" if kw.get("port") == 22 and "lan_down" in bad else
                  "rc=0\n" if kw.get("port") == 22 or "rpc_reachable" in bad else "rc=1\n",
        }[key]


def load(name):
    spec = importlib.util.spec_from_file_location(Path(name).stem, HERE / name)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def drive(script, mesh, *flags, restore=0):
    """Run a driver's main() against `mesh`; state.py is not run (save 0, restore `restore`). Returns its steps by
    name, its console lines, its exit code (or the exception that ended it) and the files it wrote (path under
    --out -> text)."""
    d = load(script)
    mesh.golden = getattr(d, "GOLDEN", mesh.golden)
    d.call, d.time, d.state = mesh.call, mesh.clock, (lambda *a: restore if a[0] == "restore" else 0)
    for name in ("sh", "stream", "sse", "shell"):
        if hasattr(d, name):
            setattr(d, name, getattr(mesh, name))
    out = tempfile.mkdtemp()
    buf, code = io.StringIO(), None
    try:
        with mock.patch.object(sys, "argv", [script, "--a", "http://a", "--b", "http://b", "--out", out, *flags]), \
                contextlib.redirect_stdout(buf):
            try:
                d.main()
            except SystemExit as e:
                code = e.code
            except Exception as e:  # a crash: results.json must still have been written
                code = e
        res = Path(out) / "results.json"
        steps = json.loads((res if res.exists() else Path(out) / "steps.json").read_text())["steps"]
        files = {str(p.relative_to(out)): p.read_text() for p in Path(out).rglob("*")
                 if p.is_file() and p.name not in ("raw.jsonl", "results.json")}
    finally:
        d.RAW and d.RAW.close()
        shutil.rmtree(out)
    return {r["step"]: r for r in steps}, buf.getvalue().splitlines(), code, files
