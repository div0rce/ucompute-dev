# Mac and Windows laptop on eduroam: connect at address and remote run (2026-10-22)

> Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

**Two-machine evidence.** This Mac and the Windows laptop on the Rutgers eduroam network, which drops multicast between clients, so mDNS discovery cannot work there (`docs/evidence/m3-two-machine-2026-10-18.md`). This is the case the connect-at-address fallback (`POST /api/peers/{id}/connect` with an address, added in 6ab9f8b) exists for.

| | Mac | Windows laptop |
|---|---|---|
| Machine | MacBook Air, Apple M2, 8 GB, macOS 26.2, 10.75.79.236 | HP, Core Ultra 7 258V, Arc 140V, Windows 11, 10.75.120.9, network profile Private |
| App | Desktop UI 2.0 packaged app from d9d8067 (daemon `623876ae…`, verified ALL PASS, `desktop-ui2-packaged-mac-m2.json`) | older installed app built from caf1497 (does not have the connect route; only the dialling side needs it) |

The laptop's SSH host key at the new address was compared with the one already known for it (`SHA256:Tpf9ki…`, identical) before the address was trusted.

| Check | Result |
|---|---|
| Before: the Mac's pinned address for the laptop was its old hotspot address | `last_error`: "no answer from 172.20.10.6:8193 within 4 s"; 0 nearby devices (no mDNS on this network) |
| `POST /api/peers/<laptop>/connect {"addr":"10.75.120.9:8193"}` on the Mac | **200 in 0.166 s**, connected; the pinned address became `10.75.120.9:8193` only after the laptop presented the key pinned at pairing (the transport's pinned-key check; an address is a hint) |
| Remote run, Mac → laptop, `stories15M-q4_0`, 32 tokens, temperature 0 | **golden completion**, `target: peer:1a54602a…`; first token 2.5 s (the laptop loaded the model on demand), 509 tok/s decode on the laptop (fixture: a path check, not performance); the Mac's new jobs list shows the request `done` with the peer target and 32 tokens |

### How each number above was produced (raw output not saved)

The raw API responses of this run (20:48-20:50 UTC) were not saved: no raw log, results file or console capture exists. The numbers above are the summary lines the commands printed, copied from the session transcript. The commands ran on the Mac against its installed app's API, exactly as below except that the laptop's 64-hex peer id is shortened here (`HP=1a54602a…`):

```sh
# host key at the new address (before trusting it): both print SHA256:Tpf9ki…
ssh-keygen -F jess-hp.local -l
ssh-keyscan -T 10 -t ed25519 10.75.120.9 > hp-ip.key; ssh-keygen -l -f hp-ip.key

# before, connect at address, after (20:50:18 UTC)
HP=1a54602a…; curl -s 127.0.0.1:8192/api/network | python3 -c "import json,sys; d=json.load(sys.stdin); print({k:d.get(k) for k in ['device_name','dial_addr','enabled','mdns']})"; curl -s 127.0.0.1:8192/api/peers | python3 -c "
import json,sys; d=json.load(sys.stdin)
for p in d['peers']: print('PEER', p['name'], p['short_label'], 'connected=',p['connected'], p.get('health'), 'last_addr=',p.get('last_addr'), 'err=',(p.get('last_error') or {}).get('message'))
print('nearby:', len(d['discovered']))"; echo "--- connect at address"; curl -s -m 30 -X POST 127.0.0.1:8192/api/peers/$HP/connect -H 'Content-Type: application/json' -d '{"addr":"10.75.120.9:8193"}' -w '\nHTTP %{http_code} in %{time_total}s\n' | python3 -c "
import sys,json; raw=sys.stdin.read(); body,_,tail=raw.rpartition('\nHTTP'); d=json.loads(body); print({k:d.get(k) for k in ['name','last_addr','last_seen_utc']} if 'error' not in d else d['error']); print('HTTP'+tail.strip())"; curl -s 127.0.0.1:8192/api/peers | python3 -c "
import json,sys; d=json.load(sys.stdin)
for p in d['peers']:
  if p['short_label'].startswith('1a54'): print('after:', p['connected'], p.get('health'), p.get('last_addr'), (p.get('connection') or {}).get('remote_addr'))"

# remote run and the jobs list (20:50:32 UTC)
HP=1a54602a…; curl -s -m 120 -X POST 127.0.0.1:8192/v1/completions -H 'Content-Type: application/json' -d "{\"model\":\"stories15M-q4_0\",\"prompt\":\"Once upon a time, a little rabbit\",\"max_tokens\":32,\"temperature\":0,\"target\":\"peer:$HP\"}" | python3 -c "
import sys,json; d=json.load(sys.stdin)
if 'error' in d: print('ERROR', d['error'])
else:
  t=d.get('timings') or {}; print('target=',d.get('target'),'| golden=', d['choices'][0]['text']==' named Benny was hopping in a field. He saw a big, red radish and he wanted to eat it. But the radish was too big', '| device=',t.get('device'),'| ttft_ms=',round(t.get('time_to_first_token_ms') or 0),'| tok/s=',round(t.get('decode_tokens_per_second') or 0,1))"; curl -s 127.0.0.1:8192/api/jobs | python3 -c "import json,sys; d=json.load(sys.stdin); r=(d['recent'] or [{}])[0]; print('job:', {k:r.get(k) for k in ['state','target','model_id','completion_tokens','duration_ms']})"
```

What they printed, number by number:
- Before: the laptop's row `connected= False disconnected last_addr= 172.20.10.6:8193 err= no answer from 172.20.10.6:8193 within 4 s`, and `nearby: 0`. The Mac's network status: `enabled: True, mdns: True, dial_addr 10.75.79.236:8193`. The laptop's row was still named "This Mac" here: the name the caf1497 build gave itself at the 2026-10-19 pairing.
- Connect: `HTTP200 in 0.165779s` (curl's `time_total`) and a record with `last_addr 10.75.120.9:8193`; after: `True connected 10.75.120.9:8193 10.75.120.9:8193` (connected, health, `last_addr`, `connection.remote_addr`).
- Remote run: `golden= True` is the comparison of `choices[0].text` with the fixture's known 32-token continuation in the command; `ttft_ms= 2535` and `tok/s= 508.9` are the response's `timings.time_to_first_token_ms` and `timings.decode_tokens_per_second`; `device= None` (not reported for a request run on another device).
- Jobs: `recent[0]` was `state: done`, `target: peer:1a54602a…`, `model_id: stories15M-q4_0`, `completion_tokens: 32`, `duration_ms: 2540`.

### Not covered by this run

Pairing in both directions, revocation, privacy, disconnect/hold, quit/crash cleanup and network change are not covered here. The next run, `tests/distributed/two_machine/run_mac_windows.sh`, is a **daemon API run, not an app UI run**: it pairs and checks through the two daemons' HTTP APIs (its drivers print "daemon API run (not app UI)" first and store it as `driven` in `results.json`), so it is never evidence for a step in an app's window. These app UI steps each need their own evidence, done by hand in the apps' windows with no API or DevTools substitute:
1. The Windows firewall consent step: done (next section).
2. Pairing started in each app: started in the Windows app, done (next section); started in the Mac app (UI 2.0), open.
3. A wrong pairing code typed by hand: open.
4. The Mac-side group and run steps (a group that allows prompts, then a run on the laptop from the Mac app's Chat): open.
5. The Windows app's own checks (plan §5.5 step 5: screenshots, keyboard, NVDA): open.

## Windows firewall consent step and pairing started on Windows (2026-10-22, 21:28-21:29 UTC)

The Windows installer built natively on the laptop from 593d4ea (`UCompute_0.1.0_x64-setup.exe`, sha256 `c8af273d467c38254fc96037d27dcc6cfdc115adf98f1e248fc4ab61c0ae05c0`, 4,608,282 bytes) was installed silently into `C:\Users\jessg\UComputeVerify-593d4ea\app` by `apps/desktop/scripts/verify_windows_app.ps1`, with a fresh data folder (new device identity `b79212f0…`). While its window was open in the laptop's desktop session, the owner's side used both apps by hand (these steps were not automated):

- **Firewall consent step:** "Allow on private networks" was pressed and the Windows elevation prompt approved at the laptop. The resulting rule, read with `netsh advfirewall firewall show rule name="UCompute (paired devices)" verbose`: Enabled Yes, Direction In, Profiles **Private**, Protocol **UDP**, Program **`C:\Users\jessg\UComputeVerify-593d4ea\app\ucompute.exe`**, Action Allow. Exactly the scope the step promises; nothing broader.
- **Pairing started on Windows** (the direction Windows Firewall dropped on 2026-10-19, pktmon `receive inspection`): the laptop's audit log records `pairing_started` (port 50969) at 21:28:41 and `pairing_succeeded` with this Mac (`89839a1e…`, role initiator, addr 10.75.79.236:8193) at 21:29:09, then `peer_connected` outbound and, after Sharing was turned on, inbound. **The direction that failed is fixed on real hardware.**
- **Device name:** this Mac lists the laptop as "JESS-HP" (its computer name), not "This Mac".

The automated verifier itself stopped at its first launch check, because the app was being used by hand at the same time (its measurements need an untouched app); it is re-run on the next build. Networking was on in that fresh install only because it was turned on by hand (audit `network_changed` at 21:28:15); the default stays off.
