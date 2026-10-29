# SINGLE-HOST S1-M diagnostic (2026-10-29), copied from the session scratchpad; run from the repository root.
import json, os, socket, subprocess, sys, time, urllib.request
ENG = ".ucompute/backends/llama.cpp/b11609"
MODEL, OUT = sys.argv[1], sys.argv[2]
def free():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p
rp, port = free(), free()
rl, ll = open(os.path.join(OUT, "sched-rpc.log"), "w"), open(os.path.join(OUT, "sched-llama.log"), "w")
r = subprocess.Popen([f"{ENG}/ggml-rpc-server", "-H", "127.0.0.1", "-p", str(rp), "-t", "6", "-d", "CPU"], stdout=rl, stderr=rl,
                     env={"PATH": "/usr/bin:/bin", "GGML_RPC_NO_RDMA": "1", "GGML_RPC_DEBUG": "1"})
time.sleep(2)
l = subprocess.Popen([f"{ENG}/llama-server", "-m", MODEL, "--host", "127.0.0.1", "--port", str(port), "-c", "2048", "-t", "4",
     "-np", "1", "-ctk", "f16", "-ctv", "f16", "--no-webui", "--offline", "--fit", "off", "-lv", "4", "--rpc", f"127.0.0.1:{rp}",
     "--device", "RPC0", "--tensor-split", "1", "--split-mode", "layer", "-ngl", "13", "--no-op-offload"],
     stdout=ll, stderr=ll, env={"PATH": "/usr/bin:/bin", "GGML_RPC_NO_RDMA": "1", "GGML_SCHED_DEBUG": "2"})
try:
    for _ in range(200):
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2); break
        except Exception: time.sleep(0.3)
    b = {"prompt": "Hello there, how", "n_predict": 4, "temperature": 0, "cache_prompt": False}
    urllib.request.urlopen(urllib.request.Request(f"http://127.0.0.1:{port}/completion", json.dumps(b).encode(),
                                                  {"Content-Type": "application/json"}), timeout=120).read()
finally:
    l.terminate(); r.terminate(); l.wait(10); r.wait(10)
