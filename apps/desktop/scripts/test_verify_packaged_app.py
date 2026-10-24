"""Offline checks of verify_packaged_app.py's daemon-log parsing, stage-6 condition and THIRD_PARTY_NOTICES check
(synthetic text, no app run)."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_packaged_app import daemon_restarted, log_holds_both_runs, names_packages  # noqa: E402

L = "listening on http://127.0.0.1:8192\n"
RUN = lambda pid: f"common-desktop: started the daemon, pid {pid}\n" + L  # noqa: E731
EARLIER = RUN(11) + "common-desktop: daemon healthy on 127.0.0.1:8192\n" + RUN(22)  # stages 1 and 5


class LogHoldsBothRuns(unittest.TestCase):
    def test_killed_run_restart_note_and_restarted_run(self):
        text = EARLIER + "common-desktop: daemon exited with code None, signal Some(9)\n" \
            "common-desktop: restarting the daemon in 1s\n" + RUN(33)
        self.assertTrue(log_holds_both_runs(text, 22, 33))

    def test_restarted_run_output_missing(self):
        # Two listening lines from earlier launches and the restart note: the restarted daemon logged nothing.
        text = EARLIER + "common-desktop: restarting the daemon in 1s\n" \
            "common-desktop: started the daemon, pid 33\n"
        self.assertFalse(log_holds_both_runs(text, 22, 33))

    def test_no_restart_note_or_wrong_pid(self):
        self.assertFalse(log_holds_both_runs(EARLIER + RUN(33), 22, 33))
        text = EARLIER + "common-desktop: restarting the daemon in 1s\n" + RUN(33)
        self.assertFalse(log_holds_both_runs(text, 2, 33), "pid 2 is not pid 22")
        self.assertFalse(log_holds_both_runs(text, 22, 3), "pid 3 is not pid 33")

    def test_killed_run_start_note_without_its_listening_line(self):
        # Round 6b: the killed daemon's start note is there but it never logged its listening line; the restarted
        # run is complete. Earlier launches' listening lines must not stand in for it.
        text = RUN(11) + "common-desktop: started the daemon, pid 22\n" \
            "common-desktop: daemon exited with code None, signal Some(9)\n" \
            "common-desktop: restarting the daemon in 1s\n" + RUN(33)
        self.assertFalse(log_holds_both_runs(text, 22, 33))


class Stage6(unittest.TestCase):
    TEXT = EARLIER + "common-desktop: restarting the daemon in 1s\n" + RUN(33)

    def test_restarted_by_the_app_within_10_s(self):
        self.assertTrue(daemon_restarted(5, 5, 9.8, self.TEXT, 22, 33))
        self.assertTrue(daemon_restarted(5, 5, 10.0, self.TEXT, 22, 33))

    def test_health_after_10_s_fails(self):
        # Round 6b: the wait's last check can pass after its deadline; the step must still fail.
        self.assertFalse(daemon_restarted(5, 5, 10.4, self.TEXT, 22, 33))

    def test_not_the_apps_child_or_log_incomplete(self):
        self.assertFalse(daemon_restarted(6, 5, 2.0, self.TEXT, 22, 33))
        self.assertFalse(daemon_restarted(5, 5, 2.0, EARLIER + RUN(33), 22, 33))


class Notices(unittest.TestCase):
    # The format scripts/gen_notices.py writes.
    HEAD = "Third-party software in this build of UCompute (formerly Common).\n\n"
    RUSTLS = "rustls 0.23.45 — Apache-2.0 OR ISC OR MIT — https://github.com/rustls/rustls — licence text T11\n"
    REACT = "react 19.3.0 — MIT — https://react.dev/ — licence text T223\n"

    def test_both_named(self):
        self.assertTrue(names_packages(self.HEAD + self.RUSTLS + self.REACT, "rustls", "react"))

    def test_missing_or_only_similar_names(self):
        self.assertFalse(names_packages("", "rustls", "react"))
        self.assertFalse(names_packages(self.HEAD + self.RUSTLS, "rustls", "react"))
        similar = "rustls-pki-types 1.14.0 — MIT OR Apache-2.0\nreact-dom 19.3.0 — MIT\n"
        self.assertFalse(names_packages(self.HEAD + similar, "rustls", "react"))
        in_licence_text = self.HEAD + self.REACT + "\n[T1]\n\nrustls 0.23 is licensed under any of ...\n"
        self.assertFalse(names_packages(in_licence_text, "rustls"))


if __name__ == "__main__":
    unittest.main()
