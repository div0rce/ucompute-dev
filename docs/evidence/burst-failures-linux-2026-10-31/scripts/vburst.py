#!/usr/bin/env python3
"""SINGLE-HOST verifier burst driver: bursts of N concurrent POST /v1/completions (soak overload body, max_tokens 64),
client timeout 300 s (> daemon first_token 120 s). Prints a summary; writes non-ok rows to --out."""
import argparse, http.client, json, re, threading, time, urllib.error, urllib.request
from collections import Counter
ap = argparse.ArgumentParser()
ap.add_argument("--url", required=True); ap.add_argument("--minutes", type=float, default=6)
ap.add_argument("--burst", type=int, default=40); ap.add_argument("--out", required=True)
a = ap.parse_args()
BODY = json.dumps({"model": "stories15M-q4_0", "prompt": "Once upon a time, a little rabbit", "max_tokens": 64, "temperature": 0}).encode()
def one(res):
    req = urllib.request.Request(a.url + "/v1/completions", data=BODY, method="POST", headers={"Content-Type": "application/json"})
    t0 = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            st, raw = r.status, r.read()
    except urllib.error.HTTPError as e:
        st, raw = e.code, e.read()
    except Exception as e:
        st, raw = 0, repr(e).encode()
    ms = (time.monotonic() - t0) * 1000
    try:
        v = json.loads(raw)
    except ValueError:
        v = raw.decode(errors="replace")
    if st == 200:
        code, msg = "ok", (v["choices"][0]["text"][:40] if isinstance(v, dict) else "?")
    elif isinstance(v, dict) and isinstance(v.get("error"), dict) and v["error"].get("code"):
        code, msg = v["error"]["code"], str(v["error"].get("message", ""))
    else:
        code, msg = f"untyped:{st}", str(v)[:300]
    res.append((code, msg, ms))
codes, causes, texts = Counter(), Counter(), Counter()
slow_adm, n, bursts = 0.0, 0, 0
end = time.monotonic() + a.minutes * 60
with open(a.out, "w") as f:
    while time.monotonic() < end:
        res = []
        ts = [threading.Thread(target=one, args=(res,)) for _ in range(a.burst)]
        [t.start() for t in ts]; [t.join() for t in ts]
        bursts += 1
        if len(res) != a.burst:
            codes["untyped:thread_lost"] += a.burst - len(res)
        for code, msg, ms in res:
            n += 1; codes[code] += 1
            if code != "overloaded":
                slow_adm = max(slow_adm, ms)
            if code == "ok":
                texts[msg] += 1
            else:
                if code != "overloaded":
                    causes[(code, re.sub(r"127\.0\.0\.1:\d+", "127.0.0.1:PORT", msg))] += 1
                f.write(json.dumps({"t": time.time(), "code": code, "msg": msg, "ms": round(ms, 1)}) + "\n")
adm = n - codes["overloaded"]
print(f"SINGLE-HOST verifier bursts={bursts} requests={n} admitted={adm} codes={dict(codes)}")
print(f"backend_failed={codes['backend_failed']} rate_of_admitted={100*codes['backend_failed']/max(adm,1):.3f}% slowest_admitted_ms={slow_adm:.0f}")
for (c, m), k in causes.most_common(10):
    print(f"cause {k} x {c}: {m[:300]}")
print("ok texts:", dict(texts))
