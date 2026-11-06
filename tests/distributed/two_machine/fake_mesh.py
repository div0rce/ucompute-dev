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
import random
import shutil
import struct
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


def gguf_text(s):
    b = s.encode()
    return struct.pack("<Q", len(b)) + b


def synthetic_gguf(path, tensors, seed, align=64, align_type=(4, "<I")):
    """A SYNTHETIC GGUF v3 file, not a model: every KV shape the reader must walk past, general.alignment among them,
    then `tensors` (name, bytes) in order, their data pseudo-random, so 64 bytes of one tensor occur nowhere else."""
    values = [("general.architecture", 8, gguf_text("synthetic")),
              ("general.alignment", align_type[0], struct.pack(align_type[1], align)),
              ("general.flag", 7, b"\x01"), ("x.big", 10, struct.pack("<Q", 7)),
              ("x.names", 9, struct.pack("<IQ", 8, 2) + gguf_text("a") + gguf_text("bc")),
              ("x.ids", 9, struct.pack("<IQ", 5, 3) + struct.pack("<3i", 1, 2, 3))]
    out = b"GGUF" + struct.pack("<IQQ", 3, len(tensors), len(values))
    for key, kind, value in values:
        out += gguf_text(key) + struct.pack("<I", kind) + value
    offset = 0
    for name, size in tensors:
        out += gguf_text(name) + struct.pack("<I2QIQ", 2, size, 1, 0, offset)
        offset += size
    out += b"\0" * (-len(out) % align)
    Path(path).write_bytes(out + random.Random(seed).randbytes(offset))


class FakeFlight:
    """SYNTHETIC stand-in for the driver's Flight: no thread and no socket; the first token and the end of the stream
    are moments on the fake clock, and an injected failure (SplitMesh.fail) sets the end."""
    LOAD, TPOT = 40.0, 0.036

    def __init__(self, mesh, base, body):
        self.mesh, self.clock, self.tokens, self.finish = mesh, mesh.clock, 0, None
        self.first_at = self.error_at = self.ended_at = self.end_at = self.end_error = None
        r = mesh.sse(base, body)
        self.status, self.error = r["status"], r["error"]
        if self.error:
            self.error_at = self.ended_at = self.clock.t
            return
        mesh.flight = self
        self.first_at = self.clock.t + self.LOAD
        self.natural = self.first_at + self.TPOT * body["max_tokens"]

    def wait_first(self, within):
        if self.first_at is None or self.first_at > self.clock.t + within:
            return False
        self.clock.t = max(self.clock.t, self.first_at)
        self.tokens = 3
        return True

    def wait_done(self, within):
        if self.ended_at is not None:
            return True
        end = self.natural if self.end_at is None else self.end_at
        if end > self.clock.t + within:
            self.clock.t += within
            return False
        self.clock.t = max(self.clock.t, end)
        self.ended_at = end
        if self.end_error:
            self.error, self.error_at = self.end_error, end
        return True

    def cancel(self):
        if self.ended_at is None:
            self.ended_at = self.clock.t
            self.mesh.flight = None
            if "cancel_ends_session" in self.mesh.bad:
                self.mesh.close("cancelled")


class SplitMesh(Mesh):
    """SYNTHETIC head (A, this Mac's dev daemon) and stage host (B, sentinel) for split_two_machine.py, unpaired at
    the start like a fresh head home. A split request passes the gates in the order openai::run and rpc.rs check
    them; each request adds the lab's registered per-token traffic for its model (R1a 14,150 B up, 3,592 B down,
    one turn per token) plus a prefill, so the lab's scorer accepts the run. The texts, timings, `ss`, `who` and
    the other shell outputs are canned. Nothing here is P1 or evidence of UCompute.

    The steps beyond the core run are as canned: the footprint and VmHWM values are worked out from the SYNTHETIC
    GGUF's layer sizes (install()), a failure ends the request in flight on the fake clock with a code chosen here (the
    real codes are for the physical run to show), and the traffic through the driver's relay is opaque random records:
    its Relay class is the real one, but no daemon sends it a datagram.

    `bad`: dirty, busy_r, other_user, desktop_login, r_engine_left, low_memory, r_tree_modified, r_tree_head,
    r_stale_build, h_dirty_build (preflight); r_ip_v6; stage_spawns_on_refusal, stage_refusal_untyped (P1.1);
    wrong_placement, public_rpc, rpc_reachable, lan_down, rdma_banner (the session); session_changes (a prompt's
    checkpoints); local_uses_split; rpc_outlives_session; leaks_prompt (R's logs); no_close_audit, extra_session,
    audit_full_page; head_keeps_all_layers, stage_holds_all, no_footprint, ctx_differs, r_no_model, ssh_drops (memory);
    bypass_relay, redial_race, relay_invisible, relay_short, wire_canary, wire_weights, blind_scanner (capture);
    c_wrong_placement, c_stage_gpu, cpu_ignored (configuration C); late_<code> (12 s), slow_<code> (7 s), hang_<code>,
    untyped_<code>, no_stage_close, no_head_close, job_stuck, cancel_ends_session, pdeathsig_slow, r_teardown_slow,
    no_reap, repair_fails, unload_refused (the failure cases)."""
    HEAD = "abc123"
    BLOBS = {"Cargo.toml": "1" * 40, "crates/ucompute-api/src/lib.rs": "2" * 40, "scripts/mac-host.sh": "3" * 40}
    # The lab's amended predictions (02ec3f5): steady state plus one decode-graph upload over 64 steps.
    PER = {"R1a": (16921, 3592), "R1b": (25058, 6152)}
    TIMES = {"R1a": (230.0, 36.0), "R1b": (350.0, 54.0)}  # TTFT and TPOT (ms) inside the lab's Wi-Fi bands
    NORM, GATE, INPUT, OUTPUT = 6_400, 32_000, 64_000, 70_400  # the SYNTHETIC GGUF's tensor sizes
    BLOCK = NORM + GATE
    BASE = 20 << 20  # a process's peak without weights
    FAKE_H_PID, FAKE_R_PID = 7001, 7002

    def __init__(self, bad=()):
        super().__init__(bad)
        self.H, self.R = self.d["http://a"], self.d["http://b"]
        self.H.pins.clear()
        self.R.pins.clear()
        self.conns.clear()
        prereg = json.loads((HERE / "split_prereg.json").read_text())
        self.models = {m["model"]: m for m in prereg["models"]}
        self.tokens = {p["text"]: p["tokens"] for p in prereg["prompts"]}
        self.canaries = [p["canary"] for p in prereg["prompts"]]
        self.session, self.sessions, self.loaded_split, self.leak, self.requests = None, 0, None, False, 0
        self.flight, self.wire, self.engine, self.orphan = None, None, "local", False
        self.revoked_once, self.raced, self.pending, self.devices, self.blobs, self.last_model = False, False, [], {}, {}, None

    def install(self, home):
        """SYNTHETIC manifests and blobs of the prereg's models in a head home: what the driver reads layers from."""
        for i, (model, m) in enumerate(self.models.items()):
            blob = Path(home, "models", "blobs", f"sha256-{i:064x}")
            blob.parent.mkdir(parents=True, exist_ok=True)
            tensors = [("token_embd.weight", self.INPUT)]
            for b in range(m["n_layer"]):
                tensors += [(f"blk.{b}.attn_norm.weight", self.NORM), (f"blk.{b}.ffn_gate.weight", self.GATE)]
            tensors += [("output_norm.weight", self.NORM), ("output.weight", self.OUTPUT - self.NORM)]
            synthetic_gguf(blob, tensors, i)
            manifest = Path(home, "models", "manifests", model + ".json")
            manifest.parent.mkdir(parents=True, exist_ok=True)
            manifest.write_text(json.dumps({"id": model, "sha256": f"{i:064x}"}))
            self.blobs[model] = blob

    def patch(self, d):
        """The driver's own Relay for its control and for what fake traffic is recorded to (`wire`)."""
        mesh, real = self, d.Relay

        class Wire(real):
            def __init__(self, target, path):
                super().__init__(target, path)
                if target[1] == 8193:  # the capture, not the control's loopback sink
                    mesh.wire = self
        d.Relay = Wire
        self.driver = d
        if "blind_scanner" in self.bad:
            d.scan_capture = lambda path, needles: {"datagrams": 0, "hits": {}}

    def Flight(self, base, body, timeout=900):
        return FakeFlight(self, base, body)

    def tick(self):
        super().tick()
        while self.pending and self.pending[0][0] <= self.clock.t:
            self.pending.pop(0)[1]()

    def pooled(self, me, pid):
        return any(pid in g["members"] and g["policy"].get("allow_pooled_inference") for g in me.groups.values())

    def close(self, reason="cancelled", head=True, stage=True):
        if self.session:
            sid = self.session["id"]
            if "no_close_audit" not in self.bad:
                if head and "no_head_close" not in self.bad:
                    self.log(self.H, "session_close", self.R.id, role="head", session=sid, reason=reason)
                if stage and "no_stage_close" not in self.bad:
                    self.log(self.R, "session_close", self.H.id, role="stage", session=sid, reason=reason)
            self.leak |= "rpc_outlives_session" in self.bad
        self.session, self.loaded_split = None, None

    def fail(self, code, after, head=True, stage=True):
        """A session's end with `code`, which reaches the request in flight `after` s later."""
        fl = self.flight
        if fl is None or fl.ended_at is not None or f"hang_{code}" in self.bad:
            return
        after = 12.0 if f"late_{code}" in self.bad else 7.0 if f"slow_{code}" in self.bad else after
        fl.end_at = self.clock.t + after
        fl.end_error = {"code": "transport" if f"untyped_{code}" in self.bad else code, "message": code}
        self.close(code, head, stage)

    def inject(self, me, method, p, body):
        """What a change does to the session (ADR-0005 D7). The codes are this fake's."""
        if me is self.R and p.endswith("/revoke"):
            self.revoked_once = True
            self.fail("peer_revoked", 1.0)
        elif me is self.H and p.endswith("/disconnect"):
            self.fail("peer_held", 1.0)
        elif me is self.R and p == "/api/groups" and method == "PUT" and not self.pooled(self.R, self.H.id):
            self.fail("peer_unauthorized", 1.2)
        elif me is self.H and p == "/api/privacy" and method == "PUT" and body.get("mode") == "local_only":
            self.fail("policy_denied", 1.2)
        elif me is self.H and p == "/api/network" and method == "PUT" and not body.get("enabled", True):
            self.fail("peer_lost", 1.5)

    def kill_daemon(self, who):
        """SIGKILL: nothing of the daemon writes another line. R's rpc-server dies with it (PDEATHSIG); H's request sees
        its socket end, and R notices at its idle timeout."""
        self.unlink(self.H, self.R)
        if who is self.R:
            self.R.up = False
            self.fail("peer_lost", 6.0, stage=False)
            return f"ms={2000 if 'pdeathsig_slow' in self.bad else 120}\n"
        self.H.up, self.orphan = False, True
        if self.flight and self.flight.ended_at is None:
            self.flight.end_at, self.flight.end_error = self.clock.t + 0.05, {"code": "transport", "message": "reset"}
        self.pending.append((self.clock.t + (9.0 if "r_teardown_slow" in self.bad else 4.5),
                             lambda: self.close("peer_lost", head=False)))
        return ""

    def start_daemon(self, who, other):
        who.up = True
        self.orphan = "no_reap" in self.bad and who is self.H
        self.dial(who, other, None)

    def weights(self, m, who):
        """The weight bytes `who` holds: all of them, H's share or R's share of config P."""
        k, n = m["split_at"], m["n_layer"]
        total = self.BLOCK * n + self.INPUT + self.OUTPUT
        return {"full": total, "h": total - self.BLOCK * k, "r": self.BLOCK * k}[who]

    def on_wire(self):
        """Opaque SYNTHETIC records where a real capture has datagrams; the relay's byte counts follow the session's."""
        c = self.conn(self.H, self.R)
        wire, s = self.wire, self.session
        if not (wire and c and s and c["addr"] == f"127.0.0.1:{wire.port}"):
            return
        wire.record(0, random.Random(self.requests).randbytes(1200))
        wire.record(1, random.Random(-self.requests).randbytes(1200))
        if "wire_canary" in self.bad:
            wire.record(0, b"xx" + self.canaries[0].encode() + b"yy")
        if "wire_weights" in self.bad:
            data = Path(self.blobs[s["model"]]).read_bytes()
            for i in range(0, len(data), 1200):
                wire.record(0, data[i:i + 1200])
        wire.bytes[0] = 1000 if "relay_short" in self.bad else s["c"]["to_r"] + 1000
        wire.bytes[1] = s["c"]["to_h"] + 1000

    def record(self, me, pid):
        r = super().record(me, pid)
        c = r["connection"]
        if c and me is self.R and self.wire and "relay_invisible" not in self.bad \
                and self.conn(self.H, self.R)["addr"] == f"127.0.0.1:{self.wire.port}":
            c["remote_addr"] = f"10.0.0.1:{self.wire.back_port}"  # R sees the relay's socket, not H's
        return r

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
            local = me is self.H and split is None
            return 200, {"loaded": me.loaded and {
                "model_id": me.loaded, "split": split, "pid": self.FAKE_H_PID if me is self.H else self.FAKE_R_PID,
                "n_ctx": 512 if local and "ctx_differs" in self.bad else 1024, "parallel_slots": 4, "threads": 4,
                "device_requested": self.devices.get(me.key, "auto"), "device_actual": "SYNTHETIC"},
                "split": s and {"session": s["id"], **s["c"],
                                "stage_device": "MTL0" if "c_stage_gpu" in self.bad else "CPU (SYNTHETIC)"}}
        if p == "/api/unload":
            if "unload_refused" in self.bad and me is self.H and self.session:
                return 409, {"error": {"code": "overloaded", "message": "overloaded"}}
            me.loaded = None
            self.devices.pop(me.key, None)
            if me is self.H:
                self.close()
            return 204, None
        if p == "/api/jobs":
            return 200, {"active": [{"id": "stuck"}] if "job_stuck" in self.bad else [], "recent": []}
        if p == "/api/models":
            return 200, {"models": [{"id": k, "downloaded": not (me is self.R and "r_no_model" in self.bad)}
                                    for k in self.models]}
        if p.startswith("/api/models/") and p.endswith("/load") and "cpu_ignored" not in self.bad:
            self.devices[me.key] = body.get("device", "auto")
        if p == "/api/pair/join" and "repair_fails" in self.bad and self.revoked_once:
            return 502, {"error": {"code": "peer_lost", "message": "peer_lost"}}
        if p.endswith("/connect") and self.wire and body.get("addr") == f"127.0.0.1:{self.wire.port}" \
                and "bypass_relay" not in self.bad:
            me.held.discard(self.R.id)
            if "redial_race" in self.bad and not self.raced:  # R's redial lands first: H's connect returns that one
                self.raced = True
                self.link(me, self.R, self.R, me.addrs[0])
                return 200, self.record(me, self.R.id)
            self.link(me, self.R, me, body["addr"])
            me.pins[self.R.id]["last_addr"] = body["addr"]
            return 200, self.record(me, self.R.id)
        reply = super().route(me, method, p, body)
        self.inject(me, method, p, body)
        return reply

    def sse(self, base, body, timeout=900):
        self.tick()
        me, tgt, m = self.d[base], body.get("target", "local"), self.models[body["model"]]
        n, tokens = body["max_tokens"], self.tokens.get(body["prompt"], 2)
        ttft, tpot = self.TIMES[m["label"]]
        self.last_model = m

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
            if me is self.H:
                self.engine = "local"
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
        self.on_wire()
        k, nl = m["split_at"], m["n_layer"]
        devs = ("MTL0", "RPC0") if "wrong_placement" in self.bad else ("RPC0", "MTL0")
        if body.get("split_mode") == "same_backend":
            devs = ("MTL0", "RPC0") if "c_wrong_placement" in self.bad else ("CPU", "RPC0")
        self.loaded_split = [{"device": devs[0], "first_block": 0, "end_block": k},
                             {"device": devs[1], "first_block": k, "end_block": nl}]
        me.loaded, self.engine = body["model"], "split"
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

    def footprint_text(self):
        m = self.last_model
        hosted = self.weights(m, "h" if self.engine == "split" and "head_keeps_all_layers" not in self.bad else "full")
        kb = (self.BASE + hosted) >> 10
        return (f"llama-server [{self.FAKE_H_PID}]: 64-bit    Footprint: {kb} KB (16384 bytes per page)\n"
                "  Dirty      Clean  Reclaimable    Regions    Category\n"
                "    ---        ---          ---        ---    ---\n"
                f" {kb} KB      35 MB          0 B       3348    TOTAL\n\nAuxiliary data:\n"
                f"    phys_footprint: {kb} KB\n"
                + ("" if "no_footprint" in self.bad else f"    phys_footprint_peak: {kb} KB\n"))

    def shell_extra(self, where, key, kw):
        """The commands of the steps beyond the core run; None for the others."""
        m = self.last_model
        if (where, key) == ("r", "proc_status"):
            if "ssh_drops" in self.bad:
                raise self.driver.RShellError("R command 'proc_status' exited 255: SYNTHETIC")
            rpc = str(kw["pid"]) != str(self.FAKE_R_PID)
            hosted = self.weights(m, "r" if rpc and "stage_holds_all" not in self.bad else "full")
            return f"Name:\tx\nVmPeak:\t  9 kB\nVmHWM:\t  {(self.BASE + hosted) >> 10} kB\nVmRSS:\t  1 kB\n"
        if (where, key) == ("h", "footprint"):
            return self.footprint_text()
        if key == "argv":
            return "SYNTHETIC argv\n"
        if (where, key) == ("r", "kill_rpc"):
            self.fail("backend_failed", 2.0)
            return ""
        if (where, key) == ("r", "kill_daemon"):
            return self.kill_daemon(self.R)
        if (where, key) == ("r", "start"):
            self.start_daemon(self.R, self.H)
            return "100\n"
        if (where, key) == ("h", "alive"):
            return "4242\n" if self.orphan else ""
        if (where, key) == ("h", "run"):
            if kw["cmd"] == "fake h kill":
                return self.kill_daemon(self.H)
            self.start_daemon(self.H, self.R)
            return ""
        return None

    def shell(self, where, key, **kw):
        self.tick()
        extra = self.shell_extra(where, key, kw)
        if extra is not None:
            return extra
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
    for name in ("sh", "stream", "sse", "shell", "Flight"):
        if hasattr(d, name) and hasattr(mesh, name):
            setattr(d, name, getattr(mesh, name))
    if hasattr(d, "Relay") and hasattr(mesh, "patch"):
        mesh.patch(d)
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
        files = {str(p.relative_to(out)): p.read_text(errors="replace") for p in Path(out).rglob("*")
                 if p.is_file() and p.name not in ("raw.jsonl", "results.json")}
    finally:
        d.RAW and d.RAW.close()
        shutil.rmtree(out)
    return {r["step"]: r for r in steps}, buf.getvalue().splitlines(), code, files
