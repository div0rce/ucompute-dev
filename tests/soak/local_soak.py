#!/usr/bin/env python3
"""Local soak of one daemon (platform readiness plan M5 step 1; F-08-08). Standard library only, loopback API only.

It starts nothing. Point it at a daemon started with its own COMMON_HOME and port and no LAN listener:

  COMMON_HOME=~/.common-soak common serve --port 18500 >~/.common-soak.log 2>&1 &
  python3 -I tests/soak/local_soak.py --api http://127.0.0.1:18500 --pid $! --home ~/.common-soak \
    --log ~/.common-soak.log --hours 24 --out soak.jsonl

Each cycle runs, in an order drawn from --seed: a golden completion (JSON), a golden stream, a stream cancelled by
closing the socket, a chat completion, a pull that must hit the cache, an overload burst, and every --kill-every
cycles a SIGKILL of the engine. Every --sample seconds one JSONL line: the daemon's RSS and open files, its engines
and any orphaned engine, the log size, the error-code histogram and the p95 of golden completions in that window.
The last line is the verdict.

Bounds, fixed before any run: zero untyped errors (an error response without a known `error.code`, or no response
at all); zero orphaned engines (an engine of --home whose parent is not the daemon) at every sample; every golden
completion golden; a golden completion again within 30 s of each engine kill; after the first hour, daemon RSS
growth <= --rss-mib (64) and open-file growth <= --fd-growth (16). Exit 0 PASS, 3 FAIL, 2 the daemon is unusable.
"""
import argparse
import http.client
import json
import os
import random
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from collections import Counter

MODEL = "stories15M-q4_0"
PROMPT = "Once upon a time, a little rabbit"
# Greedy 32-token continuation of PROMPT with stories15M-q4_0 on llama.cpp b11609 (CPU and Metal), as in the tests.
GOLDEN = " named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big"
KNOWN = {"invalid_request", "model_not_found", "model_not_downloaded", "model_file_invalid", "integrity_mismatch",
         "insufficient_memory", "context_too_long", "backend_unavailable", "backend_failed", "gpu_unavailable",
         "timeout", "cancelled", "overloaded", "download_failed", "not_implemented", "policy_denied", "unauthorized",
         "io", "internal"}
GOLD = {"model": MODEL, "prompt": PROMPT, "max_tokens": 32, "temperature": 0}


def classify(status, body):
    """'ok', a known error code, or 'untyped:<status>'."""
    if 200 <= status < 300:
        return "ok"
    code = body.get("error", {}).get("code") if isinstance(body, dict) and isinstance(body.get("error"), dict) else None
    return code if code in KNOWN else f"untyped:{status}"


def p95(xs):
    if not xs:
        return None
    s = sorted(xs)
    return s[min(len(s) - 1, -(-95 * len(s) // 100) - 1)]


def verdict(samples, untyped, not_golden, slow_recoveries, rss_mib, fd_growth):
    """Pass/fail against the bounds; samples are the per-window dicts, the first hour's excluded from growth."""
    fails = []
    if untyped:
        fails.append(f"{untyped} untyped errors")
    if not_golden:
        fails.append(f"{not_golden} golden completions not golden")
    if slow_recoveries:
        fails.append(f"{slow_recoveries} engine kills not recovered within 30 s")
    if any(s["orphans"] for s in samples):
        fails.append("orphaned engines seen")
    late = [s for s in samples if s["elapsed_s"] >= 3600 and s["rss_kib"] and s["fds"] is not None]
    if len(late) >= 2:
        if (late[-1]["rss_kib"] - late[0]["rss_kib"]) / 1024 > rss_mib:
            fails.append(f"RSS grew {(late[-1]['rss_kib'] - late[0]['rss_kib']) / 1024:.1f} MiB after the first hour")
        if late[-1]["fds"] - late[0]["fds"] > fd_growth:
            fails.append(f"open files grew by {late[-1]['fds'] - late[0]['fds']} after the first hour")
    return fails


def call(api, method, path, body=None, timeout=120):
    req = urllib.request.Request(api + path, data=None if body is None else json.dumps(body).encode(), method=method,
                                 headers={"Content-Type": "application/json"})
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        status, raw = e.code, e.read()
    except (urllib.error.URLError, OSError, http.client.HTTPException) as e:
        status, raw = 0, repr(e).encode()
    try:
        val = json.loads(raw)
    except ValueError:
        val = raw.decode(errors="replace")
    return status, val, (time.monotonic() - t0) * 1000


def sse(api, path, body, stop_after=None, timeout=120):
    """(200, data events) or (status, error body); an error event inside the stream reads as a 500 with its body.
    With stop_after, closes the socket after that many events (a client cancel)."""
    req = urllib.request.Request(api + path, data=json.dumps(body).encode(), method="POST",
                                 headers={"Content-Type": "application/json"})
    evs = []
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            for line in r:
                line = line.decode().strip()
                if not line.startswith("data:") or line == "data: [DONE]":
                    continue
                ev = json.loads(line[5:])
                if "error" in ev:
                    return 500, ev
                evs.append(ev)
                if stop_after and len(evs) >= stop_after:
                    break
    except urllib.error.HTTPError as e:
        try:
            return e.code, json.loads(e.read())
        except ValueError:
            return e.code, None
    except (urllib.error.URLError, OSError, ValueError, http.client.HTTPException) as e:
        return 0, repr(e)
    return 200, evs


def text_of(evs):
    return "".join(c.get("text") or "" for ev in evs for c in ev.get("choices", []))


def procs():
    """[(pid, ppid, command)] for every process."""
    out = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,command="], capture_output=True, text=True).stdout
    rows = []
    for line in out.splitlines():
        parts = line.split(None, 2)
        if len(parts) == 3 and parts[0].isdigit() and parts[1].isdigit():
            rows.append((int(parts[0]), int(parts[1]), parts[2]))
    return rows


def engines(home, pid):
    """(the daemon's engines, orphaned engines of this home)."""
    roots = {os.path.join(home, "backends"), os.path.join(os.path.realpath(home), "backends")}
    mine = [p for p in procs() if "llama-server" in p[2] and any(r in p[2] for r in roots)]
    return [p[0] for p in mine if p[1] == pid], [p[0] for p in mine if p[1] != pid]


def rss_kib(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    return int(out) if out.isdigit() else None


def fds(pid):
    try:
        return len(os.listdir(f"/proc/{pid}/fd"))
    except OSError:
        out = subprocess.run(["lsof", "-n", "-P", "-p", str(pid)], capture_output=True, text=True).stdout
        return max(0, len(out.splitlines()) - 1) if out else None


class Soak:
    def __init__(self, a):
        self.a, self.rng = a, random.Random(a.seed)
        self.codes, self.lat, self.untyped, self.not_golden, self.slow = Counter(), [], 0, 0, 0
        self.kills, self.cycles, self.errors = [], 0, []

    def note(self, step, status, body):
        c = classify(status, body) if status else "untyped:no_response"
        self.codes[f"{step}:{c}"] += 1
        if c.startswith("untyped"):
            self.untyped += 1
            # what is needed to find the cause of a rare one (the 2026-10-24 soak lost it)
            self.errors.append({"t": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "step": step,
                                "status": status, "detail": str(body)[:300]})
        return c

    def golden(self):
        s, v, ms = call(self.a.api, "POST", "/v1/completions", GOLD)
        if self.note("golden", s, v) == "ok":
            self.lat.append(ms)
            if v["choices"][0]["text"] != GOLDEN:
                self.not_golden += 1

    def golden_stream(self):
        s, v = sse(self.a.api, "/v1/completions", {**GOLD, "stream": True})
        if self.note("stream", s, v) == "ok" and text_of(v) != GOLDEN:
            self.not_golden += 1

    def cancel(self):
        s, v = sse(self.a.api, "/v1/completions", {**GOLD, "max_tokens": 256, "stream": True}, stop_after=3)
        self.note("cancel", s, v)

    def chat(self):
        s, v, _ = call(self.a.api, "POST", "/v1/chat/completions",
                       {"model": MODEL, "messages": [{"role": "user", "content": PROMPT}], "max_tokens": 16,
                        "temperature": 0})
        self.note("chat", s, v)

    def pull(self):
        t0 = time.monotonic()
        s, v = sse(self.a.api, f"/api/models/{MODEL}/pull", {}, timeout=60)
        if self.note("pull", s, v) == "ok":
            if not (v and v[-1].get("done")):  # a stream that ends with neither `done` nor an error
                self.codes["pull:untyped:no_done"] += 1
                self.untyped += 1
            elif time.monotonic() - t0 > 10:  # the model is in the store: a hash check of 19 MB, not a download
                self.codes["pull:slow_cache_hit"] += 1

    def overload(self):
        out = []

        def one():
            out.append(call(self.a.api, "POST", "/v1/completions", {**GOLD, "max_tokens": 64})[:2])

        ts = [threading.Thread(target=one) for _ in range(self.a.burst)]
        for t in ts:
            t.start()
        for t in ts:
            t.join()
        for s, v in out:
            self.note("overload", s, v)
        for _ in range(self.a.burst - len(out)):  # a thread that died without a result is an untyped failure too
            self.note("overload", 0, "no result: the request's thread ended without one")

    def kill_engine(self):
        mine, _ = engines(self.a.home, self.a.pid)
        if not mine:
            self.codes["kill:no_engine"] += 1
            return
        os.kill(mine[0], signal.SIGKILL)
        t0 = time.monotonic()
        while time.monotonic() - t0 < 30:
            s, v, _ = call(self.a.api, "POST", "/v1/completions", GOLD, timeout=30)
            c = classify(s, v) if s else "untyped:no_response"
            if c == "ok" and v["choices"][0]["text"] == GOLDEN:
                self.kills.append(round(time.monotonic() - t0, 2))
                return
            if c in ("model_not_found", "backend_unavailable", "backend_failed"):
                call(self.a.api, "POST", f"/api/models/{MODEL}/load", {}, timeout=60)
            time.sleep(0.5)
        self.slow += 1
        self.kills.append(None)

    def sample(self, t_start, out):
        mine, orphans = engines(self.a.home, self.a.pid)
        try:
            log = os.path.getsize(self.a.log) if self.a.log else None
        except OSError:
            log = None
        row = {"t": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()), "elapsed_s": round(time.monotonic() - t_start),
               "cycles": self.cycles, "rss_kib": rss_kib(self.a.pid), "fds": fds(self.a.pid), "engines": len(mine),
               "orphans": orphans, "log_bytes": log, "golden_p95_ms": p95(self.lat), "golden_n": len(self.lat),
               "codes": dict(self.codes), "kills_recovered_s": self.kills, "errors": self.errors}
        out.write(json.dumps(row) + "\n")
        out.flush()
        self.lat, self.codes, self.kills, self.errors = [], Counter(), [], []
        return row


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--api", default="http://127.0.0.1:18500")
    ap.add_argument("--pid", type=int, required=True, help="the daemon's pid")
    ap.add_argument("--home", required=True, help="the daemon's COMMON_HOME")
    ap.add_argument("--log", help="the daemon's log file, for its size")
    ap.add_argument("--hours", type=float, default=24)
    ap.add_argument("--out", required=True)
    ap.add_argument("--seed", type=int, default=20261023)
    ap.add_argument("--sample", type=int, default=60)
    ap.add_argument("--kill-every", type=int, default=50, help="cycles between engine kills (0: never)")
    ap.add_argument("--burst", type=int, default=40, help="concurrent requests in an overload burst")
    ap.add_argument("--only", help="comma-separated steps to run (golden,golden_stream,cancel,chat,pull,overload); all by default")
    ap.add_argument("--rss-mib", type=float, default=64)
    ap.add_argument("--fd-growth", type=int, default=16)
    a = ap.parse_args()
    if not a.api.startswith(("http://127.0.0.1:", "http://localhost:", "http://[::1]:")):
        sys.exit("loopback API only")
    a.home = os.path.abspath(os.path.expanduser(a.home))
    s, v, _ = call(a.api, "POST", f"/api/models/{MODEL}/load", {}, timeout=120)
    if classify(s, v) != "ok":
        print(f"cannot load {MODEL}: {s} {v}", file=sys.stderr)
        return 2
    soak, samples = Soak(a), []
    steps = [soak.golden, soak.golden_stream, soak.cancel, soak.chat, soak.pull, soak.overload]
    if a.only:
        steps = [getattr(soak, n) for n in a.only.split(",")]
    t_start = t_sample = time.monotonic()
    with open(a.out, "a") as out:
        while time.monotonic() - t_start < a.hours * 3600:
            for step in soak.rng.sample(steps, len(steps)):
                step()
            soak.cycles += 1
            if a.kill_every and soak.cycles % a.kill_every == 0:
                soak.kill_engine()
            if time.monotonic() - t_sample >= a.sample:
                samples.append(soak.sample(t_start, out))
                t_sample = time.monotonic()
                if soak.golden in steps and "golden:ok" not in samples[-1]["codes"]:
                    # stop instead of looping on a daemon whose inference only fails
                    samples[-1]["stopped"] = "no golden completion succeeded in this window"
                    break
        else:
            samples.append(soak.sample(t_start, out))
        fails = verdict(samples, soak.untyped, soak.not_golden, soak.slow, a.rss_mib, a.fd_growth)
        if samples[-1].get("stopped"):
            fails.append("stopped early: " + samples[-1]["stopped"])
        out.write(json.dumps({"verdict": "FAIL" if fails else "PASS", "fails": fails, "cycles": soak.cycles,
                              "hours": a.hours, "seed": a.seed}) + "\n")
    print("FAIL: " + "; ".join(fails) if fails else "PASS")
    return 3 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
