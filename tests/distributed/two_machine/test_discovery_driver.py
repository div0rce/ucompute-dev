"""Offline checks of discovery_two_machine.py's own logic. Two fake daemons stand in for the APIs and a fake
clock replaces sleeping: no network, machine or daemon is involved, so nothing here is evidence of Common.
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import contextlib
import importlib.util
import io
import json
import shutil
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest import mock

DRIVER = Path(__file__).with_name("discovery_two_machine.py")
IDS = {"A": "aa" * 32, "B": "bb" * 32}
NET = {k: {"peer_id": IDS[k], "device_name": f"mac-{k.lower()}", "short_label": k, "dial_addr": f"10.0.0.{n}:8193"}
       for n, k in ((1, "A"), (2, "B"))}


class Clock:
    def __init__(self):
        self.t = 0.0

    def monotonic(self):
        return self.t

    def sleep(self, s):
        self.t += s

    strftime, gmtime = staticmethod(time.strftime), staticmethod(time.gmtime)


class World:
    """A and B, paired and connected. `stop`: how A sees B's quit end the connection ("closed_by_peer" when
    B's daemon closes it, "timed_out" when B was killed). `names`: what each side calls the other."""

    def __init__(self, stop="closed_by_peer", names=None, pairing=True, leak=False, audit_b=0):
        self.clock, self.stop, self.pairing, self.leak, self.up = Clock(), stop, pairing, leak, True
        self.names = names or {"A": NET["B"]["device_name"], "B": NET["A"]["device_name"]}
        self.audit = {"A": [], "B": [{"ts_utc": "old", "kind": "remote_request", "n": i} for i in range(audit_b)]}

    def log(self, me, kind, **fields):
        self.audit[me].append({"ts_utc": f"{self.clock.t:012.2f}", "kind": kind, "peer_id": IDS["B"],
                               "fields": fields, "n": len(self.audit[me])})

    def call(self, base, method, path, body=None, timeout=60):
        me, other = ("A", "B") if base == "http://a" else ("B", "A")
        if me == "B" and not self.up:
            return 0, "connection refused", 1.0
        route = (method, path.split("?")[0])
        if route == ("GET", "/api/network"):
            return 200, NET[me], 1.0
        if route == ("GET", "/health"):
            return 200, {}, 1.0
        if route == ("GET", "/api/audit"):
            return 200, {"events": self.audit[me][::-1][:1000]}, 1.0  # the API's cap, newest first
        if route == ("GET", "/api/peers"):
            return 200, {"discovered": [], "peers": [{
                "id": IDS[other], "name": self.names[me], "connected": self.up, "last_addr": NET[other]["dial_addr"],
                "health": "connected" if self.up else "disconnected", "last_seen_utc": f"{self.clock.t // 60:08.0f}",
                "last_error": None if self.up else "no answer"}]}, 1.0
        if route == ("POST", "/api/pair/start"):
            return (200, {"code": "123456", "pair_addr": "10.0.0.9:5000"}, 1.0) if self.pairing else (500, {}, 1.0)
        if route == ("POST", "/api/pair/join"):
            if body["code"] == "123456":
                return 200, {"name": NET[other]["device_name"]}, 1.0
            return 401, {"error": {"code": "pairing_failed", "message": "wrong code"}}, 1.0
        if method == "POST" and path.endswith("/connect"):
            if path.split("/")[3] not in IDS.values():
                if self.leak:
                    self.log("B", "peer_rejected")
                return 403, {"error": {"code": "peer_unauthorized"}}, 1.0
            return (200, {}, 1.0) if self.up else (504, {"error": {"code": "timeout", "message": "no answer"}}, 1.0)
        raise AssertionError(route)

    def sh(self, cmd):
        if cmd in ("quit", "crash"):
            self.up = False
            self.log("A", "peer_lost", reason=self.stop if cmd == "quit" else "timed_out")
            self.log("A", "peer_rejected")
        elif cmd == "start":
            self.up = True
        return "0" if cmd == "procs" and not self.up else ""


def run(world, *flags):
    spec = importlib.util.spec_from_file_location("driver", DRIVER)
    d = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(d)
    d.call, d.sh, d.time = world.call, world.sh, world.clock
    out = tempfile.mkdtemp()
    argv = ["driver", "--a", "http://a", "--b", "http://b", "--out", out, "--steps", "state,manual,pair,quit,crash",
            "--quit-b", "quit", "--crash-b", "crash", "--start-b", "start", "--procs-b", "procs", "--watch", "5", *flags]
    buf = io.StringIO()
    try:
        with mock.patch.object(sys, "argv", argv), contextlib.redirect_stdout(buf):
            d.main()
        steps = json.loads((Path(out) / "results.json").read_text())["steps"]
    finally:
        d.RAW and d.RAW.close()
        shutil.rmtree(out)
    return {r["step"]: r for r in steps}, buf.getvalue().splitlines()


class DriverTests(unittest.TestCase):
    def test_no_mdns_run_completes_and_reports_na_apart_from_pass(self):
        steps, out = run(World(), "--no-mdns")
        na = [s for s in steps.values() if "na" in s]
        self.assertEqual(len(na), 4, out)
        self.assertTrue(all(s["ok"] is None for s in na))
        self.assertEqual(out[0], "daemon API run (not app UI): every step goes through the two daemons' loopback APIs")
        self.assertEqual(out[-1], "ALL PASS (4 N/A)")

    def test_quit_that_was_a_kill_fails(self):
        steps, out = run(World(stop="timed_out"), "--no-mdns")
        step = steps["quit: B's daemon closed the connection itself (graceful stop, not the kill fallback)"]
        self.assertIs(step["ok"], False)
        self.assertEqual(step["lost_reason"], "timed_out")
        self.assertTrue(out[-1].startswith("FAILED: "), out[-1])

    def test_repair_requires_the_host_names(self):
        steps, _ = run(World(names={"A": "Kyle", "B": "Mous"}), "--no-mdns")
        self.assertIs(steps["pair: connected again after re-pairing, named by host name"]["ok"], False)

    def test_repair_is_na_when_no_pairing_succeeded(self):
        steps, _ = run(World(pairing=False), "--no-mdns")
        self.assertIsNone(steps["pair: connected again after re-pairing, named by host name"]["ok"])

    def test_a_full_audit_window_does_not_hide_a_new_line_on_b(self):
        steps, _ = run(World(leak=True, audit_b=1000), "--no-mdns")
        self.assertIs(steps["manual: an unpaired id is refused before any packet (requirement 8)"]["ok"], False)


if __name__ == "__main__":
    unittest.main()
