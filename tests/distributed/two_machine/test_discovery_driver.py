"""Offline checks of discovery_two_machine.py's own logic against SYNTHETIC fake daemons (fake_mesh.py) and a fake
clock: no network, machine or daemon is involved, so nothing here is evidence of Common.
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import os
import unittest

from fake_mesh import Mesh, drive, load

ALL = ("--steps", "state,manual,pair,quit,crash,wifi,offline", "--quit-b", "quit", "--crash-b", "crash",
       "--start-b", "start", "--procs-b", "procs", "--wifi-cycle-b", "wifi", "--watch", "5",
       "--bogus-addr", "192.0.2.1:8193", "--stranger", "http://s", "--stranger-addr", "10.0.0.9:8193")
HINT = "manual: a hint with no Common device falls through to a known address and is not saved"
REQ5 = "manual: the address shown on B connects A to B (requirement 5)"
STRANGER = "manual: another Common device at the hinted address is not trusted (requirement 8)"
WIFI = "wifi: B's Wi-Fi drops, A loses B, then a new connection forms"
CACHED = "crash: B's signed record may stay cached, but A never shows it as connected"
TTL = "crash: the cached record leaves nearby by the mDNS TTL (about 120 s after the last announcement)"
E6 = "quit: A drops B from nearby within seconds (goodbye sent on quit, E6)"


def run(mesh, *flags, mdns=False):
    return drive("discovery_two_machine.py", mesh, *ALL, *(() if mdns else ("--no-mdns",)), *flags)


class SyntheticDiscoveryDriverTests(unittest.TestCase):
    def test_no_mdns_run_passes_and_reports_na_apart_from_pass(self):
        steps, out, code, files = run(Mesh())
        na = [s for s in steps.values() if "na" in s]
        self.assertEqual(len(na), 6, out)
        self.assertTrue(all(s["ok"] is None for s in na))
        self.assertEqual(out[0], "daemon API run (not app UI): every step goes through the two daemons' loopback APIs")
        self.assertEqual(out[-1], "ALL PASS (6 N/A)")
        self.assertEqual(code, 0)
        self.assertEqual(files["b_host"], "10.0.0.2\n")

    def test_mdns_run_passes(self):
        steps, out, code, _ = run(Mesh(nearby=True), mdns=True)
        self.assertEqual((out[-1], code), ("ALL PASS (2 N/A)", 0), out)
        self.assertIs(steps[CACHED]["ok"], True)

    def test_last_seen_waits_for_a_connection_still_forming(self):
        mesh = Mesh()
        mesh.sh("wifi")  # not connected yet; B dials A 10 s in
        steps, _, _, _ = drive("discovery_two_machine.py", mesh, "--steps", "state", "--no-mdns")
        self.assertIs(steps["state: last seen advances while connected (E5)"]["ok"], True)

    def test_a_failed_step_exits_3(self):
        steps, out, code, _ = run(Mesh(bad={"quit_is_kill"}))
        step = steps["quit: B's daemon closed the connection itself (graceful stop, not the kill fallback)"]
        self.assertEqual((step["ok"], step["lost_reason"]), (False, "timed_out"))
        self.assertTrue(out[-1].startswith("FAILED: "), out[-1])
        self.assertEqual(code, 3)

    def test_a_failed_restore_fails_the_run(self):
        steps, _, code, _ = drive("discovery_two_machine.py", Mesh(), "--steps", "state", "--no-mdns", restore=1)
        self.assertIs(steps["restore: both daemons put back as saved (state.py)"]["ok"], False)
        self.assertEqual(code, 3)

    # manual: each connect must really dial (an open connection is returned as is)
    def test_connects_that_never_dial_fail(self):
        mesh = Mesh(bad={"ignores_disconnect"})
        A, B = mesh.d["http://a"], mesh.d["http://b"]
        A.pins[B.id]["last_addr"] = B.addrs[0]  # as usual, A already has B pinned at the address B shows
        steps, _, _, _ = run(mesh)
        for name in (HINT, REQ5, STRANGER):
            self.assertIs(steps[name]["ok"], False, name)

    def test_a_hint_that_is_never_dialled_fails(self):
        steps, _, _, _ = run(Mesh(bad={"skip_hint"}))
        self.assertIs(steps[HINT]["ok"], False)
        self.assertIs(steps[STRANGER]["ok"], False)
        self.assertIs(steps[REQ5]["ok"], False)  # B's address is a hint too: never dialled there, never pinned

    def test_a_stranger_with_a_connected_device_of_its_own_is_compared_by_ids(self):
        mesh = Mesh()
        S, C = mesh.d["http://s"], mesh.d["http://c"]
        mesh.call("http://c", "POST", "/api/pair/start", {})
        mesh.call("http://s", "POST", "/api/pair/join", {"addr": C.pair_addr, "code": "123456"})  # its record moves
        steps, _, _, _ = run(mesh)
        self.assertIs(steps[STRANGER]["ok"], True)
        self.assertIn(C.id, S.pins)

    def test_a_saved_hint_fails(self):
        steps, _, _, _ = run(Mesh(bad={"saves_hint"}))
        self.assertIs(steps[HINT]["ok"], False)

    def test_a_crossed_dial_is_retried_once(self):
        steps, _, _, _ = run(Mesh(race=1))
        self.assertEqual((steps[REQ5]["ok"], steps[REQ5]["attempts"]), (True, 2))
        steps, _, _, _ = run(Mesh(race=2))
        self.assertIs(steps[REQ5]["ok"], False)

    def test_repair_requires_the_host_names(self):
        steps, _, _, _ = run(Mesh(bad={"names_not_host"}))
        self.assertIs(steps["pair: connected after re-pairing, named by host name"]["ok"], False)

    def test_repair_is_na_when_no_pairing_succeeded(self):
        steps, _, _, _ = run(Mesh(bad={"pair_start_fails"}))
        self.assertIsNone(steps["pair: connected after re-pairing, named by host name"]["ok"])

    def test_a_full_audit_window_does_not_hide_a_new_line_on_b(self):
        steps, _, _, _ = run(Mesh(bad={"leak"}, audit_b=1000))
        self.assertIs(steps["manual: an unpaired id is refused before any packet (requirement 8)"]["ok"], False)

    def test_an_unreadable_audit_is_a_fail_not_no_lines(self):
        steps, _, code, _ = run(Mesh(bad={"audit_fails_b"}))
        self.assertIs(steps["audit: the audit trail on http://b can be read"]["ok"], False)
        self.assertEqual(code, 3)

    # quit / crash
    def test_e4_needs_a_line_and_one_per_address(self):
        # (after a quit, the dead-hint connect's own dial at the pinned address writes a line even with no redial)
        for bad, hows in (("no_redial_lines", ("crash",)), ("audit_each_redial", ("quit", "crash"))):
            steps, _, _, _ = run(Mesh(bad={bad}))
            for how in hows:
                self.assertIs(steps[f"{how}: one audit line per address for B while it is away, not one per redial (E4)"]["ok"],
                              False, (bad, how))

    def test_a_silent_process_count_is_not_zero(self):
        steps, _, _, _ = run(Mesh(bad={"procs_silent"}))
        self.assertIs(steps["quit: B's daemon and engine processes end"]["ok"], False)

    def test_nearby_checks_without_their_premise_are_na(self):
        steps, _, _, _ = run(Mesh(nearby=True, bad={"crash_forgets"}), mdns=True)
        self.assertIsNone(steps[CACHED]["ok"])
        self.assertIsNone(steps[TTL]["ok"])
        steps, _, _, _ = run(Mesh(nearby=False), mdns=True)  # mDNS on, yet B was never listed
        self.assertIsNone(steps[E6]["ok"])

    def test_a_quit_without_goodbye_fails_e6(self):
        steps, _, _, _ = run(Mesh(nearby=True, bad={"no_goodbye"}), mdns=True)
        self.assertIs(steps[E6]["ok"], False)

    def test_restart_records_who_dialled(self):
        steps, _, _, _ = run(Mesh())
        self.assertEqual(steps["quit: B restarted, the two reconnect"]["a_connected_direction"], "inbound")

    def test_an_abort_while_b_is_away_starts_b_again(self):
        mesh, d = Mesh(), load("discovery_two_machine.py")
        d.call, d.sh, d.time, d.NO_MDNS = mesh.call, mesh.sh, mesh.clock, True
        d.away = lambda *a: (mesh.sh("crash"), 1 / 0)  # the run dies while B is down
        args = type("A", (), {"start_b": "start"})
        with open(os.devnull, "w") as d.RAW, self.assertRaises(ZeroDivisionError):
            d.lost_and_back("http://a", "http://b", mesh.d["http://a"].id, mesh.d["http://b"].id, args, "crash", "crash")
        self.assertTrue(mesh.d["http://b"].up)

    # wifi
    def test_a_wifi_cycle_that_never_drops_fails(self):
        steps, _, code, _ = run(Mesh(bad={"wifi_never_drops"}))
        self.assertIs(steps[WIFI + " (no mDNS: by redial)"]["ok"], False)
        self.assertEqual(code, 3)

    def test_a_new_lease_leaves_the_restore_to_the_runner(self):
        # B returns at another address; its API (the tunnel to the old one) no longer answers: no restore through it
        steps, out, code, files = run(Mesh(bad={"new_lease"}))
        restore = steps["restore: both daemons put back as saved (state.py)"]
        self.assertIsNone(restore["ok"])
        self.assertIn("left to the runner's restore", restore["na"])
        self.assertEqual((files["b_host"], out[-1], code), ("10.0.0.42\n", "ALL PASS (7 N/A)", 0))

    def test_b_not_coming_back_stops_the_run_and_keeps_the_results(self):
        steps, out, code, _ = run(Mesh(bad={"wifi_never_back"}))
        self.assertEqual(code, "wifi: B did not come back; stopping before anything else talks to B")
        self.assertIs(steps[WIFI + " (no mDNS: by redial)"]["ok"], False)
        self.assertIn("restore: both daemons put back as saved (state.py)", steps)
        self.assertNotIn("offline", steps)

    def test_freshly_signed_is_na(self):
        steps, _, _, _ = run(Mesh(nearby=True), mdns=True)
        self.assertIsNone(steps["wifi: the record A hears is freshly signed (D1)"]["ok"])
        self.assertIs(steps["wifi: A hears B's record again after the reconnect"]["ok"], True)

    # offline and missing flags
    def test_missing_flags_are_na_not_silent(self):
        steps, _, _, _ = drive("discovery_two_machine.py", Mesh(), "--steps", "quit,crash,wifi,offline", "--no-mdns")
        self.assertEqual([steps[k]["ok"] for k in ("quit", "crash", "wifi", "offline")], [None] * 4)

    def test_offline_needs_a_paired_device(self):
        mesh = Mesh()
        steps, _, _, _ = drive("discovery_two_machine.py", mesh, "--steps", "offline", "--no-mdns", "--watch", "5",
                               "--offline-peer", "cd" * 32)
        self.assertIs(steps["offline: the switched-off device is paired on A"]["ok"], False)
        steps, _, _, _ = drive("discovery_two_machine.py", Mesh(), "--steps", "offline", "--no-mdns", "--watch", "5",
                               "--offline-peer", mesh.d["http://b"].id)
        self.assertIs(steps["offline: a switched-off paired device costs at most one audit line per address (E4)"]["ok"], True)
        self.assertIsNone(steps["offline: A kept redialling it during the watch (a newer last_error)"]["ok"])


class SyntheticAuditMarkTests(unittest.TestCase):
    """The API lists the last 1000 events oldest first: a mark must find what came after it."""

    def setUp(self):
        self.d = load("discovery_two_machine.py")
        self.ev = lambda n: {"ts_utc": f"{n:06d}", "kind": "peer_rejected", "n": n}

    def test_new_events_after_the_mark(self):
        trail = [self.ev(i) for i in range(5)]
        mark = self.d.mark_of(trail)
        self.assertEqual(self.d.since(trail + [self.ev(5), self.ev(6)], mark), [self.ev(5), self.ev(6)])
        self.assertEqual(self.d.since(trail, mark), [])

    def test_a_full_window_slides_and_old_lines_stay_out(self):
        trail = [self.ev(i) for i in range(1000)]
        mark = self.d.mark_of(trail)
        later = (trail + [self.ev(1000), self.ev(1001)])[-1000:]
        self.assertEqual(self.d.since(later, mark), [self.ev(1000), self.ev(1001)])

    def test_a_mark_that_slid_out_means_everything_is_new(self):
        mark = self.d.mark_of([self.ev(i) for i in range(3)])
        later = [self.ev(i) for i in range(10, 20)]
        self.assertEqual(self.d.since(later, mark), later)

    def test_an_empty_trail_and_repeated_events(self):
        self.assertEqual(self.d.since([self.ev(1)], self.d.mark_of([])), [self.ev(1)])
        same = {"ts_utc": "1", "kind": "peer_connected"}
        mark = self.d.mark_of([same, same, same])
        self.assertEqual(self.d.since([same, same, same, same], mark), [same])


if __name__ == "__main__":
    unittest.main()
