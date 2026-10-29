# SINGLE-HOST S1-M diagnostic (2026-10-29), copied from the session scratchpad; run from the repository root.
"""SINGLE-HOST S1-M diagnostic: UCompute's configuration-C engine line, run directly, behind the Core Lab's
plaintext loopback counting proxy (rpc_dataflow.Proxy, imported read-only; -B writes no bytecode there).
No QUIC: the plaintext twin of the tunnelled run, with per-command and per-tensor counts."""
import json, os, socket, subprocess, sys, time, urllib.request
sys.dont_write_bytecode = True
sys.path.insert(0, "/Users/nasr/repos/ucompute-core-lab/.claude/worktrees/eloquent-williamson-2da66b/tools")
from rpc_dataflow import Proxy, free_port  # noqa: E402
ENG = ".ucompute/backends/llama.cpp/b11609"
MODEL, OUT = sys.argv[1], sys.argv[2]
PRE = json.load(open("tests/distributed/two_machine/split_prereg.json"))
env = {"PATH": "/usr/bin:/bin", "GGML_RPC_NO_RDMA": "1"}
rp, hp = free_port(), free_port()
log = open(os.path.join(OUT, "proxy_run.log"), "w")
procs = [subprocess.Popen([f"{ENG}/ggml-rpc-server", "-H", "127.0.0.1", "-p", str(rp), "-t", "6", "-d", "CPU"],
                          stdout=log, stderr=log, env=env)]
time.sleep(2)
px = Proxy(rp)
procs.append(subprocess.Popen([f"{ENG}/llama-server", "-m", MODEL, "--host", "127.0.0.1", "--port", str(hp), "-c", "2048",
    "-t", "4", "-np", "1", "-ctk", "f16", "-ctv", "f16", "--no-webui", "--offline", "--fit", "off", "-lv", "4",
    "--rpc", f"127.0.0.1:{px.port}", "--device", "RPC0", "--tensor-split", "1", "--split-mode", "layer", "-ngl", "13",
    "--no-op-offload"], stdout=log, stderr=log, env=env))
def comp(text, n):
    b = {"prompt": text, "n_predict": n, "temperature": 0, "seed": 42, "cache_prompt": False, "return_tokens": True}
    with urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{hp}/completion", json.dumps(b).encode(),
                                {"Content-Type": "application/json"}), timeout=300) as r:
        v = json.load(r)
    return v["tokens"], v["tokens_evaluated"]
def snap():
    time.sleep(0.2)
    s = px.snapshot()
    return {k: (dict(v) if hasattr(v, "items") else v) for k, v in s.items() if k != "shapes"}
try:
    for _ in range(400):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{hp}/health", timeout=2); break
        except Exception: time.sleep(0.3)
    comp("Hello", 1)
    rows = []
    for p in PRE["prompts"]:
        comp(p["text"], 1)  # warm-up: takes the prompt-cache save of the previous prompt
        c0 = snap(); comp(p["text"], 1); c1 = snap(); ids, pt = comp(p["text"], 65); c2 = snap()
        d = lambda k: ((c2[k] - c1[k]) - (c1[k] - c0[k])) / 64
        dd = lambda k: {x: ((c2[k].get(x, 0) - c1[k].get(x, 0)) - (c1[k].get(x, 0) - c0[k].get(x, 0))) / 64
                        for x in sorted(set(c2[k]) | set(c1[k]))}
        rows.append({"prompt": p["id"], "prompt_tokens": pt, "ids": ids,
                     "prefill": {k: c1[k] - c0[k] for k in ("c2s", "s2c", "switches")},
                     "decode_per_token": {k: d(k) for k in ("c2s", "s2c", "switches")},
                     "decode_cmd_bytes": dd("cmd_bytes"), "decode_cmd_count": dd("cmd_count"),
                     "decode_set_tensor_bytes": dd("tensors"), "decode_set_tensor_count": dd("tensor_count"),
                     "prefill_set_tensor_bytes": {x: c1["tensors"].get(x, 0) - c0["tensors"].get(x, 0) for x in c1["tensors"]}})
        print(json.dumps({k: rows[-1][k] for k in ("prompt", "prompt_tokens", "prefill", "decode_per_token")}), flush=True)
    json.dump({"shapes": px.snapshot()["shapes"], "rows": rows}, open(os.path.join(OUT, "proxy_run.json"), "w"), indent=1)
finally:
    for p in reversed(procs):
        p.terminate(); p.wait(10)
