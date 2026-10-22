# UCompute Desktop UI 2.0: design and build plan

Lead product design and frontend architecture. Written 2026-10-19 and read-only: nothing in the repo was edited, built or run.

**Revision 2 (same day).** Revised after three critiques (UX, feasibility, honesty). Every changed rule was re-checked read-only against HEAD and the working tree. §8 says which critique points were rejected or merged and why.

Inputs: `qbittorrent.md` (behaviour study), `api-inventory.md` (data sources and gaps), `current-app.md` (what exists). I re-read the working tree for the facts this plan depends on:
- `Peers.tsx::connState` (checks `last_error` before "offline") and `promptReadiness` (this computer's groups and Privacy only)
- the `client.ts` types, `send()` (every fetch failure except `AbortError` becomes `unreachable`) and the `chat()` stream loop
- `daemon.ts::refreshStatus` (any error sets `reachable:false`)
- `mesh.rs::peers_get`, `peer_revoke` (revoke unpins: a revoked device leaves `peers[]` and appears only in `revocations[]`), `network.connections = node.connections().len()` (can hold two live connections for one peer), `pair_start` (409 `overloaded` while a window is open; no cancel route), `generate_remote_audited` (client `remote_request` `finished` carries `outcome` = `done`, an error code, `incomplete` or `cancelled`)
- `openai.rs::run` (admit → `ensure_loaded` → `generate` all happen before the SSE response; `common_tokens_generated_total` rises only at `Done` and only for local runs)
- `transport.rs`: hardware and free memory are probed once at `Node` start; `issued_at` is stamped at each handshake; `close_code()` maps every refusal other than revoked/unauthorized to `CLOSE_PROTOCOL`; `allowed()` is checked at outbound (≈714) and inbound (≈790)
- `discovery.rs::peers` (`trusted` = pinned here, so a revoked id that still announces looks untrusted)
- the `health.rs` state machine (keep-alive 1 s, idle 4 s)
- `remote.rs::execute` (a served request calls `ensure_loaded` with the service defaults) and `ServeOptions`
- `lib.rs` ROUTES (32 routes; no disconnect, no pairing cancel) and the CORS middleware (`Access-Control-Allow-Headers: authorization, content-type` only)
- `admin.rs::ready` (`try_lock` on the slot: a miss is a load **or** a brief request hold)
- `firewall.rs` (localized Windows or a non-ASCII path always reads "not allowed")
- `tauri.conf.json`, `capabilities/default.json`; jsdom 30.1.2 (`HTMLDialogElement` has no `showModal()`/`close()`)

Tauri 2 menu facts come from current Tauri docs (Context7):
- `core:default` includes `core:menu:default`. The JS `Menu.default()`, `Menu.popup()` and `setAsAppMenu()` calls need no capability change.
- Tauri installs a default menu on macOS only. `Menu.default()` names the app submenu after `productName`.

Base: HEAD is `bfcf4a7`, a docs-only commit on top of `e76e7fc`. **Precondition:** UI 2.0 starts from a commit that contains today's in-flight working-tree fixes:
- `POST /api/peers/{id}/connect`
- `/api/shutdown`
- `last_error`, `addrs`, `dial_addr` and `device_name`
- the firewall commands
- `connState`
- the tests `devices.test.tsx` and `firewall.test.tsx`

If those fixes are not committed yet, wait or rebase. Never copy them by hand.

Licence boundary: qBittorrent is GPL. This plan borrows only behaviour, meaning information architecture and interaction patterns. No code, assets, icons, names or pixel layouts are taken from it.

---

## 0. Design principles (the tests every screen must pass)

1. **Inspect and control in one place.** Every section is a table with an inspector for the selected row. The contextual toolbar acts on that row. No action needs more than one navigation step. The Dashboard uses the same tables and row actions, not summary cards.
2. **Never show more than the backend knows.**
   - Unknown values render as `—` with a stated reason (for example "Not reported while disconnected").
   - Stale values are never presented as live. When a refresh fails or times out, rows stay but are dimmed under "Not updated since HH:MM:SS", live-only columns read "Unknown (not refreshed)", and row actions are disabled. Only a refused connection (the service is gone) empties the tables.
   - An mDNS record never raises a device's state, and a missing mDNS record never claims a device is off ("Not seen nearby", not "Offline").
   - A value another device reported says when it was measured. If the time is unknown, it says that, and it is never placed under a header that implies a live reading.
   - A caveat that changes what a value means is visible in the cell or the inspector. It never lives only in a tooltip.
3. **State = icon + word + colour, always together.** Colour is never the only carrier. Only real activity gets a colour. Idle and offline use the neutral text colour.
4. **An action is shown only if the backend supports it.** If it is supported but not possible right now, it stays visible, disabled, with the reason given. If it is not supported, it is absent: no queue position, no reorder, and no cancel except on the app's own streams.
5. **Dense, not cluttered.** Rows are 28 px and numbers use tabular figures. Prose is short. There are no decorative cards, heroes or stepper "journeys" except a first-run row on the Dashboard.
6. **No new runtime dependencies.** Use React 19, the existing `createStore` and native platform features:
   - `<dialog>` for dialogs
   - `Intl.Collator` for natural sort
   - `Intl.RelativeTimeFormat` for relative times
   - Tauri `Menu` for menus
   - CSS container queries for the status bar's narrow-width behaviour
   - icons vendored as inline SVG
   - system fonts, so the CSP stays unchanged

---

## 1. Information architecture

### 1.1 Window anatomy

```
┌ native title bar: "UCompute" (macOS traffic lights / Windows caption buttons) ─────────────────┐
│ RAIL 208px │ TOOLBAR 44px: [h1] [section actions] │ [selection actions]          [More ▾]     │
│ (48px icon │ SCOPE ROW 30px: [All 5 · Connected 2 · …]                       [Filter  ⌘F]     │
│  rail      │──────────────────────────────────────────────────────────────────────────────────│
│  < 900px)  │ TABLE (sticky 28px header, 28px rows)                                            │
│            │═══════════ splitter (role=separator, drag + arrow keys) ══════════════════════════│
│            │ INSPECTOR: [Overview | Hardware | Models | …]                        [collapse ▾] │
│            │ tab panel (key/value groups, sub-tables)                                          │
├────────────┴──────────────────────────────────────────────────────────────────────────────────┤
│ STATUS BAR 26px: [attention] ● Service · Engine · Slots ……… Mesh · Sharing · ↓↑ · Privacy · Disk │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

- **Rail** (persistent `<nav aria-label="Sections">`). It holds the brand mark plus the word "UCompute" (owner decision, §7) and these seven sections:

| # | Section | Rail badge |
|---|---|---|
| 1 | Dashboard | none |
| 2 | Devices | `connected/trusted`, for example `2/3`, where connected = `live(p)` (§2.2). A warn dot appears when any trusted device is **Not responding** or **Unreachable** (never for Not seen nearby, Not connected or Disconnected). |
| 3 | Models | `↓1` while app-tracked downloads run |
| 4 | Jobs | `status.queue.inflight + queued` (always polled; works without E3). Tooltip: "Includes requests served for paired devices." |
| 5 | Chat | dot while a reply is generating |
| 6 | Activity | none |
| 7 | Settings | none |

  - Badges are computed from live stores and are never cached.
  - Below 900 px window width, the rail collapses to 48 px icons, using CSS only. Labels then become `aria-label` plus a tooltip.
  - The Settings shortcut also exists in the native macOS menu.
- **Toolbar** (44 px). Left to right:
  1. The section `h1`, about 15 px. It is the page's only h1.
  2. **Section actions** (for example Networking switch, "Pair a device…"), then a separator, then **selection actions** that act on the selected row.
  3. An optional **More ▾** button (native menu) holding the section's lowest-priority actions, named per section below.
  - Only the action buttons sit inside `role="toolbar"` (one Tab stop, arrow-key roving). Selects and switches that are not buttons are separate Tab stops.
  - At window width ≥ 1200 px actions show icon and label. Below that they show the icon only, with `aria-label` and a tooltip.
- **Scope row** (30 px, owned by A as part of `SectionFrame`), directly above the table header:
  - Left: the **scope bar**, a segmented control of `aria-pressed` buttons with live counts, inside `role="group"` with a label (qBittorrent's status filter, placed next to the table it filters).
  - Right: the **filter box**, a separate Tab stop. It matches all visible text columns. Mod+F focuses it and Esc clears it. There is no "Filter by" column selector.
- **Main table**: a sortable grid with single selection. The selection lives in the URL (`#/devices/<id>?tab=hardware`), written with `history.replaceState`, so it survives reloads and supports deep links without flooding history. Only a section change pushes a history entry. Multi-select is deliberately omitted (§6).
- **Inspector**: docked at the bottom for the Devices, Models, Jobs and Activity tables. Rules:
  - Tabs follow the APG pattern. Clicking the active tab does nothing (no hidden collapse). The collapse button and Mod+I toggle it.
  - Default height: Devices sizes the table to its content plus one row, capped at 50% of the main area, and gives the inspector the rest (minimum 140 px). Models, Jobs and Activity default to 38%. Once the user drags the splitter, their height wins.
  - Height, collapsed state and the last tab persist per section in localStorage (`common.ui`), read and written inside try/catch.
  - Devices preselects the self row when the URL has no id, so its inspector is never empty on the first visit.
  - With no selection (other sections) it shows "Select a <thing> to see its details" and stops refreshing tab-specific data.
  - Chat uses a right-docked "Run" panel instead (§2.5). Dashboard and Settings have no inspector.
- **Status bar** (`<footer aria-label="Status">`, not a live region): see 1.2.

### 1.2 Status bar: every item, its exact source, its states

Items run left to right. A separator divides the left and right groups. Each item is a button or link when it has a destination, otherwise plain text. Tooltips (the `Tip` component, §3.4: shown on hover **and** focus, wired with `aria-describedby`) give exact numbers and the time they were sampled.

| # | Item | Text | Source | Rules |
|---|---|---|---|---|
| 1 | Attention | Shown only when something needs attention (list below). | `daemon`; `status.loaded.fallback_reason`; `pairing` store; dismissable event entries | Hidden when empty. Tone: danger or warn. |
| 2 | Service | `● Running`, `● Busy (not answering)`, `● Starting`, `● Not running` | `daemon` store (`/api/status` every 2.5 s; on timeout, `GET /health`, §5.3). Version, uptime and "started outside this app" (`spawned:false`) are in the tooltip at every width. | ok, warn (busy), active (starting) or danger |
| 3 | Engine | `Qwen2.5 1.5B · Metal0 (Apple M2)`, `Loading Qwen2.5 1.5B · 12 s`, `No model loaded` | `status.loaded.model_id`, `device_actual`, `loaded_for`, `loaded_at_utc` (**E2**); `status.engine.state`, `loading_model_id`, `loading_for_ms` (**E2**). Until E2 lands, two consecutive `/ready` 503s with reason "a model is loading" give `Loading a model` with no id (one miss can be a request briefly holding the slot). | Loading uses the active tone. A fallback reason adds a warn icon. Tooltip: "Loaded for a request from Laptop at 10:32" when `loaded_for` is a peer, "Loaded by this computer at 10:32" otherwise. Truncates with an ellipsis. |
| 4 | Slots | `In use 1/4 · Waiting 0` | `status.queue.inflight / max_concurrent`, `queued` | warn when `queued > 0` or `inflight == max_concurrent`. "In use", not "Running", because `inflight` includes requests still waiting for a load. Tooltip: "Includes requests served for paired devices." |
| 5 | Mesh | `Networking off`, `Not listening`, `Listening · no devices connected`, `2 devices connected`, `1 device connected · 1 not responding` | `network.enabled`, `network.listen_addr`; counts from `mesh.peers` (distinct ids): connected = `live(p)`, not responding = `p.connected && p.health === "unhealthy"`. Tooltip: "N connections" (`network.connections`) and each trusted device with its State word. | Off is neutral, because off is a choice. ok only when connected > 0 and nothing is not responding; warn when any is not responding. **Discovered entries never count**, and neither does a second connection to the same device. |
| 6 | Sharing | `Sharing on`, `Serving 1 request for Laptop`, `Not sharing` (only while networking is on) | `network.serve_peers`; E3 active rows with `kind === "served_for_peer"`; `peer_slots` in the tooltip | "Serving…" uses the active tone. Click opens Jobs with the Served for devices scope while serving, otherwise Settings › Sharing. It is not a toggle. |
| 7 | Network rate | `↓ 12 KB/s ↑ 3 KB/s`, `↓ < 1 KB/s ↑ < 1 KB/s` | Per-connection `rx_bytes`/`tx_bytes` (**E1**) sampled every 5 s by the app. Rate = Δbytes / actual Δt, only between two samples with the same `connection.id`, summed over connections. | `—` without E1 or with no connections. A sample is a gap (not 0) when the id changed or Δt > 1.5 × the interval. Under 1 KB/s reads "< 1 KB/s". Tooltip: "Includes 1 s keep-alives." |
| 8 | Privacy | `Local only` or `Trusted devices` | `privacy` store (`/api/privacy`) | A link to `#/settings/privacy` whose accessible name is frozen as **"Privacy mode: <mode>"** (used by `privacy.test.tsx`). Keeps "mode visible on every screen" (F-10-10). |
| 9 | Disk | `Disk 112 GB free` | `hardware.report.disk_free_bytes` (probe every 120 s) | `Disk —` when the probe omits it. |

**Narrow windows.** The footer is a CSS size container. As it narrows, items drop in this order: Disk, Network rate, Sharing, Slots. Engine truncates. Attention, Service, Mesh and Privacy never drop. Each droppable item carries its priority as a class, which a unit test asserts. The layout itself is checked in the real-browser and packaged-app passes at the 720 px window minimum (§5.5), because jsdom has no layout.

The Attention item covers these cases:
- **Service not running.** Retries automatically; click to retry now.
- **Service busy (not answering).** Click opens Jobs.
- **Running on CPU: <fallback_reason>.**
- **Pairing open · 1:42** while a pairing window from this app is open (`pairing` store). Click reopens the Pair dialog.
- **Dismissable event entries** (the same events the announcer speaks, so sighted and screen-reader users get the same information): "Download failed: <model>", "A reply from this app failed: <title>", "Lost connection to <device>" (a trusted device leaving `live` without the user asking). Each clears when dismissed or when its cause clears.

The Windows Firewall check is **not** an Attention item: the check cannot prove a block (§2.2 Firewall).

A single visually hidden `aria-live="polite"` region in the shell announces transitions only, throttled to at most one every 2 s:
- service stopped or started
- a trusted device connected or lost
- a download finished or failed
- a job started from this app failed

The status bar itself never announces.

### 1.3 Keyboard map

`Mod` is ⌘ on macOS and Ctrl on Windows.

| Keys | Action | Scope |
|---|---|---|
| Mod+1 … Mod+7 | Go to Dashboard … Settings | global |
| Mod+, | Settings | global (macOS convention, also on Windows) |
| Mod+F | Focus the filter box | sections with a table |
| Esc | Close the open menu or dialog. Otherwise cancel an inline confirmation. Otherwise clear the focused filter. In Chat, stop generating. | contextual |
| F6 / Shift+F6 | Cycle focus regions: rail → toolbar → scope row → table → inspector → status bar | global |
| ↑ ↓ Home End PgUp PgDn | Move the selection; the cell cursor keeps its column (selection follows focus) | table |
| ← → | Move the cell cursor within the selected row (APG grid) | table |
| Enter / double-click | Default action. Devices, Models, Jobs and Activity: open the inspector and focus its tab list. Never a destructive action. | table |
| Shift+F10 / Menu key / right-click | Native context menu for the focused row (Tauri `Menu.popup`) | table |
| Copy (⌘C / Ctrl+C) with a grid cell focused and no text selected | Copies the row identifier: device id (64 hex), model id, job id or event JSON. Implemented as a document `copy` event handler that sets the clipboard data and calls `preventDefault`, so macOS Edit › Copy and the key never both fire. Not in the keydown map. | table |
| Delete (Windows) / ⌘⌫ (macOS) | Devices: Remove (unpair), with confirmation. Models: Delete the model file, with confirmation. **Revoke is never on a key.** | table |
| Mod+I | Toggle the inspector | section |
| ← → (in the tab list) | Switch inspector tabs (automatic activation) | inspector |
| Mod+R, F5 | Refresh the current section. The webview reload is prevented in production builds, because a reload would drop app-tracked downloads, the pairing session and streams. | global |
| Mod+/ | Keyboard shortcuts sheet (`<dialog>`) | global |
| Enter / Shift+Enter | Send / insert a newline | Chat composer |

Chat keeps the role names its tests use: textbox "Message" and buttons "Send" and "Stop". There is no F2 rename, because no rename API exists.

### 1.4 Platform conventions

| Concern | macOS | Windows |
|---|---|---|
| Modifier | ⌘ | Ctrl |
| Menu bar | Built in JS on startup (Tauri runtime only). Steps below the table. | No menu bar, which matches modern Windows apps. Every command is reachable from the rail, toolbars, context menus and shortcuts. |
| Shortcut dispatch | Menu accelerators fire first. The webview `keydown` handler skips combinations the menu owns (including ⌘C, handled by the `copy` event), so nothing fires twice. | `keydown` handler plus the same `copy` handler |
| Window chrome | Native title bar. Decorations are unchanged; no overlay title bar and no drag regions. | Native title bar with snap layouts |
| Title-bar theme | Follows the chosen theme via `getCurrentWindow().setTheme()`. Needs `core:window:allow-set-theme` added to `capabilities/default.json` (the only capability change). | same |
| Context menus | Native, via `Menu.new({items}).popup()` (`core:menu:default` covers it) | same (native Win32 menu) |
| Fonts | `-apple-system` (SF Pro), `ui-monospace` (SF Mono) | `"Segoe UI Variable Text"`, `"Segoe UI"`; `"Cascadia Mono"`, Consolas |
| Delete key | ⌘⌫ | Delete |
| Refresh | ⌘R | F5, Ctrl+R |
| Quit | ⌘Q. The existing shell behaviour stops the service if this app started it. | Closing the window quits; existing `/api/shutdown` path |
| Scrollbars | System (not styled) | System (not styled) |

The macOS menu is built in these steps:
1. Start from `Menu.default()`, which provides the app, Edit, Window and Help menus. Edit is required for ⌘C/⌘V in text fields.
2. Insert "Settings…" (⌘,) into the app submenu.
3. Append a **Go** submenu: Dashboard … Settings (⌘1–7), Find (⌘F), Toggle Inspector (⌘I) and Refresh (⌘R).
4. Add "Keyboard Shortcuts" (⌘/) to Help.

Default webview context menu: suppressed on tables and chrome, kept on text inputs and selectable text (copy/paste).

---

## 2. Sections

Every derivation below uses the client types in `client.ts`. Fields marked **E1–E5** are the backend additions in §4. The UI feature-detects each one by field presence or a 404, and shows the stated "unavailable" text when it is missing. No version sniffing.

**`live(p)`** = `p.connected && p.health !== "unhealthy"`. Everything that says or counts "connected" uses it: the Devices Connected scope and State, Availability, the Chat destination picker, Last seen "Now", the rail badge, the Dashboard and the status bar Mesh item.

Shared states, implemented once in `SectionFrame` from the stores the section needs (each store keeps its last good data and the time of that data, §5.3):

| Condition | Rendering |
|---|---|
| `daemon.reachable === null` | Toolbar renders. The table shows `Connecting to the background service…`. |
| `daemon.reachable === false` (connection refused, and `GET /health` does not answer) | The table shows an EmptyState: "The background service is not running", with the next step from `errors.ts` `unreachable` and a "Try again" button. This is the only state that empties tables. |
| `daemon.responding === false` (a poll timed out but `/health` answers) | Rows stay, dimmed. Banner: "Service running, busy (likely loading a model). Not updated since HH:MM:SS." Live-only columns read "Unknown (not refreshed)". Row actions are disabled with that reason. |
| A store refresh failed (`error` set, last data kept) | Rows stay, dimmed. Banner: "Not updated since HH:MM:SS: <errors.ts title>", with the ErrorNotice details (code, next step, hint) behind a disclosure. Live-only columns and actions as above. |
| Loaded and empty | A reasoned EmptyState specific to the section (below) |

In every case focus stays on the same row key across updates. If that row is gone, focus moves to the table.

### 2.1 Dashboard (`#/dashboard`; aliases `#/home`, `#/` and unknown routes)

**Layout.** One column of dense tables built from the same components as the sections, each with a 12 px caption header that links to its section. No inspector, no panel grid, no sparklines, no "This computer" panel (the status bar already shows that on every screen).

| # | Block | Content | Sources |
|---|---|---|---|
| 1 | Attention (only when non-empty) | One row per item, with an inline fix button: service not running → **Retry**; a trusted device Unreachable → **Connect**; pairing open → **Resume pairing**; Windows only, no firewall rule found → **Allow…** (worded as in §2.2 Firewall, warn tone); plus the `lastError` store ("Last problem", dismissable). | as in 1.2, `firewall_status()` |
| 2 | First run (only when no model is downloaded) | One row: the recommended model for this computer (`tags` includes `recommended`, first that fits), with size, licence and fit, plus Download (opens the Models download dialog) and Run / Open chat once downloaded. Replaces the Home hero and journey. | `/api/models` |
| 3 | Computers | The Devices `DataTable` (grid label "Computers") with the self row plus trusted devices and the §2.2 default columns. Activating a row opens `#/devices/<id>` with it selected. Same context menu as Devices. | `deviceRows()` |
| 4 | Running now | The Jobs `DataTable` (grid label "Running now"): active jobs first, then the last 5 finished, with Stop on this app's own rows. Without E3: "Job list not reported by this service version", plus the slot counts. | `jobs` (E3), `status.queue` |
| 5 | Recent activity | The Activity `DataTable` (grid label "Recent activity"), 8 rows after duplicate collapse. | §2.6 |

### 2.2 Devices (`#/devices[/<id>]`; aliases `#/peers` → devices, `#/hardware` → `devices/self?tab=hardware`)

**Rows**, built by `deviceRows()` in `src/state/devices.ts`:
1. **This computer**, pinned first regardless of sort. Its key is always the literal `"self"`, so `#/devices/self` and the `#/hardware` alias keep their selection before and after the network loads.
2. **Paired devices**: one per `mesh.peers[]`, key = id.
3. **Nearby, not paired**: `mesh.discovered[]` where `!trusted`, `id !== network.peer_id`, the id is not in `peers[]`, **and the id is not in `revocations[]`**.
4. **Revoked**: `mesh.revocations[]` whose `peer_id` is not in `peers[]` (a later re-pair supersedes a revocation). Listed only under the Revoked scope. If the id is also in `discovered[]`, the row says "Revoked · announced nearby 12 s ago".

**Device state vocabulary.**

*Trust* (shown as an icon in the Name cell with its word as the accessible text; the Trust column is hidden by default)

| Word | Rule | Tone/icon |
|---|---|---|
| This computer | the self row | neutral, monitor |
| Trusted | id ∈ `peers[]`. Pinned means trusted now, because revoke unpins. | ok, shield-check |
| Untrusted (nearby) | from `discovered[]`, not paired, not revoked | neutral, shield-off |
| Revoked | from `revocations[]`, not re-paired | danger, shield-x |

*Connection* (paired devices). The first matching rule wins. This is the working-tree `connState` with the changes marked ★.

| # | Rule | Word | Tone |
|---|---|---|---|
| ★1 | `!network.enabled` | Networking off | neutral |
| ★2 | `p.held === true && !p.connected` (**E5**) | Disconnected by you | neutral |
| 3 | `p.connected && p.health === "unhealthy"` | Not responding | warn |
| 4 | `p.connected` | Connected | ok |
| 5 | `dialing === p.id` (this app's connect call in flight) | Connecting | active |
| ★6 | `p.last_error?.code === "peer_held"` (**E5**, the other device refused this one) | Disconnected by <name>. Detail: "It must reconnect from its side." | neutral |
| ★7 | `p.last_error?.code === "peer_lost"` && `network.mdns` && `!seenNearby(p.id)` | Not seen nearby. Detail: "Nothing answered at <addr>, <time>; not announced nearby." | neutral |
| 8 | `p.last_error != null` (any other case, including announced nearby but nothing answered, `peer_unauthorized`, `protocol_incompatible`, `timeout`) | Unreachable, plus `last_error.message` and hint | warn |
| 9 | `p.health === "reconnecting"` | Reconnecting | warn |
| ★10 | `network.mdns && !seenNearby(p.id)` | Not seen nearby | neutral |
| 11 | otherwise | Not connected | neutral |

- `seenNearby(id)` = `discovered.some(d => d.id === id)`.
- **Only `connected` (an authenticated QUIC connection) produces Connected.** A missing mDNS record means only that the device was not announced here; the plan never says "Offline".
- The rail warn dot and the Attention "Connect" row fire only on rules 3 and 8.

*State column* (replaces Connection and Trust in the default view):
- Self: "Listening · N connected" (N = `live` count), "Not listening", or "Networking off".
- Paired: the Connection word.
- Nearby: "Not paired · announced 12 s ago" (`seen_utc`).
- Revoked: "Revoked", plus "· announced nearby 12 s ago" when announcing.

*Can run there* (can work sent from this computer run on that device now?)

| Row | Rule (first match) | Word |
|---|---|---|
| Self | `daemon.reachable !== true` | Not running (danger) |
| Self | `daemon.responding === false` | Busy (not answering) (warn) |
| Self | `status.engine.state === "loading"` (E2) | Loading model (active) |
| Self | `queue.inflight >= queue.max_concurrent \|\| queue.queued > 0` | Busy · `i/max` in use, `q` waiting (warn) |
| Self | otherwise | Available (ok). Note: "No model loaded" when `loaded == null`. |
| Peer | `!live(p)` | `—` (the State column says why) |
| Peer | `!promptReadiness(p, groups, localOnly).ok` | Not permitted here (neutral). Detail: the readiness sentence. |
| Peer | last client `remote_request` `finished` for this peer since the connection opened\* has outcome `policy_denied`, `peer_unauthorized` or `peer_revoked` | Refused last request (its permissions) (warn). Detail: errors.ts title and time. |
| Peer | … outcome `overloaded` | Refused last request (busy or over quota) (warn) |
| Peer | … outcome `done` | Can send · last request done (ok). Detail: time. |
| Peer | … any other outcome | Can send · last request failed: <title> (warn) |
| Peer | no finished request on this connection | Can send (neutral). Note: "Its own permissions, serving setting and load are not reported." |
| Nearby / Revoked | always | `—` |

\* Connection start = `connection.opened_at_utc` (E1), else the newest `peer_connected` audit event for that peer. The peer's own load is not reported (needs N2), so **Busy is never inferred for a peer.** "Available" is never used for a peer.

*Can use this computer* (what this computer grants that device; mirrors `PolicyEngine::may_serve`)

| Row | Rule (first match) | Word |
|---|---|---|
| Peer | `!network.enabled` | No · networking off |
| Peer | `!network.serve_peers` | No · sharing off |
| Peer | no group in `groups` contains it with `allow_remote_inference` | No · not in a prompt group |
| Peer | otherwise | Yes. Note: "Quotas and free slots still apply." |
| Self / Nearby / Revoked | always | `—` |

**Table columns.** Natural sort is `Intl.Collator(undefined,{numeric:true,sensitivity:"base"})`. Unknown values always sort last. The secondary key defaults to Name. The default set is budgeted to about 890 px, the content width of the default 1100 px window, so the table does not scroll sideways.

| Column | Value / source | Sort key | Default (px) |
|---|---|---|---|
| Name | Trust icon, then self: `settings.device_name \|\| network.device_name`; paired: `p.name \|\| p.short_label`; nearby: `short_label` (note "Name is shared at pairing"); revoked: short label of `peer_id` | name (natural) | 160 |
| State | above | Connected, Not responding, Connecting, Reconnecting, Unreachable, Not seen nearby, Not connected, Disconnected by <name>, Disconnected by you, Networking off, Not paired, Revoked | 140 (live) |
| Can run there | above | Available, Can send, Busy, Loading, Refused, Not permitted, — | 130 (live) |
| Can use this computer | above | Yes, No · … | 110 |
| Hardware | Self: `hardware.host.chip · humanBytes(memory.total_bytes)`. Peer: `capabilities.hardware.host.chip · memory.total_bytes` (E1; static fields). Not connected: `—` "Not reported while disconnected". | memory_bytes | 140 |
| Free memory | Self only: `hardware.memory.available_bytes`, note with probe age. Peers: `—`, note "Not reported live (needs N2)". The peer's start-time snapshot appears only in the inspector, with its caveat. | bytes | 90 (live) |
| Models | Self: count of downloaded models. Peer: `capabilities.models.length` (E1; caveat in the inspector) | number | 50 |
| Last seen | `Now` while `live` (works around evidence defect 5: `last_seen_utc` does not advance while connected); else relative `p.last_seen_utc`; nearby: `seen_utc` | timestamp | 80 (live) |
| Trust | above | trust order | hidden |
| Address | Self: `network.dial_addr`. Paired: `connection.remote_addr` (E1, note "connected"), else `discovered.addrs[0]` ("seen nearby"), else `p.last_addr` ("last known"), else `—` "not known". Nearby: `addrs[0] ?? addr`. Provenance is a visible muted note. | string | hidden |
| Jobs | E3 active jobs targeting this device from this computer | number | hidden (live) |
| RTT | `connection.rtt_ms` (E1) | number | hidden (live) |
| ↓ / ↑ | rate per §1.2 item 7 | number | hidden (live) |
| Groups | `p.groups.join(", ")` | string | hidden |
| Last error | `last_error.code` via the `errors.ts` title; detail: message, hint and `at_utc` | code | hidden |
| Paired | `p.paired_at_utc` | timestamp | hidden |
| Device ID | full 64 hex, monospace | id | hidden |
| Short label | `short_label` | string | hidden |
| Version | `capabilities.common_version` (E1) | string | hidden |

The column chooser sits in the header context menu. The last visible column cannot be hidden. Layout persists per table.

**Scope bar** (with counts): All, Connected (`live`), Not connected (paired and not `live`), Nearby, Revoked.

**Toolbar** (each action enabled only when the stated condition holds; otherwise disabled with the reason as tooltip and `aria-describedby`):

| Group | Action | Enabled when | Call |
|---|---|---|---|
| Section | Networking (switch, `role=switch`, own Tab stop) | daemon reachable | `PUT /api/network {enabled, serve_peers}`. Turning it off also clears `serve_peers` (existing rule). |
| Section | Pair a device… | daemon reachable (also while networking is off) | Opens the Pair dialog. With networking off, its first step is "Networking is off. [Turn on networking]". While a window is open the button reads **"Pairing open · 1:42"** and reopens the same session. |
| Selection | Connect | paired, not `connected`, networking on, `dialing === null` (one dial at a time) | `POST /api/peers/{id}/connect`. The optional address field lives in the inspector Connection tab. |
| Selection | Disconnect | paired, connected, and the `held` field present (E5) | `POST /api/peers/{id}/disconnect`. **Absent** without E5, never faked with DELETE. |
| Selection | Pair… (nearby row) | nearby row selected | Opens the Pair dialog, Join tab, prefilled with `pairingAddr(discovered)` |
| Selection | Remove | paired | `DELETE /api/peers/{id}` after an inline confirmation that explains the device can pair again |

- **Revoke** is permanent and fanned out to group co-members, so it is not in the toolbar. It is in the row context menu and in a separated "Danger" group at the bottom of the Overview tab, after a confirmation explaining it is signed, fanned out and permanent unless re-paired. Its button name stays "Revoke <name>".
- **Copy ID** is in the row context menu and on the copy shortcut (§1.3).
- No More ▾ menu is needed here after these moves.

**Pair dialog** (`<dialog>`, two APG tabs) reuses the existing flows and test names unchanged:
- **Show a code**: `pairStart`, the six-digit code in group "Pairing code", the countdown, `pair_addr`, the QR payload in `<details>`, and `pairStatus` polled every 1 s.
- **Enter a code**: address plus code, or a pasted `common-pair:v1:` payload; button "Join".
- **Session survives the dialog.** `pair/start` returns 409 while a window is open, `pair/status` does not return the code, and there is no cancel route. So the session (code, address, payload, `expires_at`) lives in the app-level `pairing` store (§5.3), not in dialog state. Esc or Close hides the dialog; reopening it resumes the same code and countdown with no second `pair/start`. The store clears at `expires_at` or when `pairStatus` reports a final state.
- Pairing a device that is in `revocations[]` shows the Revoke-aware confirmation first ("You revoked this device on <date>; pairing again trusts it again").
- Windows only: the Firewall notice (below) sits inside the dialog.

**Firewall (Windows).** `firewall_status()` cannot prove a block: a localized Windows, a non-ASCII path, a broader rule, a disabled or third-party firewall all read as "not allowed". So the wording is "No Windows Firewall rule for the service was found; other devices may be unable to connect", warn tone (not danger), with **Allow…**. It appears as a banner above the Devices table, in the Pair dialog, in Settings › Network and on the Dashboard Attention list. It is never in the status bar, and no error is ever relabelled as a firewall problem.

**Inspector tabs and fields:**

| Tab | Self | Paired peer | Nearby / Revoked |
|---|---|---|---|
| Overview | Name (edit lives in Settings › Network), id (8×8 grouped, copy), short label, service version, uptime, "started by this app" (`spawned`), State / Can run there with detail | Name, Trust, State plus `last_error` (message, hint, addr, time), Can run there plus the `promptReadiness` text and last request outcome, Can use this computer with its reason, id (8×8, copy), short label, paired at, last seen, groups, software (E1: `common_version`, `engines[]`, `protocol_minor`). A separated **Danger** group: Revoke. | Nearby: id, short label, `addrs[]`, "announced a pairing window <age> ago" (`pairing`; the cached record can outlive the window), `proto`, seen. Revoked: `peer_id`, `at_utc`, `revoked_by` (resolved name or short label), `seq`, and "announced nearby <age> ago" when announcing. |
| Hardware | `/api/hardware` in full: chip, OS, CPU (cores/threads, features), memory (total, available, unified), GPUs table (name, vendor, backend, cores, memory; `—` where optional fields are absent), backends table (available plus reason), disk free, probe time, a "Re-probe" button, and the raw JSON `<details>` (keeps F-10-04 parity with `common hardware --json`) | `capabilities.hardware` with its own layout. Header: "Hardware and free memory measured when <name>'s networking started (time not reported). Sent and signed at <issued_at_utc> when this connection opened. Not independently verified." "Available memory", "free memory" and "disk free" each carry the visible tag "at its network start". If `caps === null`: "Not reported while not connected." If connected but `caps.hardware === null`: "Sent in a format this version cannot read." If E1 is missing: "Not reported by this service version." | — |
| Models | Downloaded models: id, size, loaded state | `capabilities.models[]`: id, or "Unknown model `<sha256[:12]>…` (not in this computer's catalog; cannot be run from here)". Caption: "Reported when this connection opened; models pulled or removed there since then are not shown." | — |
| Performance | Measured here: `loaded.device_actual`, offload layers, `n_ctx`, threads, `parallel_slots`, `load_ms`, `context_note`, `loaded_for` (E2). From E3 recent jobs on this computer, grouped by (model, `device_actual`): runs, median TTFT, median decode tok/s, captioned "last N finished runs since the service started". Without E3: "Not reported by this service version". | Table 1, from E3 recent jobs with this target, grouped by model: runs and median decode tok/s, captioned "measured by <name>'s engine; GPU/CPU not reported". No median TTFT in this table (as built, 2026-10-22): a job's TTFT is timed on this computer from arrival, so for another device it would mix the network and that device's queue and load into an "engine" figure; the screen says so. Table 2, separate and below it, from audit `remote_request` finished events for this peer, grouped by model: count, outcomes, tokens/s = `completion_tokens / duration_ms`, captioned "end to end from this computer, including queue, load, prompt and network". The two figures never share a table. No benchmark numbers. | — |
| Connection | Networking on/off, `listen_addr`, `dial_addr`, mDNS, devices connected (`live` count) and `connections`, serving devices, Windows Firewall status with Allow (Tauri) | State word; E1 `remote_addr`, direction, open since (`opened_at_utc`), RTT, bytes tx/rx with since-open rates, lost packets; known addresses with provenance (seen nearby with `addrs[]` and age, last known `last_addr`); `last_error`; **Connect** form (optional `ip:port`, validated; the address is only a hint because the pinned key decides), **Disconnect** (E5, tooltip "Until you connect again or the service restarts") | Nearby: addrs, Pair… |
| Permissions | What this computer grants all devices: networking, sharing (`serve_peers`), `peer_quota` (requests/min, tokens/min, max concurrent; read-only, "set in the service configuration; applies after restart"), `peer_slots`, privacy mode. Link to Settings › Sharing. | **What this computer grants this device**: the Can use this computer word and reason, each group containing it with its "Members may run prompts for each other" switch (`PUT /api/groups`, the whole group, other fields preserved), plus add to / remove from a group (`GroupMembership`). **What this device grants this computer**: "Not reported. Its last answer to a request from here:" plus the last `remote_request` outcome and time, or "No request tried yet". | — |
| Activity | Audit events without `peer_id`, plus local E3 jobs | Audit events with `peer_id === id`, plus E3 jobs for this target. Same table component and rules as §2.6. | Audit events for that id |

**Empty states**:
- Networking off: "Networking is off. Paired devices can't connect and nearby devices aren't listed." with a [Turn on] button.
- On, nothing paired: "No paired devices." with [Pair a device…].
- Nearby scope empty, with the existing reasons: "discovery is off", "networking is off", "this network may block discovery (mDNS); pair with an address instead".

### 2.3 Models (`#/models[/<id>]`; keeps `?confirm=<id>`)

**Model state derivation** (first match):

| Rule | Word | Tone |
|---|---|---|
| `pulls[id]?.state === "downloading"` | Downloading 42% (`downloaded/total`; "resumed from X") | active |
| `status.engine.loading_model_id === id` (E2) | Loading · 12 s | active |
| `m.loaded` | Loaded · `status.loaded.device_actual` (+ warn icon "CPU fallback: <reason>" if `fallback_reason`). Note when `loaded_for` is a peer: "for a request from <device>". | ok |
| `pulls[id]?.state === "error"` | Download failed: <errors.ts title> | danger |
| `m.downloaded` | Downloaded | neutral |
| otherwise | Not downloaded | neutral |

**Columns:**

| Column | Source | Sort | Default |
|---|---|---|---|
| Name | `name`; `id` in the detail; a "Recommended" tag when tagged | natural | shown |
| State | above | state order | shown (live) |
| Size | `bytes` | number | shown |
| Params | `parameters` | natural | shown |
| Quant | `quantization` | string | shown |
| Fit | `fit.verdict`: "Fits · X headroom" (ok) or "Too large · needs X of Y" (warn). Visible note "est. at service start"; full basis (`estimate_basis`, n_ctx ≤ 4096) in the detail and the Memory tab. | headroom (negative for too_large) | shown |
| Memory | `estimate.total_bytes` at `estimate.n_ctx` | number | shown |
| On devices | "This computer" if downloaded, plus connected peers whose `capabilities.models[].id === m.id` (E1), with the visible note "reported at connect". Note "+N not reported" for trusted devices that are not connected. | count | shown |
| Licence | `license` | string | shown |
| Family | `family` | string | hidden |
| Source | `source` | string | hidden |
| Redistribution | `redistribution` | string | hidden |
| ID | `id` | string | hidden |

**Scope bar:** All, Downloaded, Loaded, Downloading, Not downloaded, Too large. The filter box searches name, id, family, quant, licence and tags. This is the existing F-10-02 search, kept.

**Toolbar:**

| Group | Action | Enabled when | Call / notes |
|---|---|---|---|
| Selection | Download… | not downloaded, not downloading, `pulls` count < 2 (the server allows ≤ 2) | Opens a `<dialog>` with licence, size, source and redistribution, "View licence" (opener), and the Confirm button focused. Same content and order as today. `startPull(id)` puts progress in the app-level `pulls` store, so it survives navigation (fixes the double-pull debt). **No Cancel**: no route exists, and the dialog says "Downloads can't be cancelled; a stopped download resumes next time". |
| Selection | Load / **Load (replaces <current>)** | downloaded, not loaded, nothing loading | `POST /api/models/{id}/load {device, n_ctx}` with the Settings defaults. When another model is loaded the label names it and an inline confirmation appears; while sharing is on, it adds "Paired devices' requests can load a different model here." The Locations tab offers a one-off device and context. While waiting the row shows Loading (E2, or optimistically). Then focus moves to "Open in Chat". |
| Selection | Unload | loaded | `POST /api/unload`. `overloaded` maps to "In use by N requests; try when they finish" via `errors.ts`. |
| Selection | Open in Chat | downloaded | navigates to `#/chat?model=<id>` |
| Selection | Delete… | downloaded | Inline confirmation (`role=group`): "Removes the file; the catalog entry stays". `DELETE /api/models/{id}`. |
| More ▾ | Import GGUF… | daemon reachable | `<dialog>` with an absolute path field, optional id and licence. `POST /api/models/import`. A native file picker would need `plugin-dialog` and is deferred (§6). |

**Inspector tabs:**

| Tab | Fields |
|---|---|
| Overview | id, name, family, parameters, quantization, size (human plus exact bytes), licence plus View licence, source, redistribution, tags, chat-capable |
| Memory | FitBar (existing component: weights / KV / buffers / overhead vs available), fit sentence (`fitText`), `estimate` breakdown, `n_ctx`, `estimate_basis`, the startup-snapshot caveat |
| Locations | This computer: downloaded/loaded; if loaded: `device_actual`, offload layers/total, `n_ctx`, threads, slots, `load_ms`, `in_flight` (E2), `context_note`, "loaded for" (E2); a one-off Load form with device and context. Each trusted device: Has it (reported when its connection opened), Not reported (with reason: not connected, or service lacks E1), or Doesn't report it. "Loaded there" is **not reported** (needs N2). |
| Download | Active pull: progress bar, `downloaded_bytes/total_bytes`, `network_bytes`, `resumed_from`, elapsed. Otherwise "Not downloading". Partial resumable size: "not reported" (N4). |

**Empty states:**
- No catalog match for the filter: "No models match "<q>"", with a [Clear filter] button.
- Catalog empty: "No catalog entries; import a GGUF file", with [Import GGUF…].

### 2.4 Jobs (`#/jobs[/<id>]`; alias `#/performance`)

Source: `GET /api/jobs` → `{active: Job[], recent: Job[]}` (**E3**), polled every 2 s while Jobs, Dashboard or Activity is visible and every 5 s otherwise (the status bar Sharing item needs it). Without E3 (404), the page shows the slot strip plus "This service version does not report jobs", and nothing else. No client-made rows.

**Header strip** (one line above the table): `Slots 1 of 4 in use · devices may use up to 3 · waiting 0 of 32 · rejected since service start: 2 (queue full 2, timed out 0)`. Sources: `status.queue`, `network.peer_slots`, and `/metrics` `common_requests_rejected_total{reason}` (polled every 5 s on Jobs only).

To the right, one **token-rate sparkline** with three series: on this computer (local target, local origin), sent to devices (`target` is a peer), served for devices (`kind === "served_for_peer"`). Each sample is the change in summed `completion_tokens` over active and newly finished jobs between two job polls, divided by the actual interval. Requires E3 live `completion_tokens`; without it the sparkline is absent. Gaps are stored as null. Caption: "Tokens/s, sampled while Dashboard, Jobs or Activity was open, last ≤ 1 h." No `/metrics` token series exists anywhere.

**State words**, all real E3 states. There is **no position column, no reorder and no priority**.

| `state` | Word | Tone |
|---|---|---|
| queued | Waiting for a slot | neutral, clock |
| connecting | Connecting to the device (added 2026-10-22; a service from before it reports this as `queued` with a `peer:` target, which reads the same) | active |
| loading | Loading model | active |
| running | Running | active |
| done | Done (`finish_reason` in the detail; served rows with outcome `ended` read the same) | ok |
| cancelled | Stopped (`client_gone` reads "Requester disconnected") | neutral |
| error | Failed: <errors.ts title of `error_code`> | danger |

**Columns:**

| Column | Source | Sort | Default |
|---|---|---|---|
| State | above | running, loading, queued, error, cancelled, done | shown (live) |
| Model | `model_id` | natural | shown |
| Runs on | `target === "local"` → `This computer · <device_actual>`; `peer:<hex>` → peer name (`deviceRows` lookup), visible note "GPU/CPU not reported" (N13) | string | shown |
| Requested by | `origin === "local"` → **This app** when `client_ref` matches `ownStreams` or a `chatHistory` message (falling back to `id`), else **This computer (another API client or not matched)**; `peer:<hex>` → peer name (served for that device) | string | shown |
| Started | `started_at_utc ?? created_at_utc` (relative; absolute in the detail) | time | shown |
| Wait | `queue_ms` | number | shown |
| TTFT | `ttft_ms` | number | shown |
| Duration | `duration_ms`; active rows: live elapsed `now − started_at`, note "so far" | number | shown (live) |
| Tokens | `completion_tokens`; active rows: the live count from E3, note "so far" (prompt tokens in the detail) | number | shown (live) |
| tok/s | finished: `decode_tokens_per_second`; active: Δ`completion_tokens` / Δt between the last two polls, note "so far" | number | shown (live) |
| Result | `finish_reason` or `error_code` | string | hidden |
| Kind | `kind` (Chat, Completion, Embeddings, Served for device) | string | hidden |
| Job ID / Request ID / Client ref | `id` / `request_id` / `client_ref` | string | hidden |

**Scope bar:** **All** (default; active rows first by state order, then recent), Active, Recent, Failed, Served for devices (`kind === "served_for_peer"`). The filter matches model, device and id.

**Toolbar:**

| Group | Action | Enabled when | Call |
|---|---|---|---|
| Selection | Stop | row is active **and** `ownStreams[client_ref]` exists (a stream this app started, registered before the request is sent) | `ownStreams[ref].stop()`, which aborts the fetch. The daemon cancels on disconnect, and a peer run sends Cancel. This is the only cancel. For other rows the button is disabled with "Only requests started in this app can be stopped". There is no cancel-by-id route (N1). If the E3 test shows that an abort while `queued` does not cancel, a queued own row's button reads "Stop (takes effect when it starts)". |
| Selection | Open in Chat | own row | `#/chat` scrolled to that reply |
| Selection | Show device | `target` or `origin` is a peer | `#/devices/<id>` |

Copy ID is in the context menu and on the copy shortcut.

**Inspector** (two tabs):
- **Overview** shows every field above. An error shows the ErrorNotice (title, next step, code, hint).
- **Timing & route**:
  - A phase bar drawn only from measured, non-null values: wait (`queue_ms`), load (`load_ms`), prompt (`prompt_ms`), decode (`decode_ms`), total `duration_ms`. Unmeasured phases are omitted, not estimated.
  - Route: `target`, `origin`, `request_id`, the group(s) whose policy allowed it (served rows: groups containing the requester with `allow_remote_inference`), and the executing device's hardware summary (E1 for peers, with its "at its network start" tags).
  - Persistence note: "Jobs are kept in memory (last 256) and cleared when the service restarts. Requests to and from other devices also remain in Activity."

**Empty state:**
- All/Active scope: "Nothing is running. Requests from Chat, other programs on this computer and paired devices appear here."

### 2.5 Chat (`#/chat`)

**Layout:**
- Toolbar: the h1 "Chat"; then the **Destination** and **Model** selects (each its own Tab stop, outside `role=toolbar`); then the action buttons Load (only when needed), New conversation (clears, with confirmation if non-empty) and Stop.
- Main: the transcript, `role=log`, full height, with text in a 72ch column.
- Right **Run panel** (280 px, collapsible with Mod+I): destination summary and last-reply metrics.
- Composer at the bottom.

**Session controller (`src/sections/chat/session.ts`, owner D).** A module-level controller owns the in-flight reply: the `client_ref`, the `AbortController`, the partial text and the history writes. The Chat view only subscribes. So a reply keeps streaming when the user navigates away, and Jobs → Stop (via `ownStreams`) saves `stopped:true` with the partial text.

**Own-request identity.** `api.chat` generates `client_ref = crypto.randomUUID()`, registers `ownStreams[client_ref]` **before** `fetch`, and sends the header `X-Common-Client-Ref` (only when `status.engine` is present, i.e. the service is E2 or later and allows the header in CORS, §4 E2). The id from the first chunk is still captured as `job_id`. Both are stored on the history message.

**Destination** (the `runTarget` store; the existing RunOn becomes `DestinationPicker`):
- Options:
  - "This computer".
  - Each trusted device that is `live` and whose `promptReadiness.ok` is true, labelled with its Can run there word (for example "Laptop · Can send · last request done", or "Laptop · Refused last request (its permissions)"). These are enabled: only the other device knows whether it will serve.
  - Other trusted devices are listed as disabled options, with the reason, for example "Laptop: not connected", "Laptop: not responding" or "Laptop: not permitted here: add it to a group that allows prompts".
- **Local only** privacy: the picker is replaced by the text "Local only: replies run on this computer" and the target is forced to local inside Chat (existing enforcement and test).
- If the selected peer leaves `live`, the target falls back to local with an announced notice (existing).

**Model:**

| Destination | Model list | Notes |
|---|---|---|
| Local | downloaded models | If nothing is loaded, a Load button uses the Settings defaults. Switching to another downloaded model while one is loaded offers "Load <model> (replaces <current>)", with the sharing caveat from §2.3 when sharing is on. |
| Peer (with E1) | first, models in `capabilities.models` with a known id ("reported when the connection opened") | Then a separate optgroup, "Not reported by <device>", with the rest of the local catalog. Choosing one shows the hint "may not be on that device"; a failure keeps the existing plain message "<device> does not have <model>". |
| Peer (without E1) | today's behaviour: every model id this service knows | |

**Per-reply footer** (from the chat result, never estimated):
- Stopped / Done
- Ran on <device> (from the echoed `target`); for local replies, `· <device_actual>` from the E3 row matched by `client_ref`, else from `status.loaded` captured at completion when its `model_id` matches, else `—`
- model
- `completion_tokens` tokens; while streaming, "counted when finished"
- TTFT `timings.time_to_first_token_ms` ms
- `decode_tokens_per_second` tok/s
- a link to the job: `Job chatcmpl-…`, opening Jobs; when the service no longer holds it, Jobs shows "No longer held by the service (cleared at restart or past 256)"

For peer runs the timing label reads "measured by <device>; GPU/CPU not reported". All timing fields are nullable on the wire, and null renders as `—`.

**Run panel:**
- *Destination*: State and Can run there words, hardware summary (self hardware, or E1 with its tags), loaded model and `device_actual` (local), RTT (E1).
- *Last reply*: prompt and completion tokens, TTFT, `prompt_ms`, prompt tok/s, `decode_ms`, decode tok/s, `finish_reason`.
- No per-conversation aggregate: a conversation spans destinations and models, so one median would mix them.

**History:**
- One conversation, kept in localStorage `common.chat` (unchanged key; reads and writes in try/catch). Each message gains `client_ref` and `job_id` when known.
- Settings › Privacy states what is kept where (§2.7) and offers "Delete chat history on this computer".

**Must keep (tests):** incremental streaming; Stop under 300 ms; the stopped flag with partial text saved; "Ran on"; local-only enforcement; remote runs with nothing loaded locally; the role names "Message", "Send" and "Stop".

**Empty and error states:**
- Service not running: the composer is disabled with the reason.
- Nothing downloaded: "Download a model first", with a link to Models.
- In-stream errors: an ErrorNotice in the reply slot (existing behaviour).

### 2.6 Activity (`#/activity[/<key>]`; alias `#/diagnostics`)

Sources:
- `GET /api/audit?n=1000`, every 5 s while visible and every 30 s otherwise (for the Dashboard and the Devices Activity tab). Events arrive oldest first and are shown newest first.
- E3 `recent` jobs, merged in as **Execution** rows. A job whose `request_id` matches a `remote_request` event is not added (the audit row already covers it), so Execution rows are effectively local jobs. Their category caption reads "Local jobs since the service started".

Everything is filtered client-side.

**Row keys** are stable across polls: an audit row is keyed by `ts_utc|kind|peer_id` of the oldest event in its collapsed run; an Execution row by `job|<id>`. `#/activity/<key>` selects by that key.

**Event → row mapping** lives in `src/sections/activity/describe.ts`. It is one table-driven function, unit-tested per kind:

| kind | Category | Severity | Sentence |
|---|---|---|---|
| `pairing_started` | Pairing | Info | Pairing window opened for 120 s |
| `pairing_succeeded` | Pairing | Info | Paired with <device> (<role>) |
| `pairing_failed` | Pairing | Warning (Info if `reason == cancelled`) | Pairing failed: <reason title> |
| `pairing_locked` | Pairing | Warning | Pairing locked after N wrong codes |
| `peer_connected` | Connection | Info | Connected to <device> (<direction>) at <addr> |
| `peer_disconnected` | Connection | Info | Disconnected <device>: <revoked \| unpaired \| by you (E5)> |
| `peer_lost` (E4) | Connection | Warning | Lost connection to <device>: <reason> after <connected_s> |
| `peer_rejected` | Connection | Warning (Info for `reason:"held"`) | Refused/failed: <reason title> (<addr>) |
| `peer_revoked` / `peer_removed` | Trust | Info | Revoked / unpaired <device> |
| `network_changed` / `privacy_changed` / `group_updated` | Settings | Info | the change, from → to |
| `remote_request` (client, `sent`) | Remote run | Info | Sent <model> to <device> |
| `remote_request` (client or server, `finished` or with an outcome) | Remote run | Info if `done`/`ended`, Warning if `cancelled`/`client_gone`/`incomplete`, Error otherwise | <model> on <device>: <outcome>, N tokens in X s |
| E3 job (Execution) | Execution | Info if `done`, Warning if `cancelled`, Error if `error` | <model> on this computer: <state word>, N tokens in X s |
| `audit_rotated` | Maintenance | Info | Log rotated |
| unknown kind | Other | Info | the kind verbatim, with fields in the inspector |

- **Duplicate collapse** applies only to `peer_rejected`, `pairing_failed`, `peer_connected`, `peer_lost` and `audit_rotated`: adjacent events (newest first) with the same `kind`, `peer_id`, `fields.reason`, `fields.outcome` and `fields.addr` merge into one row with a "×N" count, a first time and a last time. This contains the redial flood from evidence defect 4. `remote_request` and Execution rows are never collapsed, because each is a separate job.
- **Columns:**
  - shown: Time (relative; absolute UTC and local in the detail), Severity, Category, Event (sentence), Device (resolved name, short label, or "This computer"), Code (`reason`/`outcome`), Count
  - hidden: Kind, Request ID, Address
- **Scope bar:** All, Warnings and errors, Connections, Pairing, Remote runs, Execution, Settings. A Device select (own Tab stop) sits beside the filter box.
- **Toolbar:**
  - Section: **Copy diagnostics** (the existing redacted bundle with the status and hardware fallback, labelled "never your messages").
  - Selection: **Show device**, **Show job** (when `request_id` or the Execution id matches an E3 row).
  - More ▾: **Error codes…** (a dialog with the full `errors.ts` table, F-10-06).
  - Copy event is in the context menu and on the copy shortcut.
- **Inspector** (single panel, no tabs): every field as a key/value list and the raw JSON.
- **Privacy:** the audit never holds prompt or completion text; the daemon refuses those keys. E3 holds ids, counts and durations only. The UI adds no client logging.
- **Empty state:** "No activity yet. Pairing, connections and requests to or from other devices are recorded here; local jobs appear until the service restarts."

### 2.7 Settings (`#/settings[/<page>]`; aliases `#/privacy` → `settings/privacy`)

**Layout:**
- A page list on the left inside main (`<nav aria-label="Settings pages">`), with the form on the right, max 720 px wide.
- No inspector.
- Controls apply immediately: each change calls its route and shows an inline `role=status`, except the privacy mode, which needs acknowledgement.
- A setting that is supported but read-only shows its value with "Configured in the service settings file; applies after restart".

| Page | Controls | Source / call |
|---|---|---|
| General | Appearance: System / Light / Dark (`ui.theme`, sets `data-theme` and the window theme). About: app version, service version, service address (read-only), started by this app or not. | `ui` store, `daemon`, `daemon_info` |
| Execution | Default device Auto/CPU/GPU; context length (null = model default). Both are client-side, sent with each load (existing). Caption: "Applies to models this app loads; loads requested by other programs or paired devices use the service defaults." Admission limits (read-only): `max_concurrent`, `max_queue`. | `settings` store; `status.queue` |
| Network | Networking switch; device name (pairing name override, placeholder `network.device_name`); discovery (mDNS) on/off (read-only); `listen_addr`, `dial_addr`; Windows only: the Firewall notice with the §2.2 wording and "Allow on private networks" | `PUT /api/network`; Tauri `firewall_status` / `allow_through_firewall`; `FirewallNotice` (`src/ui/`) |
| Sharing & permissions | "Run requests for paired devices" (`serve_peers`, disabled while networking is off, with the reason); device quota (read-only); `peer_slots` (read-only); **Groups** editor (`GroupsEditor`, `src/ui/`): create, members, "Members may run prompts for each other". There is no group delete route, so the page says "Remove all members to stop a group applying" (N9 deferred). The pooled flag is **not shown** (pooled inference does not exist) and is round-tripped unchanged on PUT. | `PUT /api/network`; `/api/groups` |
| Privacy | Mode: Local only / Trusted devices, with the disclosure and "I understand" before PUT (existing flow). There is no "Public compute" control: no backend exists, and F-10-10's "public (when gated)" is met by its absence until a gate exists. What is kept: "The service never stores prompts or replies. It keeps request details (model, device, tokens, timing) in memory until it restarts. It also keeps a log of pairing, connections and requests to and from other devices (`audit.jsonl`). Your conversation is stored only in this app on this computer." Action: "Delete chat history on this computer", which clears `common.chat` after confirmation. | `/api/privacy` |
| Security | This device: id grouped 8×8 for comparison, short label. Revoked devices table: short label, revoked at, by, seq (`revocations[]`). | `/api/network`, `/api/peers` |
| Storage | Disk free; total size of downloaded models (sum of `bytes` where `downloaded`). Model folder: "not reported by the service" (COMMON_HOME is not served). | `hardware`, `models` |

**Absent by design:** contributor, credit, public compute, Swarm and Cloud controls; the API key entry (nothing sets it today; YAGNI); notifications.

---

## 3. Design system

### 3.1 Colour tokens

Tokens live in `src/ui/tokens.css`. `:root` holds light, overridden by `@media (prefers-color-scheme: dark)` under `:root:not([data-theme="light"])` and again under `:root[data-theme="dark"]`. The ratios below were computed with the WCAG 2.x formula (`tools/contrast.py` in this scratch folder) and are enforced by `tokens.test.ts`.

| Token | Dark | Light | Use |
|---|---|---|---|
| `--bg` | `#101412` | `#ECF0ED` | rail, status bar, app background |
| `--surface` | `#151A17` | `#FFFFFF` | tables, panels, inspector body |
| `--surface-2` | `#1B211E` | `#F4F6F4` | toolbar, scope row, table header, inspector tab bar |
| `--hover` | `#212925` | `#EBF1EC` | row/button hover |
| `--selected` | `#1D3A2B` (deep green) | `#DDEEDC` | selected row, active rail item |
| `--line` | `#2A332E` | `#D3DBD5` | hairlines (decorative) |
| `--line-input` | `#76857D` | `#7A867E` | input, checkbox and button borders (≥ 3:1 on every surface, including `--selected`) |
| `--text` | `#E4EBE6` | `#121813` | primary text |
| `--muted` | `#A3B0A8` | `#4F5D54` | secondary text, notes, idle/neutral state |
| `--faint` | `#7E8B84` | `#6B776F` | **disabled only** (exempt from contrast) |
| `--accent` | `#B5F25C` (lime) | `#1D6B43` (deep green) | focus ring, primary button fill, links, selected-row 2 px inset bar |
| `--accent-ink` | `#0F1A0B` | `#FFFFFF` | text on `--accent` |
| `--lime` | `#B5F25C` | `#B5F25C` | rail active indicator and count badges (light theme: always with `--lime-ink #0F1A0B` text) |
| `--brand` | `#2F8F5B` | `#1D6B43` | brand mark |
| `--ok` | `#7FDC8E` | `#16723A` | Connected, Done, Fits, Available |
| `--active` | `#B5F25C` | `#3F6F0C` | Running, Downloading, Loading, Connecting, Serving |
| `--warn` | `#F2C14E` | `#8A5A00` | Not responding, Reconnecting, Unreachable, Busy, Refused, Too large, fallback, firewall rule not found |
| `--danger` | `#FF8A80` | `#B3261E` | Failed, Revoked, service not running |

Measured minimum contrast across `bg`, `surface`, `surface-2`, `hover` and `selected`:

| Pair | Dark | Light | Target |
|---|---|---|---|
| `text` | 10.24 | 14.87 | ≥ 4.5 |
| `muted` | 5.51 | 5.73 | ≥ 4.5 |
| `ok` | 7.40 | 4.95 | ≥ 4.5 |
| `active` | 9.35 | 4.97 | ≥ 4.5 |
| `warn` | 7.39 | 4.89 | ≥ 4.5 |
| `danger` | 5.44 | 5.40 | ≥ 4.5 |
| `accent-ink` on `accent` | 13.49 | 6.49 | ≥ 4.5 |
| focus ring (`accent`) vs `surface` / `selected` | 13.27 / 9.35 | 6.49 / 5.36 | ≥ 3 |
| `line-input` vs all five surfaces (min is on `selected`) | 3.20 | 3.13 | ≥ 3 |

Lime never carries text or required meaning on a light surface: lime against white is 1.33:1.

Other tokens: `--shadow-pop` (menus, dialogs and tooltips only): `0 8px 28px rgb(0 0 0 / .45)` dark, `/ .16` light. `--r-1: 4px` for controls, `--r-2: 6px` for menus and dialogs. Pill shapes are used only for count badges.

### 3.2 Typography

- `--font-ui`: `-apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif`.
- `--font-mono`: `ui-monospace, "SF Mono", "Cascadia Mono", Consolas, Menlo, monospace`. Used for ids, addresses and codes.
- No bundled or web fonts. The CSP is unchanged and the look stays native.
- Scale:

| Size | Use |
|---|---|
| 11 px | status bar, badges, cell notes |
| 12 px | table headers (600, muted), scope bar, inspector labels, captions |
| 13 px | body and cells |
| 15 px / 600 | section h1 |
| 32 px | the pairing code (the only large type) |

- Weights 400, 500 and 600. Line height 1.35 for UI and 1.5 for prose, with prose capped at 64ch.
- `font-variant-numeric: tabular-nums` on tables, the status bar and metrics. Numeric columns are right-aligned.

### 3.3 Spacing, density, sizes

- Spacing scale: 4 / 8 / 12 / 16 / 24 / 32 (`--s1`…`--s6`).
- **Styles:** A owns `tokens.css` and `base.css`; `base.css` is frozen at the end of Phase 0. Each section owns `src/sections/<x>/<x>.css`. Inline `style` is allowed only to set CSS custom properties for measured geometry (FitBar, Sparkline, phase bar, splitter, column widths). The 47 existing inline styles are removed with their views.
- Fixed heights: row 28, header 28, toolbar 44, scope row 30, inspector tab bar 30, status bar 26, rail item 30, icon 16, icon button 28 × 28.
- Every pointer target is at least 24 × 24 (WCAG 2.2 2.5.8).
- Inspector: defaults as in §1.1, minimum 140 px, collapsed 30 px.
- The window minimum stays 720 × 480, and every section must work there: rail collapsed, actions icon-only, status bar items dropped per §1.2, inspector collapsible, table scrolling horizontally inside its own scroller.

### 3.4 Components (`src/ui/`), the shared contract

```ts
type Tone = "ok" | "active" | "warn" | "danger" | "neutral";

interface Column<R> {
  id: string; header: string;
  cell: (r: R) => ReactNode;
  sort?: (r: R) => string | number | null;   // null = unknown → always last
  align?: "start" | "end"; width?: number;      // px budget
  hidden?: boolean;                             // hidden by default (column chooser)
  note?: (r: R) => string | null;               // VISIBLE muted suffix; any caveat that changes meaning goes here
  detail?: (r: R) => string | null;             // extra text in the Tip of the focused/hovered cell (aria-describedby)
  live?: boolean;                               // shows "Unknown (not refreshed)" while the frame is stale
}
function DataTable<R>(p: {
  id: string; label: string;                    // persistence key; grid aria-label (contract in 5.3)
  columns: Column<R>[]; rows: R[]; rowKey: (r: R) => string;
  selected: string | null; onSelect: (key: string | null) => void;
  onActivate?: (r: R) => void;                  // Enter / double-click
  onCopy?: (r: R) => string;                    // text for the document copy handler
  menu?: (r: R) => MenuItem[];                  // native context menu items (Tauri only; omitted in browser/tests)
  pinned?: (r: R) => boolean;                   // e.g. this computer first
  stale?: boolean;                              // dims rows, blanks live columns
  limit?: number;                               // Dashboard blocks
  empty: ReactNode;
}): JSX.Element;

type FrameState = "ok" | "connecting" | "down" | "busy" | { stale: { since: number; error: MappedError } };
function SectionFrame(p: { title: string; actions?: ReactNode; selectionActions?: ReactNode; more?: MenuItem[];
  scope?: ReactNode; filter?: FilterProps; banner?: ReactNode; children: ReactNode; inspector?: ReactNode;
  state: FrameState }): JSX.Element;
function frameState(daemon: DaemonState, ...stores: Live<unknown>[]): FrameState;
function Inspector(p: { id: string; label: string; hasSelection: boolean; empty: string; defaultHeight?: "fit" | "38%";
  tabs: { id: string; label: string; render: () => ReactNode }[] }): JSX.Element;
function ScopeBar(p: { label: string; items: { id: string; label: string; count: number }[]; value: string; onChange: (id: string) => void }): JSX.Element; // role=group
function ToolButton(p: { icon: IconName; label: string; onClick: () => void; disabledReason?: string | null; pressed?: boolean; shortcut?: string }): JSX.Element;
function StateBadge(p: { tone: Tone; icon: IconName; label: string; note?: string; detail?: string }): JSX.Element;
function KV(p: { items: { k: string; v: ReactNode; note?: string }[] }): JSX.Element;      // <dl>
function Unavailable(p: { reason: string }): JSX.Element;                                  // "—" + reason, always visible (note style in cells)
function Tip(p: { text: string; children: ReactElement }): JSX.Element;                    // role=tooltip on hover AND focus, aria-describedby; never a title attribute
function Dialog(p: { open: boolean; label: string; onClose: () => void; children: ReactNode }): JSX.Element; // native <dialog>.showModal()
function Sparkline(p: { series: { label: string; values: (number | null)[] }[]; label: string }): JSX.Element; // role=img, aria-label with last/min/max per series; null = gap
function EmptyState(p: { title: string; reason?: string; action?: ReactNode }): JSX.Element;
function GroupsEditor(): JSX.Element;                          // moved from components/Groups.tsx (A, Phase 0)
function GroupMembership(p: { peerId: string }): JSX.Element;  // same file
function FirewallNotice(p: { compact?: boolean }): JSX.Element; // moved from Peers.tsx (A, Phase 0); §2.2 wording
```

`ErrorNotice` (existing) moves to `src/ui/` unchanged.

Table behaviour:
- Two-level sort: clicking a new header demotes the old key to secondary.
- `aria-sort` sits on the primary header.
- Rows update in place by key, and focus stays on the same key across polls.
- No virtualization: at most about 1000 rows (audit).
- No column resize or drag-reorder in v1 (§6).

### 3.5 Iconography

Icons come from **Lucide**, vendored as an inline-SVG subset of about 35 glyphs in `src/ui/icons.tsx`, drawn 16 px with stroke 1.5 and `currentColor`.

- **Licence: ISC**, with the Feather-derived portions under **MIT**. Before vendoring, copy `LICENSE` from the exact pinned Lucide tag into `src/ui/icons/LICENSE-lucide.txt` and re-check it.
- No npm dependency and no CSP change.
- Icons are always `aria-hidden`. They appear with a word, or on an icon-only button that has an `aria-label` and a tooltip.
- No qBittorrent icons. Their binaries include GPLv3+ assets.

Glyph set:
- layout-dashboard, monitor, laptop, cpu, memory-stick, hard-drive, network, radio (mDNS)
- plug, unplug, link, shield-check, shield-off, shield-x
- box (model), download, upload, play, square (stop), trash-2, refresh-cw, search, filter, columns-3, panel-bottom, panel-right, ellipsis (More)
- list-checks (jobs), message-square, activity, settings
- circle-check, circle-alert, circle-x, clock, loader, copy, key-round, users (groups), chevron-down, arrow-down, arrow-up

The brand mark is an original geometric glyph (a lime node on a deep green rounded square), made as an SVG in the repo. It is not derived from any third-party mark.

### 3.6 Motion

| What | Duration / easing |
|---|---|
| hover/press colour | 120 ms `cubic-bezier(.2,0,0,1)` |
| inspector collapse (height) | 160 ms |
| dialog/menu/tooltip fade | 120 ms |
| in-flight action icon (loader) | 1 s rotation, only while a request is actually pending |

Never animated:
- row reordering
- number changes
- state-word changes (no flashing)
- live data
- skeleton shimmer

`@media (prefers-reduced-motion: reduce)` sets every duration to 0 and removes the loader rotation; the word "Connecting…" or "Loading…" still carries the state.

### 3.7 Accessibility (WCAG 2.2 AA, F-10-11)

**Grid (APG grid pattern, cell focus).** A native `<table role="grid" aria-label=… aria-rowcount=…>`.
- Header `<th scope="col" aria-sort>` contains a `<button>` that sorts.
- One cell has `tabIndex=0` (in the selected row, or the first row when nothing is selected); the table is one Tab stop.
- ↑/↓ move to the same column in the next row and select that row (`aria-selected` on the row). ←/→ move between cells of the selected row, so a screen reader reads one cell with its column header instead of the whole row.
- Cell notes are part of the cell text. A cell's `detail` is exposed through `Tip` and `aria-describedby` while focused.
- Verify with VoiceOver and NVDA (§5.5): header announced per cell, selected state, sort.
- Sort changes are announced through the shell live region, for example "Sorted by State, descending".

**Tabs.** The inspector, Pair dialog and Settings use APG tabs (`tablist`/`tab`/`tabpanel`, arrow keys, `aria-controls`).

**Toolbar.** `role=toolbar` wraps only the action buttons, with arrow-key roving. The scope bar is `role=group` (Tab to it, buttons in order). The filter box, switches and selects are separate Tab stops. Disabled actions use `aria-disabled="true"`, so they stay focusable, with the reason in `aria-describedby`.

**Dialogs.** Native `<dialog>.showModal()` handles the focus trap and Esc. Initial focus goes to the primary safe control (Confirm for download, Cancel for destructive actions), and focus returns to the opener on close. jsdom does not implement these: `setup.ts` polyfills `showModal`/`close`/Esc (§5.4), and the trap, inert background and focus return are checked in the real-browser pass.

**Labels.** Every control has a label. Icon buttons name their target, for example "Connect to Laptop" or "Revoke Studio Mac" (existing convention).

**Tooltips.** `Tip` only: shown on hover and keyboard focus, dismissable with Esc, wired with `aria-describedby`. No `title` attributes. Anything that changes a value's meaning is also visible (§0.2).

**Focus.**
- Every focusable element shows a 2 px `--accent` `:focus-visible` outline with a 2 px offset.
- `scroll-padding-top` equals the header height, so a focused cell is never hidden behind the sticky header (2.4.11).
- Contrast targets are as in 3.1 and machine-checked twice: `tokens.test.ts` on the token pairs (including `line-input` on `selected`), and axe `color-contrast` in a real browser (§5.5).

**Live regions.** Only the shell announcer (1.2), the chat log and the existing `role=status`/`alert` outcomes. Tables and the status bar never announce.

**Reduced motion.** As in 3.6.

---

## 4. Backend and telemetry additions

The essential ones are E1 through E5. The rules for every addition:
- Loopback API only.
- Ids, counts, codes and durations only, never prompt or completion text.
- Documented in `crates/common-api/openapi.json`, which the `route_inventory_matches_openapi_document` test keeps equal to `ROUTES`.
- Tested, including a shape test against the UI's golden fixtures (below).
- Multi-daemon tests are **SINGLE-HOST** in their names and evidence, and never satisfy a two-machine criterion.

**Golden fixtures.** A writes `apps/desktop/src/test/fixtures/api/{peers_e1,status_e2,jobs_e3,peers_e5}.json` in Phase 0 from this section, and `mocks.ts` loads them. E adds `crates/common-api/tests/ui_fixtures.rs`, which reads those files (read-only) and asserts each real response has the same keys and JSON types, recursively, ignoring values. A mismatch is resolved by message between A and E, never by one editing the other's file.

### E0. User-facing strings that name old sections (with E2)

`error.rs` ("…in Peers.", "open Diagnostics", "Review Privacy and Network settings"), `mesh.rs:555` ("enter its address under Peers") and `pairing.rs:126` ("allow Common under Peers") name sections that will not exist. E rewords them so they also work in CLI output, for example "in the desktop app's Devices section" and "open Activity in the desktop app, or run `common diagnostics`" (the CLI command exists). A updates `errors.ts` in the same merge, because `errors.test.ts` requires the two to match exactly.

### E1. Peer capabilities and connection details in `GET /api/peers` (no new route, no protocol change)

**Fields, per `peers[]`:**
- `capabilities: { common_version, protocol_minor, engines: string[], issued_at_utc, free_memory_bytes, hardware: HardwareReport | null, models: { sha256, id: string | null }[] } | null`
  - `hardware` comes from a typed parse of `hardware_json`. Garbage becomes `null` (the UI says "Sent in a format this version cannot read"); raw peer JSON is never passed through.
  - `models` resolves `cached_model_sha256` against the local store and catalog.
  - The openapi description states the timing honestly: `hardware` and `free_memory_bytes` are measured when the peer's networking started; `issued_at_utc` is when the record was signed, at this connection's handshake.
- `connection: { id, opened_at_utc, remote_addr, direction: "outbound" | "inbound", rtt_ms, tx_bytes, rx_bytes, lost_packets } | null`, from `PeerConnection` plus quinn `stable_id()`, `rtt()` and `stats()`. `id` lets the client keep byte-rate deltas within one connection.
- Both are `null` while the peer is not connected.

**Why the UI needs it:** the Devices Hardware, Models, Performance and Connection tabs; the Hardware, Models, RTT and ↓/↑ columns; Can run there's connection start; Models "On devices"; the Chat peer model list; the status bar network rate. The data is already received and signature-checked. It is just not served.

**Test (`tests/mesh.rs`, SINGLE-HOST, using the `pair(...)` helper):**
- After connecting:
  - `peers[0].capabilities.hardware.cpu.logical_cores > 0`
  - `engines` is non-empty
  - `models` contains the fixture sha with its id
  - `connection.id` is stable across two GETs
- After one remote completion:
  - `connection.rx_bytes > 0`
  - `direction` matches the dialler
- A pinned but unconnected peer has `capabilities: null, connection: null`.
- `PeerCapabilities` and `PeerConnectionInfo` schemas are added to openapi.

### E2. Non-blocking engine state in `GET /api/status` (and `/api/models`, `/v1/models`)

**Fields:**
- `engine: { state: "idle" | "loading" | "ready", loading_model_id: string | null, loading_for_ms: number | null }`.
- `loaded` gains `in_flight: number`, `loaded_for: "local" | "peer:<hex>"`, `loaded_at_utc`, and `context_note` (already on the wire; add it to openapi and the client).
- No `rss_bytes`: it costs a synchronous `ps` per poll on unix and is never available on Windows (YAGNI; §6).

**CORS.** Add `x-common-client-ref` to `Access-Control-Allow-Headers` in `lib.rs::cors`. The client sends the header only when `status.engine` is present, so an older service never sees a preflight it would refuse.

**Implementation (suggested):**
- `ModelManager` keeps a std-mutex snapshot of `LoadedModelInfo` plus `loaded_for`/`loaded_at`, updated on load and unload, and a `loading: Option<(id, Instant)>` set and cleared around the load.
- Add `ensure_loaded_for(id, opts, origin)`; `ensure_loaded` stays as the `Local` wrapper, so only `remote.rs::execute` changes its call (to `Peer(peer)`). The other callers (`admin.rs`, `openai.rs`, `common-cli/src/run.rs`, tests) are untouched.
- `status`, `/api/models` and `/v1/models` read the snapshot, never the slot mutex.

**Why:** today the 2.5 s status poll and `/api/models` hang for the whole hash-verify plus engine start, so the UI cannot show "Loading X", and a paired device's load is indistinguishable from a local one.

**Test:**
- The test holds `st.manager.loaded.lock().await`. `GET /api/status` with a 1 s client timeout returns 200 with `engine.state == "loading"` or the snapshot. This fails today by timing out.
- A real stories15M load ends with `engine.state == "ready"`, `loaded_for == "local"`.
- SINGLE-HOST: a served request that loads a model on B gives B `loaded_for == "peer:<A>"`.
- A CORS preflight with `Access-Control-Request-Headers: x-common-client-ref` from an allowed origin succeeds.

### E3. Job registry: `GET /api/jobs`

**Response:** `{ active: Job[], recent: Job[] }`, where `recent` is capped at 256 and lives in memory only. It is lost at restart, which the UI says.

**Job fields:**

| Field | Type / values |
|---|---|
| `id` | the response id (`chatcmpl-…` / `cmpl-…`) once known; before the first token, a registry id; for served-for-peer rows, the peer's `request_id` |
| `client_ref?` | from the optional `X-Common-Client-Ref` header (≤ 64 chars of `[A-Za-z0-9-]`, otherwise ignored), on `/v1/chat/completions` and `/v1/completions`. An id, never text. |
| `kind` | `chat` \| `completion` \| `embeddings` \| `served_for_peer` |
| `model_id` | string |
| `target` | `"local"` \| `"peer:<hex>"` |
| `origin` | `"local"` \| `"peer:<hex>"` |
| `state` | `queued` \| `connecting` \| `loading` \| `running` \| `done` \| `error` \| `cancelled` (an unknown state is shown as "State not known to this app" and kept with the active jobs, never as an outcome) |
| `created_at_utc`, `started_at_utc?`, `first_token_at_utc?`, `finished_at_utc?` | timestamps |
| `queue_ms?`, `load_ms?`, `ttft_ms?`, `prompt_ms?`, `decode_ms?`, `duration_ms?` | durations |
| `prompt_tokens?`, `completion_tokens?`, `decode_tokens_per_second?` | numbers. **`completion_tokens` is live on active rows** (counted per chunk by the drop-guard), final on finished rows. |
| `finish_reason?`, `error_code?` | strings |
| `request_id?` | remote runs; joins the audit |
| `device_actual?` | local runs, from the loaded info at admit time |

All the states are real:
- `queued` = waiting in `Admission::acquire`
- `connecting` = a request to a paired device, until it is sent on the connection (added 2026-10-22)
- `loading` = inside `ensure_loaded`
- `running` = generating

**Hooks:** one drop-guard per request in `openai::run`, `run_remote`, `embeddings`, and the served stream in `remote.rs` (`handle_stream`/`execute`). The row is created at the start of the handler, before admission, so `client_ref` is visible while queued and loading. The guard updates on admit, load, every chunk (token count), first token, `Done`, error and drop. Served-for-peer rows come from a registry handle placed in `common-observability`, passed through `remote::ServeOptions`, and finished next to the existing `remote_request` audit line.

**Why:** the Jobs section, the Dashboard "Running now" block, the Jobs sparkline, the Devices Jobs column, Performance tab and "Requested by", the status bar Sharing item, Activity's Execution rows and the Chat → job link. Without E3 only aggregate counts exist.

**Tests (`real_engine.rs` style, stories15M):**
- During a streaming chat there is one active row with `state:"running", target:"local"`, and its `completion_tokens` rises across two polls.
- With `max_concurrent=1` and a second request carrying `X-Common-Client-Ref`, that ref is on a row with `state:"queued"`.
- Aborting that queued request ends its row as `cancelled`; if the server cannot observe the abort until admission, the test asserts and documents that instead (the UI then labels Stop "takes effect when it starts").
- After `[DONE]` the row is in `recent` with `completion_tokens == usage.completion_tokens` and `ttft_ms` set.
- A queue-full rejection appears with `error_code:"overloaded"`.
- The serialized registry never contains the prompt canary.
- SINGLE-HOST: a `peer:` request appears on the sender (`target: peer:…`) and on the server (`kind: served_for_peer`) with the same `request_id`.

**Client-side counterpart (Phase 0, A):** `api.chat` accepts a `clientRef`, sends the header (gated as in E2), and captures `chunk.id` into `ChatResult.id`. The chat session controller (D) creates the ref and registers `ownStreams[ref]` before sending.

### E4. Record involuntary connection losses: audit `peer_lost`

**Fields:** `{reason: "timed_out" | "reset" | "closed_by_peer", addr, connected_s}`, emitted in `health::watch` after the "another live connection" early return.
- **Not** emitted for closes this device made: Node stop (networking off), service shutdown, revoke, unpair, and E5 hold. Those keep `peer_disconnected` where it exists, or nothing.

**Why:** without it, Activity shows every connect but no involuntary disconnect, which misrepresents what the network did. It also feeds the "Lost connection" Attention entry.

**Tests (SINGLE-HOST):**
- Pair and connect, then stop B's network. Within 5 s, A's audit has exactly one `peer_lost` for B, and B's audit has none for A.
- Turning networking off on A produces no `peer_lost` on A.
- The revoke path still yields only `peer_disconnected`.

### E5. Disconnect: `POST /api/peers/{id}/disconnect` and `peers[].held: bool`

**Semantics:**
1. Close every connection to the device. Audit `peer_disconnected` with `reason: "by_user"`.
2. Put it on a **hold** list. The hold is in memory, held by the `Node` (`hold`/`release`).
3. The hold is enforced **inside `Node`** next to both `allowed()` checks (outbound ≈714, inbound ≈790), so no dial path can bypass it: not the sync loop, not `run_remote`'s `st.peer_connection`, not the CLI.
   - A local dial to a held device fails with `policy_denied`, "disconnected by you; connect it first". A chat with `target: peer:<held>` therefore returns that typed error and does not connect.
   - An inbound handshake from a held device is closed with a new `CLOSE_HELD` code (audit `peer_rejected` with `reason: "held"`, deduped like the existing refusals). `close_code()` gains the mapping, so the refusal is not sent as `CLOSE_PROTOCOL`.
   - New builds map `CLOSE_HELD` to a new stable error code **`peer_held`**, message "the other device disconnected this one". Older builds already map unknown close codes to `peer_lost` "closed by peer".
4. `POST …/connect` releases the hold.
5. A networking restart or service restart also clears it. The UI tooltip says so.

**Why:** the owner directive lists Disconnect, and there is no honest substitute. DELETE unpairs, and turning networking off disconnects everything. Without the inbound refusal the other device would simply redial, and the button would lie; without `peer_held` the other device would show a false protocol or firewall error.

**Test (SINGLE-HOST):**
- Pair and connect A and B. A disconnects B. Within 1 s, A shows `connected:false, held:true`.
- After 2 sync ticks plus B's redial attempts:
  - the two are still not connected
  - B's `/api/peers` never showed `connected:true` for A in between (polled)
  - B's `last_error.code == "peer_held"`
  - B's audit has no new `peer_connected` for A
  - A's audit holds a `peer_rejected reason:"held"`
- A chat on A with `target: peer:B` returns `policy_denied` and does not connect.
- A connects to B: `connected:true, held:false`.
- `held` is added to openapi `Peer`; `peer_held` to the error-code list; `CLOSE_HELD` to `control.proto` as a wire-visible comment.

**Order:** E0 + E2 → E1 → E3 (local and outbound first, then served-for-peer) → E4 → E5. The UI feature-detects each addition, so the UI agents never wait on the backend agent. **Release gate:** E2, E1 and E3 merged and green (without E3, Jobs would show less than today's Performance view). E4 and E5 may follow, because their UI is absent without them.

### Not added. The UI shows each of these as unavailable, with this wording.

| Gap | UI text | Revisit when |
|---|---|---|
| N1 cancel by job id | "Only requests started in this app can be stopped" | Jobs needs to stop CLI/script requests |
| N2 peer busy / loaded model / live free memory (`Status` message) | No Busy for peers; "Loaded there: not reported"; peer Free memory "Not reported live (needs N2)" | **owner decision**: it discloses the loaded model to every paired device |
| N3 last-known capabilities of disconnected peers | "Not reported while not connected" | users ask for offline inventory |
| N4 downloads registry / partial size | Downloads tracked by this app only; a reload loses the view (the server continues) | reload or CLI pulls matter |
| N5 `/api/diagnostics` | Copy diagnostics uses the status and hardware fallback (works today) | the bundle is needed in-app |
| N9 group delete | "Remove all members to stop a group applying" | |
| N10 quota edit | quota shown read-only, "applies after restart" | |
| N11 pairing cancel | The session is kept in the app and resumable; it ends at `expires_at` | users need to close a window early |
| N12 next redial time | not shown | |
| N13 remote run GPU/CPU | "GPU/CPU not reported" | |
| Event stream (SSE/WebSocket) | polling at 1–5 s (cheap on loopback once E2 lands) | poll fan-out becomes measurable |
| Device name in mDNS | nearby devices show short labels until paired | owner privacy decision |
| What a peer permits this device | "Not reported" plus the last request outcome | protocol field |

---

## 5. Build plan

### 5.1 Agents and disjoint file ownership

All paths are under `apps/desktop/` unless they start with `crates/`, `proto/` or `docs/`. Each agent works in its own worktree branch off the same base commit (§0 precondition). **Ownership is exclusive.** To change another agent's file, an agent sends that owner a message; it never edits the file itself.

**A. Shell, design system and foundation, plus Dashboard and final integration.**
Owns:
- `src/App.tsx`, `src/main.tsx` (including the dev-only `?axe=1` hook), `src/router.ts`, `index.html`
- `src/styles.css`, which becomes `src/ui/tokens.css` and `src/ui/base.css` (frozen after Phase 0); the old file is deleted at the end
- `src/ui/**` (components in 3.4, `icons.tsx`, the Lucide licence, `contextMenu.ts`, and the moved `Groups.tsx`, `FirewallNotice.tsx`, `ErrorNotice.tsx`)
- `src/shell/**`: Rail, StatusBar, `announcer.ts`, `attention.ts`, `shortcuts.ts`, `copy.ts`, `menu.ts` (macOS), `sections.ts`
- `src/state/store.ts`, `src/state/stores.ts`, `src/state/live.ts`, `src/state/poll.ts`, `src/state/ui.ts`
- `src/api/client.ts`, `src/api/errors.ts`, `src/api/*.test.ts`
- `src/format.ts`, `src/daemon.ts`
- `src/test/mocks.ts`, `src/test/setup.ts`, `src/test/fixtures/api/*.json`, `src/test/a11y.test.tsx` (shell only after Phase 0), `src/test/devdaemon.test.tsx`, `src/test/shell.test.tsx` (new), new `src/ui/*.test.tsx`
- `src/sections/dashboard/**` (including its `.css` and `.a11y.test.tsx`)
- `src-tauri/tauri.conf.json` (window title only), `src-tauri/capabilities/default.json` (`core:window:allow-set-theme` only)
- At the end, deletes `views/Home.tsx` and `components/Later.tsx`

**B. Devices.**
Owns:
- `src/sections/devices/**`: `Devices.tsx`, inspector tabs, `PairDialog.tsx`, `devices.css`, `devices.a11y.test.tsx`
- `src/state/devices.ts` (`deviceRows`, `connState`, `promptReadiness`, `live`, `canUseThisComputer`; created by A in Phase 0, then handed to B) and `src/state/devices.test.ts`
- `src/test/peers.test.tsx`, `src/test/devices.test.tsx` (minus the cases A moves in Phase 0), `src/test/firewall.test.tsx`
- Deletes `views/Peers.tsx` and `views/Hardware.tsx`

**C. Models and Jobs.**
Owns:
- `src/sections/models/**`, including `pulls.ts` (`startPull`, writes the `pulls` store), `DownloadDialog`, `ImportDialog`, `models.css`, `models.a11y.test.tsx`
- `src/components/FitBar.tsx`
- `src/sections/jobs/**` (including `jobs.css`, `jobs.a11y.test.tsx`)
- `src/test/keyboard.test.tsx` (the Models → Chat → Stop journey, using D's contract names)
- Deletes `views/Models.tsx` and `views/Performance.tsx`

**D. Chat, Activity and Settings.**
Owns:
- `src/sections/chat/**`, including `session.ts`, `chat.devices.test.tsx` (moved in by A), `chat.css`, `chat.a11y.test.tsx`
- `src/components/RunOn.tsx`, which becomes `DestinationPicker`
- `src/sections/activity/**` (`describe.ts` plus its test, `activity.css`, `activity.a11y.test.tsx`)
- `src/sections/settings/**` (including `settings.css`, `settings.a11y.test.tsx`)
- `src/test/privacy.test.tsx` (minus the Home case A moves in Phase 0)
- Deletes `views/Chat.tsx`, `views/Diagnostics.tsx`, `views/Privacy.tsx` and `views/Settings.tsx`

**E. Backend.**
Owns:
- `crates/common-api/src/{lib.rs, mesh.rs, admin.rs, openai.rs, jobs.rs (new)}`
- `crates/common-api/openapi.json`
- `crates/common-api/tests/{mesh.rs, api.rs, real_engine.rs, jobs.rs, ui_fixtures.rs, common/mod.rs}`
- `crates/common-core/src/error.rs` (E0 strings, `peer_held`)
- `crates/common-runtime/src/manager.rs`
- `crates/common-network/src/{health.rs, transport.rs, remote.rs, pairing.rs}`
- `crates/common-network/tests/remote_single_host.rs` (its `ServeOptions {…}` literal at ≈745 breaks when E3 adds a field; switch it to `..ServeOptions::standalone()` or add the field)
- `crates/common-observability/src/lib.rs`
- `proto/common/v1/control.proto` (the `CLOSE_HELD` comment only)

E never edits `apps/desktop` (it only reads the golden fixtures).

**Integrator (the lead, after merges):**
- `docs/REQUIREMENTS.csv`, then `python3 scripts/gen_registry.py` and `validate_registry.py`
- `docs/VERIFICATION.md`, `docs/PROGRESS.md`, `apps/desktop/README.md`
- `docs/decisions/ADR-0003-*.md` note on `CLOSE_HELD`/`peer_held` (wire-visible)
- `docs/evidence/desktop-ui2-*.md`

Agents report their evidence in their final messages and do not edit these files.

**Machine budget (this Mac: 8 GB RAM, about 6 GiB free disk, `target/` 18 GB).**
- E builds with `CARGO_TARGET_DIR=/Users/nasr/repos/common/target`, shared and serial. No second target directory.
- Frontend worktrees never run cargo.
- At most two vitest runs at a time.
- Free cache space (owner-authorized) before the `pnpm tauri build` in §5.5 step 4.

### 5.2 Phases

| Phase | Who | Deliverable | Gate |
|---|---|---|---|
| 0 Foundation | A alone (E starts in parallel) | (1) The `client.ts` contract-drift fixes from inventory §5 plus the new types and calls in 5.3; `TimeoutError` maps to a new client code `slow`; `refreshStatus` on `slow` asks `GET /health` and sets `responding:false` instead of `reachable:false`. (2) Golden fixtures and `mocks.ts` with every new route stubbed from them (E1 fields, E2 `engine`/`loaded_for`, `/api/jobs` with `client_ref`, disconnect plus `held`, `peer_held`) and the extension hook. (3) `live.ts` stores (last good data kept), `poll.ts` with injectable `timeoutMs`, the `ui.ts` prefs, the `pairing` store. (4) The `src/ui/` primitives (including `Tip`, `frameState`, moved `Groups`/`FirewallNotice`/`ErrorNotice`), `tokens.css`, `base.css` (then frozen) and empty `src/sections/<x>/<x>.css` files. (5) The router with sections, ids, params, aliases and `replaceState` selection. (6) Stub section files that render the **old views** inside the new shell. (7) `src/state/devices.ts` created from the working-tree `connState`/`promptReadiness` (verbatim) plus `deviceRows`. (8) `setup.ts` `<dialog>` polyfill. (9) The test split (§5.4). (10) Record vitest's reported total at the base commit in `VERIFICATION.md` (the floor). | `pnpm test` green with all existing cases; `pnpm build` (tsc strict) clean; contracts frozen. |
| 1 Sections | A (shell polish, status bar, menu, Dashboard), B, C, D in parallel; E continues | Each section agent replaces its stubs and migrates its tests in the same commit. | Per agent: its own test files (including its section `.a11y.test.tsx`) green; no edits outside its ownership. |
| 2 Integration | A, then the lead | Merge B, C, D and E (disjoint files, so no conflicts). A finalizes the shell a11y test, deletes the old views, `Later.tsx` and `styles.css`, and removes the `runs` store. A and E land E0 and `peer_held` in `errors.ts` with E's merge. | Full `pnpm test` (count ≥ the Phase 0 floor), `pnpm build`, `cargo test`; the verification in 5.5. **Release** additionally needs E2, E1 and E3 merged and green. |

### 5.3 Shared contracts (frozen at the end of Phase 0)

**Router (`src/router.ts`, owner A):**
- `type Section = "dashboard" | "devices" | "models" | "jobs" | "chat" | "activity" | "settings"`
- `useLocation(): { section; id: string | null; params: URLSearchParams }`
- `go(section, id?, params?)` pushes; `select(id, params?)` replaces (`history.replaceState`).
- Aliases:

| Old route | New route |
|---|---|
| `home` | `dashboard` |
| `peers` | `devices` |
| `hardware` | `devices/self?tab=hardware` |
| `performance` | `jobs` |
| `diagnostics` | `activity` |
| `privacy` | `settings/privacy` |
| `cluster`, `contributions` | `dashboard` |

`#/models?confirm=<id>` keeps opening the download dialog.

**Daemon store (`daemon.ts`, owner A):** gains `responding: boolean`. `reachable:false` only when the request failed at the network level and `GET /health` (1 s timeout) also fails.

**Stores (`src/state/live.ts`, owner A).** Each is `Live<T> = { data: T | null; at: number | null; error: MappedError | null }`, where `data` and `at` are the last good refresh and `error` the latest failure:
```ts
network: NetworkState                     mesh: { peers: Peer[]; discovered: Discovered[]; revocations: Revocation[] }
groups: Group[]                           models: Model[]
hardware: { report: Hardware; at: number } jobs: { active: Job[]; recent: Job[] } | "unsupported"
audit: AuditEvent[]                       ready: { loadingStreak: number }              // until E2
samples: Sample[]   // {at, conn: Record<connectionId, {peer, rx, tx}>, tokens: {local, sent, served} | null}, ring of 720
pulls: Record<modelId, { state: "downloading" | "done" | "error"; ev: PullEvent | null; error?: MappedError; startedAt: number }>
pairing: { code; addr; payload; expiresAt: number } | null       // written by B's PairDialog, read by A's StatusBar/toolbar
attention: { id; tone; text; action? }[]                        // dismissable event entries (A's attention.ts)
dialing: string | null                    ownStreams: Record<clientRef, { stop: () => void }>
```
- `refresh(key)` is single-flight with a 5 s timeout (injectable for tests).
- `useNeed(keys)` lets a view declare the polled data it needs. The poller runs the union of all needs plus the always-on set.
- Existing stores stay in `stores.ts` with the same keys: `daemon`, `privacy`, `settings`, `chatHistory` (messages gain `client_ref?` and `job_id?`), `runTarget`, `lastError`. `runs` is removed in Phase 2.

**Poll cadence (`poll.ts`):**

| Data | Interval |
|---|---|
| status | 2.5 s, always (on timeout: `/health` once) |
| `/ready` | 2.5 s, only while `status.engine` is absent (pre-E2) |
| network | 5 s, always |
| mesh | 2 s while any peer is `connected` (so Not responding, which lasts about 2 s, is drawn); 5 s while networking is on; else 30 s |
| privacy, groups | on change, plus 30 s |
| hardware | on start, plus 120 s, plus on demand |
| models | on need, after actions, and 5 s while a pull runs |
| jobs | 2 s on Jobs, Dashboard, Activity and Devices › Activity/Performance; 5 s otherwise (status bar Sharing) |
| metrics | 5 s on Jobs only (rejected counts) |
| audit | 5 s on Activity, otherwise 30 s |
| pair status | 1 s while a pairing session exists (B) |
| firewall | on Devices/Settings/Dashboard mount and after Allow |

**Derivations (`src/state/devices.ts`, owner B after Phase 0):**
- `live(p)`, `connState(...)`, `promptReadiness(...)`, `canUseThisComputer(p, network, groups)`, `deviceRows(input): DeviceRow[]`
- `DeviceRow = { key; trust; name; shortLabel; peer?; discovered?; revocation?; state: { word; tone; detail?; note? }; runThere: { word; tone; detail?; note? } | null; useHere: { word; note? } | null; addr: { value; provenance } | null; caps: PeerCapabilities | null | undefined; link: PeerConnectionInfo | null | undefined; activeJobs: number | null }`
- `undefined` means the service does not report the field, and `null` means not connected.
- Dashboard (A) and Chat (D) consume it.

**Client additions (`client.ts`, owner A):**
- Types: `Revocation`, `PeerCapabilities`, `PeerConnectionInfo` (with `id`, `opened_at_utc`), `Peer.capabilities?`, `Peer.connection?`, `Peer.held?`, `Status.engine?`, `Loaded.in_flight?`, `Loaded.loaded_for?`, `Loaded.loaded_at_utc?`, `Loaded.context_note`, `Job` (with `client_ref?`, `first_token_at_utc?`), `NetworkState.peer_quota`/`peer_slots`, `Model.estimate_basis`, nullable `Timings` (plus `prompt_ms`, `decode_ms`), optional `Hardware` GPU fields and `disk_free_bytes`, `ChatResult.id`.
- Codes: `slow` (client-only, like `unreachable`); `peer_held` (with E5, mirrored from `error.rs`).
- Calls: `api.jobs()` (404 becomes `"unsupported"`), `api.disconnectPeer(id)`, `api.audit(n)`, `api.health()`, `api.ready()`, `api.chat(..., { clientRef })`, `api.importModel` (exists).

**Test hook (`mocks.ts`, owner A):** `installFakeDaemon({ routes?: Record<"METHOD /path", handler> })`, defaulting to the golden fixtures. Section agents keep extra fixtures in `src/sections/<x>/fixtures.ts` and do not edit `mocks.ts`.

**Accessible-name contract** (used by the a11y and journey tests):
- Grids: "Devices", "Models", "Jobs", "Activity"; Dashboard: "Computers", "Running now", "Recent activity".
- Regions: "Status" (footer).
- Links: "Privacy mode: <mode>" (status bar item 8).
- Navs: "Sections", "Settings pages".
- Chat: textbox "Message"; buttons "Send", "Stop"; selects "Destination", "Model".
- Pairing: group "Pairing code"; buttons "Pair this device", "Join".
- Per-device buttons: "Connect to <name>", "Revoke <name>".

**Section contract:** `export default function <Section>(): JSX.Element`, rendering `SectionFrame`. `sections.ts` (A) maps a route to its component.

### 5.4 Test migration (keep every assertion; rewrite selectors)

**Phase 0 split (A).** The current tests cross the ownership lines (`a11y.test.tsx` hard-codes the "Home", "Hardware" and "Peers" headings plus Peers and Models cases; `devices.test.tsx` mixes Chat cases with device cases; `privacy.test.tsx` asserts the sidebar link and RunOn on Home; `keyboard.test.tsx` drives Chat). A moves cases, unchanged except for imports, so each file has one owner:

| From | Case(s) | To (owner) |
|---|---|---|
| `a11y.test.tsx` | per-view cases and the Peers/Models scenario cases | `src/sections/<x>/<x>.a11y.test.tsx` (that section's owner) |
| `a11y.test.tsx` | shell, rail, status bar | stays (A) |
| `devices.test.tsx` | the Chat cases ("offers every model…", "tells the user plainly…", "a local run still needs…") and the plural cases ("one device, two devices, one token", "a one-token reply says 1 token") | `src/sections/chat/chat.devices.test.tsx` (D) |
| `devices.test.tsx` | the 7-route "names this computer neutrally" sweep | `src/test/shell.test.tsx` (A) |
| `privacy.test.tsx` | "RunOn on Home" | replaced by a Dashboard case in `dashboard.a11y.test.tsx`/`shell.test.tsx` (A) |
| `privacy.test.tsx` | sidebar link "Privacy mode: …" | kept; satisfied by status bar item 8's frozen name |

`keyboard.test.tsx` (C) keeps driving Chat through the frozen contract names only.

**Section migrations:**

| File | Owner | Change |
|---|---|---|
| `src/sections/*/…a11y.test.tsx` | each owner | Views become sections, plus the Pair dialog, the Download dialog and Settings › Privacy with the disclosure open. Probes use the accessible-name contract. |
| `peers.test.tsx`, `devices.test.tsx`, `firewall.test.tsx` | B | `.model-name` / `closest("tr")` / cell index becomes `getByRole("row", {name})` plus `within(row)` cells by column header. `.closest(".section")` under "Nearby, not paired" becomes the Nearby scope. All invariant assertions stay: cached mDNS never Connected, self never nearby, one dial at a time, unpaired rows get Pair only, impostor refused, firewall flow. **Deliberate wording change**, listed in B's commit with old and new expectations: "Offline" becomes "Not seen nearby" at `peers.test.tsx:66`, `:199`, `:200`, `:218` and `devices.test.tsx:277`; the firewall notice text changes to the §2.2 wording. |
| `keyboard.test.tsx` | C | `row.querySelector("button.btn-primary")` becomes toolbar `getByRole("button", {name: "Download"})`. Same journey, same Stop < 300 ms bound. |
| `privacy.test.tsx` | D | RunOn becomes DestinationPicker; the "Privacy" heading becomes Settings › Privacy. Same disclosure, ack, local-only and "Ran on" assertions. **Deliberate change**: "public compute is present but disabled and honest about it" becomes "offers no public compute control" (listed in D's commit). |
| `client.test.ts`, `errors.test.ts`, `devdaemon.test.tsx` | A | Extended, never weakened. |

Every removed or renamed case is listed in its commit message with its replacement. The total reported by vitest must not drop below the Phase 0 floor.

**jsdom dialog polyfill (`setup.ts`, A).** jsdom 30.1.2's `HTMLDialogElement` has only `open`. The polyfill: `showModal()` sets `open`; `close()` removes it and fires `close`; Esc inside an open dialog fires `cancel` then `close`. The focus trap, inert background and focus return are not emulated; they are checked in the real-browser pass (§5.5 step 3).

**New tests (minimum).** Each failing-first test proves one rule:
- **A:**
  - DataTable: two-level sort, unknowns last, `aria-sort`/`aria-selected`, cell focus (↑↓ keep column and select, ←→ move cells), notes in cell text, `detail` via `aria-describedby`, layout persistence, stale rows dimmed with live columns "Unknown (not refreshed)".
  - Inspector: collapse via button and Mod+I; clicking the active tab does not collapse; no selection clears it.
  - Poll/daemon: a status timeout with `/health` answering gives "Busy (not answering)", not "Not running"; a models timeout keeps the rows with the "Not updated since" banner; only a refused connection empties tables; single-flight; injected 50 ms timeout.
  - StatusBar: each item from fixtures; one peer with `connected:true` and two connections reads "1 device connected"; an unhealthy connected peer reads "· 1 not responding" in warn and is not counted as connected; Mesh never ok with only discovered entries; network rate gaps on a `connection.id` change and shows "< 1 KB/s" for keep-alive traffic; Sharing reads "Serving 1 request for Laptop" from an active served row; droppable items carry their priority classes; the privacy link name.
  - Attention: pairing open shows the countdown; a failed download adds a dismissable entry; the firewall never appears in the status bar.
  - Copy handler: a focused cell with no text selected copies the row id; with text selected, the browser copy is untouched.
  - Shortcuts: Cmd on macOS, Ctrl on Windows, no double fire.
  - Router aliases; selection uses `replaceState`.
  - `tokens.test.ts`: the contrast pairs in 3.1, including `line-input` on `selected`.
- **B:**
  - Table-driven `deviceRows` covering every row of the Connection, State, Can run there and Can use this computer tables, including: `peer_lost` not announced → Not seen nearby (neutral, no warn dot); `peer_lost` announced → Unreachable (warn); `peer_held` → Disconnected by <name>; unhealthy → `—` in Can run there; last outcome `policy_denied` → Refused last request; `done` → Can send · last request done; a revoked id that still announces → Revoked row, not Nearby.
  - Capabilities `null` shows "Not reported while not connected"; connected with `hardware: null` shows "Sent in a format this version cannot read"; field absent shows "Not reported by this service version"; peer Free memory cell is `—` with its note.
  - Pair dialog: start, Esc, reopen shows the same code with no second `pair/start`; with networking off the first step is "Turn on networking".
  - Disconnect absent without `held`; Revoke only in the context menu and Overview Danger group.
  - Revoked scope; self pinned first and preselected; `#/hardware` selects `self`.
- **C:**
  - Downloads survive navigation, and no second pull starts.
  - Load/Unload/Delete confirmations; "Load (replaces <current>)" with the sharing caveat; Import path validation.
  - Jobs rows from fixtures; All is the default scope; Requested by "This app" via `client_ref` on a queued row; unmatched local rows read "This computer (another API client or not matched)"; live tokens and tok/s "so far" from two polls.
  - Stop only on `ownStreams` rows; absence of any position, reorder or cancel controls.
  - 404 shows the unsupported state; the sparkline is absent without live tokens.
  - The phase bar omits null phases.
- **D:**
  - `describe.ts` per kind: severity and sentence; Execution rows from E3 and de-duplication by `request_id`.
  - Duplicate collapse only for the listed kinds; two finished `remote_request` events give two rows.
  - Stable row keys across a poll that adds events.
  - The session controller: start a reply, go to `#/jobs`, press Stop; history holds `stopped:true` with the partial text within 300 ms.
  - `client_ref` is registered before fetch and sent only when `status.engine` is present; the job id is captured and linked.
  - Esc stops generation.
  - Peer model list from capabilities; destination options use `live` and the Can run there words.
  - The theme persists. The `serve_peers` PUT body. Group pooled flag preserved on PUT. No public compute control. The privacy "what is kept" text.

### 5.5 Verification plan

1. **Unit and UI:**
   - `cd apps/desktop && pnpm install && pnpm test` passes (vitest, jsdom, axe with serious/critical treated as failures), with a reported total ≥ the Phase 0 floor.
   - `pnpm build` passes (tsc strict plus vite).
   - Record both in `docs/VERIFICATION.md`.
2. **Backend:**
   - `CARGO_TARGET_DIR=$PWD/target COMMON_HOME=$PWD/.common cargo test -p common-api -p common-runtime -p common-network -p common-observability -p common-core` passes, including the new E0–E5 tests, `ui_fixtures.rs`, the route inventory ↔ openapi test and the real-engine stories15M tests.
   - `python3 scripts/validate_registry.py` passes.
3. **Real-render contrast and a11y:**
   - Start two SINGLE-HOST dev daemons on loopback ports and pair them, so the warn, danger, connected and refused states actually appear.
   - Run the dev UI (`pnpm dev`) with `?daemon=http://127.0.0.1:<port>&axe=1`. The dev-only `?axe=1` hook (compiled out of production like `devDaemonUrl`) dynamically imports the already-installed `axe-core` and runs it with `color-contrast` **enabled** on every section in light and dark.
   - Save the JSON results to `docs/evidence/desktop-ui2-a11y-<date>/` (labelled SINGLE-HOST).
   - Do a keyboard-only pass: Download → Load → Chat → Stop; Pair (both tabs; Esc and reopen keeps the code); Connect/Disconnect; F6 regions; dialog focus trap, inert background and focus return.
   - At a 720 × 480 window: status bar drop order, icon-only actions, no horizontal page scroll.
4. **Packaged macOS app:**
   - Free cache space first (owner-authorized). Build with `pnpm tauri build` (the sidecar is staged by `build-sidecar`) and install the dmg.
   - Take screenshots of all 7 sections, the Pair dialog and the Download dialog, in light and dark, at 1100 × 720 and at the 720 × 480 minimum.
   - Check: the menu bar (Settings ⌘,, Go ⌘1–7, Edit ⌘C/⌘V in fields, ⌘C on a grid cell copies the id once), the native context menu, ⌘R not reloading the webview, VoiceOver reading the Devices grid cell by cell with column headers, selected state and sort.
5. **Packaged Windows app:**
   - Build natively on the Windows machine. This plan's author does not touch it; the implementer follows the owner's standing authorization.
   - Take the same screenshots and check:
     - no menu bar
     - Ctrl shortcuts work and F5/Ctrl+R do not reload
     - Shift+F10 opens the context menu
     - the firewall notice wording and the Allow flow through UAC
     - the title-bar theme follows the app theme
     - NVDA (or Narrator) reads grid cells with their column headers
6. **Physical two-machine checks** (Mac plus Windows laptop). These are **not SINGLE-HOST**; evidence goes in `docs/evidence/desktop-ui2-two-machine-<date>.md`.
   1. Pair through the Pair dialog on both apps and add each to a group that allows prompts. Each Devices table shows the other with State "Connected", Can run there "Can send" and Can use this computer "Yes". After one remote chat, the sender shows "Can send · last request done".
   2. The Mac's Hardware tab for the laptop equals `common hardware --json` run on the laptop, and vice versa, on static fields only: host, cpu, gpus, backends and `memory.total_bytes`. Record `issued_at_utc`.
   3. Run a remote chat from the Mac. It appears as an active job on the Mac (`target peer:`, live tokens rising) and as Served for device on the laptop, with the same `request_id`; the laptop's status bar reads "Serving 1 request for <Mac>". Activity on both shows `remote_request`.
   4. Pull the laptop's network. Within about 10 s the Mac shows Reconnecting, Unreachable or Not seen nearby (Not responding may appear briefly). Activity shows one `peer_lost` (E4). **Nothing** shows Connected from a cached mDNS record.
   5. Disconnect the laptop from the Mac. The Mac stays "Disconnected by you"; the laptop shows "Disconnected by <Mac>" and its redials are refused. Connect from the Mac restores the link (E5).
   6. The status bar ↓/↑ moves during the remote run and returns to the keep-alive level (< 1 KB/s) after it.
   7. On a Windows account with no rule, the firewall notice appears with the §2.2 wording, and the Allow flow fixes inbound pairing.
7. **Ledger** (integrator):
   - Re-run and record evidence for F-10-02, 03, 05, 06, 10 and 11 against UI 2.0. Their current evidence describes the old views.
   - F-10-04 and F-10-11 stay `IMPLEMENTED_UNVERIFIED` until their own criteria are demonstrated: the automated UI-vs-CLI test, and the packaged app on each OS.
   - New API contracts are recorded with their test names.

---

## 6. Deliberately not built

| Not built | Why / what the UI does instead |
|---|---|
| Queue position column, Move up/down, priority, Force start | The admission queue is FIFO with no reorder API. Only `queued` counts and the "Waiting for a slot" state are shown. |
| Cancel on jobs this app did not start; download cancel; pairing cancel | No route (N1, N11). The download continues server-side; the pairing window ends at its expiry. Stop appears only on the app's own streams. |
| Peer "Busy", peer live free memory, "Available" for a peer | Needs N2 (owner decision). Peers show "Can send" plus the last real outcome. |
| Pooled or partitioned execution, shard/piece bars, Cluster view, "pool memory" switch | No model partitioning exists. The pooled flag is preserved on PUT but never shown. |
| Credits, contributor controls, public compute, Swarm, Cloud, billing, Contributions page | No backend (D-019). F-10-08 stays NOT_STARTED. The `Later` placeholders and the disabled "Public compute" radio are deleted. |
| Benchmarks UI (F-10-09) and fabricated performance figures | Only measured values from responses, jobs and the audit, each labelled with its scope. No `/metrics` token-rate series, no "last decode tok/s", no all-model mean TTFT, no per-conversation median. |
| Engine RSS (`rss_bytes`) | A synchronous `ps` per poll on unix, never available on Windows. Add when memory pressure needs diagnosing. |
| Devices Group and Errors dropdowns, a "Filter by" column selector | 2–5 devices; the filter box matches visible columns and the Last error column sorts by code. |
| Event stream / WebSocket | Polling is cheap on loopback once E2 lands. |
| Multi-select and bulk actions | Every action is per device or per model. Bulk Revoke/Remove is a foot-gun. |
| Column resize and drag-reorder | Hide/show plus budgeted widths cover v1. Add when users ask. |
| OS notifications, tray icon, Dock badge, title-bar speeds | Need new plugins and capabilities. The Attention item, the announcer and Activity cover v1. |
| Native file picker for Import | Needs `@tauri-apps/plugin-dialog` plus a capability. v1 uses an absolute-path field. |
| Multiple chat conversations, system prompt, sampling controls | "History where supported": one local conversation. Add later. |
| UI lock / password, WebUI, remote API exposure | Out of scope and security-sensitive. The API stays loopback-only. |
| Rename device (F2), per-device quotas, scheduler | No API. |
| Web fonts, a component library, a chart library, a table library | No new dependencies (§0.6). |
| Anything copied from qBittorrent (code, icons, layout, names) | GPL. Only behaviour was studied. |

---

## 7. Open owner decisions (the plan works with either answer)

1. **Visible brand.** The plan shows "UCompute" in the rail and window title (D-018: public name in use, clearance not claimed). `productName` and `identifier` in `tauri.conf.json`, which affect bundle and installer names and data paths, stay "Common" unless the owner approves a change. Until then the macOS app menu (built by `Menu.default()` from `productName`), the Dock and the installer say "Common" while the window and rail say "UCompute"; this mismatch is known and accepted.
2. **N2 peer status broadcast.** It would enable a true peer Busy state, live peer free memory and the "Loaded there" field, but discloses the loaded model to every paired device.
3. **E5 hold persistence and reach.** The plan holds in memory until Connect or a restart, and the hold also refuses the other device's dials (it reads "Disconnected by <name>"). Persisting holds across restarts would need a `peers.json` field.

---

## 8. Critique disposition (revision 2)

Accepted in full unless listed here. Merged or rejected points:

- **Peer availability words.** Critique 1 proposed "Permitted here / Available / Refused last request"; critique 3 proposed "Can send" (neutral). Merged: "Can send" by default, upgraded to ok only by a real `done` outcome on the current connection, warn for refusals. "Available" is never used for a peer.
- **Client reference header name.** Critique 2's `X-Common-Client-Tag`/`client_tag` was not used; critiques 1 and 3 agreed on `X-Common-Client-Ref`/`client_ref`. Added what none of them noted: the CORS allow-list currently admits only `authorization, content-type`, so E2 adds the header and the client sends it only to services that report `status.engine`.
- **What the held device sees.** Critique 2 mapped `CLOSE_HELD` to the existing `peer_lost`. Rejected in favour of critique 3's distinct code (`peer_held`): under the new Connection rules `peer_lost` would read as "Not seen nearby" or "Unreachable", which is false for a device that was deliberately refused.
- **Token-rate sparklines.** Critique 3 asked to delete them; critique 1 asked to feed them from live E3 token counts. Kept one sparkline (Jobs only, three series) fed by live E3 counts, which is a real rate; the `/metrics` series, "last decode tok/s" and the all-model mean TTFT are deleted as critique 3 asked. Critique 3's optional plain "tokens generated" counter was not kept (YAGNI).
- **Peer free memory.** Critique 1's "muted with age from `issued_at`" was rejected: `issued_at` is the handshake time while the value was measured at the peer's network start (critique 3, verified in `transport.rs`). Peer cells show `—`; the snapshot is in the inspector with its caveat.
- **Firewall in Attention.** Critique 2 removed it from the persistent Attention set; critique 1 wanted an inline "Allow" fix on the Dashboard. Both kept: never in the status bar, present on the Dashboard Attention list with critique 3's honest wording.
- **Busy service detection.** Critique 2's `slow` code and critique 3's `/health` probe are combined (timeout → `/health` → busy or not running).
- **Grid pattern.** Critique 3 offered APG cell focus or `treegrid`; chose cell focus (critique 1 asked for the same).
- **Conversation median.** Critique 3 offered deleting it or grouping by (destination, model); deleted.
- **`rss_bytes`.** Critique 2 offered caching or dropping; dropped.
- **StatusBar 720 px test.** Critique 1 asked for a jsdom test at a 720 px container. Rejected as specified: jsdom performs no layout, so such a test would pass vacuously. Replaced by a unit test of the drop-priority classes plus the real-browser and packaged checks at 720 px.
- **"More ▾" menu.** Accepted as a mechanism, but after removing Copy ID and moving Revoke, Devices needs none; it is used only for Models (Import GGUF…) and Activity (Error codes…).
- **Devices Group dropdown.** Not raised by the critiques; dropped with the Errors dropdown and the "Filter by" selector to make the toolbar fit, recorded in §6.
