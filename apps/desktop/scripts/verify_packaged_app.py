#!/usr/bin/env python3
"""Verify a packaged macOS UCompute.app and its .dmg end to end, on the exact shipped binaries.

    python3 -I apps/desktop/scripts/verify_packaged_app.py --dmg target/release/bundle/dmg/UCompute_0.1.1_aarch64.dmg \
        --work <empty scratch dir> [--out evidence.json]

What it checks (stdlib only, no repo build artefacts are used at run time):
 1. Binaries: the .app inside the .dmg is arm64, ad-hoc signed and valid, links only system libraries, and its
    bundled `ucompute` accepts the exact arguments the app starts it with (src-tauri/daemon-args.txt). The app ships
    Contents/Resources/THIRD_PARTY_NOTICES.txt (scripts/gen_notices.py at build time) naming rustls and react.
 2. Fresh install: the app is copied out of the mounted .dmg into <work>/Applications and started with an empty
    HOME, a minimal PATH (/usr/bin:/bin:/usr/sbin:/sbin) and no UCOMPUTE_HOME, so nothing from the development
    environment is visible. Its daemon must become healthy on 127.0.0.1:8192, be the bundled binary with the
    expected arguments, and download the pinned engine into the fresh HOME.
 3. Real inference through the app's daemon: pull the stories15M fixture, load it, check the golden completion.
 4. Quit (SIGTERM to the app, the same RunEvent::Exit path as Cmd+Q): daemon and engine stop, port 8192 is free.
 5. Restart: healthy again, one daemon, one engine, inference again.
 6. Daemon killed (SIGKILL to the daemon only, the app stays): the app restarts one daemon, /health answers within
    10 s of the kill (plan M4 step 4.1), the restarted daemon stops the killed one's engine, inference works again
    with one engine, and the app's daemon.log holds both runs (each one's start note and listening line).
 7. Crash (SIGKILL to the app): the daemon notices via --exit-with-parent and stops itself and its engine.
 8. Restart after the crash, quit, and nothing from the install is left running.
Needs network access (engine archive from GitHub, fixture from Hugging Face) and a free port 8192.
"""
import argparse
import hashlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

PORT = 8192
MODEL = "stories15M-q4_0"
PROMPT = "Once upon a time, a little rabbit"
GOLDEN = (" named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it."
          " But the radish was too big")
REPO = Path(__file__).resolve().parents[3]
DAEMON_ARGS = (REPO / "apps/desktop/src-tauri/daemon-args.txt").read_text().split()
EVIDENCE = {"label": "SINGLE-HOST (one Mac); packaged binaries", "steps": []}


def step(name, ok, **facts):
    EVIDENCE["steps"].append({"step": name, "ok": ok, **facts})
    print(("PASS " if ok else "FAIL ") + name + " " + json.dumps(facts, default=str), flush=True)
    if not ok:
        raise SystemExit(f"verification failed at: {name}")


def sh(*cmd, check=True, **kw):
    r = subprocess.run(cmd, capture_output=True, text=True, **kw)
    if check and r.returncode != 0:
        raise SystemExit(f"{' '.join(map(str, cmd))} failed ({r.returncode}): {r.stderr.strip()}")
    return r


def sha256(p):
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def without_linkedit_vmsize(path):
    """Bytes of an (unsigned) arm64 Mach-O with the __LINKEDIT segment's vmsize zeroed.

    `codesign` grows that one field when it signs and `--remove-signature` does not shrink it back, so after
    removing signatures a packaged and a freshly built binary differ in exactly those 8 bytes."""
    import struct
    b = bytearray(Path(path).read_bytes())
    magic, _, _, _, ncmds, _, _, _ = struct.unpack_from("<IiiIIIII", b, 0)
    assert magic == 0xFEEDFACF, f"not a 64-bit Mach-O: {path}"
    off = 32
    for _ in range(ncmds):
        cmd, size = struct.unpack_from("<II", b, off)
        if cmd == 0x19 and bytes(b[off + 8:off + 24]).rstrip(b"\0") == b"__LINKEDIT":  # LC_SEGMENT_64
            b[off + 32:off + 40] = bytes(8)
        off += size
    return bytes(b)


def procs():
    out = sh("ps", "-axo", "pid=,ppid=,args=").stdout
    rows = []
    for line in out.splitlines():
        parts = line.strip().split(None, 2)
        if len(parts) == 3:
            rows.append((int(parts[0]), int(parts[1]), parts[2]))
    return rows


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def http(method, path, body=None, timeout=300):
    data = None if body is None else json.dumps(body).encode()
    req = urllib.request.Request(f"http://127.0.0.1:{PORT}{path}", data=data, method=method,
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return r.status, r.read()


def healthy():
    try:
        return http("GET", "/health", timeout=2)[0] == 200
    except Exception:
        return False


def port_open():
    import socket
    s = socket.socket()
    s.settimeout(0.5)
    try:
        return s.connect_ex(("127.0.0.1", PORT)) == 0
    finally:
        s.close()


def wait(cond, within, what):
    t0 = time.monotonic()
    while not cond():
        if time.monotonic() - t0 > within:
            step(what, False, waited_s=within)
        time.sleep(0.1)
    return round(time.monotonic() - t0, 3)


class Install:
    def __init__(self, work, app_src):
        self.work = work
        self.app = work / "Applications" / "UCompute.app"
        self.home = work / "home"
        (work / "Applications").mkdir(parents=True)
        self.home.mkdir()
        (work / "tmp").mkdir()
        sh("ditto", str(app_src), str(self.app))
        self.env = {"HOME": str(self.home), "USER": os.environ.get("USER", "user"),
                    "LOGNAME": os.environ.get("USER", "user"), "TMPDIR": str(work / "tmp") + "/",
                    "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "LANG": "en_US.UTF-8"}
        self.proc = None
        self.runs = 0

    def launch(self):
        self.runs += 1
        log = open(self.work / f"app-run{self.runs}.log", "w")
        self.proc = subprocess.Popen([str(self.app / "Contents/MacOS/ucompute-desktop")], env=self.env,
                                     cwd=str(self.work), stdout=log, stderr=subprocess.STDOUT,
                                     start_new_session=True)
        return self.proc.pid

    def sidecars(self):
        exe = str(self.app / "Contents/MacOS/ucompute") + " "
        return [(p, pp, a) for p, pp, a in procs() if a.startswith(exe)]

    def engines(self):
        return [p for p, _, a in procs() if "llama-server" in a and str(self.home) in a]

    def everything(self):
        me = {os.getpid(), os.getppid()}
        return [(p, a) for p, _, a in procs()
                if str(self.work) in a and p not in me and "verify_packaged_app.py" not in a]

    def log(self):
        return (self.work / f"app-run{self.runs}.log").read_text(errors="replace")


def log_holds_both_runs(text, killed, restarted):
    """The app's daemon.log has the killed daemon's start note and listening line, then the restart note, then the
    restarted daemon's start note and listening line. The log persists across launches (earlier stages wrote to it
    too), so a plain count of listening lines would pass without the restarted daemon's output."""
    listen = f"listening on http://127.0.0.1:{PORT}"
    a = text.find(f"ucompute-desktop: started the daemon, pid {killed}\n")
    b = text.find("ucompute-desktop: restarting the daemon in", a) if a >= 0 else -1
    c = text.find(f"ucompute-desktop: started the daemon, pid {restarted}\n", b) if b >= 0 else -1
    return a >= 0 and listen in text[a:b] and c > b and listen in text[c:]


def daemon_restarted(parent, app_pid, restarted_after_s, text, killed, restarted):
    """Stage 6: the restarted daemon's parent is the app, /health answered within 10 s of the kill (the wait can
    return later than that when its last check passes slowly) and the log holds both runs."""
    return parent == app_pid and restarted_after_s <= 10 and log_holds_both_runs(text, killed, restarted)


def gatekeeper_rejected(returncode, output):
    """`spctl --assess --type execute` refused the app: what a downloaded, ad-hoc-signed app meets on first open."""
    return returncode != 0 and "rejected" in output


def gatekeeper_on_download(work, dmg):
    """Read-only: a copy of the DMG marked as downloaded (com.apple.quarantine), its app copied out still quarantined,
    and Gatekeeper's assessment of it. Approving the app (Open Anyway) changes a security setting and stays manual."""
    dl, qmnt, app = work / "downloaded.dmg", work / "qmnt", work / "downloaded" / "UCompute.app"
    shutil.copy(dmg, dl)
    sh("xattr", "-w", "com.apple.quarantine", f"0081;{int(time.time()):x};Safari;", str(dl))
    qmnt.mkdir()
    sh("hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", str(qmnt), str(dl))
    try:
        app.parent.mkdir()
        sh("ditto", str(qmnt / "UCompute.app"), str(app))
    finally:
        sh("hdiutil", "detach", str(qmnt), check=False)
    quarantined = "com.apple.quarantine" in sh("xattr", str(app)).stdout
    if not quarantined:  # what a browser download and a Finder copy would have set
        sh("xattr", "-w", "com.apple.quarantine", f"0081;{int(time.time()):x};Safari;", str(app))
    r = sh("spctl", "--assess", "--type", "execute", "-vv", str(app), check=False)
    out = (r.stdout + r.stderr).strip()
    shutil.rmtree(app.parent)
    dl.unlink()
    return {"quarantine_propagated": quarantined, "spctl_exit": r.returncode, "spctl": out[:300],
            "rejected": gatekeeper_rejected(r.returncode, out)}


def names_packages(notices, *names):
    """Each of `names` has its own entry in THIRD_PARTY_NOTICES: a line `<name> <version> — <licence>…`
    (scripts/gen_notices.py), so `rustls-pki-types` or a licence text that mentions the name does not count."""
    return all(re.search(rf"^{re.escape(n)} \d\S* — ", notices, re.M) for n in names)


def infer(tag):
    t0 = time.monotonic()
    status, body = http("POST", f"/api/models/{MODEL}/load", {"device": "auto"})
    load = json.loads(body)
    status, body = http("POST", "/v1/completions",
                        {"model": MODEL, "prompt": PROMPT, "max_tokens": 32, "temperature": 0})
    out = json.loads(body)
    text = out["choices"][0]["text"]
    step(f"{tag}: real inference through the app's daemon", status == 200 and text == GOLDEN,
         device=load.get("device_actual"), text=text,
         decode_tok_s=round(out.get("timings", {}).get("decode_tokens_per_second", 0), 1),
         total_s=round(time.monotonic() - t0, 2))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--dmg", required=True, type=Path)
    ap.add_argument("--work", required=True, type=Path)
    ap.add_argument("--out", type=Path)
    ap.add_argument("--built", type=Path,
                    help="cargo release dir (target/release): also prove the packaged binaries are these builds")
    a = ap.parse_args()
    work = a.work.resolve()
    if work.exists() and any(work.iterdir()):
        raise SystemExit(f"--work must be empty: {work}")
    work.mkdir(parents=True, exist_ok=True)
    if port_open():
        raise SystemExit(f"port {PORT} is in use; quit any running UCompute daemon first (the app would adopt it)")

    EVIDENCE["dmg"] = {"path": str(a.dmg), "sha256": sha256(a.dmg), "bytes": a.dmg.stat().st_size}
    mnt = work / "mnt"
    mnt.mkdir()
    sh("hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", str(mnt), str(a.dmg))
    inst = None
    try:
        app_src = mnt / "UCompute.app"
        macos = app_src / "Contents/MacOS"
        bins = {n: {"sha256": sha256(macos / n), "archs": sh("lipo", "-archs", str(macos / n)).stdout.strip(),
                    "libs": [l.split()[0] for l in sh("otool", "-L", str(macos / n)).stdout.splitlines()[1:]]}
                for n in ("ucompute", "ucompute-desktop")}
        EVIDENCE["binaries_in_dmg"] = bins
        system_only = all(l.startswith(("/usr/lib/", "/System/Library/")) for b in bins.values() for l in b["libs"])
        step("binaries are arm64 and link only system libraries",
             all(b["archs"] == "arm64" for b in bins.values()) and system_only,
             archs={n: b["archs"] for n, b in bins.items()})
        if a.built:
            # Tauri re-signs bundled executables (hardened runtime), so compare with signatures removed.
            same = {}
            for n in ("ucompute", "ucompute-desktop"):
                x, y = work / f"unsigned-packaged-{n}", work / f"unsigned-built-{n}"
                shutil.copy(macos / n, x)
                shutil.copy(a.built / n, y)
                sh("codesign", "--remove-signature", str(x))
                sh("codesign", "--remove-signature", str(y))
                same[n] = without_linkedit_vmsize(x) == without_linkedit_vmsize(y)
                x.unlink()
                y.unlink()
            step("packaged binaries are the build (identical once signatures and __LINKEDIT vmsize are set aside)",
                 all(same.values()),
                 built=str(a.built), identical=same)
        notices = app_src / "Contents/Resources/THIRD_PARTY_NOTICES.txt"
        text = notices.read_text(encoding="utf-8", errors="replace") if notices.is_file() else ""
        step("the app ships THIRD_PARTY_NOTICES naming the Rust crate rustls and the JS package react",
             names_packages(text, "rustls", "react"), path=str(notices), bytes=len(text),
             sha256=sha256(notices) if text else None)
        v = sh("codesign", "--verify", "--deep", "--strict", str(app_src), check=False)
        step("app signature verifies (ad-hoc; no Developer ID, B-008)", v.returncode == 0, stderr=v.stderr.strip())
        gk = gatekeeper_on_download(work, a.dmg)
        step("Gatekeeper refuses a downloaded copy on first open, as the install notes say (ad-hoc signed; read-only "
             "spctl assessment, the Open Anyway approval stays manual)", gk["rejected"], **gk)
        r = sh(str(macos / "ucompute"), *DAEMON_ARGS, "--help", check=False)
        step("bundled daemon accepts the app's exact arguments", r.returncode == 0,
             args=DAEMON_ARGS, exit=r.returncode, stderr=r.stderr.strip()[:300])
        EVIDENCE["daemon_version"] = sh(str(macos / "ucompute"), "--version").stdout.strip()

        inst = Install(work, app_src)
        sh("hdiutil", "detach", str(mnt))
        mnt = None

        # 2. fresh install, first launch
        app_pid = inst.launch()
        t = wait(healthy, 180, "first launch: daemon healthy on 127.0.0.1:8192")
        sc = inst.sidecars()
        ok = len(sc) == 1 and sc[0][1] == app_pid and sc[0][2].split()[1:] == DAEMON_ARGS
        step("first launch: daemon healthy, started by the app with its exact arguments", ok, healthy_after_s=t,
             sidecar=sc)
        data = inst.home / "Library/Application Support/UCompute"
        engine_dir = data / "backends/llama.cpp"
        step("fresh install provisioned the pinned engine inside the new HOME", engine_dir.is_dir()
             and any(engine_dir.iterdir()), data_dir=str(data), engine=[p.name for p in engine_dir.iterdir()])

        # 3. real inference
        t0 = time.monotonic()
        with urllib.request.urlopen(urllib.request.Request(
                f"http://127.0.0.1:{PORT}/api/models/{MODEL}/pull", method="POST"), timeout=600) as r:
            last = ""
            for raw in r:
                line = raw.decode().strip()
                if line.startswith("data:"):
                    last = line[5:].strip()
        done = json.loads(last) if last else {}
        step("model pulled and hash-verified by the app's daemon", done.get("done") is True,
             bytes=done.get("downloaded_bytes"), seconds=round(time.monotonic() - t0, 1))
        infer("first launch")
        daemon1, engines1 = sc[0][0], inst.engines()

        # 4. quit
        os.kill(app_pid, signal.SIGTERM)
        t_app = wait(lambda: inst.proc.poll() is not None, 20, "quit: app exits after SIGTERM")
        t_d = wait(lambda: not alive(daemon1), 15, "quit: daemon exits with the app")
        wait(lambda: not any(alive(e) for e in engines1), 10, "quit: engine stops")
        wait(lambda: not port_open(), 10, "quit: port 8192 released")
        step("quit stops daemon and engine and frees the port", "daemon stopped" in inst.log(),
             app_exit_s=t_app, daemon_gone_s=t_d, engines=engines1)

        # 5. restart
        app_pid = inst.launch()
        t = wait(healthy, 60, "restart: daemon healthy")
        sc = inst.sidecars()
        step("restart: one daemon, started by the new app process, no port conflict",
             len(sc) == 1 and sc[0][1] == app_pid and sc[0][0] != daemon1, healthy_after_s=t, sidecar=sc)
        infer("restart")
        step("restart: one engine", len(inst.engines()) == 1, engines=inst.engines())
        daemon2, engines2 = sc[0][0], inst.engines()

        # 6. the daemon dies, the app stays: it restarts one daemon and kept the output of both
        os.kill(daemon2, signal.SIGKILL)
        killed = time.monotonic()
        wait(lambda: not alive(daemon2), 10, "daemon killed: it exits")

        def restarted():
            sc = inst.sidecars()
            return len(sc) == 1 and sc[0][0] != daemon2 and healthy()
        wait(restarted, 10 - (time.monotonic() - killed), "daemon killed: /health within 10 s of the kill")
        t = round(time.monotonic() - killed, 3)
        sc = inst.sidecars()
        wait(lambda: not any(alive(e) for e in engines2), 10, "daemon killed: its engine is stopped")
        log = inst.home / "Library/Logs/org.ucompute.desktop/daemon.log"
        text = log.read_text(errors="replace") if log.is_file() else ""
        step("daemon killed (SIGKILL daemon only): the app restarts one daemon, /health within 10 s, its log holds both runs",
             daemon_restarted(sc[0][1], app_pid, t, text, daemon2, sc[0][0]), restarted_after_s=t, sidecar=sc,
             log=str(log), log_bytes=len(text))
        infer("daemon restarted")
        step("daemon restarted: one engine", len(inst.engines()) == 1, engines=inst.engines())
        daemon2, engines2 = sc[0][0], inst.engines()

        # 7. crash
        os.kill(app_pid, signal.SIGKILL)
        inst.proc.wait()
        t_d = wait(lambda: not alive(daemon2), 10, "crash: daemon exits after the app was killed")
        wait(lambda: not any(alive(e) for e in engines2), 10, "crash: engine stops")
        wait(lambda: not port_open(), 10, "crash: port 8192 released")
        step("crash (SIGKILL app): --exit-with-parent stops daemon and engine", True, daemon_gone_s=t_d)

        # 8. restart after crash, quit, nothing left
        app_pid = inst.launch()
        t = wait(healthy, 60, "restart after crash: daemon healthy")
        sc = inst.sidecars()
        step("restart after crash: one daemon", len(sc) == 1 and sc[0][1] == app_pid, healthy_after_s=t)
        os.kill(app_pid, signal.SIGTERM)
        wait(lambda: inst.proc.poll() is not None, 20, "final quit: app exits")
        wait(lambda: not inst.everything(), 15, "final quit: nothing from the install still running")
        step("final: no process from the install is running and port 8192 is free", not port_open(),
             leftovers=inst.everything())
    finally:
        if inst and inst.proc and inst.proc.poll() is None:
            inst.proc.kill()
        if mnt:
            sh("hdiutil", "detach", str(mnt), check=False)
        if a.out:
            a.out.write_text(json.dumps(EVIDENCE, indent=2, default=str))
    print("ALL PASS")


if __name__ == "__main__":
    main()
