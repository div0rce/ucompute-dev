# SINGLE-HOST S1-M diagnostic (2026-10-29), copied from the session scratchpad; run from the repository root.
"""SINGLE-HOST diagnostic (S1-M): the pinned engine run directly, plaintext loopback RPC, no UCompute tunnel.
Greedy ids for the P1 prompts in four configurations, to locate the cause of a split/local token difference."""
import json, os, socket, subprocess, sys, time, urllib.request
ENG = ".ucompute/backends/llama.cpp/b11609"
MODEL = sys.argv[1]
OUT = sys.argv[2]
PRE = json.load(open("tests/distributed/two_machine/split_prereg.json"))
ENV = {"PATH": "/usr/bin:/bin", "GGML_RPC_NO_RDMA": "1"}

def free():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p

def wait_http(port, t=120):
    t0 = time.time()
    while time.time() - t0 < t:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as r:
                if r.status == 200: return
        except Exception: pass
        time.sleep(0.3)
    raise SystemExit(f"llama-server on {port} not ready")

def complete(port, text, n):
    b = {"prompt": text, "n_predict": n, "temperature": 0, "seed": 42, "cache_prompt": False,
         "return_tokens": True, "n_probs": 3, "stream": False}
    req = urllib.request.Request(f"http://127.0.0.1:{port}/completion", json.dumps(b).encode(),
                                 {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=300) as r:
        v = json.load(r)
    probs = [[(c.get("id"), round(c.get("prob", c.get("logprob", 0)), 6)) for c in p.get("top_probs", p.get("top_logprobs", []))]
             for p in v.get("completion_probabilities", [])]
    return {"ids": v["tokens"], "text": v["content"], "top": probs}

def run(name, extra, rpc):
    logs = open(os.path.join(OUT, f"{name}.log"), "w")
    procs = []
    args = []
    if rpc:
        rp = free()
        procs.append(subprocess.Popen([f"{ENG}/ggml-rpc-server", "-H", "127.0.0.1", "-p", str(rp), "-t", "4", "-d", "CPU"],
                                      stdout=logs, stderr=logs, env=ENV))
        time.sleep(2)
        args = ["--rpc", f"127.0.0.1:{rp}", "--device", "RPC0", "--tensor-split", "1", "--split-mode", "layer",
                "-ngl", "13", "--no-op-offload"]
    else:
        args = ["-ngl", "0", "--device", "none", "--no-kv-offload", "--no-op-offload"]
    port = free()
    cmd = [f"{ENG}/llama-server", "-m", MODEL, "--host", "127.0.0.1", "--port", str(port), "-c", "2048", "-t", "4",
           "-np", "1", "-ctk", "f16", "-ctv", "f16", "--no-webui", "--offline", "--fit", "off", "-lv", "4", *args, *extra]
    procs.append(subprocess.Popen(cmd, stdout=logs, stderr=logs, env=ENV))
    try:
        wait_http(port)
        res = {"cmd": cmd, "prompts": [complete(port, p["text"], 65) for p in PRE["prompts"]]}
    finally:
        for p in procs:
            p.terminate()
        for p in procs:
            p.wait(10)
    json.dump(res, open(os.path.join(OUT, f"{name}.json"), "w"), indent=0)
    print(name, "done", flush=True)

for name, extra, rpc in [("split_direct_norepack_t4", ["--no-repack"], True)]:
    run(name, extra, rpc)
