"""Offline checks of split_two_machine.py, the WP-04 P1 driver, against SYNTHETIC fake daemons (fake_mesh.SplitMesh):
canned completions, timings, counters and shell outputs; no machine, daemon, network or inference is involved, so
nothing here is evidence of UCompute. The steps beyond the core run are checked the same way: footprint and VmHWM values
worked out from a SYNTHETIC GGUF, failures on a fake clock, and the driver's real Flight and Relay classes only against
loopback stubs of this file. Also checks that its lab-input.json is accepted by the Core Lab's scorer
(ucompute-core-lab p1-predictions/score_p1.py, read only; skipped when that checkout is absent; set UCOMPUTE_CORE_LAB).
Run: python3 -m unittest discover -s tests/distributed/two_machine -v
"""
import argparse
import http.server
import importlib.util
import json
import os
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading
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
        # the display manager's login screen (as on sentinel after its 2026-10-30 reboot: gdm's greeter on seat0 and
        # its early manager) is no login; a user's desktop on the same seat still is
        greeter = ("Id=c1\nName=gdm-greeter\nClass=greeter\nType=wayland\nState=active\nRemote=no\nRemoteHost=\n"
                   "Service=gdm-launch-environment\nSeat=seat0\n\n"
                   "Id=2\nName=gdm-greeter\nClass=manager-early\nType=unspecified\nState=active\nRemote=no\n"
                   "RemoteHost=\nService=systemd-user\nSeat=\n\n")
        self.assertEqual(D.sessions_others(greeter + "self=44\nuser=div0rce\nssh_client=10.0.0.1\n"), [])
        self.assertEqual(len(D.sessions_others(greeter + loginctl)), 2)
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


RESTARTS = ("--r-start", "fake r start", "--h-start", "fake h start", "--h-kill", "fake h kill")
CORE_STEPS = ["preflight", "deny_unpaired", "pair", "deny_remote_only", "deny_stage_refuses", "deny_unacknowledged"]
for label in ("R1a", "R1b"):
    CORE_STEPS += [f"session_{label}"] + [f"split_{label}_p{i}" for i in (1, 2, 3)]
    CORE_STEPS += [f"local_stays_local_{label}", f"local_{label}", f"stage_gone_{label}"]
CORE_STEPS += ["audit_join", "canary_scan", "teardown"]
CASES = ["1_client_cancel", "2_kill_rpc_server", "3_revoke_on_r", "4_hold_on_h", "5_pooling_off_on_r",
         "6_local_only_on_h", "7_networking_off_on_h", "8_sigkill_r_daemon", "9_sigkill_h_daemon"]


def extra(steps, *bad, flags=(), mesh=None):
    """A run of `steps` against the SYNTHETIC mesh, with the SYNTHETIC GGUFs in its head home."""
    mesh = mesh or fake_mesh.SplitMesh(bad)
    with tempfile.TemporaryDirectory() as home:
        mesh.install(home)
        return fake_mesh.drive("split_two_machine.py", mesh, "--h-home", home, *R_SH, "--steps", steps, *RESTARTS,
                               *flags)


class ExtraStepsSyntheticTests(unittest.TestCase):
    def test_the_default_run_is_the_core_run_and_nothing_else(self):
        steps, lines, code, files = run()
        self.assertEqual(list(steps), CORE_STEPS)
        self.assertFalse([f for f in files if f.startswith(("capture/", "failure/")) or "extra" in f or "memory" in f])

    def test_every_step_passes_and_the_core_comes_first_whatever_the_order_asked(self):
        steps, lines, code, files = extra("failures,config_c,capture,memory,core")
        self.assertEqual(code, 0, "\n".join(lines))
        names = list(steps)
        self.assertEqual(names[:len(CORE_STEPS)], CORE_STEPS)
        ran = names[len(CORE_STEPS):]
        self.assertLess(ran.index("memory_R1b_r"), ran.index("capture"))
        self.assertLess(ran.index("capture"), ran.index("config_c_session"))
        self.assertLess(ran.index("config_c_local_mac_cpu"), ran.index("failure_1_client_cancel"))
        self.assertEqual(ran[-11:], [f"failure_{c}" for c in CASES] + ["canary_scan_extra", "teardown_extra"])
        for name, s in steps.items():
            self.assertIs(s["ok"], None if name in ("config_c_logprobs", "config_c_compare") else True, s)
        for f in ("memory-R1a.json", "h/footprint-split-R1a.txt", "h/footprint-local-R1a.txt",
                  "r/proc-status-rpc-R1a.txt", "r/proc-status-local-R1a.txt", "capture/relay-summary.json",
                  "capture/scan.json", "scan-extra.json", "capture/datagrams.bin", "capture/control.bin",
                  "tokens/C-split-R1b-1.json", "tokens/local-mac-cpu-R1b-3.json",
                  "tokens/local-sentinel-cpu-R1b-2.json", "tokens/compare-C-R1b.json", "bytes/config-c.json",
                  "failure/client_cancel.json", "failure/sigkill_h_daemon.json"):
            self.assertIn(f, files)
        self.assertEqual({r["kind"] for r in json.loads(files["lab-input.json"])["runs"]}, {"split", "local"})
        self.assertEqual(len(json.loads(files["lab-input.json"])["runs"]), 12, "the extras never reach the lab's input")

    def test_steps_without_core_pair_and_set_the_groups_but_run_no_core_step(self):
        steps, lines, code, _ = extra("memory")
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertEqual(list(steps), ["preflight", "pair", "memory_R1a_h", "memory_R1a_r", "memory_R1b_h",
                                       "memory_R1b_r", "canary_scan_extra", "teardown_extra"])

    def test_steps_are_checked_before_anything_runs(self):
        for flags in (("--steps", "core,nonsense"), ("--steps", "failures")):  # failures needs the restart commands
            with tempfile.TemporaryDirectory() as out, self.subTest(flags=flags):
                p = subprocess.run([sys.executable, "-I", str(HERE / "split_two_machine.py"), "--a", "http://a", "--b",
                                    "http://b", "--out", out, *R_SH, *flags], capture_output=True, text=True)
                self.assertEqual(p.returncode, 2, p.stderr)
                self.assertFalse(os.listdir(out))

    def test_memory_compares_each_host_with_its_full_model_peak(self):
        steps, lines, _, files = extra("memory")
        m = fake_mesh.SplitMesh
        h, r = steps["memory_R1a_h"], steps["memory_R1a_r"]
        self.assertEqual(h["absent_bytes"], 12 * m.BLOCK)
        self.assertEqual(r["absent_bytes"], 12 * m.BLOCK + m.INPUT + m.OUTPUT)
        self.assertEqual(steps["memory_R1b_h"]["absent_bytes"], 14 * m.BLOCK)
        self.assertEqual(h["saved_over_absent"], 1.0)
        self.assertIn("phys_footprint_peak", files["h/footprint-split-R1a.txt"])
        self.assertIn("VmHWM", files["r/proc-status-local-R1a.txt"])
        record = json.loads(files["memory-R1a.json"])
        self.assertEqual(record["layer_bytes"]["blocks"], [m.BLOCK] * 24)
        self.assertEqual((record["layer_bytes"]["input"], record["layer_bytes"]["output"]), (m.INPUT, m.OUTPUT))

    def test_memory_without_the_model_on_r_is_na_not_a_pass(self):
        steps, lines, code, _ = extra("memory", "r_no_model")
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertIs(steps["memory_R1a_h"]["ok"], True)
        self.assertIs(steps["memory_R1a_r"]["ok"], None)
        self.assertIn("OD-4", steps["memory_R1a_r"]["na"])

    def test_capture_dials_through_the_relay_and_puts_the_dial_back(self):
        mesh = fake_mesh.SplitMesh()
        steps, lines, code, files = extra("capture", mesh=mesh)
        self.assertEqual(code, 0, "\n".join(lines))
        summary = json.loads(files["capture/relay-summary.json"])
        self.assertTrue(summary["via"].startswith("127.0.0.1:"))
        self.assertTrue(summary["raw_kept"])
        self.assertEqual(summary["dial_restored_to"], "10.0.0.2:8193")
        self.assertEqual(mesh.H.pins[mesh.R.id]["last_addr"], "10.0.0.2:8193")
        self.assertEqual(mesh.conn(mesh.H, mesh.R)["addr"], "10.0.0.2:8193")
        scan = json.loads(files["capture/scan.json"])
        self.assertEqual(scan["hits"], {})
        self.assertEqual(len(scan["needles"]), 7, "three canaries in two encodings and the weight pattern")
        self.assertEqual(len(scan["control"]["hits"]), 7, "and the positive control found each of them")
        self.assertTrue(any(k.startswith("weight blk.5.") for k in scan["needles"]), "the middle of R's range")

    def test_a_redial_that_wins_the_race_is_retried(self):
        steps, lines, _, _ = extra("capture", "redial_race")
        self.assertIs(steps["capture"]["ok"], True, lines)

    def test_a_large_capture_keeps_its_hash_and_not_its_bytes(self):
        d = fake_mesh.load("split_two_machine.py")  # the limit is the module's; the fake's traffic is small
        self.assertEqual(d.CAPTURE_KEEP, 32 << 20)
        mesh = fake_mesh.SplitMesh()
        real = mesh.patch

        def small(driver):
            real(driver)
            driver.CAPTURE_KEEP = 0
        mesh.patch = small
        steps, lines, _, files = extra("capture", mesh=mesh)
        self.assertNotIn("capture/datagrams.bin", files)
        summary = json.loads(files["capture/relay-summary.json"])
        self.assertFalse(summary["raw_kept"])
        self.assertEqual(len(summary["sha256"]), 64)
        self.assertIs(steps["capture"]["ok"], True, lines)

    def test_configuration_c_asks_for_same_backend_and_reports_tokens_as_measured(self):
        steps, lines, code, files = extra("config_c")
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertEqual(steps["config_c_session"]["placement"], [["CPU", 0, 14], ["RPC0", 14, 28]])
        self.assertEqual(steps["config_c_local_mac_cpu"]["device_requested"], "cpu")
        self.assertIn("logprobs", steps["config_c_logprobs"]["na"])
        self.assertEqual(len(json.loads(files["tokens/compare-C-R1b.json"])), 9)

    def test_configuration_c_runs_without_the_model_on_r(self):
        steps, lines, code, files = extra("config_c", "r_no_model")
        self.assertEqual(code, 0, "\n".join(lines))
        self.assertIs(steps["config_c_local_sentinel_cpu"]["ok"], None)
        self.assertEqual(len(json.loads(files["tokens/compare-C-R1b.json"])), 3)

    def test_the_killed_daemons_close_line_is_not_asked_for_and_the_others_are(self):
        steps, lines, _, files = extra("failures")
        self.assertEqual(set(steps["failure_2_kill_rpc_server"]["session_close"]), {"h", "r"})
        self.assertEqual(set(steps["failure_8_sigkill_r_daemon"]["session_close"]), {"h"})
        self.assertEqual(set(steps["failure_9_sigkill_h_daemon"]["session_close"]), {"r"})
        self.assertEqual(steps["failure_9_sigkill_h_daemon"]["code"], "transport", "no daemon is left to type it")
        self.assertEqual(json.loads(files["failure/sigkill_r_daemon.json"])["rpc_gone_after_ms"], 120)
        case1 = json.loads(files["failure/client_cancel.json"])
        self.assertEqual(case1["next_request"]["session"], case1["session"])
        self.assertTrue(case1["checks"]["same_session"])

    def test_each_case_gets_a_fresh_session(self):
        steps, lines, _, files = extra("failures")
        ids = [json.loads(files[f"failure/{c[2:]}.json"])["session"] for c in CASES]
        self.assertEqual(len(set(ids)), 9)

    def test_each_broken_behaviour_fails_its_step(self):
        case = {c[0]: f"failure_{c}" for c in CASES}
        for run_steps, bad, name, reason in (
                ("memory", "head_keeps_all_layers", "memory_R1a_h", None),
                ("memory", "stage_holds_all", "memory_R1a_r", None),
                ("memory", "no_footprint", "memory_R1a_h", None),
                ("memory", "ctx_differs", "memory_R1a_h", None),
                ("memory", "ssh_drops", "memory_stopped", None),
                ("capture", "bypass_relay", "capture", "dialled_through_relay"),
                ("capture", "relay_invisible", "capture", "r_saw_relay"),
                ("capture", "relay_short", "capture", "relay_carried_tunnel"),
                ("capture", "wire_canary", "capture", "no_hits"),
                ("capture", "wire_weights", "capture", "no_hits"),
                ("capture", "blind_scanner", "capture_control", None),
                ("capture", "blind_scanner", "capture", "control"),
                ("config_c", "c_wrong_placement", "config_c_session", None),
                ("config_c", "c_stage_gpu", "config_c_session", None),
                ("config_c", "cpu_ignored", "config_c_local_mac_cpu", None),
                ("failures", "late_backend_failed", case["2"], "typed_in_time"),
                ("failures", "hang_backend_failed", case["2"], "ended_in_time"),
                ("failures", "untyped_backend_failed", case["2"], "typed_in_time"),
                ("failures", "late_peer_revoked", case["3"], "ended_in_time"),
                ("failures", "slow_peer_revoked", case["3"], "typed_in_time"),
                ("failures", "slow_peer_held", case["4"], "typed_in_time"),
                ("failures", "slow_peer_unauthorized", case["5"], "typed_in_time"),
                ("failures", "slow_policy_denied", case["6"], "typed_in_time"),
                ("failures", "slow_peer_lost", case["7"], "typed_in_time"),
                ("failures", "untyped_peer_lost", case["8"], "typed_in_time"),
                ("failures", "rpc_outlives_session", case["2"], "rpc_gone"),
                ("failures", "no_stage_close", case["2"], "session_close_with_outcome"),
                ("failures", "no_head_close", case["3"], "session_close_with_outcome"),
                ("failures", "job_stuck", case["1"], "no_job_left"),
                ("failures", "unload_refused", case["2"], "fresh_session"),
                ("failures", "cancel_ends_session", case["1"], "same_session"),
                ("failures", "pdeathsig_slow", case["8"], "pdeathsig_within_1s"),
                ("failures", "r_teardown_slow", case["9"], "r_teardown_within_5s"),
                ("failures", "no_reap", case["9"], "engine_reaped")):
            with self.subTest(bad=bad, step=name):
                steps, lines, code, _ = extra(run_steps, bad)
                self.assertIs(steps[name]["ok"], False, lines)
                self.assertEqual(code, 3)
                if reason:
                    self.assertIn(reason, steps[name]["failed"])

    def test_seven_seconds_passes_a_ten_second_case_and_fails_a_five_second_one(self):
        steps, lines, _, _ = extra("failures", "slow_backend_failed", "slow_peer_held")
        self.assertIs(steps["failure_2_kill_rpc_server"]["ok"], True, lines)
        self.assertEqual(steps["failure_2_kill_rpc_server"]["error_after_s"], 7.0)
        self.assertIs(steps["failure_4_hold_on_h"]["ok"], False)

    def test_a_case_whose_hosts_do_not_come_back_stops_the_rest(self):
        steps, lines, code, _ = extra("failures", "repair_fails")
        self.assertEqual(code, 3)
        self.assertIn("recovered", steps["failure_3_revoke_on_r"]["failed"])
        for c in CASES[3:]:
            self.assertIs(steps[f"failure_{c}"]["ok"], None, c)
        self.assertIn("canary_scan_extra", steps)

    def test_no_generation_in_flight_fails_the_case_and_the_next_one_runs(self):
        mesh, flights = fake_mesh.SplitMesh(), []

        def refused_once(base, body, timeout=900):
            flights.append(body)
            target = "split:" + "0" * 64 if len(flights) == 1 else body["target"]  # not paired: refused at once
            return fake_mesh.FakeFlight(mesh, base, {**body, "target": target})
        mesh.Flight = refused_once
        steps, lines, _, _ = extra("failures", mesh=mesh)
        self.assertIs(steps["failure_1_client_cancel"]["ok"], False)
        self.assertEqual(steps["failure_1_client_cancel"]["why"], "no generation in flight")
        self.assertIs(steps["failure_2_kill_rpc_server"]["ok"], True, lines)


class ExtraStepsPureTests(unittest.TestCase):
    def test_memory_rule_at_its_bound(self):
        self.assertTrue(D.memory_verdict(1000, 150, 1000)["met"])  # saved 850 of 1000: exactly -15 %
        self.assertFalse(D.memory_verdict(1000, 151, 1000)["met"])
        self.assertTrue(D.memory_verdict(3000, 100, 1000)["met"], "more saved than absent is still at least")
        self.assertFalse(D.memory_verdict(1000, 1200, 1000)["met"], "a split above the full model")
        for full, split in ((None, 1), (1, None)):
            self.assertFalse(D.memory_verdict(full, split, 1000)["met"], "no reading is no pass")

    def test_footprint_and_status_parsers(self):
        # the layout of `footprint -p` as the lab's R0 recorded it on this Mac's macOS (SYNTHETIC values)
        rule = "    ---        ---          ---        ---    ---\n"
        text = (f"  Dirty      Clean  Reclaimable    Regions    Category\n{rule}"
                f"    0 B      27 MB          0 B          8    mapped file\n{rule}"
                "  45 MB    2848 KB          0 B       3348    TOTAL\n\nAuxiliary data:\n    phys_footprint: 45 MB\n"
                "    phys_footprint_peak: 1.5 GB\n\n")
        self.assertEqual(D.footprint_peak(text), 3 << 29)
        self.assertEqual(D.footprint_totals(text), {"dirty": 45 << 20, "clean": 2848 << 10})
        self.assertIsNone(D.footprint_peak("phys_footprint: 45 MB\n"))
        self.assertEqual(D.footprint_totals("nothing"), {})
        self.assertEqual(D.vm_hwm("VmPeak:\t 99 kB\nVmHWM:\t  2048 kB\nVmRSS:\t 7 kB\n"), 2048 << 10)
        self.assertIsNone(D.vm_hwm("VmPeak:\t 99 kB\n"))

    def test_gguf_reader_against_a_synthetic_file(self):
        tensors = [("token_embd.weight", 128), ("blk.0.a.weight", 64), ("blk.0.b.weight", 192), ("blk.1.a.weight", 64),
                   ("blk.2.a.weight", 64), ("output.weight", 256)]
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "m.gguf"
            fake_mesh.synthetic_gguf(f, tensors, 1, align=64)
            sizes, offsets, start = D.gguf_sizes(f)
            self.assertEqual(sizes, dict(tensors))
            self.assertEqual(start % 64, 0)
            self.assertEqual(start + sum(sizes.values()), f.stat().st_size)
            self.assertEqual(D.layer_bytes(sizes, 2), {"blocks": [256, 64], "input": 128 + 64, "output": 256},
                             "a block past n_layer reads as input, as gguf.rs")
            name, at, pattern = D.weight_pattern(f, 0)
            self.assertEqual(name, "blk.0.b.weight")
            self.assertEqual(at, start + offsets[name] + 96)
            self.assertEqual(len(pattern), 64)
            self.assertEqual(f.read_bytes().count(pattern), 1)
            f.write_bytes(b"GGUX" + f.read_bytes()[4:])
            with self.assertRaises(ValueError):
                D.gguf_sizes(f)

    def test_any_unsigned_alignment_is_read_and_32_is_the_default(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "m.gguf"
            fake_mesh.synthetic_gguf(f, [("a", 256)], 1, align=256, align_type=(10, "<Q"))  # gguf.rs: any unsigned type
            self.assertEqual(D.gguf_sizes(f)[2] % 256, 0)
            self.assertEqual(D.gguf_sizes(f)[0], {"a": 256})
            fake_mesh.synthetic_gguf(f, [("a", 64)], 1, align=32, align_type=(5, "<i"))  # signed: not read, as 32 anyway
            self.assertEqual(D.gguf_sizes(f)[2] % 32, 0)

    def test_tensors_out_of_file_order_get_the_gap_to_the_next_offset(self):
        with tempfile.TemporaryDirectory() as d:
            f = Path(d) / "m.gguf"
            fake_mesh.synthetic_gguf(f, [("a", 64), ("b", 128)], 1, align=64)
            real = D.gguf_tensors
            try:
                D.gguf_tensors = lambda p: ([("b", 64), ("a", 0)], real(p)[1])  # the table lists them out of order
                self.assertEqual(D.gguf_sizes(f)[0], {"a": 64, "b": 128})
            finally:
                D.gguf_tensors = real

    def test_failure_bounds_are_inclusive_and_need_a_typed_code(self):
        def flight(code, error_at, ended_at):
            return argparse.Namespace(error=code and {"code": code}, error_at=error_at, ended_at=ended_at)
        t0 = 100.0
        self.assertTrue(all(D.failure_checks(True, flight("peer_lost", 105.0, 105.0), t0, 5, True).values()))
        late = D.failure_checks(True, flight("peer_lost", 105.1, 105.1), t0, 5, True)
        self.assertEqual(late, {"ended_in_time": False, "typed_in_time": False})
        self.assertFalse(D.failure_checks(True, flight("peer_lost", 101.0, 105.1), t0, 5, True)["ended_in_time"],
                         "a typed error then a stream left open past the bound")
        for code in (None, "transport", "unparsed"):
            self.assertFalse(D.failure_checks(True, flight(code, 101.0, 101.0), t0, 5, True)["typed_in_time"], code)
        self.assertEqual(D.failure_checks(True, flight(None, None, 101.0), t0, 10, False), {"ended_in_time": True})
        self.assertFalse(D.failure_checks(False, flight("peer_lost", 101.0, 101.0), t0, 5, True)["ended_in_time"],
                         "the wait ran out: an end time from the driver's own cancel does not count")

    def test_prefix(self):
        self.assertEqual((D.prefix("abcd", "abxd"), D.prefix("abc", "abcd"), D.prefix("", "x")), (2, 3, 0))


class RelaySyntheticTests(unittest.TestCase):
    """The driver's real Relay and scanner on loopback, between two sockets of this test: SYNTHETIC bytes, no daemon."""

    def relay_pair(self, d, tmp):
        server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        server.bind(("127.0.0.1", 0))
        server.settimeout(5)
        self.addCleanup(server.close)
        client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        client.bind(("127.0.0.1", 0))
        client.settimeout(5)
        self.addCleanup(client.close)
        relay = d.Relay(server.getsockname(), Path(tmp) / "c.bin")
        return client, server, relay

    def test_it_forwards_both_ways_from_one_back_socket_and_records_each_datagram(self):
        d = D
        with tempfile.TemporaryDirectory() as tmp:
            client, server, relay = self.relay_pair(d, tmp)
            client.sendto(b"to server", ("127.0.0.1", relay.port))
            data, seen = server.recvfrom(100)
            self.assertEqual((data, seen[1]), (b"to server", relay.back_port))
            server.sendto(b"to client", seen)
            self.assertEqual(client.recvfrom(100)[0], b"to client")
            other = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.addCleanup(other.close)
            other.sendto(b"stray", ("127.0.0.1", relay.back_port))  # from nobody the relay talks to: not forwarded
            client.sendto(b"end", ("127.0.0.1", relay.port))
            server.recvfrom(100)
            relay.stop()
            self.assertEqual((relay.datagrams, relay.bytes), ([2, 1], [12, 9]))
            scan = d.scan_capture(Path(tmp) / "c.bin", {"s": b"to server", "c": b"to client", "x": b"stray"})
            self.assertEqual(scan, {"datagrams": 3, "hits": {"s": 1, "c": 1}})

    def test_the_scanner_finds_a_needle_split_between_two_datagrams_of_one_direction_only(self):
        d = D
        with tempfile.TemporaryDirectory() as tmp:
            f = Path(tmp) / "c.bin"
            with open(f, "wb") as out:
                for direction, data in ((0, b"xxNEE"), (1, b"zzzzz"), (0, b"DLEyy"), (1, b"NEE"), (0, b"qq"),
                                        (1, b"qqDLE")):
                    out.write(d.Relay.HEAD.pack(direction, len(data)) + data)
            self.assertEqual(d.scan_capture(f, {"n": b"NEEDLE"}), {"datagrams": 6, "hits": {"n": 1}})

    def test_the_control_finds_a_pattern_across_a_datagram_boundary_and_not_what_is_not_there(self):
        d = D
        with tempfile.TemporaryDirectory() as tmp:
            pattern = bytes(range(64))
            stream = b"\xff" * 1170 + pattern + b"\xfe" * 2000  # bytes 1170..1233: across the 1200-byte datagrams
            scan = d.scanner_control(Path(tmp) / "c.bin", stream, {"in": pattern, "out": bytes(range(64, 128))})
            self.assertEqual(scan, {"datagrams": 3, "hits": {"in": 1}})


class FlightStub(http.server.BaseHTTPRequestHandler):
    """A SYNTHETIC loopback stub of H's /v1/completions, one reply per test. Chunked, like the daemon's SSE."""
    protocol_version = "HTTP/1.1"
    reply = []  # lines of the stream; "wait" holds the stream open, "drop" ends the socket without a final chunk

    def log_message(self, *a):
        pass

    def do_POST(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        if self.reply and isinstance(self.reply[0], int):
            body = json.dumps({"error": {"code": "policy_denied", "message": "no"}}).encode()
            self.send_response(self.reply[0])
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        self.send_response(200)
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()
        for item in self.reply:
            if item == "wait":
                self.connection.settimeout(10)
                self.server.gone.set() if self.connection.recv(1) == b"" else None
                return
            if item == "drop":
                self.close_connection = True
                return
            line = f"data: {item}\n\n".encode()
            self.wfile.write(b"%x\r\n%s\r\n" % (len(line), line))
            self.wfile.flush()
        self.wfile.write(b"0\r\n\r\n")


class FlightSyntheticTests(unittest.TestCase):
    """The driver's real Flight (a thread and a socket) against FlightStub: no daemon, no generation."""
    TOKEN = json.dumps({"choices": [{"text": "a"}]})

    def fly(self, reply):
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), type("Stub", (FlightStub,), {"reply": reply}))
        server.gone = threading.Event()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        return D.Flight(f"http://127.0.0.1:{server.server_port}", {"model": "x"}), server

    def test_a_stream_to_done(self):
        fl, _ = self.fly([self.TOKEN, json.dumps({"choices": [{"text": "", "finish_reason": "length"}]}), "[DONE]"])
        self.assertTrue(fl.wait_first(5))
        self.assertTrue(fl.wait_done(5))
        self.assertEqual((fl.tokens, fl.finish, fl.error, fl.status), (1, "length", None, 200))
        self.assertLessEqual(fl.first_at, fl.ended_at)

    def test_a_typed_error_mid_stream(self):
        fl, _ = self.fly([self.TOKEN, json.dumps({"error": {"code": "peer_lost", "message": "gone"}}), "[DONE]"])
        self.assertTrue(fl.wait_done(5))
        self.assertEqual(fl.error["code"], "peer_lost")
        self.assertIsNotNone(fl.error_at)

    def test_a_refusal_before_the_stream(self):
        fl, _ = self.fly([403])
        self.assertTrue(fl.wait_done(5))
        self.assertFalse(fl.wait_first(0))
        self.assertEqual((fl.status, fl.error["code"]), (403, "policy_denied"))

    def test_a_stream_that_stops_without_done_ends_in_transport(self):
        fl, _ = self.fly([self.TOKEN, "drop"])
        self.assertTrue(fl.wait_done(5))
        self.assertEqual(fl.error["code"], "transport")

    def test_cancel_ends_the_socket_as_a_client_going_away_does(self):
        fl, server = self.fly([self.TOKEN, "wait"])
        self.assertTrue(fl.wait_first(5))
        self.assertFalse(fl.wait_done(0))
        fl.cancel()
        self.assertTrue(fl.wait_done(5))
        self.assertIsNone(fl.error, "the client's own cancel is no failure")
        self.assertTrue(server.gone.wait(5), "the server saw the client leave")

    def test_nothing_listening(self):
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
        s.close()
        fl = D.Flight(f"http://127.0.0.1:{port}", {"model": "x"})
        self.assertTrue(fl.wait_done(5))
        self.assertFalse(fl.wait_first(0))
        self.assertEqual(fl.error["code"], "transport")


class RunnerPassThroughTests(unittest.TestCase):
    """run_mac_linux.sh SPLIT=1, copied into a temporary tree with every tool it drives a stub (ssh, curl, python3,
    pgrep, pkill, caffeinate, the daemon itself, which only sleeps): what it hands the driver. No host, daemon or owner
    process is reached: a stub first on PATH answers each of those names."""

    def runner(self, **env):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d) / "tree"
            run = root / "tests" / "distributed" / "two_machine" / "run_mac_linux.sh"
            run.parent.mkdir(parents=True)
            shutil.copy(HERE / "run_mac_linux.sh", run)
            for f, text in {"scripts/mac-host.sh": "#!/bin/sh\nexit 0\n",
                            "target/release/ucompute": "#!/bin/sh\nexec sleep 30\n"}.items():
                p = root / f
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(text)
                p.chmod(p.stat().st_mode | stat.S_IEXEC)
            (root / ".ucompute").mkdir()
            stubs, log = Path(d) / "stubs", Path(d) / "calls.log"
            stubs.mkdir()
            record = f'for a in "$@"; do printf "%s\\n" "$a" >>"{log}"; done; echo "--end" >>"{log}"; exit 0'
            tools = {"ssh": "exit 0", "curl": "exit 0", "pgrep": "exit 1", "pkill": "exit 0", "open": "exit 0",
                     "pax": "exit 0", "caffeinate": 'shift; exec "$@"', "python3": record}
            for tool, body in tools.items():
                (stubs / tool).write_text(f"#!/bin/sh\n{body}\n")
                (stubs / tool).chmod(0o755)
            full = dict(os.environ, PATH=f"{stubs}:{os.environ['PATH']}", LINUX_HOST="192.0.2.1", LINUX_USER="u",
                        LINUX_KEY="k", SPLIT="1", OUT="out", **env)
            p = subprocess.run(["bash", str(run)], env=full, capture_output=True, text=True, timeout=60)
            self.assertEqual(p.returncode, 0, p.stdout + p.stderr)
            calls = log.read_text().split("--end\n")
            driver = next(c.splitlines() for c in calls if "split_two_machine.py" in c)
            self.assertTrue((root / "out" / "h" / "daemon.pid").read_text().strip().isdigit())
            return driver

    def test_the_default_passes_the_driver_nothing_new(self):
        driver = self.runner()
        for flag in ("--steps", "--r-start", "--h-start", "--h-kill"):
            self.assertNotIn(flag, driver)

    def test_steps_split_passes_the_steps_and_the_restart_commands(self):
        text = "\n".join(self.runner(STEPS_SPLIT="memory,failures"))
        self.assertIn("--steps\nmemory,failures\n--r-start\n", text)
        self.assertIn('pgrep -f "^target/release/ucompute serve"', text)
        self.assertIn("--h-start\nUCOMPUTE_HOME=", text)
        self.assertIn("& echo $! >'out/h/daemon.pid'", text)
        self.assertIn("--h-kill\nkill -9 $(cat 'out/h/daemon.pid')", text)


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
