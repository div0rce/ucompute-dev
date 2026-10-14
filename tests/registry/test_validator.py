"""Negative and positive tests for scripts/validate_registry.py.

Each test copies docs/ and scripts/ into a temp dir, mutates the ledger, and checks
that the validator rejects it with the expected message.
Run: python3 -m unittest discover -s tests/registry -v
"""
import csv
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


class ValidatorTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        for d in ("docs", "scripts"):
            shutil.copytree(ROOT / d, self.tmp / d)
        self.csv = self.tmp / "docs" / "REQUIREMENTS.csv"

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def run_validator(self):
        p = subprocess.run([sys.executable, str(self.tmp / "scripts" / "validate_registry.py")],
                           capture_output=True, text=True)
        return p.returncode, p.stdout + p.stderr

    def rows(self):
        with self.csv.open(newline="", encoding="utf-8") as f:
            r = csv.DictReader(f)
            return r.fieldnames, list(r)

    def write(self, fields, rows, regen=True):
        with self.csv.open("w", newline="", encoding="utf-8") as f:
            w = csv.DictWriter(f, fieldnames=fields, lineterminator="\n")
            w.writeheader()
            w.writerows(rows)
        if regen:  # keep generated MD in sync so only the targeted rule fires
            subprocess.run([sys.executable, str(self.tmp / "scripts" / "gen_registry.py")],
                           check=True, capture_output=True)

    def test_baseline_valid(self):
        code, out = self.run_validator()
        self.assertEqual(code, 0, out)
        self.assertIn("148 P0 rows", out)

    def test_verified_without_evidence_rejected(self):
        f, rows = self.rows()
        rows[0]["status"], rows[0]["observed_evidence"], rows[0]["last_verified_commit"] = "VERIFIED", "", ""
        self.write(f, rows)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("VERIFIED without observed_evidence", out)
        self.assertIn("VERIFIED without a valid last_verified_commit", out)

    def test_verified_with_unverified_dependency_rejected(self):
        f, rows = self.rows()
        r = next(x for x in rows if x["id"] == "F-01-03")  # depends on F-01-01, F-01-02
        r.update(status="VERIFIED", observed_evidence="docs/VERIFICATION.md#x", last_verified_commit="abc1234")
        self.write(f, rows)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("VERIFIED but dependency F-01-01", out)

    def test_missing_row_rejected(self):
        f, rows = self.rows()
        self.write(f, rows[1:], regen=False)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("expected 148", out)

    def test_unknown_status_rejected(self):
        f, rows = self.rows()
        rows[5]["status"] = "DONE"
        self.write(f, rows, regen=False)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("invalid status 'DONE'", out)

    def test_cycle_detected(self):
        data = self.tmp / "scripts" / "registry_data.py"
        s = data.read_text()
        s = s.replace('("F-01-01","M1","runtime","",', '("F-01-01","M1","runtime","F-01-02",', 1)
        data.write_text(s)
        subprocess.run([sys.executable, str(self.tmp / "scripts" / "gen_registry.py")], check=True, capture_output=True)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("dependency cycle", out)

    def test_stale_generated_markdown_rejected(self):
        md = self.tmp / "docs" / "requirements" / "REGISTRY.md"
        md.write_text(md.read_text() + "\nhand edit\n")
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("STALE", out)

    def test_regeneration_preserves_ledger(self):
        f, rows = self.rows()
        rows[10]["observed_evidence"] = "kept"
        rows[10]["status"] = "IN_PROGRESS"
        self.write(f, rows)
        _, after = self.rows()
        self.assertEqual(after[10]["observed_evidence"], "kept")
        self.assertEqual(after[10]["status"], "IN_PROGRESS")


if __name__ == "__main__":
    unittest.main()
