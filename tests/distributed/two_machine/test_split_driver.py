"""Offline checks of split_two_machine.py, the WP-04 P1 driver, against SYNTHETIC fake daemons (fake_mesh.SplitMesh):
canned completions, timings, counters and shell outputs; no machine, daemon, network or inference is involved, so
nothing here is evidence of UCompute. Also checks that its lab-input.json is accepted by the Core Lab's scorer
(ucompute-core-lab p1-predictions/score_p1.py, read only; skipped when that checkout is absent; set UCOMPUTE_CORE_LAB).
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import fake_mesh

HERE = Path(__file__).parent
LAB = Path(os.environ.get("UCOMPUTE_CORE_LAB",
                          "/Users/nasr/repos/ucompute-core-lab/.claude/worktrees/eloquent-williamson-2da66b"))
SCORER = LAB / "p1-predictions" / "score_p1.py"
D = fake_mesh.load("split_two_machine.py")


def lab_module(name):
    """A lab module, imported without writing bytecode into the lab's checkout."""
    sys.dont_write_bytecode = True
    for p in (LAB / "p1-predictions", LAB / "tools"):
        if str(p) not in sys.path:
            sys.path.insert(0, str(p))
    spec = importlib.util.spec_from_file_location(name, LAB / "p1-predictions" / f"{name}.py")
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def score(lab_input_text):
    """The lab's scorer, as P1's runbook runs it (-I; -B keeps its checkout unwritten)."""
    with tempfile.TemporaryDirectory() as d:
        f = Path(d) / "lab-input.json"
        f.write_text(lab_input_text)
        p = subprocess.run([sys.executable, "-I", "-B", str(SCORER), str(f)], capture_output=True, text=True)
    return p.returncode, p.stdout, p.stderr


R_SH = ("--r-sh", "unused: the fake answers R's commands")


def run(*bad, flags=(), mesh=None):
    with tempfile.TemporaryDirectory() as home:
        return fake_mesh.drive("split_two_machine.py", mesh or fake_mesh.SplitMesh(bad), "--h-home", home, *R_SH,
                               *flags)


class SplitDriverSyntheticTests(unittest.TestCase):
    def test_synthetic_run_passes_every_step(self):
        steps, lines, code, files = run()
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertTrue(all(s["ok"] is True for s in steps.values()), lines)
        for k in ("prereg.json", "manifest.json", "steps.json", "lab-input.json", "bytes/checkpoints.json",
                  "bytes/per-token.json", "scan.json", "h/audit.jsonl", "r/audit.jsonl", "tokens/compare-R1a.json"):
            self.assertIn(k, files)
        pre = json.loads(files["prereg.json"])
        self.assertEqual([p["tokens"] for p in pre["prompts"]], [44, 50, 47])
        self.assertTrue(all(p["canary"] in p["text"] for p in pre["prompts"]))
        self.assertEqual({(m["label"], m["split_at"]) for m in pre["models"]}, {("R1a", 12), ("R1b", 14)})

    @unittest.skipUnless(SCORER.exists(), f"the Core Lab checkout is not at {LAB}")
    def test_synthetic_runs_lab_input_is_accepted_by_the_lab_scorer(self):
        _, _, _, files = run()
        lab = json.loads(files["lab-input.json"])
        self.assertEqual(lab["link"], "wifi")
        split = [r for r in lab["runs"] if r["kind"] == "split"]
        self.assertEqual(len(split), 6)
        self.assertEqual(len([r for r in lab["runs"] if r["kind"] == "local"]), 6)
        rc, out, err = score(files["lab-input.json"])
        self.assertEqual(rc, 0, err)
        v = json.loads(out)
        for model in ("R1a", "R1b"):
            self.assertTrue(v[model]["bytes_ok"] and v[model]["turns_ok"] and v[model]["tpot_consistent"]
                            and v[model]["ttft_ok"], v[model])
        _, _, _, wired = run(flags=("--link", "wired"))
        self.assertEqual(json.loads(wired["lab-input.json"])["link"], "wired")
        self.assertEqual(score(wired["lab-input.json"])[0], 0)

    def test_each_broken_behaviour_fails_its_step(self):
        for bad, name in (("dirty", "preflight"), ("busy_r", "preflight"), ("other_user", "preflight"),
                          ("desktop_login", "preflight"), ("r_engine_left", "preflight"), ("low_memory", "preflight"),
                          ("r_tree_modified", "preflight"), ("r_tree_head", "preflight"),
                          ("r_stale_build", "preflight"), ("h_dirty_build", "preflight"), ("r_ip_v6", "r_ip"),
                          ("lan_down", "session_R1a"), ("extra_session", "audit_join"),
                          ("audit_full_page", "audit_join"),
                          ("stage_spawns_on_refusal", "deny_stage_refuses"),
                          ("stage_refusal_untyped", "deny_stage_refuses"), ("rdma_banner", "session_R1a"),
                          ("rpc_outlives_session", "stage_gone_R1a"), ("rpc_outlives_session", "teardown"),
                          ("wrong_placement", "session_R1a"), ("public_rpc", "session_R1a"),
                          ("rpc_reachable", "session_R1a"), ("session_changes", "split_R1a_p1"),
                          ("local_uses_split", "local_stays_local_R1a"), ("leaks_prompt", "canary_scan"),
                          ("no_close_audit", "audit_join")):
            with self.subTest(bad=bad):
                steps, lines, code, files = run(bad)
                self.assertIs(steps[name]["ok"], False, lines)
                self.assertEqual(code, 3)
                if name == "preflight":  # nothing is sent after a failed preflight
                    self.assertEqual(list(steps), ["preflight"])
                    self.assertEqual(json.loads(files["lab-input.json"])["runs"], [])

    def test_the_runners_untracked_evidence_folder_does_not_fail_preflight(self):
        # run_mac_linux.sh creates $OUT under docs/evidence before the driver starts.
        steps, lines, code, _ = run("untracked_out")
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertIs(steps["preflight"]["ok"], True, lines)

    def test_a_canary_in_a_head_log_fails_the_scan(self):
        with tempfile.TemporaryDirectory() as home:
            (Path(home) / "logs").mkdir()
            (Path(home) / "logs" / "engine-x.log").write_text("prompt: ... UCOMPUTE-CANARY-4F2DF3728656CA72 ...")
            steps, _, _, _ = fake_mesh.drive("split_two_machine.py", fake_mesh.SplitMesh(), "--h-home", home, *R_SH)
        self.assertIs(steps["canary_scan"]["ok"], False)
        self.assertEqual(len(steps["canary_scan"]["hits"]["h"]), 1)

    def test_r_sh_is_required(self):
        with tempfile.TemporaryDirectory() as out:  # no default host to fall back to; nothing is run
            p = subprocess.run([sys.executable, "-I", str(HERE / "split_two_machine.py"), "--a", "http://a", "--b",
                                "http://b", "--out", out], capture_output=True, text=True)
        self.assertEqual(p.returncode, 2)
        self.assertIn("--r-sh", p.stderr)

    def test_a_full_audit_page_that_reaches_back_past_the_run_passes(self):
        mesh = fake_mesh.SplitMesh()
        for d in (mesh.H, mesh.R):  # 1200 events from before the run: the page of 1000 is full
            d.audit += [{"ts_utc": "2026-01-01T00:00:00Z", "kind": "network_changed", "fields": {}}] * 1200
        steps, lines, code, files = run(mesh=mesh)
        self.assertIs(steps["audit_join"]["ok"], True, lines)
        self.assertEqual(steps["audit_join"]["whole_run"], {"h": True, "r": True})
        self.assertEqual(len(files["h/audit.jsonl"].splitlines()), 1000)
        self.assertEqual(steps["audit_join"]["sessions"], steps["audit_join"]["driver_sessions"])

    def test_parsers(self):
        ss = ('tcp LISTEN 0 1024 127.0.0.1:8192 0.0.0.0:* users:(("ucompute",pid=1,fd=9))\n'
              'udp UNCONN 0 0 0.0.0.0:8193 0.0.0.0:* users:(("ucompute",pid=1,fd=10))\n'
              "tcp LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\n"
              'tcp LISTEN 0 1 127.0.0.1:40001 0.0.0.0:* users:(("ggml-rpc-server",pid=2,fd=3))\n')
        self.assertEqual(D.listeners_bad(ss), ["tcp 127.0.0.1:40001 ggml-rpc-server"])
        self.assertEqual(D.listeners_bad(ss, rpc_allowed=True), [])
        self.assertEqual(D.rpc_ports(ss), [40001])
        self.assertEqual(D.listeners_bad('tcp LISTEN 0 1 0.0.0.0:8192 0.0.0.0:* users:(("ucompute",pid=1,fd=9))'),
                         ["tcp 0.0.0.0:8192 ucompute"])
        self.assertEqual(D.listeners_bad('tcp LISTEN 0 1 0.0.0.0:41 0.0.0.0:* users:(("ggml-rpc-server",pid=2,fd=3))',
                                         rpc_allowed=True), ["tcp 0.0.0.0:41 ggml-rpc-server"])
        self.assertEqual(D.who_others("me pts/0 x (10.0.0.1)\nssh_client=10.0.0.1\n"), [])
        self.assertEqual(D.who_others("ssh_client=10.0.0.1\n"), [])
        self.assertEqual(D.who_others("you tty1 x\nssh_client=\n"), ["you tty1 x"])
        # OD-10: loginctl show-session blocks as the driver's "sessions" command prints them (SYNTHETIC text in
        # loginctl's key=value format; not captured on sentinel): a GNOME Wayland login on seat0 that `who` does not
        # list, the user manager, a closing session, the run's own command and tunnel sessions, and a second SSH login
        loginctl = ("Id=2\nName=div0rce\nClass=user\nType=wayland\nState=active\nRemote=no\nRemoteHost=\n"
                    "Service=gdm-password\nSeat=seat0\n\n"
                    "Id=3\nName=div0rce\nClass=manager\nType=unspecified\nState=active\nRemote=no\nRemoteHost=\n"
                    "Service=systemd-user\nSeat=\n\n"
                    "Id=9\nName=div0rce\nClass=user\nType=tty\nState=closing\nRemote=yes\nRemoteHost=10.0.0.5\n"
                    "Service=sshd\nSeat=\n\n"
                    "Id=41\nName=div0rce\nClass=user\nType=tty\nState=active\nRemote=yes\nRemoteHost=10.0.0.1\n"
                    "Service=sshd\nSeat=\n\n"
                    "Id=44\nName=div0rce\nClass=user\nType=tty\nState=active\nRemote=yes\nRemoteHost=10.0.0.1\n"
                    "Service=sshd\nSeat=\n\n"
                    "Id=45\nName=div0rce\nClass=user\nType=tty\nState=online\nRemote=yes\nRemoteHost=10.0.0.7\n"
                    "Service=sshd\nSeat=\n\n"
                    "self=44\nuser=div0rce\nssh_client=10.0.0.1\n")
        self.assertEqual(D.sessions_others(loginctl), [
            "Id=2 Name=div0rce Class=user Type=wayland State=active Seat=seat0 RemoteHost=",
            "Id=45 Name=div0rce Class=user Type=tty State=online Seat= RemoteHost=10.0.0.7"])
        self.assertEqual(D.sessions_others("self=44\nuser=div0rce\nssh_client=10.0.0.1\n"), [])
        # without the run's client address only the run's own session is its own
        self.assertEqual(len(D.sessions_others(loginctl.replace("ssh_client=10.0.0.1", "ssh_client="))), 3)
        # R's tree against HEAD: changed, missing and extra paths; a symlink in HEAD is not a regular file
        head = ("100644 blob aaa\tCargo.toml\n100755 blob bbb\tscripts/x.sh\n100644 blob ccc\tgone.rs\n"
                "120000 blob ddd\tlink\n")
        self.assertEqual(D.tree_diff(head, "Cargo.toml\taaa\nscripts/x.sh\tbbb\ngone.rs\tccc\n"), [])
        self.assertEqual(D.tree_diff(head, "Cargo.toml\taaX\nscripts/x.sh\tbbb\nnew.rs\teee\n"),
                         ["Cargo.toml", "gone.rs", "new.rs"])
        self.assertEqual(D.free_bytes("(page size of 4096 bytes)\nPages free: 10.\nPages active: 99.\n"
                                      "Pages inactive: 5.\nPages speculative: 1.\n"), 16 * 4096)
        # tpot = ([DONE] - ttft) / (completion_tokens - 1), both client-side
        self.assertEqual(D.timing({"ttft_ms": 200.0, "done_ms": 2504.0, "usage": {"completion_tokens": 65}}),
                         (200.0, 36.0))
        self.assertEqual(D.timing({"ttft_ms": 200.0, "done_ms": None, "usage": {"completion_tokens": 65}}),
                         (None, None))

    @unittest.skipUnless(SCORER.exists(), f"the Core Lab checkout is not at {LAB}")
    def test_lab_helper_input_through_this_drivers_writer_is_accepted_and_scored_alike(self):
        """SYNTHETIC counters from the lab's own test helpers (test_score_p1.counters/run), written by this driver's
        lab_input(): the lab's scorer accepts them, and this driver's per-token method equals the lab's."""
        t, s = lab_module("test_score_p1"), lab_module("score_p1")
        runs = [t.run("R1a", p, n, 16921, 3592, 1.0, 36.0, 230.0) for p, n in ((1, 44), (2, 50), (3, 47))]
        runs += [t.run("R1b", 1, 44, 25058, 6152, 1.0, 54.0, 350.0)]
        for r in runs:
            del r["ids"]  # UCompute's /v1/completions returns no token ids
            self.assertEqual(D.per_token(r["counters"]), s.per_token(r["counters"]))
        rc, out, err = score(json.dumps(D.lab_input(runs, "wifi")))
        self.assertEqual(rc, 0, err)
        v = json.loads(out)
        self.assertTrue(v["R1a"]["bytes_ok"] and v["R1a"]["turns_ok"] and v["R1b"]["bytes_ok"], v)
        # the shortcut (after_n65 - after_n1) / 64 would also count a prefill; the decode-only method does not
        c = t.counters(14150, 3592, 1.0)
        self.assertEqual(D.per_token(c)["to_r"], 14150)


class RShellFailureTests(unittest.TestCase):
    """privacy:F1: the driver's own shell() with an R prefix that exits 255 with empty stdout, as ssh does when it
    cannot reach R. Each R check FAILs; none reads the empty output as a clean result. Only this Mac's read-only
    commands (git, pgrep, pmset, vm_stat, ps, sysctl) run for real; no daemon is called."""

    def test_r_side_ssh_failure_fails_every_r_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            ssh = Path(tmp) / "ssh"
            ssh.write_text("#!/bin/sh\nexit 255\n")
            ssh.chmod(0o755)
            d = fake_mesh.load("split_two_machine.py")
            d.ARGS = argparse.Namespace(r_sh=str(ssh), out=tmp, h_home=tmp, h_log=None, r_home="$HOME/.x",
                                        r_log="$HOME/.x.log", r_tree="$HOME/src/ucompute", r_ip="10.0.0.2")
            d.RAW = open(Path(tmp) / "raw.jsonl", "w")
            d.call = lambda *a, **k: (200, {"commit": "x"}, 1.0)
            d.RUN_START = d.utc()
            prereg = json.loads((HERE / "split_prereg.json").read_text())
            try:
                d.preflight(prereg, "http://a", "http://b")
                d.canary_scan(prereg, {})
                d.stage_gone("http://a", "R1a")
                d.teardown("http://a")
            finally:
                d.RAW.close()
            raw = [json.loads(x) for x in (Path(tmp) / "raw.jsonl").read_text().splitlines()]
        steps = {r["step"]: r for r in d.RESULTS}
        for name in ("preflight", "canary_scan", "stage_gone_R1a", "teardown"):
            self.assertIs(steps[name]["ok"], False, steps[name])
            self.assertIn("exited 255", steps[name]["r_shell_error"])
        self.assertTrue(any(r.get("shell") == "r" and r["rc"] == 255 and r["out"] == "" for r in raw))


if __name__ == "__main__":
    unittest.main()
