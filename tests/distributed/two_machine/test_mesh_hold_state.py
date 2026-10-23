"""Offline checks of mesh_two_machine.py, hold_two_machine.py and state.py against SYNTHETIC fake daemons
(fake_mesh.py) and a fake clock: no network, machine, daemon or inference is involved (the completion text is
canned), so nothing here is evidence of Common.
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest
import urllib.error
from unittest import mock

from fake_mesh import Mesh, drive, load

CROSS = "cross-hold: revocations cross a hold both ways; the hold stays (trust-5, ADR-0003 D7)"
SHARING = "held: turning Sharing off and on keeps the hold"
CONNECT = "connect: ends the hold, both sides connected"
REVOKE = "revoke while held: A unpins B, B's request to A is refused as revoked"
REFUSAL = "held: a proven refusal ends B's dial (no later address replaces peer_held)"
RESTORE = "restore: both daemons put back as saved (state.py)"


def names(mesh):
    A, B = mesh.d["http://a"], mesh.d["http://b"]
    return A.pins[B.id]["name"], B.pins[A.id]["name"]


def owner_names(mesh):
    """Names the owners gave at pairing (not the host names a pairing without a name stores)."""
    A, B = mesh.d["http://a"], mesh.d["http://b"]
    A.pins[B.id]["name"], B.pins[A.id]["name"] = "Jess's laptop", "Mous"
    return mesh


class SyntheticMeshDriverTests(unittest.TestCase):
    def test_no_mdns_run_starts_unpaired_passes_and_keeps_the_names(self):
        mesh = owner_names(Mesh())
        steps, out, code, _ = drive("mesh_two_machine.py", mesh, "--no-mdns")
        self.assertEqual((out[-1], code), ("ALL PASS (2 N/A)", 0), out)
        self.assertEqual(steps["setup: A and B start unpaired (unpinned, not revoked)"]["delete_statuses"], [204, 204])
        self.assertIs(steps["an unpaired peer cannot receive work"]["ok"], True)
        self.assertEqual(names(mesh), ("Jess's laptop", "Mous"))

    def test_mdns_run_lists_the_unpaired_peer_as_untrusted(self):
        steps, out, code, _ = drive("mesh_two_machine.py", Mesh(nearby=True))
        self.assertEqual((out[-1], code), ("ALL PASS", 0), out)
        self.assertIs(steps["discovery: an unpaired peer is listed as untrusted"]["ok"], True)

    def test_setup_puts_are_checked(self):
        steps, _, code, _ = drive("mesh_two_machine.py", Mesh(bad={"groups_put_fails"}), "--no-mdns")
        self.assertIs(steps["group and privacy set on both"]["ok"], False)
        self.assertEqual(code, 3)

    def test_an_unread_or_leaking_audit_fails(self):
        steps, _, _, _ = drive("mesh_two_machine.py", Mesh(bad={"audit_fails_a"}), "--no-mdns")
        self.assertIs(steps["audit on A has no prompt text"]["ok"], False)
        steps, _, _, _ = drive("mesh_two_machine.py", Mesh(bad={"leaks_prompt"}), "--no-mdns")
        self.assertIs(steps["audit on B has no prompt text"]["ok"], False)

    def test_revocation_needs_a_refusal_as_revoked(self):
        for bad in ("revoked_reads_lost", "revoked_unaudited"):  # B reads no refusal; A never refused it as revoked
            steps, _, _, _ = drive("mesh_two_machine.py", Mesh(bad={bad}), "--no-mdns")
            self.assertIs(steps["revocation: B disconnected and refused as revoked"]["ok"], False, bad)

    def test_no_revocation_after_a_failed_remote_run(self):
        mesh = owner_names(Mesh())
        mesh.d["http://b"].serve = False
        steps, _, _, _ = drive("mesh_two_machine.py", mesh, "--no-mdns")
        self.assertIs(steps["revocation: skipped (pairing or remote execution failed; B left paired)"]["ok"], False)
        self.assertNotIn(mesh.d["http://b"].id, mesh.d["http://a"].revoked)
        self.assertEqual(names(mesh), ("Jess's laptop", "Mous"))  # the one pairing (step 2) kept them

    def test_a_failed_pairing_after_unpairing_stops_the_run(self):
        for bad in ("pair_start_fails", "pair_join_fails"):
            steps, _, code, _ = drive("mesh_two_machine.py", Mesh(bad={bad}), "--no-mdns")
            self.assertTrue(str(code).startswith("A and B were unpaired for this run"), (bad, code))
            self.assertIn(RESTORE, steps)


class SyntheticHoldDriverTests(unittest.TestCase):
    def test_run_passes_and_keeps_the_names(self):
        mesh = owner_names(Mesh())
        steps, out, code, _ = drive("hold_two_machine.py", mesh)
        self.assertEqual((out[-1], code), ("ALL PASS (2 N/A)", 0), out)
        self.assertIsNone(steps[CROSS]["ok"])
        self.assertEqual(names(mesh), ("Jess's laptop", "Mous"))

    def test_cross_hold(self):
        steps, out, code, _ = drive("hold_two_machine.py", Mesh(), "--c", "http://c", "--d", "http://d")
        self.assertEqual((out[-1], code), ("ALL PASS (1 N/A)", 0), out)
        steps, _, _, _ = drive("hold_two_machine.py", Mesh(bad={"held_drops_revocations"}), "--c", "http://c", "--d", "http://d")
        self.assertIs(steps[CROSS]["ok"], False)

    def test_a_sharing_change_that_drops_the_hold_fails_and_connect_has_no_hold_to_end(self):
        steps, _, _, _ = drive("hold_two_machine.py", Mesh(bad={"sharing_drops_hold"}))
        self.assertIs(steps[SHARING]["ok"], False)
        self.assertIs(steps[CONNECT]["ok"], False)
        self.assertIs(steps[CONNECT]["held_before"], False)

    def test_a_sharing_put_that_changes_nothing_fails(self):
        steps, _, _, _ = drive("hold_two_machine.py", Mesh(bad={"network_put_refused"}))
        self.assertIs(steps[SHARING]["ok"], False)

    def test_a_refused_sharing_put_fails_and_nothing_is_revoked(self):
        mesh = Mesh(bad={"network_put_fails"})
        steps, _, _, _ = drive("hold_two_machine.py", mesh)
        self.assertIs(steps[SHARING]["ok"], False)
        self.assertIs(steps["revoke while held: skipped (connect, run or networking failed; B left paired)"]["ok"], False)
        self.assertNotIn(mesh.d["http://b"].id, mesh.d["http://a"].revoked)

    def test_one_held_line_per_hold(self):
        name = "held: no reconnection for 30 s and exactly one audit line on A for B's attempts"
        for bad in ("audit_each_redial", "held_unaudited", "audit_fails_a"):  # two lines, none, an unread audit
            steps, _, _, _ = drive("hold_two_machine.py", Mesh(bad={bad}))
            self.assertIs(steps[name]["ok"], False, bad)
        self.assertIsNone(steps[name]["held_rejections"])

    def test_revoke_needs_a_refusal_as_revoked(self):
        steps, _, _, _ = drive("hold_two_machine.py", Mesh(bad={"held_before_revoked"}))
        self.assertEqual((steps[REVOKE]["ok"], steps[REVOKE]["b_code"]), (False, "peer_held"))
        steps, _, _, _ = drive("hold_two_machine.py", Mesh(bad={"revoked_unaudited"}))
        self.assertEqual((steps[REVOKE]["ok"], steps[REVOKE]["a_refused_as_revoked"]), (False, False))

    def test_a_failed_re_pair_stops_the_run(self):
        steps, _, code, _ = drive("hold_two_machine.py", Mesh(bad={"pair_join_fails"}))
        self.assertTrue(str(code).startswith("A revoked B and the re-pair failed"), code)
        self.assertIn(RESTORE, steps)

    def test_a_proven_refusal_ends_the_dial(self):
        mesh = Mesh(nearby=True)
        mesh.d["http://a"].ghost = ["10.0.0.11:8193"]  # A shows a second address, which does not answer
        steps, _, _, _ = drive("hold_two_machine.py", mesh)
        self.assertIs(steps[REFUSAL]["ok"], True)
        mesh = Mesh(nearby=True, bad={"refusal_continues"})
        mesh.d["http://a"].ghost = ["10.0.0.11:8193"]
        steps, _, _, _ = drive("hold_two_machine.py", mesh)
        self.assertIs(steps[REFUSAL]["ok"], False)

    def test_a_crash_still_restores_and_writes_the_results(self):
        mesh = Mesh()
        call = mesh.call
        mesh.call = lambda base, method, path, *a, **k: (_ for _ in ()).throw(RuntimeError("tunnel gone")) \
            if path.endswith("/revoke") else call(base, method, path, *a, **k)
        steps, _, code, _ = drive("hold_two_machine.py", mesh)
        self.assertIsInstance(code, RuntimeError)
        self.assertIs(steps[RESTORE]["ok"], True)


def api(mesh):
    """state.py's urllib call, answered by the fake daemons."""
    def call(base, method, path, body=None, timeout=30):
        s, v, _ = mesh.call(base, method, path, body)
        if s == 0:
            raise urllib.error.URLError(v)
        if s >= 400:
            raise urllib.error.HTTPError(base + path, s, json.dumps(v), {}, None)
        return v
    return call


class SyntheticStateTests(unittest.TestCase):
    def setUp(self):
        self.st, self.mesh = load("state.py"), owner_names(Mesh())
        self.st.call = api(self.mesh)
        self.A, self.B, self.C = (self.mesh.d[f"http://{k}"] for k in "abc")
        self.A.groups["home"] = {"name": "home", "members": [self.B.id], "policy": {"allow_remote_inference": True}}
        self.A.loaded = "owner-model"
        fd, self.file = tempfile.mkstemp()
        os.close(fd)
        self.addCleanup(os.remove, self.file)

    def main(self, *args):
        err = io.StringIO()
        with mock.patch.object(sys, "argv", ["state.py", *args]), contextlib.redirect_stdout(io.StringIO()), \
                contextlib.redirect_stderr(err):
            try:
                self.st.main()
                code = 0
            except SystemExit as e:
                code = e.code
        return code, err.getvalue()

    def save(self):
        self.assertEqual(self.main("save", self.file, "http://a", "http://b")[0], 0)

    def test_restore_puts_back_what_a_run_changes(self):
        self.save()
        m, A, B, C = self.mesh, self.A, self.B, self.C
        m.call("http://a", "POST", f"/api/peers/{B.id}/disconnect")  # a hold the run left
        m.call("http://a", "PUT", "/api/groups", {"name": "two-machine", "members": [B.id], "policy": {"allow_remote_inference": True}})
        m.call("http://b", "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})
        A.serve, A.loaded = False, "stories15M-q4_0"  # Sharing changed (a hold survives that), another model
        m.call("http://c", "POST", "/api/pair/start", {})
        m.call("http://a", "POST", "/api/pair/join", {"addr": C.pair_addr, "code": "123456"})  # paired during the run
        m.call("http://b", "POST", "/api/pair/start", {})
        m.call("http://a", "POST", "/api/pair/join", {"addr": B.pair_addr, "code": "123456"})  # re-paired, host names
        self.assertEqual(names(m), ("mac-b", "mac-a"))
        code, err = self.main("restore", self.file)
        self.assertEqual(code, 0, err)
        self.assertEqual((A.held, A.serve, A.loaded, B.privacy), (set(), True, "owner-model", "local_only"))
        self.assertEqual(A.groups["two-machine"]["members"], [])
        self.assertFalse(A.groups["two-machine"]["policy"]["allow_remote_inference"])
        self.assertEqual(A.groups["home"]["members"], [B.id])
        self.assertNotIn(C.id, A.pins)
        self.assertEqual(names(m), ("Jess's laptop", "Mous"))

    def test_networking_left_off_is_turned_on_again(self):
        self.save()
        self.A.enabled = False  # a Sharing change whose endpoint restart failed records networking off
        self.assertEqual(self.main("restore", self.file)[0], 0)
        self.assertTrue(self.A.enabled)

    def test_a_hold_the_run_left_is_ended(self):
        self.save()
        self.mesh.call("http://a", "POST", f"/api/peers/{self.B.id}/disconnect")
        self.assertEqual(self.main("restore", self.file)[0], 0)
        self.assertEqual(self.A.held, set())
        self.assertTrue(self.mesh.conn(self.A, self.B))

    def test_a_hold_set_before_the_run_stays(self):
        self.mesh.call("http://a", "POST", f"/api/peers/{self.B.id}/disconnect")
        self.save()
        self.assertEqual(self.main("restore", self.file)[0], 0)
        self.assertEqual(self.A.held, {self.B.id})

    def test_a_lost_pairing_is_reported_and_fails(self):
        self.save()
        self.mesh.call("http://a", "POST", f"/api/peers/{self.B.id}/revoke")
        code, err = self.main("restore", self.file)
        self.assertEqual(code, 1)
        self.assertIn(f"http://a lost its pairing with Jess's laptop {self.B.id}", err)
        self.assertEqual(self.A.groups["home"]["members"], [])

    def test_an_unreachable_daemon_does_not_stop_the_other(self):
        self.save()
        self.mesh.call("http://a", "PUT", "/api/privacy", {"mode": "trusted_group", "acknowledged": True})
        self.B.up = False
        code, err = self.main("restore", self.file)
        self.assertEqual(code, 1)
        self.assertIn("http://b not (fully) restored", err)
        self.assertEqual(self.A.privacy, "local_only")

    def test_a_file_saved_before_the_new_fields_still_restores(self):
        with open(self.file, "w") as f:
            json.dump({"http://a": {"groups": [], "privacy": "local_only", "serve_peers": True}}, f)
        self.A.enabled = False
        self.assertEqual(self.main("restore", self.file)[0], 0)
        self.assertTrue(self.A.enabled)


if __name__ == "__main__":
    unittest.main()
