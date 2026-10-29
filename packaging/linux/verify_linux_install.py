#!/usr/bin/env python3
"""Verify the Linux user-level install from its tarball, on the Linux host (platform readiness plan M6 step 4).

  python3 -I packaging/linux/verify_linux_install.py --tarball <ucompute-*.tar.gz> --out <result.json> [--port 18600]

Unpacks the tarball into a scratch folder, installs its binary there and its systemd user unit under a throwaway
name, with only ExecStart's path and port changed and UCOMPUTE_HOME set to a fresh data folder (the shipped unit's
Restart and default KillMode are what is tested). Never touches ~/.local/bin, the real unit name or port 8192.
Checks: SHA256SUMS; `systemd-analyze verify` of the shipped unit; healthy within 10 s of start; `ucompute setup` provisions the pinned engine and ldd resolves it;
golden CPU completion; a SIGKILL of the daemon restarts it within 10 s with the old engine gone, golden again;
`systemctl --user stop` frees the port with no engine left; removing the unit and binary keeps the data folder.
Exit 0 ALL PASS, 3 any FAIL. SINGLE-HOST.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import urllib.error
import urllib.request

MODEL = "stories15M-q4_0"
PROMPT = "Once upon a time, a little rabbit"
GOLDEN = " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big"
CHECKS = []


def check(name, ok, **facts):
    CHECKS.append({"check": name, "result": "PASS" if ok else "FAIL", **facts})
    print(("PASS " if ok else "FAIL ") + name, facts if facts else "")
    return ok


def sh(*cmd, env=None):
    r = subprocess.run(cmd, capture_output=True, text=True, env=env)
    return r.returncode, r.stdout + r.stderr


def call(port, method, path, body=None, timeout=60):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method,
                                 data=None if body is None else json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except (urllib.error.URLError, OSError) as e:
        return 0, str(e).encode()


def healthy_within(port, s):
    t0 = time.monotonic()
    while time.monotonic() - t0 < s:
        if call(port, "GET", "/health", timeout=1)[0] == 200:
            return round(time.monotonic() - t0, 2)
        time.sleep(0.2)
    return None


def engines(data):
    """Pids of llama-server processes running from this data folder's engine."""
    _, out = sh("ps", "-A", "-o", "pid=,command=")
    root = os.path.join(data, "backends")
    return [int(l.split(None, 1)[0]) for l in out.splitlines() if "llama-server" in l and root in l]


def golden(port):
    s, raw = call(port, "POST", "/v1/completions",
                  {"model": MODEL, "prompt": PROMPT, "max_tokens": 32, "temperature": 0}, timeout=120)
    try:
        return s == 200 and json.loads(raw)["choices"][0]["text"] == GOLDEN
    except (ValueError, KeyError, IndexError):
        return False


def main_pid(unit):
    _, out = sh("systemctl", "--user", "show", "-p", "MainPID", "--value", unit)
    return int(out.strip() or 0)


def run(a, scratch, unit, unit_path):
    pkg = os.path.join(scratch, "pkg")
    with tarfile.open(a.tarball) as t:
        t.extractall(pkg, filter="data")
    (top,) = os.listdir(pkg)
    pkg = os.path.join(pkg, top)
    r = subprocess.run(["sha256sum", "-c", "SHA256SUMS"], cwd=pkg, capture_output=True, text=True)
    check("SHA256SUMS verifies", r.returncode == 0, output=r.stdout.strip())
    bindir, data = os.path.join(scratch, "bin"), os.path.join(scratch, "data")
    os.makedirs(bindir)
    binary = shutil.copy2(os.path.join(pkg, "ucompute"), bindir)
    rc, out = sh("systemd-analyze", "--user", "verify", os.path.join(pkg, "ucompute.service"))
    # Only lines about this unit (it also warns about unrelated installed units); %h/.local/bin/ucompute does not
    # exist here, which is expected.
    lint = [l for l in out.splitlines() if "ucompute.service" in l and ".local/bin/ucompute" not in l]
    check("systemd-analyze verify finds nothing wrong in the shipped unit", not lint, output=lint)
    text = open(os.path.join(pkg, "ucompute.service")).read()
    shipped = "ExecStart=%h/.local/bin/ucompute serve --host 127.0.0.1 --port 8192"
    if not check("the shipped unit has the documented ExecStart", shipped in text):
        return
    text = text.replace(shipped, f"Environment=UCOMPUTE_HOME={data}\nExecStart={binary} serve --host 127.0.0.1 --port {a.port}")
    os.makedirs(os.path.dirname(unit_path), exist_ok=True)
    with open(unit_path, "w") as f:
        f.write(text)
    sh("systemctl", "--user", "daemon-reload")
    rc, out = sh("systemctl", "--user", "start", unit)
    check("unit starts", rc == 0, output=out.strip())
    check("healthy within 10 s of start", healthy_within(a.port, 10) is not None)

    env = {**os.environ, "UCOMPUTE_HOME": data}
    rc, out = sh(binary, "setup", env=env)
    check("ucompute setup provisions the pinned engine", rc == 0, output=out.strip()[-400:])
    servers = [os.path.join(d, f) for d, _, fs in os.walk(os.path.join(data, "backends")) for f in fs if f == "llama-server"]
    rc, out = sh("ldd", servers[0]) if servers else (1, "no llama-server under backends/")
    check("ldd resolves the engine", rc == 0 and "not found" not in out, missing=[l for l in out.splitlines() if "not found" in l])
    s, raw = call(a.port, "POST", f"/api/models/{MODEL}/pull", {}, timeout=300)
    check("fixture pulled", s == 200 and b'"done":true' in raw.replace(b" ", b""), status=s)
    s, raw = call(a.port, "POST", f"/api/models/{MODEL}/load", {}, timeout=120)
    check("fixture loads on the CPU", s == 200, status=s, body=raw.decode(errors="replace")[:300])
    check("golden CPU completion", golden(a.port))

    old_pid, old_engines = main_pid(unit), engines(data)
    os.kill(old_pid, 9)
    t0 = time.monotonic()
    while time.monotonic() - t0 < 10 and main_pid(unit) in (0, old_pid):
        time.sleep(0.2)
    back = healthy_within(a.port, max(0.1, 10 - (time.monotonic() - t0)))
    check("SIGKILL: systemd restarts the daemon and it is healthy within 10 s", back is not None and main_pid(unit) != old_pid,
          old_pid=old_pid, new_pid=main_pid(unit), seconds=round(time.monotonic() - t0, 2))
    left = [p for p in old_engines if p in engines(data)]
    check("SIGKILL: the killed daemon's engine is gone", bool(old_engines) and not left, old_engines=old_engines, left=left)
    call(a.port, "POST", f"/api/models/{MODEL}/load", {}, timeout=120)
    check("golden again after the restart", golden(a.port))

    rc, out = sh("systemctl", "--user", "stop", unit)
    time.sleep(0.5)
    check("stop frees the port", call(a.port, "GET", "/health", timeout=1)[0] == 0, output=out.strip())
    check("stop leaves no engine", not engines(data), left=engines(data))

    sh("systemctl", "--user", "disable", unit)
    os.remove(unit_path)
    os.remove(binary)
    sh("systemctl", "--user", "daemon-reload")
    check("removing the unit and binary keeps the data folder",
          os.path.isdir(os.path.join(data, "models")) and bool(os.listdir(os.path.join(data, "models"))))


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--tarball", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--port", type=int, default=18600)
    a = ap.parse_args()
    if a.port == 8192:
        sys.exit("not port 8192: that is the real install's")
    scratch = tempfile.mkdtemp(prefix="ucompute-verify-")
    unit = f"ucompute-verify-{os.getpid()}.service"
    unit_path = os.path.expanduser(f"~/.config/systemd/user/{unit}")
    try:
        run(a, scratch, unit, unit_path)
    finally:
        sh("systemctl", "--user", "stop", unit)
        if os.path.exists(unit_path):
            os.remove(unit_path)
        sh("systemctl", "--user", "daemon-reload")
        sh("systemctl", "--user", "reset-failed", unit)
        shutil.rmtree(scratch, ignore_errors=True)
    _, kernel = sh("uname", "-srm")
    _, osr = sh("sh", "-c", ". /etc/os-release && echo $PRETTY_NAME")
    ok = all(c["result"] == "PASS" for c in CHECKS)
    with open(a.out, "w") as f:
        json.dump({"label": "SINGLE-HOST", "tarball": os.path.basename(a.tarball), "os": osr.strip(),
                   "kernel": kernel.strip(), "result": "ALL PASS" if ok else "FAIL", "checks": CHECKS}, f, indent=1)
    print("ALL PASS" if ok else "FAIL")
    return 0 if ok else 3


if __name__ == "__main__":
    sys.exit(main())
