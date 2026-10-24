"""Offline check of verify_packaged_app.py's daemon-log parsing (synthetic log text, no app run)."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from verify_packaged_app import log_holds_both_runs  # noqa: E402

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


if __name__ == "__main__":
    unittest.main()
