#!/usr/bin/env python3
"""Direct llama-server probe (not the daemon): time to response headers vs time to first SSE data line for a
streaming /completion with a long prompt. argv: port keyfile"""
import http.client, json, sys, time
port, key = int(sys.argv[1]), open(sys.argv[2]).read().strip()
prompt = " ".join(["The quick brown fox jumps over the lazy dog."] * 150)
for n in range(3):
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=300)
    body = json.dumps({"prompt": prompt, "n_predict": 4, "stream": True, "cache_prompt": False})
    t0 = time.monotonic()
    c.request("POST", "/completion", body, {"Content-Type": "application/json", "Authorization": "Bearer " + key})
    r = c.getresponse(); th = time.monotonic() - t0
    first = None
    while True:
        line = r.readline()
        if not line:
            break
        if line.startswith(b"data:") and first is None:
            first = time.monotonic() - t0
            ev = json.loads(line[5:])
    tend = time.monotonic() - t0
    print(f"run {n}: status={r.status} headers_s={th:.3f} first_data_s={first:.3f} end_s={tend:.3f} prompt_n={ev.get('tokens_evaluated', '?')}")
    c.close()
