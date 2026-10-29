#!/usr/bin/env python3
"""Validate docs/REQUIREMENTS.csv against spec section 2 and section 21 rules.

Exit code 0 = valid, 1 = violations (printed). Intended for CI and pre-commit.
"""
from __future__ import annotations

import csv
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
from gen_registry import FIELDS, STATUSES, parse_spec  # noqa: E402

# The only rewording allowed in frozen acceptance criteria: D-028's rename of the product's commands.
D028_NAMES = (("`ucompute hardware --json`", "`common hardware --json`"), ("`ucompute serve`", "`common serve`"),
              ("`ucompute run <model>", "`common run <model>"), ("Two UCompute instances", "Two Common instances"))

CSV_PATH = ROOT / "docs" / "REQUIREMENTS.csv"
VAGUE = re.compile(r"works as intended|works correctly|as expected|TBD|TODO", re.I)
COMMIT = re.compile(r"^[0-9a-f]{7,40}$")


def ms_num(m: str) -> int:
    return int(m[1:])


def main() -> int:
    errs: list[str] = []
    try:
        spec = parse_spec()
    except SystemExit as e:
        print(f"REGISTRY INVALID: {e}")
        return 1
    original_ids = [rid for rid in spec if not rid.startswith("U-")]
    if len(original_ids) != 148:
        errs.append(f"spec parse found {len(original_ids)} original requirements, expected 148")
    with CSV_PATH.open(newline="", encoding="utf-8") as f:
        reader = csv.DictReader(f)
        if reader.fieldnames != FIELDS:
            errs.append(f"CSV header mismatch: {reader.fieldnames}")
        rows = list(reader)
    ids = [r["id"] for r in rows]
    original = [r for r in rows if not r["id"].startswith("U-")]
    if len(original) != 148:
        errs.append(f"CSV has {len(original)} original rows, expected 148")
    # Freeze original scope at ddc39f6 (ADR-0004 decision 4, amended 2026-10-19); ledger progress remains mutable.
    # D-028 renamed the product's commands in six acceptance criteria; only these exact names map back to the frozen text.
    def frozen(k: str, v: str) -> str:
        if k == "acceptance_criteria":
            for new, old in D028_NAMES:
                v = v.replace(new, old)
        return v
    protected = [{k: frozen(k, r[k]) for k in ("id", "priority", "milestone", "dependencies", "description", "acceptance_criteria")} for r in original]
    digest = hashlib.sha256(json.dumps(protected, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    if digest != "c22f992989e5fa50be35af7967cf48d38f3eed943ca1b071c2d3fb01f42259ce":
        errs.append("original IDs, priorities, milestones, dependencies, descriptions or acceptance criteria changed from ddc39f6")
    dupes = {i for i in ids if ids.count(i) > 1}
    if dupes:
        errs.append(f"duplicate ids: {sorted(dupes)}")
    if set(ids) != set(spec):
        errs.append(f"id set differs from spec: missing={sorted(set(spec)-set(ids))} extra={sorted(set(ids)-set(spec))}")
    by_id = {r["id"]: r for r in rows}

    for r in rows:
        rid = r["id"]
        if r["priority"] not in ("P0", "P1", "P2") or r["priority"] != spec.get(rid, ("", "", ""))[2]:
            errs.append(f"{rid}: invalid priority {r['priority']!r} or differs from spec")
        if not re.fullmatch(r"M(?:[0-9]|1[0-4])", r["milestone"]):
            errs.append(f"{rid}: invalid milestone {r['milestone']!r}")
        if r["status"] not in STATUSES:
            errs.append(f"{rid}: invalid status {r['status']!r}")
        if spec.get(rid, ("",))[0] != r["description"]:
            errs.append(f"{rid}: description does not match spec verbatim")
        for fld in ("owner", "milestone", "design", "acceptance_criteria"):
            if not r[fld].strip():
                errs.append(f"{rid}: empty {fld}")
        if VAGUE.search(r["acceptance_criteria"]):
            errs.append(f"{rid}: acceptance criteria contains vague wording")
        if r["status"] == "VERIFIED":
            if not r["observed_evidence"].strip():
                errs.append(f"{rid}: VERIFIED without observed_evidence")
            if not COMMIT.match(r["last_verified_commit"].strip()):
                errs.append(f"{rid}: VERIFIED without a valid last_verified_commit hash")
        if r["status"] == "INFEASIBLE_WITH_EVIDENCE" and not r["observed_evidence"].strip():
            errs.append(f"{rid}: INFEASIBLE_WITH_EVIDENCE without evidence")
        if r["status"] == "BLOCKED" and not r["blocker"].strip():
            errs.append(f"{rid}: BLOCKED without blocker")
        if r["status"] == "IMPLEMENTED_UNVERIFIED" and not r["observed_evidence"].strip():
            errs.append(f"{rid}: IMPLEMENTED_UNVERIFIED without any implementation reference")
        for d in filter(None, r["dependencies"].split(";")):
            if d not in by_id:
                errs.append(f"{rid}: unknown dependency {d!r}")
            elif (re.fullmatch(r"M(?:[0-9]|1[0-4])", by_id[d]["milestone"])
                  and re.fullmatch(r"M(?:[0-9]|1[0-4])", r["milestone"])
                  and ms_num(by_id[d]["milestone"]) > ms_num(r["milestone"])):
                errs.append(f"{rid} ({r['milestone']}): depends on {d} in later milestone {by_id[d]['milestone']}")
            elif r["status"] == "VERIFIED" and by_id[d]["status"] != "VERIFIED":
                errs.append(f"{rid}: VERIFIED but dependency {d} is {by_id[d]['status']}")

    # cycle detection
    graph = {r["id"]: [d for d in r["dependencies"].split(";") if d in by_id] for r in rows}
    state: dict[str, int] = {}

    def visit(n: str, path: list[str]) -> None:
        if state.get(n) == 2:
            return
        if state.get(n) == 1:
            errs.append("dependency cycle: " + " -> ".join(path[path.index(n):] + [n]))
            return
        state[n] = 1
        for m in graph[n]:
            visit(m, path + [n])
        state[n] = 2

    for n in graph:
        visit(n, [])

    gen = subprocess.run([sys.executable, str(ROOT / "scripts" / "gen_registry.py"), "--check"],
                         capture_output=True, text=True)
    if gen.returncode != 0:
        errs.append(gen.stdout.strip() or gen.stderr.strip())

    if errs:
        print(f"REGISTRY INVALID: {len(errs)} problem(s)")
        for e in errs:
            print(" -", e)
        return 1
    counts = {s: sum(r["status"] == s for r in rows) for s in STATUSES}
    print(f"registry valid: 148 P0 rows (original); {len(rows) - 148} added; {len(rows)} total;", ", ".join(f"{k}={v}" for k, v in counts.items()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
