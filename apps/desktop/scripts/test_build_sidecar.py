"""Offline check of build-sidecar.mjs's THIRD_PARTY_NOTICES step (synthetic: stub python3, rustc and cargo on PATH;
nothing is built). A licence failure in scripts/gen_notices.py must fail the build before the daemon is built."""
import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
NODE = shutil.which("node")
TRIPLE = "x86_64-pc-windows-msvc"  # what Tauri sets for the bundle being built (TAURI_ENV_TARGET_TRIPLE)


@unittest.skipIf(os.name == "nt" or not NODE, "needs node and /bin/sh")
class NoticesStep(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.stub("rustc", 'echo "host: aarch64-apple-darwin"')
        self.stub("cargo", f'echo ran > "{self.tmp}/cargo-ran"; exit 1')

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def stub(self, name, body):
        p = self.bin / name
        p.write_text(f"#!/bin/sh\n{body}\n")
        p.chmod(0o755)

    def python_stub(self, code):
        # --version answers; the real call records its argv, cwd and PYTHONUTF8, then exits with `code`.
        self.stub("python3", f'[ "$1" = --version ] && exit 0\n'
                  f'printf "%s\\n" "$PWD" "$PYTHONUTF8" "$@" > "{self.tmp}/python-argv"; exit {code}')

    def run_script(self):
        env = {"PATH": str(self.bin), "HOME": str(self.tmp), "TAURI_ENV_TARGET_TRIPLE": TRIPLE}
        return subprocess.run([NODE, str(HERE / "build-sidecar.mjs")], env=env, capture_output=True, text=True)

    def test_a_licence_failure_fails_the_build_before_the_daemon_is_built(self):
        self.python_stub(1)
        r = self.run_script()
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("build-sidecar: scripts/gen_notices.py failed", r.stderr)
        self.assertFalse((self.tmp / "cargo-ran").exists(), "cargo ran after the notices failed")
        cwd, utf8, *argv = (self.tmp / "python-argv").read_text().splitlines()
        self.assertEqual(Path(cwd).resolve(), ROOT)
        self.assertEqual(utf8, "1")
        out = str(ROOT / "apps/desktop/src-tauri/gen/THIRD_PARTY_NOTICES.txt")
        self.assertEqual(argv, ["scripts/gen_notices.py", "--out", out, "--crate", "common-cli", "--crate",
                                "common-desktop", "--js", "--target", TRIPLE], json.dumps(argv))

    def test_notices_written_then_the_daemon_is_built(self):
        self.python_stub(0)
        r = self.run_script()
        self.assertTrue((self.tmp / "cargo-ran").exists(), r.stderr)
        self.assertNotEqual(r.returncode, 0, "the stub cargo fails, so the script stops there")

    def test_no_python_fails_the_build(self):
        r = self.run_script()
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("THIRD_PARTY_NOTICES needs Python 3", r.stderr)
        self.assertFalse((self.tmp / "cargo-ran").exists())


if __name__ == "__main__":
    unittest.main()
