"""Offline checks of the two-machine runner scripts (run_*.sh): they parse, shellcheck finds nothing, and each one
stops on each missing required variable before it reaches any host. ssh, curl and the other tools a runner drives are
stubs that record a call and fail, so no machine, daemon or network is involved and nothing here is evidence of UCompute.
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import glob
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
RUNNERS = sorted(glob.glob(os.path.join(HERE, "run_*.sh")))
# Each runner's required variables, in the order its guard checks them.
REQUIRED = {
    "run_calibration_linux.sh": ("LINUX_HOST", "LINUX_USER", "LAB", "NETWORK"),
    "run_mac_linux.sh": ("LINUX_HOST", "LINUX_USER", "OUT"),
    "run_mac_mac.sh": ("OUT",),
    "run_mac_windows.sh": ("WIN_HOST", "OUT"),
}
UNSET = ("LINUX_HOST", "LINUX_USER", "LINUX_KEY", "WIN_HOST", "MAC_HOST", "MAC_USER", "MAC_KEY", "OUT", "LAB", "NETWORK")


class RunnerScriptTests(unittest.TestCase):
    def test_every_runner_is_known_here(self):
        self.assertEqual(sorted(map(os.path.basename, RUNNERS)), sorted(REQUIRED))

    def test_every_runner_parses(self):
        for r in RUNNERS:
            with self.subTest(runner=os.path.basename(r)):
                p = subprocess.run(["bash", "-n", r], capture_output=True, text=True)
                self.assertEqual(p.returncode, 0, p.stderr)

    def test_every_script_a_runner_names_exists(self):
        named = set()
        for r in RUNNERS:
            text = pathlib.Path(r).read_text()
            named |= set(re.findall(r"tests/distributed/two_machine/(\w+\.py)", text))
            named |= {f"{d}_two_machine.py" for d in re.findall(r"^\s*driver (\w+)", text, re.M)}
        self.assertIn("split_two_machine.py", named, "run_mac_linux.sh SPLIT=1 runs the WP-04 P1 driver")
        for n in named:
            self.assertTrue(os.path.exists(os.path.join(HERE, n)), n)

    @unittest.skipUnless(shutil.which("shellcheck"), "shellcheck is not installed")
    def test_shellcheck_finds_nothing(self):
        p = subprocess.run(["shellcheck", *RUNNERS], capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stdout + p.stderr)

    def test_each_missing_required_variable_stops_the_runner_before_any_host_is_reached(self):
        with tempfile.TemporaryDirectory() as d:
            log = os.path.join(d, "calls.log")
            for tool in ("ssh", "scp", "curl", "open", "pkill", "pgrep", "python3"):
                with open(os.path.join(d, tool), "w") as f:
                    f.write(f'#!/bin/sh\necho "{tool} $*" >> "{log}"\nexit 1\n')
                os.chmod(os.path.join(d, tool), 0o755)
            env = {k: v for k, v in os.environ.items() if k not in UNSET}
            env["PATH"] = f"{d}:{env['PATH']}"
            for r in RUNNERS:
                name = os.path.basename(r)
                required = REQUIRED[name]
                for i, missing in enumerate(required):
                    # Set every variable checked before this one (placeholders: the stubs fail any use of them).
                    run_env = dict(env, **{v: f"stub-{v.lower()}" for v in required[:i]})
                    with self.subTest(runner=name, missing=missing):
                        pathlib.Path(log).unlink(missing_ok=True)  # a stub call shows in this subtest only
                        p = subprocess.run(["bash", r], env=run_env, capture_output=True, text=True, timeout=30)
                        self.assertNotEqual(p.returncode, 0)
                        self.assertIn(f"set {missing}", p.stderr)
                        self.assertFalse(os.path.exists(log), os.path.exists(log) and pathlib.Path(log).read_text())


if __name__ == "__main__":
    unittest.main()
