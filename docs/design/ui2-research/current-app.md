# Current desktop app inventory (input to Desktop UI 2.0)

Source: `apps/desktop` at HEAD e76e7fc plus the uncommitted working tree (2026-10-19). Read-only survey; no build run.
"WT" marks things that exist only in the working tree (in-flight discovery/desktop fixes) and not in HEAD.

## 1. Snapshot

- Stack: Tauri 2 shell + React 19 + TypeScript 6 (strict, `noUnusedLocals/Parameters`) + Vite 8; Vitest 5 + jsdom + Testing Library + axe-core. Runtime deps are only `react`, `react-dom`, `@tauri-apps/api`, `@tauri-apps/plugin-opener`. No router lib, no state lib, no CSS framework, no icon set, no component library, no chart lib.
- Size: ~5.7k lines in `src/` (848 CSS, 695 Peers.tsx, 435 client.ts, 424 mocks.ts; every other file under 270).
- Layout of `src/`: `main.tsx`, `App.tsx` (shell + nav), `router.ts` (hash router), `daemon.ts` (daemon discovery, polling, Tauri invokes), `format.ts`, `api/client.ts` (the only module that talks HTTP), `api/errors.ts` (plain-language error table), `state/store.ts` + `state/stores.ts`, `components/` (ErrorNotice, FitBar, Groups, Later, RunOn), `views/` (Home, Models, Chat, Hardware, Performance, Diagnostics, Peers, Privacy, Settings), `test/`, one `styles.css`.
- Visible brand everywhere is "Common" (window title, `<title>`, sidebar brand, productName). UCompute naming (D-018) has not reached the UI; identifier `org.commoncompute.desktop`.
- WT changes in flight (19 files, +647/-136): Peers connection-state UX (`connState`, `promptReadiness`, Connect panel, `last_error`, discovered `addrs`, `dial_addr`, device name from daemon), Windows Firewall consent notice (`firewall.rs` + `firewallStatus`/`allowThroughFirewall`), Chat can run on a peer with nothing loaded locally (remote model picker), "this Mac" wording removed (fits `docs/evidence/desktop-two-machine-2026-10-19.md` defects 1-3), `plural()`, `/api/shutdown` used by the shell on Windows. New tests: `devices.test.tsx`, `firewall.test.tsx`.

## 2. Shell, navigation, routing

- `App.tsx`: CSS grid `216px | 1fr`. Left `<aside class="sidebar">`: brand (teal square + "Common"), privacy mode pill (link to `#/privacy`, green "Local only" / amber "Trusted devices", "Privacy mode unknown" until fetched), `<nav aria-label="Main">` with grouped links, footer with status dot + "Service running 0.1.0" / "Service not running" / "Connecting". Right: `<main id="main" tabIndex=-1>` renders one view. Skip link "Skip to content" first in DOM.
- Nav groups (labels as shown): **Run**: Home, Models, Chat. **This computer**: Hardware, Performance, Diagnostics. **Devices and network**: Peers, Cluster (M3, placeholder), Jobs (M3, placeholder), Contributions (M7, placeholder), Privacy. Ungrouped: Settings. Placeholders render `components/Later.tsx` ("Not available yet (milestone M3)" + one sentence); no fake data.
- `router.ts`: hash router, 12 routes (`home models chat hardware performance settings diagnostics peers cluster jobs contributions privacy`), unknown hash falls back to `home`. `href(r)` = `#/r`, `navigate(r)` sets the hash. Query after `?` is ignored by the router but read by views: `#/models?confirm=<id>` opens the download confirmation (Home's Download button uses it).
- No per-item routes (no `#/peers/<id>`), so no deep link to a selected device/model/job. No back/forward handling beyond the browser hash.
- App effects: on `daemon.reachable` becoming true it fetches `GET /api/privacy` into the `privacy` store (so the pill is always the daemon's mode, never cached). `startDaemonPolling()` unless `poll={false}` (tests).
- Responsive: under 760 px the sidebar becomes a wrapping top bar (`.nav { display: contents }`, group labels hidden). Window min width is 720, so this only bites in a browser dev tab.
- No top bar, no status bar, no toolbar shell, no inspector pane, no command palette, no keyboard shortcuts beyond native Tab/Enter, no window menu defined in code (`lib.rs` sets none).

## 3. State

`state/store.ts`: 33-line `createStore(initial, storageKey?)` on `useSyncExternalStore`; optional localStorage mirror wrapped in try/catch; `{get, set, subscribe, use}`. Reusable as is.

`state/stores.ts`:
| store | persisted | contents / use |
|---|---|---|
| `settings` | `localStorage["common.settings"]` | `device` (auto/cpu/gpu), `n_ctx` (null = model default), `device_name` ("" = daemon's hostname; WT migrates a stored "This Mac" to "") |
| `daemon` | no | `reachable` (null = unchecked), `version`, `status` (`/api/status`), `spawned` (shell started it) |
| `chatHistory` | `localStorage["common.chat"]` | one conversation: `{role, content, model?, usage?, timings?, stopped?, target?, ran_on?}[]` |
| `runs` | no (session only) | per-reply `{at, model, completion_tokens, ttft_ms, decode_tps, stopped}`; newest first; written only by Chat |
| `lastError` | no | last `MappedError`, shown on Home as "Last problem" |
| `privacy` | no | `{mode, disclosure}` from daemon |
| `runTarget` | no (deliberately) | `"local"` or `"peer:<id>"`; reset to local in local-only mode or when the peer disappears |

Everything else is component-local `useState`: model list, download progress/busy, pairing session, peers, groups, network state, hardware. Leaving a view drops it (see debt).

Polling/refresh cadence (no push channel exists besides the two SSE streams):
- `daemon.ts startDaemonPolling`: `GET /api/status` every 2.5 s (also the reachability probe).
- Peers: `GET /api/network` + `/api/peers` + `/api/groups` on mount and every 5 s while networking is on; `GET /api/pair/status` every 1 s while a pairing code is shown, plus a 1 s countdown tick.
- RunOn: `GET /api/peers` on mount and on `<select>` focus (no polling).
- Others fetch once on mount / when `reachable` flips; Performance refetches `/metrics` when `runs.length` changes.

## 4. API client (`api/client.ts`) and errors

- Single module, typed interfaces mirror daemon JSON: `Status`, `Loaded`, `Hardware`, `Model` (+`Fit`, `Estimate`), `PullEvent`, `ChatResult`/`Usage`/`Timings`, `NetworkState`, `PeerRecord`, `Peer` (+`health?`, WT `last_error?: DialError`), `Discovered` (WT `addrs?`), `PairingSession`/`PairingStatus`, `Group`, `Privacy`, `AuditEvent`, `Target`. Helpers: `expiresAtMs`, `shortLabel`, `pairingAddr`, `sseData` (SSE parser), `parseMetrics` (Prometheus text), `configure({baseUrl, token})`, `getBaseUrl`.
- Errors: every non-2xx becomes `ApiError(code, message, hint, status)`; fetch failure becomes code `unreachable`; abort passes through. `api.chat` turns an in-stream `{error}` chunk into a typed `ApiError` (never an empty reply). `errors.ts` maps 26 stable codes to `{title, next_step}`; `errors.test.ts` parses `crates/common-core/src/error.rs` and fails on drift. Reuse verbatim.
- Calls and who uses them:

| call | endpoint | used by |
|---|---|---|
| status | GET /api/status | daemon.ts poll, Diagnostics fallback |
| hardware | GET /api/hardware | Home, Hardware, Diagnostics fallback |
| models | GET /api/models | Home, Models, Chat |
| pull | POST /api/models/{id}/pull (SSE progress) | Models |
| load | POST /api/models/{id}/load {device, n_ctx} | Home, Models, Chat |
| remove | DELETE /api/models/{id} | Models |
| metrics | GET /metrics | Performance |
| chat | POST /v1/chat/completions stream + `stream_options.include_usage`, `target` only for peers | Chat |
| network / setNetwork | GET/PUT /api/network | Peers |
| peers | GET /api/peers | Peers, RunOn |
| connectPeer (WT) | POST /api/peers/{id}/connect {}/{addr} | Peers |
| removePeer / revokePeer | DELETE /api/peers/{id}, POST .../revoke | Peers |
| pairStart / pairStatus / pairJoin | /api/pair/* | Peers |
| groups / putGroup | GET/PUT /api/groups | Peers (Groups) |
| privacy / setPrivacy | GET/PUT /api/privacy | App, Privacy |
| audit | GET /api/audit (last 100, oldest first) | Diagnostics |
| diagnostics | GET /api/diagnostics | Diagnostics (route does not exist in the daemon; 404 triggers the status+hardware fallback) |
| health, unload, importModel | /health, POST /api/unload, POST /api/models/import | **no UI** |

- Daemon routes with no client at all: `/ready`, `/openapi.json`, `/v1/models`, `/v1/models/{id}`, `/v1/completions`, `/v1/embeddings`, `POST /api/shutdown` (WT; used by the Tauri shell on Windows, not the webview).
- Pull has no cancel route; the client passes no AbortSignal from Models. Chat stop = client abort (daemon cancels on disconnect; remote outcome audited as `cancelled`).

## 5. Views today

### Home (`#/home`)
Purpose: onboarding landing. Fetches hardware + models once reachable. Shows: one-line "glance" status (`role=status`: connecting / not running / "<model> is loaded on <device>" / "No model is loaded" + chip, memory, first GPU backend); RunOn selector (only outside local-only); 3-step "journey" (Choose a model, Run, Chat) with `aria-current="step"`; "Recommended for this computer" hero (first catalog model tagged `recommended`) with Run / Download (deep-links to Models confirm) / Open chat and its FitBar; "Last problem" ErrorNotice from `lastError`. Run loads with Settings' device/n_ctx then navigates to Chat.
Redesign: becomes Dashboard; the journey and hero are decorative and only valuable for first run.

### Models (`#/models`)
Catalog table (Model, Size, License, Memory, actions) with client-side text search over name/id/family/quant/license/tags and a Refresh button. Row badges: Recommended, Loaded, Downloaded. Memory column: fit sentence + FitBar (weights / KV / buffers / overhead vs available). Actions: Download (inline confirmation shows license, size, source, redistribution, "View license" opens system browser via opener plugin, focus moves to the confirm button), live `<progress>` with "x of y, resumed from z", Run (load, then navigate to Chat; focus lands on Run after download), Open chat, Delete (inline confirm; "catalog entry stays"). Per-row ErrorNotice + `lastError`. No sort, no column choice, no Unload, no Import GGUF (the `model_not_found` text even tells users to import one), no per-device (peer) model view, no "where loaded" beyond this computer.
Must keep: license-before-download confirmation, resumable progress text, fit explanation, keyboard focus hand-offs, `?confirm=` deep link (or an equivalent).

### Chat (`#/chat`)
Grid `head | log | composer`, `max-width 820px`, height `calc(100vh - 76px)`. Head: context line (remote target / loaded model + device + fallback reason + n_ctx / not running / nothing loaded), RunOn, model `<select>`: local mode picks among downloaded models + "Load model" button (only when nothing is loaded); remote mode (WT) offers every model this daemon knows, nothing needs loading locally. Log `role=log` with user bubbles (right, accent-soft) and assistant blocks (left rule), streaming text with block cursor, footer "Stopped. / N tokens / first token in X ms, Y tok/s / Ran on <device>". Visually-hidden status "Generating a reply" / "Reply complete". Composer: textarea (Enter sends, Shift+Enter newline), Send / Stop (danger) / Clear. Remote `model_not_downloaded` is rewritten to "<device> does not have <model>". Each reply appends to `runs`.
Gaps: single conversation only, no system prompt, temperature, max_tokens, no copy/regenerate, no model switching while loaded (must unload elsewhere, and there is no Unload UI), history in localStorage only.
Must keep: incremental streaming, Stop within 300 ms (tested), stopped flag + partial content saved, local-only enforcement in Chat itself (not just hiding RunOn), "Ran on" attribution, history local only.

### Hardware (`#/hardware`)
`GET /api/hardware` once. `dl.kv` for chip, OS, processor (cores/threads, WT pluralised), memory (raw bytes + human, unified note), free disk, probe time; GPU table (name, vendor, backend, cores, memory); backends table (available Yes/No + reason); CPU feature list; raw JSON `<details>`. Values intentionally match `common hardware --json` (F-10-04). This computer only; nothing for paired devices (the wire `Capabilities` message carries `hardware_json`, free memory, engines, cached model hashes, but no API route exposes it).

### Performance (`#/performance`)
"Last run" kv and "Recent runs" table (20) from the session-only `runs` store (Chat replies only, local or remote, without target column), plus "Service counters": filterable table of every `/metrics` series (default filter "token"). Nothing estimated. This is the closest thing to a job history today.

### Diagnostics (`#/diagnostics`)
Background service kv (Running/Not running, base URL with dev `?daemon=` note, started by app or not, version, uptime, loaded model with backend/version/device/offload layers/load ms, queue inflight/queued/limits). "Copy diagnostics" builds JSON (app info + redacted bundle, falling back to status + hardware) into the clipboard, says "never your messages". "Off-device activity": audit table (When, Event code, Device short label, Details JSON), most recent first; handles not_implemented. "What error messages mean": full error-code table.

### Peers (`#/peers`, 695 lines WT; F-10-05)
One long page, sections in order:
1. **Networking**: `role=switch` checkbox "Let this computer talk to paired devices" (turning off also clears serve), "Run models for paired devices when they ask" (disabled while off), help text with listen addr and mDNS note; Windows Firewall notice (WT) when supported, networking on and no rule.
2. **This device**: editable name (placeholder = daemon `device_name`), short label, full id grouped 8x8 hex, WT `dial_addr` with help.
3. **Pair this device** / **Join a device** two-panel grid: start shows a 56 px six-digit code ("482 913"), countdown, `pair_addr` (select-all), QR payload as text in `<details>`, polls status, outcome text; join form takes address + code or a pasted `common-pair:v1:` payload, maps `pairing_failed` with a hint to "check the other device's firewall", success notice "Add it to a group".
4. **Paired devices** table: Device (name + short label; inline revoke/remove confirm; WT inline Connect form with optional address), State (WT `connState`: Connected / Not responding / Connecting / Reconnecting / Unreachable / Offline / Not connected, with last dial error message + hint), Address (seen via mDNS with all addrs / last known / not known), Groups (+ WT `promptReadiness` sentence: what is missing for prompts to run there), Last seen, actions (Connect only while networking on and not connected, one dial at a time; Revoke; Remove). Focus returns to the opening button on cancel; outcome paragraph takes focus after connect.
5. **Groups** (component): per-group fieldset, member chips with remove, add-device select, two policy checkboxes ("run prompts for each other", "pool memory for one model"), create group form.
6. **Nearby, not paired**: strangers from mDNS (self id filtered out, paired ids excluded), only action "Pair" (prefills Join with `pairingAddr`), explicit empty-state reasons (networking off, discovery off, network blocks discovery).
Invariants encoded (keep): an mDNS record never makes a device connected or ready; only `connected` counts; this device never listed as nearby; unpaired devices get no action except Pair; connect address is only a hint (key decides); compare full id, short label is not identity.

### Privacy (`#/privacy`, F-10-10)
Current-mode glance; radio cards Local only / Trusted devices / Public compute (disabled, "Coming later"); leaving local-only shows daemon `disclosure` and requires an "I understand" checkbox before `PUT`; returning to local-only needs none; Cancel; status text. Mode pill in sidebar on every screen.

### Settings (`#/settings`)
Default device radios (Automatic / CPU / GPU) and context length, both client-side only (sent with each load, not stored by the daemon); read-only service address; pointers to Privacy and Peers; "contributing comes later". No storage location, no model directory, no API key/token entry (client supports `configure({token})` but nothing sets it), no theme choice, no about/version page.

### Placeholders
Cluster, Jobs, Contributions render `Later` notices.

## 6. Reusable pieces

- `api/client.ts` whole (types, `sseData`, `parseMetrics`, error shaping, target handling). `api/errors.ts` + its drift test.
- `state/store.ts` (tiny external store) and the stores above; `runTarget` safety semantics.
- `daemon.ts`: `discoverDaemon` (Tauri `daemon_info`, dev-only `?daemon=` loopback override compiled out in production), `refreshStatus`, `startDaemonPolling`, `openExternal`, `firewallStatus`/`allowThroughFirewall` (WT).
- `components/ErrorNotice` (title, next step, code: detail (hint), optional Dismiss; `role=alert`), `FitBar` (memory composition bar, `role=img` + label), `RunOn` + `targetName`, `Groups`, `Later`.
- Peers logic worth lifting out of the view as-is: `connState`, `promptReadiness`, `grouped`, `sentence`, `isPayload`, `pairingAddr`, the pairing poll effect, the one-dial-at-a-time rule.
- `format.ts`: `humanBytes`, `humanDuration`, `fitText`, `num` (em dash for unknown), `plural`.
- Test harness `test/mocks.ts`: `installFakeDaemon()` serves the whole contract from mutable state (catalog, load, pull SSE, chat SSE with delay/abort, network on/off, peers with `answersAt`/`impostors`/`peerModels`, pairing success/failure/lockout, groups upsert, privacy ack rule, audit, optional diagnostics), plus fixtures `HARDWARE`, `MODELS` (fits / tiny fixture / too large), `LOADED`, `STATUS`, `NETWORK(_ON)`, `PEER`, `STRANGER`, `PRIVACY`, `AUDIT`, `sse()`, `apiError()`. Extend it for any new route.

## 7. Styling system and current look

- One stylesheet `src/styles.css` (848 lines), tokens on `:root`, light by default, dark via `@media (prefers-color-scheme: dark)` only. No manual theme toggle, no `data-theme`, `color-scheme: light dark` on `:root` and in `index.html`.
- Tokens (light / dark): `--bg #f3f4f6 / #14171b`, `--surface #ffffff / #1c2026`, `--surface-2 #e9ebee / #252a31`, `--text #1a1d21 / #e8eaed`, `--muted #5b6370 / #a3abb5`, `--line #d6dae0 / #2f353d`, `--accent #0b6b74 / #5bc8d2` (teal), `--accent-ink #fff / #0f1a1c`, `--accent-soft` teal 10% / 14%, `--ok #1e7b3c / #6ccb86`, `--warn #8a5300 / #f0b24a`, `--danger #b3261e / #ff8a80`, `--danger-soft`, `--focus` = accent, `--radius 6px`. No spacing, size, elevation, z-index or motion tokens; spacing is literal px everywhere plus many inline `style={{ marginTop: 10 }}` in TSX.
- Type: system stack (`-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", "Segoe UI", system-ui`), 14 px / 1.5 body, `tabular-nums` globally, h1 24/600, h2 16/600, small 12.5 px, mono `ui-monospace, SFMono-Regular, Menlo`. No bundled fonts; CSP `default-src 'self'` means any web font must be bundled locally (Google Fonts would be blocked).
- Look: calm, light-grey document pages. White 216 px sidebar with a teal 18 px rounded square as the only brand mark; active nav item teal text on teal-tint. Content column capped at 960 px (`.view`), generous padding (28/32 px), sections 28 px apart, prose-heavy descriptions under every heading (max 64ch). Tables are plain: 8x10 px cells, bottom hairlines, muted 12.5 px headers, no zebra, no sticky header, no sorting, no selection highlight. Buttons 32 px (28 px small), bordered, primary = solid teal, danger = red text. Notices = bordered box with 3 px coloured left rule. Empty states = dashed box. Status = 8 px dots (grey / green / red). Tags = 10 px-radius outlined pills. The intended "memorable element" (per the CSS header comment) is the memory fit bar; the loudest element is the 56 px teal pairing code. Chat is a narrow column with tinted user bubbles. Animation: none apart from browser defaults; `prefers-reduced-motion` kills transitions anyway.
- Class vocabulary in use: `app sidebar brand brand-mark nav nav-group later sidebar-foot dot(.ok/.bad) view view-head section muted small ok warn danger btn btn-primary btn-danger btn-sm field help check radio-row toolbar grow kv notice(.error/.ok/.disclosure) empty glance journey journey-step model-hero fit fit-track fit-seg fit-legend model-row model-name model-meta tag(.recommended/.member) tag-x row-actions confirm progress-text chat chat-log msg(.user/.assistant) body foot cursor composer pair-grid pair-panel pair-code pair-addr qr-payload device-id run-on mode-pill(.away) modes mode group group-members visually-hidden skip`. `.model-name`/`.model-meta` are reused for device rows too (tests select on `.model-name`).

## 8. Accessibility patterns (F-10-11)

- Skip link; `main` focusable; every view `<section aria-labelledby="view-title">` with one `h1#view-title`; nav `aria-current="page"`; journey `aria-current="step"`.
- Live regions: `role=status` for glance lines, outcomes, pairing wait, copy result, chat generating/complete; `role=alert` for ErrorNotice and firewall failure; chat log `role=log`.
- Inline confirmations are `role=group` with `aria-label` ("Confirm download of X") and receive focus; cancel returns focus to the opener (`refocus`); after download focus goes to Run; after connect focus goes to the outcome paragraph.
- Form semantics: labels on every input (visually hidden where needed), `aria-describedby` help text, `fieldset/legend`, `role=radiogroup`, `role=switch` for networking, buttons with explicit `aria-label`s that include the device name ("Connect to Laptop", "Revoke Studio Mac").
- `:focus-visible` 2 px accent outline; `prefers-reduced-motion` honoured; FitBar `role=img` + text label.
- Tests: axe on every view with realistic data (serious/critical = fail; `color-contrast` disabled because jsdom has no layout, so contrast is unverified in CI), plus keyboard-only Models -> Download -> Run -> Chat -> Stop and keyboard-only pairing.
- Missing: no roving focus or arrow-key table navigation, no shortcuts, no `aria-sort` (no sorting exists), sidebar is a plain link list (fine), contrast never measured.

## 9. Tests (71 cases, 9 files)

| file | cases | covers |
|---|---|---|
| api/client.test.ts | 11 | metrics/SSE parsing, ApiError, unreachable, bearer token, chat stream/stop/target/in-stream error, connect and pairing bodies, pull progress |
| api/errors.test.ts | 7 | error table equals `error.rs` both ways; mapError fallbacks |
| test/a11y.test.tsx | 3 (one `it.each` over 9 views) | axe per view + Peers error panel + Models confirm |
| test/keyboard.test.tsx | 1 | keyboard primary journey incl. Stop < 300 ms, history in localStorage |
| test/peers.test.tsx | 14 | networking off by default, pair start/poll/success/failure, join wrong code/locked/QR, strangers, pair_addr round trip, old daemon port, health words, self never nearby, revoke/remove confirm, groups PUT, keyboard pairing |
| test/devices.test.tsx (WT) | 18 | no "this Mac" on any view, neutral naming, device name default, remote run without local load, peer lacks model, plurals, address/state/last error, connect by address with impostor refusal, cached mDNS never "connected", offline only where discovery runs, invalid addr, Connect only with networking, empty-state reasons, keyboard Connect/Cancel, one dial at a time |
| test/firewall.test.tsx (WT) | 6 | Windows rule offer, cancel/fail, hidden off-Windows/outside app, axe, placement by pairing code, join "nothing answered" hint |
| test/privacy.test.tsx | 6 | disclosure + ack before PUT, public disabled, RunOn hidden in local-only (no target sent), peer target + "Ran on", device paired later appears on focus, RunOn on Home only outside local-only |
| test/devdaemon.test.tsx | 5 | `?daemon=` loopback only, dev only, compiled out in production |

Layout-coupled selectors a redesign will break (rewrite the selectors, keep the assertions): h1 names "Home", "Peers" etc. (a11y VIEWS); `findByText(..., {selector: ".model-name"})` + `.closest("tr")` + cell index (peers, devices); `.closest(".section")` under heading "Nearby, not paired"; `row.querySelector("button.btn-primary")` (keyboard); `{selector: "dd"}` for Hardware chip; `{selector: "code"}` for URLs/dial addr; `{selector: "td"}` for group name; button/role names ("Pair this device", "Join", "Connect", "Run", "Stop", "Message", group "Pairing code"); `#/models?confirm=` deep link; localStorage keys `common.chat`, `common.settings`.

## 10. Build and Tauri configuration

- Scripts: `dev` (Vite :1420, strictPort), `build` (`tsc && vite build`), `test` (`vitest run`), `tauri`, `build-sidecar` (`scripts/build-sidecar.mjs`: cargo release build of `common-cli` for the target triple, staged as `binaries/common`, checked against `daemon-args.txt`; runs before every `tauri build`).
- `tauri.conf.json`: productName/title "Common", one window 1100x720, min 720x480, default decorations (native title bar), no custom menu/tray, no updater. CSP: `default-src 'self'`, `connect-src 'self' http://127.0.0.1:8192 ipc: http://ipc.localhost`, `img-src 'self' data:`, `style-src 'self' 'unsafe-inline'`, `script-src 'self'`. Any new daemon port, remote origin, font or worker needs a CSP change. Bundle targets all, sidecar `binaries/common`, icons in `src-tauri/icons` (32/128/256 png, icns, ico, Windows Square* logos), macOS ad-hoc signing `-`.
- Capabilities (`default.json`): `core:default`, `opener:default`, shell execute/spawn restricted to the sidecar with args `serve --host 127.0.0.1 --port <2-5 digits>`. No fs, dialog, clipboard, notification, window-state or event permissions (clipboard uses `navigator.clipboard`). A native file picker for Import GGUF would need `@tauri-apps/plugin-dialog` + capability.
- Rust shell (`src-tauri/src/lib.rs`): if `/health` already answers on 127.0.0.1:8192 it does not spawn; else spawns the sidecar with `serve --host 127.0.0.1 --port 8192 --exit-with-parent`, logs readiness for 20 s; on exit stops it gracefully (SIGTERM on unix, WT `POST /api/shutdown` on Windows, then kill after 10 s; Windows job object ties the daemon to the app). Quit signals route through the same path. Commands: `daemon_info` -> `{base_url, spawned}`; WT `firewall_status`, `allow_through_firewall` (netsh via elevation prompt; inbound UDP, Private profile, this program only). No Tauri events are emitted to the webview; the UI learns everything over HTTP polling.
- The daemon port is fixed at 8192 by `daemon-args.txt`; the app is not multi-daemon in production.

## 11. Must preserve (working flows and invariants)

1. Model journey: catalog search, license/size/source confirmation before any download, resumable progress, fit estimate with composition, load with device + n_ctx from Settings, delete with confirmation, recommended model surfaced, keyboard-only completion.
2. Chat: SSE streaming render, Stop (client abort, under 300 ms), stopped replies kept with partial text, usage/TTFT/tok/s per reply, local-only enforcement inside Chat, peer target sent only for peers, "Ran on <device>", remote run without a local model (WT), plain message when the peer lacks the model, history stored locally only.
3. Pairing both directions (code + address, pasted QR payload), countdown, typed failure/lockout messages, full-id comparison guidance, firewall-aware hints, Windows consent step (WT).
4. Device truthfulness: `connState` semantics (cached mDNS never means connected/ready), `promptReadiness`, last dial error with hint, address provenance (seen / last known / not known), Connect with address hint only for paired devices, one dial at a time, revoke vs remove semantics with confirmations, self never listed.
5. Groups: default deny, two policy switches, membership edits via one-group PUT.
6. Privacy: three modes with Public disabled, disclosure + explicit acknowledgement before leaving local-only, mode visible on every screen, RunOn hidden and target forced local in local-only.
7. Errors: every failure shown via the code table (title, next step, code: detail, hint); unreachable daemon degrades each view with "Available once the background service is running".
8. Diagnostics: copy redacted report ("never your messages"), audit tail without prompts or keys, service info, error-code reference.
9. Settings: default device, context length, service address read-only.
10. Platform-neutral wording (no "this Mac"), dev-only `?daemon=` override, no fabricated numbers (em dash / "unknown" for missing values), placeholders honest about milestones.

## 12. Technical debt and gaps that matter for the redesign

- Information architecture is page-per-topic with long prose; no master/detail, no selection model, no inspector, no persistent toolbar or status bar. Peers alone is six stacked sections on one scrolling page.
- Tables: no sorting, no column alignment for numbers except a few `.num`, no sticky headers, no row selection, actions crammed into the last column, confirm/connect forms injected inside the first cell (breaks a dense grid).
- Transient operation state is view-local: leaving Models mid-download loses progress and re-enables Download for a model still being pulled (a second pull can start); leaving Chat mid-stream loses the Stop button (the reply still lands in history). A global operations store is needed for a Jobs/transfers view.
- No push channel: 2.5 s status poll, 5 s peers poll, 1 s pairing poll; no event stream from the daemon. Activity/Jobs views must poll `/api/audit` and `/api/status` or get a new endpoint.
- `runs` is session-only and fed only by Chat; local requests are not audited by the daemon (only `remote_request` sent/finished with model_id, duration_ms, outcome, request_id, plus pairing_*/peer_* events). There is no daemon job list, no per-request id for local runs, no server-side cancel by id, only `queue.inflight/queued` counts and `/metrics` counters (`common_requests_total`, `common_remote_requests_total`, `common_requests_rejected_total`, `common_inflight`, `common_queue_depth`, `common_ttft_seconds`, `common_decode_tokens_per_second`, `common_tokens_generated_total`, `common_prompt_tokens_total`).
- Remote device data the redesign wants (hardware, free memory, engines, installed models, measured performance) is not exposed: `Capabilities` exists on the wire (`proto.rs`: hardware_json, free_memory_bytes, engines, cached_model_sha256) but no `/api/peers/{id}` detail route. Chat therefore offers every local catalog id for a peer.
- `last_seen_utc` does not advance while a connection stays up (evidence defect 5); unreachable peers are redialled every ~35 s forever with an audit line each time (defect 4) and will flood an Activity view.
- Unused capabilities: `api.unload` (no Unload button anywhere), `api.importModel` (no Import UI, yet error text points to it), `api.health`, `configure({token})` (no API-key setting).
- `/api/diagnostics` does not exist in the daemon; the bundle path in the UI is dead code until it does.
- Styling: no spacing/size/elevation tokens, 47 inline `style={{...}}` attributes in TSX, magic `calc(100vh - 76px)` for Chat, `.model-name` reused for devices, `.view { max-width: 960px }` wastes wide windows, dark mode only via OS preference, teal accent conflicts with the charcoal/deep-green/lime direction, contrast never tested.
- Router has no params; selection/detail state cannot be linked or restored.
- `Later` placeholders for Cluster/Jobs/Contributions advertise features ("run models larger than any single device", "what it cost") that are not implemented and conflict with "no model partitioning exists" and no billing; the redesign should drop or reword them.
- Brand: UI says "Common"; whether UI 2.0 shows "UCompute" needs an owner call (D-018: name in use, clearance not claimed; code and paths stay `common`).

## 13. Data available today per required UI 2.0 section

| section | real data now | missing (show as unavailable or needs API) |
|---|---|---|
| Dashboard | status (version, uptime, loaded model, queue), hardware, privacy mode, network state, peer counts/states, last error, session runs | network throughput, per-device load |
| Devices | this computer: full hardware; peers: name, ids, connected/health, last_error, addrs, last_addr, groups, paired/last seen; nearby strangers | peer hardware, memory, models, measured capability, activity per peer (needs Capabilities route; audit can be filtered by peer_id) |
| Models | local catalog, downloaded/loaded, fit, estimate, license, pull progress (while the view lives) | models on peers, load progress, unload/import UI (routes exist), pull cancel |
| Jobs | inflight/queued counts, session runs from Chat, remote_request audit (sent/finished, duration, outcome) | daemon job list with ids, local request records, cancel by id |
| Chat | everything above in Chat | multiple conversations, system prompt and sampling params (wire supports max_tokens/temperature) |
| Activity | audit tail (100), error codes | local execution history, filtering server-side |
| Settings | device, n_ctx, service address, privacy, networking, serve_peers, groups, firewall (Windows) | storage paths/usage, API key, theme preference, contributor/credit controls (no backend; keep absent) |
