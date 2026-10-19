# Desktop to desktop: macOS app + Windows app on two physical machines (2026-10-19)

**Two-machine evidence.** Two installed desktop apps on two different computers, on one iPhone personal hotspot:

| | Mac | Windows laptop |
|---|---|---|
| Machine | MacBook Air, Apple M2, 8 GB, macOS 26.2 (172.20.10.2) | HP, Intel Core Ultra 7 258V, 32 GB, Arc 140V, Windows 11 Home 26200 (172.20.10.6) |
| App | `/Applications/Common.app` from 5f0c580 (daemon `3168e521…`, shell `c66c8c35…`, verified 2026-10-15/17) | `Common_0.1.0_x64-setup.exe` built natively on the laptop from caf1497 (sha256 `df13fe79e80a447d7e495a62cceae1ce6bcbbdbb0768daed6cf44889763a4ce0`, 4.24 MiB, NSIS), installed per-user |
| Daemon | `common serve --host 127.0.0.1 --port 8192 --exit-with-parent`, child of the app | same command line, `common.exe` child of `common-desktop.exe`, in the signed-in session |

## Windows installer and app

| Check | Result |
|---|---|
| `pnpm tauri build --bundles nsis` on the laptop (VS 2022 Build Tools 17.14, Rust 1.98.0 MSVC, Node 24, pnpm 10.34.4) | PASS: `build-sidecar: staged daemon accepts: serve --host 127.0.0.1 --port 8192 --exit-with-parent`, then the app and the NSIS installer |
| Silent install (`/S /D=…`), bundled `common.exe` accepts the app's exact arguments | PASS |
| App launched by the owner from the desktop | PASS: daemon healthy, started by the app with the exact arguments, session 1 |
| App force-killed (`Stop-Process -Force`, like a crash or Task Manager) | PASS: its daemon was gone 0.11 s later and port 8192 released (kill-on-close job object) |
| App launched over SSH (session 0) | Not possible: WebView2 cannot create a window outside a desktop session (`HRESULT 0x80070578`). A harness limit, not an app defect; automated launches use a scheduled task that runs only in the signed-in session |

## Pairing through the UI

- The owner paired the two apps from their screens only: Mac → Peers → **Pair this device**; Windows → Peers → **Nearby, not paired** → **Pair** → code → **Join**. Both apps then listed the other as **Connected**. Audit: Mac `pairing_succeeded` (initiator) and Windows `pairing_succeeded` (joiner) at 03:39:18 UTC.
- Wrong code, entered in the real Windows app (driven through its WebView2 DevTools port over SSH; the Mac's pairing window opened through its daemon API): the app showed **"Pairing did not complete. Check the code and that both devices are on the same network, then try again."** Screenshot: `img/windows-app-wrong-pairing-code-2026-10-19.png`.

## Running prompts on the other machine

| Direction | How | Result |
|---|---|---|
| Windows app → Mac | Real Windows window: group `home` with the Mac and "Members may run prompts" ticked; Chat → load stories15M → Run on **Mous** → send | 112 tokens, first token 2,325 ms (the Mac loaded the model on demand), 508 tok/s, "Ran on Mous". Mac audit: `remote_request` done, served on MTL0 (Apple M2). Screenshot: `img/windows-app-chat-ran-on-mac-2026-10-19.png` |
| Mac → Windows | Mac daemon API, the same request its Chat view sends (computer use was not available in this session to click the Mac window) | 48 tokens, engine TTFT 128 ms, 910 tok/s on the Arc GPU; Windows audit: `remote_request` done |

Both apps' Mac group was created through the Mac daemon API for the same reason; the Windows group through the Windows window.

## Defects found (open, not fixed in this run)

1. **Windows shows macOS wording**: "Run on: This Mac", "leaves this Mac", "Fits on this Mac", and the default device name "This Mac" (the Mac listed the laptop as "This Mac").
2. **Chat requires a model loaded on this machine even when running on another**: the input stays disabled ("Load a model first") until a local load, which defeats running a model that does not fit here.
3. **"Group home with 1 devices"** (plural).
4. **An unreachable paired peer is redialled every ~35 s forever** with an audit line each time (seen for an old pairing on the Mac).
5. **Paired device "Last seen"** does not advance while the connection stays up.
