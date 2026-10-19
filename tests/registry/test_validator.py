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
        # Pick any row whose first dependency is not VERIFIED (the ledger evolves; do not hardcode ids).
        by_id = {x["id"]: x for x in rows}
        r, dep = next(
            (x, d)
            for x in rows
            for d in x["dependencies"].split(";")
            if d and by_id[d]["status"] != "VERIFIED"
        )
        r.update(status="VERIFIED", observed_evidence="docs/VERIFICATION.md#x", last_verified_commit="abc1234")
        self.write(f, rows)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn(f"VERIFIED but dependency {dep}", out)

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

    def test_additive_priorities_and_cohort_counts(self):
        _, rows = self.rows()
        added = [r for r in rows if r["id"].startswith("U-")]
        self.assertEqual([r["priority"] for r in added], ["P1", "P2", "P1", "P1"])
        code, out = self.run_validator()
        self.assertEqual(code, 0, out)
        self.assertIn("4 added; 152 total", out)
        md = (self.tmp / "docs/requirements/REGISTRY.md").read_text()
        self.assertIn("| Status | Original | Added | Total |", md)

    def test_additive_initial_status_without_existing_ledger(self):
        self.csv.unlink()
        subprocess.run([sys.executable, str(self.tmp / "scripts/gen_registry.py")],
                       check=True, capture_output=True)
        _, rows = self.rows()
        self.assertTrue(all(r["status"] == "NOT_STARTED" for r in rows if r["id"].startswith("U-")))

    def test_original_acceptance_and_priority_protected(self):
        for field, value in (("acceptance_criteria", "Any output counts"), ("priority", "P2"), ("milestone", "M14"), ("dependencies", "F-06-01")):
            with self.subTest(field=field):
                f, rows = self.rows()
                old = rows[0][field]
                rows[0][field] = value
                self.write(f, rows, regen=False)
                code, out = self.run_validator()
                self.assertEqual(code, 1)
                self.assertIn("changed from ddc39f6", out)
                rows[0][field] = old
                self.write(f, rows, regen=False)

    def test_missing_additive_row_rejected(self):
        f, rows = self.rows()
        self.write(f, [r for r in rows if r["id"] != "U-01"], regen=False)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("missing=['U-01']", out)

    def test_duplicate_csv_id_rejected(self):
        f, rows = self.rows()
        self.write(f, rows + [rows[-1]], regen=False)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("duplicate ids", out)

    def test_duplicate_spec_id_rejected(self):
        spec = self.tmp / "docs/SPEC_HANDOFF_V2.md"
        spec.write_text(spec.read_text() + "\n• P1 / U-01: Duplicate.\n")
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("duplicate id in spec", out)

    def test_duplicate_design_id_rejected(self):
        data = self.tmp / "scripts/registry_data.py"
        data.write_text(data.read_text() + "\nR.append(R[-1])\n")
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("duplicate id in registry_data", out)

    def test_invalid_spec_priority_rejected(self):
        spec = self.tmp / "docs/SPEC_HANDOFF_V2.md"
        spec.write_text(spec.read_text().replace("• P1 / U-01:", "• P9 / U-01:"))
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("invalid requirement declaration", out)

    def test_unknown_additive_id_rejected(self):
        f, rows = self.rows()
        rows[-1]["id"] = "Z-01"
        self.write(f, rows, regen=False)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("id set differs from spec", out)

    def test_invalid_priority_and_milestone_rejected_without_traceback(self):
        f, rows = self.rows()
        rows[-1].update(priority="urgent", milestone="next")
        self.write(f, rows, regen=False)
        code, out = self.run_validator()
        self.assertEqual(code, 1)
        self.assertIn("invalid priority", out)
        self.assertIn("invalid milestone", out)
        self.assertNotIn("Traceback", out)

    def test_additive_progress_preserved(self):
        f, rows = self.rows()
        rows[-1].update(status="IN_PROGRESS", observed_evidence="design inspected", blocker="test host", last_verified_commit="")
        before = rows[-1].copy()
        self.write(f, rows)
        _, after = self.rows()
        self.assertEqual(before, after[-1])


if __name__ == "__main__":
    unittest.main()
