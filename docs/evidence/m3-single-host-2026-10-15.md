# M3 SINGLE-HOST evidence run, 2026-10-15

> Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

**Every result here is SINGLE-HOST.** Both daemons ran on one Apple M2 MacBook Air (8 GB, macOS 26.2 arm64). This is not two-machine evidence, so it does not satisfy F-03-01, F-03-02 or Gate D (blocker B-002).

**Result: all 8 steps passed. Nothing was retried and no product code was changed.**

## Setup
- **Code under test:** HEAD `49f9148` plus 31 uncommitted modified files. The baseline `a146ba1` named in the task is an ancestor of HEAD. The run tests the working tree as it was built.
- **Build:** `cargo build -p ucompute-cli`. It finished in 9.69 s (debug profile; release builds were not allowed). I copied the binary to `m3-evidence/bin/ucompute` (sha256 `875a345c…0ad2ba1`) so that other agents rebuilding `target/` could not change it mid-run.
- **Two UCOMPUTE_HOMEs:** `m3-evidence/{a,b}`. In each:
  - `backends` is a symlink to `/Users/nasr/repos/ucompute/.ucompute/backends`.
  - `models/blobs/sha256-6696…1739` is a hard link to the existing stories15M-q4_0 blob (same APFS volume).
  - The manifest was copied.
  - Each home generated its own identity. Nothing was downloaded.
- **Daemons:**
  - A: `UCOMPUTE_HOME=$E/a $E/bin/ucompute serve --port 18301 --network --serve-peers --listen-port 18401 -v`
  - B: `UCOMPUTE_HOME=$E/b $E/bin/ucompute serve --port 18302 --network --serve-peers --listen-port 18402 -v`
  - `E=/private/tmp/claude-501/-Users-nasr-repos-ucompute/a9002575-f6f8-460a-ae76-955170ae1acf/scratchpad/m3-evidence`
  - The API listened on 127.0.0.1. The peer endpoint used the product default bind `0.0.0.0` (UDP 18401/18402). With the macOS firewall off, that port was reachable on the LAN at 192.168.1.176 during the run; it is QUIC and only accepts pinned keys.
- **Driver:** `M3_DIR=$E python3 -I tests/distributed/single_host/m3_evidence.py step1 … step7 | stop`. This is a new file and uses only the Python standard library. It logs every HTTP call (UTC timestamp, an equivalent `curl` line, status, response body, ms) to `$E/raw/<step>.jsonl`.
- **Peer ids:** A = `89a854ab…f2bf13213a` (89a8-54ab-41e2), B = `f33dfaf7…2ac4b4ce` (f33d-faf7-ec28).
- **Timeline:** daemons started at 14:45:47.889Z; daemons stopped at 14:47:56Z; the crate test ran 14:48:05–14:48:19Z.

## Step 1: discovery, F-02-01 (SINGLE-HOST)
| Expected | Observed | Result | Numbers |
|---|---|---|---|
| A's `GET /api/peers` lists B under `discovered` | Yes: B at `192.168.1.176:18402`, `trusted:false`, proto 1.0 | PASS | 1.00 s after both daemons were healthy (4.93 s after spawn) |
| B's `GET /api/peers` lists A under `discovered` | Yes: A at `127.0.0.1:18401`, `trusted:false` | PASS | ≤1.00 s after both were healthy. This is an upper bound: B was polled after A. |
| Daemon start | `/health` answered 200 | — | 3.93 s (A) and 3.93 s (B) from spawn, debug build |

Caveat: mDNS here is multicast loopback on one host. B resolved A through the `lo0` answer (127.0.0.1) and A resolved B through `en0`. This says nothing about a real LAN.

## Step 2: pairing (SINGLE-HOST)
| Expected | Observed | Result | Numbers |
|---|---|---|---|
| `POST /api/pair/start` on A, then `POST /api/pair/join` on B with the code: both pinned | join returned 200 with A's id; A's `pair/status` shows B; each side's `/api/peers` lists the other | PASS | join round trip **25.7 ms** (QUIC handshake + SPAKE2 + confirmation MAC + pin); both `connected:true` 1.107 s later (sync loop runs every 1 s) |
| One wrong code returns `pairing_failed` | HTTP 401 `pairing_failed` ("closed by peer (code 0x5046 pairing_failed)") | PASS | 16.4 ms |
| 6 wrong codes return `pairing_locked` | Attempts 1–5: 401 `pairing_failed`. Attempt 6: **429 `pairing_locked`**. A's `pair/status`: `pairing_locked`, "6 failed pairing attempts within 60 s; pairing closed". A's audit: `pairing_failed` with failures 1…6, then `pairing_locked`. | PASS | 8.4–16.4 ms per attempt |
| After the lock, even the correct code fails | 401 `pairing_failed` ("server refused to accept a new connection") | PASS | 1.4 ms |

## Step 3: group and privacy (SINGLE-HOST)
| Expected | Observed | Result |
|---|---|---|
| `PUT /api/groups {name:"evidence", members:[other], policy:{allow_remote_inference:true}}` on A and on B | 200 on both; `GET /api/groups` echoes the group | PASS |
| `PUT /api/privacy {mode:"trusted_group"}` on B without `acknowledged` is refused | 403 `policy_denied`; the message is the disclosure text | PASS |
| The same request with `acknowledged:true` | 200, mode `trusted_group` | PASS |

A stayed on `local_only` and still served B. Serving a peer depends on `serve_peers` plus the group, not on the server's own privacy mode.

## Step 4: remote execution from B to A, Mode B (SINGLE-HOST)
Request on B: `POST /v1/completions {model:"stories15M-q4_0", prompt:"Once upon a time, a little rabbit", max_tokens:32, temperature:0, target:"peer:<A hex>"}`.

| Run | Golden text | `target` echoed | Client total | Engine TTFT | Decode tok/s | Result |
|---|---|---|---|---|---|---|
| Cold (A loads the model) | exact match | `peer:89a854ab…` | 13 492.6 ms | 13 396.6 ms (A's `load_ms=12624`; Metal init of a fresh engine) | 450.1 | PASS |
| Warm | exact match | yes | **86.3 ms** | 4.77 ms | 419.2 | PASS |
| `stream=true` (`include_usage`) | exact match | every chunk carries it | 71.6 ms | engine 3.89 ms; **first text chunk seen by client at 4.8 ms** | engine 465.0; **client 467.6** | PASS |

The stream delivered 34 SSE chunks: 32 text chunks, the finish chunk and the usage chunk. Usage was 10 prompt tokens and 32 completion tokens on every run.

Audit:
- **A** has three `remote_request` entries: request_id, peer label, model id, token counts, duration_ms, `outcome:"done"`.
- **B** has `sent`/`finished` pairs with the same fields.
- Neither audit contains the prompt, "little rabbit", "Benny" or "radish". The leak scan over 17 files (both daemon logs, both homes' audit, config, peers and engine logs) found **0 hits**. **PASS**

## Step 5: B set to `local_only` (SINGLE-HOST)
| Expected | Observed | Result |
|---|---|---|
| The same request is refused with `policy_denied` | 403 `policy_denied` "your privacy settings require local execution" in 0.6 ms | PASS |
| A's audit shows no new `remote_request` | 3 before, 3 after | PASS |
| B writes no `sent` entry | B still has 6 `remote_request` entries | PASS |

B was then set back to `trusted_group` (acknowledged), so that step 6 tests revocation rather than privacy.

## Step 6: revocation (SINGLE-HOST)
| Expected | Observed | Result | Numbers |
|---|---|---|---|
| `POST /api/peers/<A>/revoke` on B | 200, signed revocation seq 1; B's audit shows `peer_disconnected {connections:2, reason:revoked}` and `peer_revoked` | PASS | 7.9 ms |
| B refuses `target: peer:<A>` with `peer_revoked` | 403 `peer_revoked` "device 89a8-54ab-41e2 was revoked" | PASS | 0.5 ms |
| A's connection to B closes within 5 s | A's `/api/peers` showed B `connected:false` at the first poll after the revoke call returned | PASS | **≤0.01 s** |
| A's reconnects keep failing inside the handshake | A redialled at 14:46:57, :58, 47:00, :04 and :12 (backoff 1, 2, 4, 8 s). A logged `peer_rejected reason=peer_revoked` each time. B logged `peer_rejected` "the peer's key is revoked (… cryptographic handshake failed: error 44)" each time. Still disconnected 6 s later. | PASS | — |

## Step 7: disconnect detection (SINGLE-HOST)
Before this step, B re-paired with A (join 200) and re-added A to its group. Both sides showed `connected:true` 0.666 s later.

| Expected | Observed | Result | Numbers |
|---|---|---|---|
| SIGKILL A's daemon (pid 29146, at 14:47:21.284Z); B's `/api/peers` shows A disconnected within 6 s (+1 s) | B: `connected:false`; B logged "peer connection ended code=peer_lost" at 14:47:26.137Z | PASS | **4.855 s** (polled every 50 ms) |
| Restart A and it reconnects | A was ready (`/health`) 3.674 s after spawn. It stopped its orphaned engine (pid 29446) at start: log line `stopped orphaned llama-server engines … pids=[29446]`. Both sides showed connected 3.681 s after spawn. | PASS | ≈7 ms after A became ready |
| (extra) remote request after reconnecting | 200, golden text, correct target | PASS | 1048 ms (engine reload `load_ms=222`) |

## Step 8: packet capture
`tcpdump` needs sudo, so I did not run it. Instead:

`UCOMPUTE_HOME=/Users/nasr/repos/ucompute/.ucompute UCOMPUTE_REQUIRE_FIXTURES=1 cargo test -p ucompute-network --test remote_single_host -- --nocapture`

Exit 0. Verbatim output:
```
running 10 tests
test peer_supplied_grammar_refused_before_load ... ok
test local_only_privacy_never_leaves_device ... ok
test unauthorized_peer_is_refused ... ok
bench:transport-overhead SINGLE-HOST loopback decode TPOT (median of 5, 64 tokens, stories15M-q4_0): local 2.289 ms, remote 2.278 ms, ratio 0.995 (overhead -0.5 %); local runs [2.275312ms, 2.288917ms, 2.291884ms, 2.263019ms, 2.320307ms], remote runs [2.290219ms, 2.305284ms, 2.271031ms, 2.278496ms, 2.275194ms]
test decode_tpot_overhead_measured_single_host ... ok
bench:transport-overhead SINGLE-HOST loopback QUIC, 32 tokens: client tx 2584 B (21 datagrams), rx 6792 B (39 datagrams), 212 B/token rx, 81 B/token tx, client ttft 861.9 ms, total 932.11175ms
test remote_golden_text_single_host ... ok
SINGLE-HOST: peer loss mid-stream surfaced as PeerLost after 152.665583ms
test peer_lost_mid_stream_is_typed ... ok
SINGLE-HOST cancel latency: client stream ended 10.917µs, server released the model 6.598292ms after cancel (bound 500 ms)
test remote_cancel_stops_server_generation_within_500ms ... ok
bench:abuse-flood SINGLE-HOST: local p95 38.3 ms baseline vs 42.7 ms under a peer flood (ratio 1.116, +11.6 %); peer requests served 24, refused overloaded 168, other []; local median 38.0 -> 39.0 ms
test peer_flood_throttled_local_p95_within_20pct_single_host ... ok
bench:transport-overhead SINGLE-HOST relay capture, whole session incl. handshake, 32 tokens: client→server 11638 B in 33 datagrams, server→client 12150 B in 50 datagrams (380 B/token server→client); 0 plaintext hits over 11 needles (8 per-token: ["around", "excited", "grass", "happening", "looking", "pped", "something", "what"])
test packet_capture_contains_no_plaintext_prompt_single_host ... ok
SINGLE-HOST streaming: 32 token events, first at 861.900417ms, Done at 931.62675ms
test remote_streaming_delivers_before_done ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 13.40s
```
This is an in-process UDP relay capture, not a kernel packet capture. **PASS**

## Daemon RSS, MB (SINGLE-HOST)
| Point | A `ucompute` | A `llama-server` | B `ucompute` |
|---|---|---|---|
| After start, idle | 18.2 | — | 18.0 |
| After step 4 (model loaded, 3 remote runs) | 16.4 | 93.5 | 15.0 |
| Before SIGKILL | 18.1 | 63.3 | 17.5 |
| End (A restarted, 1 remote run) | 21.0 | 112.5 | 17.7 |

## Shutdown
- SIGTERM: A exited in 0.21 s, B in 3.08 s.
- Afterwards nothing listens on 18301, 18302, 18401 or 18402 (`lsof` empty), and no process under `$E` remains.
- The driver's "leftover_processes" list contains only the driver's own shell, because it matches `$E` in its own command line.

## Things the run surfaced
1. **Each daemon lists itself in `discovered`.** `GET /api/peers` includes the node's own id, at 127.0.0.1. Either the API or the desktop UI should filter out the node's own id.
2. **Loopback addresses can end up in discovery.** On one host, B first recorded A at `127.0.0.1:18401`, because the answer over `lo0` arrived first. `discovery::pick_addr` picks the lowest IPv4 address, so if a LAN record ever carried 127.0.0.1, a remote machine would dial its own loopback. Suggested hardening: skip loopback addresses in `pick_addr`. This was not verified on two machines.
3. **Two QUIC connections per pair.** Both sync loops dial each other, so there is one inbound and one outbound connection (B's audit: `peer_disconnected {connections:2}`). It works, but the duplicate costs keep-alive traffic and an extra `serve_peer` task.
4. **Denied requests are not audited.** A request refused by `policy_denied` writes no audit entry on B. This matches the rule that only off-device actions are audited; noted for F-06-10 reviewers.
5. **Cold remote TTFT is engine startup.** The 13.4 s is the engine starting on A (`load_ms=12624`, first Metal init). It is not transport cost: reload after restart took 222 ms, and warm TTFT is about 4–5 ms.
6. **Odd engine log line.** It reads `offloaded=7 total=6`, meaning more layers offloaded than exist. This is a cosmetic counting issue in ucompute-runtime and outside this task.
7. **Leftover engine from the desktop app.** Before the run, an orphaned `llama-server` (pid 23456, ppid 1, running 19 h, qwen2.5-0.5b) from the desktop app's data directory `~/Library/Application Support/UCompute` was holding memory. I left it alone. The desktop app's orphan reaper should stop it on its next start.

## Not done or unverifiable here
- Anything that needs two physical machines: real-LAN mDNS, real network latency, Gate D, F-03-01 and F-03-02 (B-002).
- A kernel-level packet capture, which needs sudo; the in-process relay capture above stands in for it.
- These results are evidence only. The ledger rows should stay at most `IMPLEMENTED_UNVERIFIED` until two-machine runs exist.
- No signature changes, no new dependencies, no git operations.


- The driver script is the only repo change: `/Users/nasr/repos/ucompute/tests/distributed/single_host/m3_evidence.py` (untracked, new directory).
- Raw outputs are outside the repo, under `$E/raw/`: `00-build.txt`, `00-git-state.txt`, `step1.jsonl` … `step7.jsonl`, `step5-b-audit-check.txt`, `step6-audit-tail.txt`, `stop.jsonl`, `daemon-a.log`, `daemon-b.log`, `08-cargo-test.txt`.
- Node state (identity, peers, config, audit, engine logs) is in `$E/a` and `$E/b`; peer ids and timestamps are in `$E/state.json`.
