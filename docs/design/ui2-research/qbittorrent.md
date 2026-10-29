# qBittorrent desktop UX study and how it maps to UCompute Desktop UI 2.0

Names: this record uses the current names (D-030). What it describes ran under the former name; the original text is in git history (commit cab8a09 and earlier).

Researched 2026-10-19. The latest release named on qbittorrent.org is v5.2.4 (news page date: 2026-09-28). [S1][S3]

**Method and licence boundary.** I used the official site, the wiki (FAQ, Options, WebUI API), the release news and the GUI source tree at `master` on GitHub. I read the source only to describe behaviour. This document reproduces no code, layouts, icons or assets. The source code is GPLv2+. The binary distribution is GPLv3+ because it bundles GPLv3+ assets such as images (COPYING [S7]). That is a second reason, beyond branding, never to reuse its icons. UI labels below are given as plain names. Tooltips are paraphrased.

---

## 1. Main window anatomy (Qt desktop GUI)

```
+----------------------------------------------------------------------------------+
| Menu bar: File | Edit | View | Tools | (Plugins) | Help                          |
+----------------------------------------------------------------------------------+
| Toolbar: [Add file][Add link][Remove] | [Start][Stop][Open folder]               |
|          [Top][Up][Down][Bottom] | [Creator] | [Options][Lock]   Filter: [____] by [Name v] |
+----------------------------------------------------------------------------------+
| Tabs: Transfers (N) | Search | RSS | Execution Log      (Alt+1..4)               |
+------------------+---------------------------------------------------------------+
| FILTER SIDEBAR   | TRANSFER LIST (sortable table, multi-select)                  |
| [x] Status       |  # | Name | Size | Progress | Status | Seeds | Peers | Down.. |
|   All (n)        |  ...                                                          |
|   Downloading(n) +---------------------------------------------------------------+
|   ...            | PROPERTIES PANEL (bottom, collapsible)                        |
| [x] Categories   |  General | Trackers | Peers | HTTP Sources | Content | Speed  |
| [x] Tags         |  (details of the ONE current torrent)                         |
| [x] Trackers     |                                                               |
| [ ] Tracker stat.|                                                               |
+------------------+---------------------------------------------------------------+
| Status bar: [restart warning] Free space | External IP | DHT: n nodes | (conn) | (alt) | v speed [limit] (session) | ^ speed [limit] (session) |
+----------------------------------------------------------------------------------+
```

The main window is a tab widget. The Transfers tab is a horizontal splitter: the filter sidebar on the left, and on the right a vertical splitter with the transfer list above the properties panel. All splitter positions persist. The Transfers tab title carries a live row count, for example Transfers (12). Search, RSS and Execution Log are optional tabs. [S9]

### 1.1 Menu bar [S8]
- **File:** Add torrent file, Add torrent link, Close window, Exit.
- **Edit:** Start, Stop, Remove, Top of queue, Move up, Move down, Bottom of queue, Pause session, Resume session.
- **View:** check toggles for Top toolbar, Status bar, Filters sidebar and Speed in title bar; Search engine and RSS reader; a Log submenu with Show and severity filters (Normal, Information, Warning, Critical); Statistics (Ctrl+I); Lock (Ctrl+L).
- **Tools:** Torrent creator, Manage cookies, Options, and an On downloads done submenu (do nothing, exit, suspend, hibernate, reboot, shutdown).
- **Help:** Documentation (F1), Check for updates, Donate, About.

### 1.2 Toolbar [S8][S9]
- The toolbar is fixed and cannot float. Its button style follows the system setting. In order: Add file, Add link, Remove | Start, Stop, Open destination folder, the four queue moves | Torrent creator | Options, Lock.
- A filter box sits at the right, with placeholder text inviting the user to filter torrents. Next to it is a Filter by combo: Name, Save path, Info hash v1, Info hash v2.
- The box's context menu has a Use regular expressions toggle, which persists.
- Ctrl+F (or Ctrl+E) focuses the filter. In 5.2.0, Ctrl+F was extended to focus filter fields generally. [S3]
- Toolbar and Edit-menu actions apply to the current multi-selection.

### 1.3 Filter sidebar [S14]
- **Sections, in order:** Status, Categories, Tags, Trackers, and an optional Tracker status section. Each section header has a checkbox that enables or disables that section's filter, and the state persists.
- **Status items:** All, Downloading, Seeding, Completed, Running, Stopped, Active, Inactive, Stalled, Stalled uploading, Stalled downloading, Checking, Moving, Errored.
  - Each label carries a live count, for example Downloading (3). Counts update incrementally.
  - With the 5.1 option Auto hide zero status filters [S3], empty items disappear. All never hides. If the selected item hides, the selection falls back to All.
- **Trackers items:** All (n), Trackerless (n), one entry per tracker host (with an optional favicon), plus error buckets: Tracker error (n), Other error (n), Warning (n).
  - Since 5.2.0, the error buckets can split into the separate advanced Tracker status section. [S3]
  - In 5.0.0, tracker errors got their own filter item and a Moving status filter was added. [S3]
- **Context menu on any filter item:** Start, Force start, Stop or Remove all torrents currently matched by that filter. This acts on the filtered set, not on the selection. Removing a tracker host asks for confirmation and offers a "don't ask again" option.
- **Combination rule:** a row is shown only if it passes status AND category AND tag AND tracker host AND tracker status AND the text filter. Any criterion can be "any". [S13]
- 5.1 added filtering by save path. In 5.2.3, filters clear when their section is collapsed. [S3]

### 1.4 Transfer list table [S11][S12][S13]
- **Columns:** 38 in total.
  - The default visible set is roughly (inferred from the hidden-by-default list): # (queue position), Name, Size, Progress, Status, Seeds, Peers, Down speed, Up speed, Ratio, ETA, Category, Tags, Availability, and a few others.
  - About two dozen are hidden on first run: dates, transferred amounts, paths, hashes, ratio limit, popularity, last activity, total size, reannounce, private.
  - The full list is #, Name, Size, Progress, Status, Seeds, Peers, Down speed, Up speed, Ratio, Popularity, ETA, Category, Tags, Created on, Added on, Completed on, Tracker, Down limit, Up limit, Downloaded, Uploaded, Session downloaded, Session uploaded, Remaining, Time active, Save path, Incomplete save path, Completed, Ratio limit, Last seen complete, Last activity, Total size, Availability, Info hash v1, Info hash v2, Reannounce in, Private.
- **Value display rules (the "honest values" set):**
  - A zero or negative limit shows ∞. A ratio of -1 or above the maximum also shows ∞. Ratio uses 2 decimals and availability 3 decimals.
  - Unknown availability, an invalid hash or an unknown Private value shows N/A.
  - Progress shows 100% only when complete, otherwise 1 decimal.
  - Session and total amounts show as "value (total)". Last activity shows "N ago".
  - Queue position shows as position+1, or * when there is no position.
  - Optional Hide zero and infinity values setting: never, only for stopped torrents, or always.
  - Numeric columns are right-aligned. Name, Status, Category, Tags, Tracker, paths and hashes get the full text as a tooltip.
- **Sorting:**
  - Name and other text columns sort case-insensitively in natural order (file2 before file10).
  - Unknown, infinite or negative values sort after known values ascending, so they come first descending.
  - Status sorts by internal state order.
  - Seeds and Peers sort by active count, then total.
  - Two-level sort: when the user switches the sort column, the previous column becomes the secondary key with its own direction. Default secondary key is Name. Both keys persist.

### 1.5 Properties panel (bottom inspector) [S4][S15]
- **Tabs:** General (Alt+G), Trackers (Alt+C), Peers (Alt+R), HTTP Sources (Alt+B), Content (Alt+Z), Speed (Alt+D).
- **Collapse and persistence:**
  - Clicking the active tab collapses the panel to its tab bar. Clicking again restores the size and refreshes.
  - The visible/collapsed state, splitter size, selected tab and file-list column layout persist.
  - The 5.2.2 release restored an explicit expand/collapse button. [S3]
- **Selection behaviour:**
  - The panel shows the current single torrent.
  - With no selection it clears every field and stops refreshing (5.1). [S3][S15]
  - Multi-select has no aggregate view.
- **General tab:** two groups.
  - Transfer: time active (and seeding time), ETA, connections, downloaded/uploaded (with session amounts), seeds, peers, current and average speeds, limits, share ratio, reannounce in, last seen complete, popularity, wasted.
  - Information: total size, pieces × piece size, created by, added on, completed on, created on, info hashes (N/A when absent), save path, comment, private.
  - Two compact bars show which pieces are held or in progress, and per-piece availability across peers. The availability bar is hidden when it is meaningless (finished, stopped, queued, checking).
  - Since 5.1.1 the text is selectable. [S3]
- **Trackers tab** [S18]:
  - Columns: URL/endpoint, Tier, BT protocol, Status, Peers, Seeds, Leeches, Times downloaded, Message, Next announce, Min announce.
  - Statuses: Working, Updating…, Not working, Tracker error, Unreachable, Not contacted yet.
  - Three grey pseudo-rows (DHT, PeX, LSD) sit at the top. They show Working, Disabled, or Disabled for this torrent. Private torrents carry a message saying so.
  - Countdown columns refresh every 4 s and never go below zero. Unknown numbers show N/A.
- **Peers tab** [S17]:
  - Columns: Country/Region, IP/Address, Port, Flags, Connection, Client, Peer ID client, Progress, Down speed, Up speed, Downloaded, Uploaded, Relevance, Contribution, Files.
  - Rows update in place: new peers are added and disconnected peers removed.
  - Context menu: Add peers…, Copy IP:port, Ban peer permanently. Add peers is disabled for private, checking or queued torrents.
  - Keys: Ctrl+C copies peers as IP:port lines. Delete bans after a confirmation.
  - Each flags cell has a per-flag legend tooltip. The single-letter flag codes are documented in the FAQ. [S4]
- **HTTP Sources tab:** web seeds for the torrent.
- **Content tab** [S19]:
  - A file tree with checkboxes, plus Select all and Select none buttons.
  - Priority per file: Do not download, Normal, High, Maximum, or By shown file order (splits the selection into thirds).
  - Context menu: Open, Open containing folder, Copy path, Rename, Batch rename, Priority. F2 renames. Space toggles the checks on the selected rows.
- **Speed tab:** the global speed graph (§6).

### 1.6 Status bar, left to right [S10]
1. A **restart-required warning**, shown only when needed.
2. **Free space: X**, optional (5.2.0). It shows N/A until the first measurement.
3. **External IP(s)**, optional. It shows N/A until known.
4. **DHT: n nodes**. It is hidden entirely when DHT is disabled.
5. A **connection status icon** with three states (§7). Its click handler is outside statusbar.cpp, so the click target is unverified.
6. An **alternative speed limits toggle**. It is a pressed/unpressed button whose icon and tooltip change with the mode, and the tooltip says what a click will do.
7. **Download** and 8. **Upload** speed buttons. Each shows the current rate, then [limit] when a limit is set, then (session total). A click opens the Global speed limits dialog (regular and alternative up/down, ∞ = unlimited). [S21]

Separators divide the items. Every item is either live or labelled N/A. The bar never shows a stale number as current.

### 1.7 Ambient indicators [S9][S20]
- **Window title:** optionally prefixed with download/upload speeds, and with a paused marker when the session is paused.
- **Tray icon:** its tooltip shows speeds. Its menu has Show/Hide, Open, Download from URL, Alternative speed toggle, Set global limits, Resume/Pause session, and Exit.
- **macOS:** a Dock badge and a menu bar status item show speeds (options). The first close-to-tray triggers a one-time notification explaining the behaviour.
- **Lock:** password-protected. It requires a password of at least 3 characters, hides the window and disables the tray menu.
- **Statistics dialog** [S21]: all-time upload/download/ratio, session waste, connected peers, cache and I/O performance counters, and queued tracker announces.

---

## 2. Table interaction behaviours (summary) [S12][S13][S17][S19]

| Behaviour | qBittorrent |
|---|---|
| Sort | Click a header. Two-level sort persists. Natural text order. Unknowns go last. |
| Resize | Drag edges. Header menu has Resize columns (fit to contents). 5.1 added auto-resize. [S3] |
| Reorder | Columns drag-reorder. Order, width and sort indicator persist. |
| Column chooser | Right-click the header for a checkable list of every column. The last visible column cannot be hidden. The queue column is hidden from the chooser when queueing is off. |
| Multi-select | Extended selection (Shift/Ctrl/Cmd). Toolbar, menu and context actions apply to all selected rows. In 5.2.4, rows hidden by a filter cannot be selected. [S3] |
| Context menu | State-conditional: an item appears only when it applies to the selection (Start vs Stop vs Force start; sequential only when unfinished). An item that is not possible right now (Force reannounce) stays visible but disabled, with a tooltip explaining why. Copy submenu for identifiers. |
| Double-click / Enter | Configurable separately for unfinished and finished items: start/stop, open folder, preview, options dialog, or nothing. It only acts when exactly one row is selected. |
| Destructive actions | Delete removes the torrent only. A separate shortcut (Shift+Delete) also removes the data. Optional confirmation dialog states what will be deleted. The context menu Remove is always the soft variant. Ctrl+R recheck is optionally confirmed (5.1). [S3] |
| Inline edit | F2 renames. |
| Live updates | Rows update in place (peers diff in place). In 5.2.0 the list became virtual for performance. [S3] |
| Keyboard map | Ctrl+O/Ctrl+Shift+O add, Ctrl+V add links from clipboard, Ctrl+S/Ctrl+P start/stop, Ctrl+Shift+S/P all, Ctrl+M force start, Ctrl+(Shift)+Plus/Minus queue, Alt+1..4 main tabs, Alt+letter properties tabs, Alt+O options, Ctrl+I statistics, Ctrl+L lock, Ctrl+Q quit, F1 docs, Esc closes dialogs (5.2). [S4][S9][S3] |
| Accessibility | 5.1.3 fixed screen-reader accessibility of the torrent list [S3]. Dense tables need explicit a11y work. |

---

## 3. State vocabulary, icons and colours

### 3.1 States (API names, from the WebUI API wiki) [S6]
- **Error:** error, missingFiles.
- **Upload side:** uploading, stalledUP, queuedUP, pausedUP (shown as Completed in 5.x), checkingUP, forcedUP.
- **Download side:** downloading, metaDL, stalledDL, queuedDL, pausedDL (shown as Stopped in 5.x), checkingDL, forcedDL, allocating.
- **Other:** checkingResumeData, moving, unknown.

The key semantic distinction is **stalled versus active**. A stalled torrent is running but has no connection that is actually transferring. Stalled is a separate state and filter, so "running" never reads as "working". [S6][S14]

5.0.0 renamed Pause/Resume to Stop/Start. The "pausing" vocabulary was reserved for the whole session (Pause session). [S3][S8]

### 3.2 Display text [S11]
Downloading; Stalled; Downloading metadata; Seeding; Queued; Checking; Checking resume data; Stopped; Completed; Moving; Missing files; Errored: <reason>.

Forced items carry an **[F]** prefix, for example [F] Downloading. This tells the user the item bypasses the queue.

### 3.3 Icons [S11]
The Name column shows one icon per state family: downloading, stalled-down, stalled-up, uploading, stopped, completed, queued, checking, moving, error. The icon appears only in the Name column, so the status signal does not repeat across the row.

### 3.4 Colours [S4][S11][S20]
Row text is optionally coloured by state, on by default. Light and dark themes have separate palettes. Values are from the FAQ.

| State family | Light | Dark |
|---|---|---|
| Downloading / forced / metadata | forest green | lime green |
| Allocating, stalled (up and down) | black | grey 80 |
| Uploading / forced | royal blue | steel blue 1 |
| Stopped (unfinished) | salmon | salmon |
| Completed (stopped and finished) | dark blue | steel blue 3 |
| Error / missing files | red | red |
| Queued / checking / checking resume data | teal | cyan 3 |

Two takeaways:
- Stalled items render in the neutral text colour. Only real activity earns a colour.
- Colour is always paired with a status word and an icon. Colour is never the only carrier, so the 5.x options can switch row colouring off with no loss of information.

---

## 4. Queueing semantics [S20][S24][S4][S12]

- **Off by default.** When off, the # column and Queue actions disappear from the column chooser and the context menu. The UI does not show a queue that does not exist.
- **Limits:**
  - Maximum active downloads, uploads and torrents (∞ = unlimited).
  - An optional "do not count slow torrents" exemption, with download and upload rate thresholds (default 2 KiB/s each) and an inactivity timer (default 60 s). Torrents that are not really moving do not occupy slots.
- **Ordering:**
  - Unfinished torrents have a contiguous queue position. New items go to the end. Removing an item, or an item finishing, closes the gap.
  - Users move items with Top, Up, Down and Bottom.
  - Seeding torrents have no user position. The engine ranks them by demand. The FAQ explains this to users rather than pretending the position is user-controlled.
- **Bypass:** Force start takes an item out of the queue (not auto-managed). The state shows [F], so the bypass is never hidden.
- **Checking queue:** checking and allocating have their own concurrency limit (libtorrent). In the UI, both appear as Queued/Checking.
- **Queued is a first-class state with its own colour (teal),** distinct from Stopped and Stalled.

---

## 5. Transfer limits [S10][S20][S21][S5]

- **Global limits:** upload and download, KiB/s (∞ = none).
- **Alternative global limits:** a second set. The UI calls this "slow mode"; it is widely known as the turtle. Defaults are 10 KiB/s each.
  - It toggles from the status bar button, the tray menu and the WebUI API.
  - An optional scheduler runs it from a start time to an end time, every day, weekdays or weekends (default 08:00–20:00, off).
  - Separate checkboxes decide whether limits apply to µTP, transport overhead and LAN peers.
- **Per-torrent limits:** in the Torrent options dialog. Speed limits and share limits there are explicitly capped by the global limits ("will not exceed the global limits"). The dialog also holds per-torrent switches to disable DHT/PeX/LSD, sequential order, and first/last pieces first.
- **Share limits:** ratio, total seeding time and inactive seeding time. Any or all of them can trigger an action: stop, remove, remove with files, or super-seed.
- **Connection limits:** global and per-torrent maximum connections and upload slots.
- **Visibility:** the effective limit is visible wherever the rate is shown, as `rate [limit]` in the status bar and as the Down/Up limit columns in the list.

---

## 6. Speed graphs [S16]

- **Scope:** a single global graph in the Speed tab. It is not per-torrent.
- **Periods:** 1 min, 5 min, 30 min, 3 h, 6 h, 12 h, 24 h, chosen from a dropdown.
- **Series:** 10 upload/download pairs: total, payload, overhead, DHT, tracker. A Select graphs menu shows each series as a checkable item. All are on the first time.
- **Persistence:** the selected period and series persist. Data is sampled from session statistics updates.
- **Disabled state:** the graph can be disabled in Advanced settings. The Speed tab then shows a placeholder that links to that setting. [S15]

---

## 7. Connection status indicator [S10][S6]

There are three states. `/transfer/info` exposes the same three as `connection_status` (connected / firewalled / disconnected). [S6]

| State | Condition | Tooltip meaning |
|---|---|---|
| Disconnected (offline) | Not listening on the configured port | Offline: the client failed to listen on the selected port for incoming connections |
| Firewalled | Listening, but no incoming connection has been seen. This is also the initial state. | No direct connections, which may indicate a network configuration problem |
| Connected | Listening and at least one incoming connection has been seen | Online |

Firewalled is a **pessimistic default**. The client never claims "online" until it has evidence of an incoming connection. Outgoing traffic still works while firewalled, and the tooltip names the likely cause.

---

## 8. Preferences organisation [S5][S20]

The dialog has a left page list with nine pages: Behavior, Downloads, Connection, Speed, BitTorrent, Search, RSS, WebUI, Advanced. Pages hold titled group boxes with checkbox-gated sub-groups, and dependent controls stay disabled until their checkbox is on.

- **Behavior:**
  - Interface: language, style, colour scheme, theme, system icons. Changes are restart-flagged.
  - Transfer list: confirm delete, alternating rows, state colours, hide zero and infinity values, auto-hide zero status filters, separate tracker-status filter, double-click actions.
  - Status bar: free space, external IP.
  - Desktop: startup, splash screen, start minimised, confirm exit, tray behaviour, file associations, update check, Dock and menu bar speed.
  - Power management: no sleep while active.
  - Log files: path, backup size, retention 6 months.
  - Performance warnings.
- **Downloads:** add-dialog behaviour, paths, pre-allocation, incomplete-files folder, watched folders, email on completion, run a program on completion.
- **Connection:** protocol, listening port (random port, UPnP/NAT-PMP), connection limits, I2P, proxy, IP filter.
- **Speed:** global limits, alternative limits, schedule, scope of the limits.
- **BitTorrent:** privacy (DHT, PeX, LSD, encryption mode, anonymous mode), torrent queueing, seeding limits.
- **WebUI:** off by default. Bind address, port, HTTPS, credentials and API key, localhost/subnet bypass, ban policy, session timeout, CSRF/clickjacking/host-header protections, reverse proxy, DDNS.
- **Advanced:** a long key/value table of engine tunables, kept apart from everyday settings.

---

## 9. Notifications and completion actions [S23][S9][S20]

- **Desktop notifications fire for:**
  - Download completed.
  - I/O error, which names the item and the reason.
  - Add torrent failed, which names the source and the reason.
  - Torrent added. This is the only one gated by its own preference.
  - Search finished or failed, only when the Search tab is not visible.
  - The one-time "closed to tray" notice.
- The update prompt is a non-modal message box, not a notification.
- **Optional email and external program on completion.**
- **On downloads done:** exit, suspend, hibernate, reboot or shut down. A confirmation dialog appears unless suppressed. The choice is one-shot and resets after use.

Pattern: notifications are reserved for **completion, failure, and events the user cannot see right now**. Routine state changes go to the log, not to toasts.

The Execution log [S22][S8] has a General tab with severity filters (Normal, Information, Warning, Critical) and a Blocked IPs tab, with Copy and Clear in the context menu. Log rows are colour-coded since 5.1. [S3]

---

## 10. Translation to UCompute (devices ~ peers, jobs ~ transfers, models ~ content)

The "backed by" column cites real API surfaces as of HEAD e76e7fc plus the working tree. I read `crates/ucompute-api/src/lib.rs` ROUTES, `apps/desktop/src/api/client.ts` types, `crates/ucompute-network/src/health.rs`, `proto/ucompute/v1/control.proto`, and `crates/ucompute-api/src/mesh.rs` network_status. A gap means the UI must show the item as unavailable, or a small, justified backend addition is needed (§11).

### 10.1 Information architecture

| qBittorrent principle | UCompute analogue | Backed by / gap |
|---|---|---|
| One main window: persistent left filter rail + central table + bottom inspector + status bar | App shell. Left rail: Dashboard, Devices, Models, Jobs, Chat, Activity, Settings. Under the active section, rail sub-filters with live counts (as in §1.3). Central sortable table. Bottom or right inspector for the selected row. Global status bar. | UI only |
| Transfers (N) tab count | Rail badges such as Devices (3 connected / 5 trusted) and Jobs (inflight/queued). Counts are computed from live data, never cached. | `/api/peers`, `/api/status.queue` |
| Status filter items with counts; auto-hide zero; filter-item context menu acts on the filtered set | Devices rail filters: All, Connected, Unhealthy, Reconnecting, Disconnected, Discovered (not paired), Revoked, Has error. Models rail filters: All, Downloaded, Loaded, Not downloaded, Won't fit. A filter context menu offers only safe bulk actions, for example Connect all disconnected. Never bulk Revoke/Remove without confirmation. | `PeerHealth` (connected/unhealthy/reconnecting/disconnected) + `last_error`; `Discovered[]`; `Model.downloaded/loaded/fit` |
| Category/Tag sections | **Groups** section in the Devices rail. Membership and policy come from `/api/groups`. | `/api/groups` |
| Trackers section with error buckets (Tracker error / Other error / Warning) | A Network-errors bucket under Devices, grouped by `last_error.code`, so failures cluster ("3 devices: firewall-blocked"). | `DialError{code,message,hint,at_utc}` |
| Toolbar with contextual actions + filter box with Filter by column + Ctrl+F | Per-section contextual toolbar. Devices: Pair, Connect, Revoke, Remove, Copy ID. Models: Download, Load, Unload, Delete, Import. Plus a filter box with a column selector. Ctrl+F focuses it, Esc clears it. | UI only |
| Properties panel tabs; click the active tab to collapse; the panel clears on no selection | **Device inspector** tabs: Overview, Hardware, Models, Connection, Permissions, Activity. **Model inspector** tabs: Overview (size, quant, licence, fit estimate), Locations (which devices hold it), Load. **Job inspector** tabs: Timing, Tokens, Route, Error. The inspector collapses, persists its tab and size, and shows "No selection" rather than stale data. | Mixed, see 10.3 |
| Status bar (free space, external IP, DHT nodes, connection icon, limits toggle, speeds [limit] (session)) | **Status bar**, left to right: a restart/daemon warning if any; Disk free; device name + dial address; mDNS on/off; mesh connection indicator (§10.5); Serving peers on/off toggle (the turtle analogue, §10.6); loaded model + device actual (GPU/CPU); queue `inflight/max_concurrent · queued/max_queue`; decode tok/s (last) | `/api/hardware.disk_free_bytes`, `/api/network.{device_name,dial_addr,mdns,connections,serve_peers}`, `/api/status.{loaded,queue}`, `/metrics` |
| Execution log tab with severity filters | **Activity**: `/api/audit` events with filters for kind, device and time, plus text search. Severity derives from kind, for example `pairing_failed`, `peer_rejected` and `peer_lost` count as warnings. Copy row. No prompt or completion content (privacy). | `/api/audit` |
| Statistics dialog | Dashboard "since start" figures: uptime, `ucompute_requests_total`, `ucompute_tokens_generated_total`, `ucompute_remote_requests_total`, `ucompute_requests_rejected_total`, TTFT histogram. Each figure is labelled with its scope (daemon session), never presented as all-time. | `/metrics`, `/api/status.uptime_seconds` |
| Window title / tray / Dock speed | Optional later (YAGNI). If added, show "n devices · tok/s" only from live data. | n/a |

### 10.2 Tables: Devices, Models, Jobs
These rules apply to all three tables:
- Sortable columns, resizable and reorderable.
- Header right-click column chooser with "Fit columns". The last visible column cannot be hidden.
- Persisted layout (localStorage via the existing UI state store).
- Two-level sort with Name as the default secondary key. Natural name order. Unknowns sort last.
- Extended multi-select. Context menu filtered by applicability, with impossible actions kept visible but disabled with a reason tooltip.
- Numeric columns right-aligned. N/A for unknown, ∞ only for genuinely unlimited quotas.
- Needs a proper ARIA grid or table, with keyboard row navigation and Enter for the default action. qBittorrent itself needed an a11y fix here. [S3]

**Devices table, suggested defaults:**
- Name (device_name or label)
- State (word + icon + colour)
- Trust (Trusted / Discovered / Revoked)
- Address (last_addr / discovered addrs)
- Last seen (relative, "N ago")
- Groups
- Last error (code)
- Paired at
- **Hidden by default:** peer ID, short label, proto, pairing port.
- **Not available yet (needs G1):** Hardware summary, Free memory, Models (count).

**Models table:**
- Name, Family, Params, Quant, Size, Fit (word + icon), State, Licence.
- State values: Not downloaded / Downloading n% / Downloaded / Loaded on GPU|CPU.
- "On devices" column: local only until G1 lands.

**Jobs table:** see 10.4. The table can only be honest at the level the backend exposes.

### 10.3 Inspector content versus reality

| Device inspector tab | Available now | Unavailable now (show "Not reported by this device") |
|---|---|---|
| Overview | name, short label, peer id, trust, paired_at, last_seen, health, last_error with hint | — |
| Connection | last_addr, discovered addrs, dial_addr (self), connections count, error history (audit `peer_*` events) | per-connection RTT, bytes in/out |
| Hardware | local device: `/api/hardware` | remote: `Capabilities.hardware_json` is received over QUIC but **not exposed** by `/api/peers` → G1 |
| Models | local: `/api/models` | remote: `Capabilities.cached_model_sha256` not exposed → G1 |
| Measured capabilities | local: `Status.loaded` (offload, n_ctx, threads, slots, load_ms) + `/metrics` tok/s | remote measured performance: not tracked anywhere |
| Permissions | groups + group policy (`allow_remote_inference`, `allow_pooled_inference`), privacy mode, `serve_peers`, `peer_quota` (requests/min, tokens/min, bytes/s, max_concurrent) + `peer_slots` (read-only: quotas come from config.json; `PUT /api/network` only accepts `enabled`/`serve_peers`) | per-device quota overrides (none exist) |
| Activity | `/api/audit` filtered by `peer_id` | — |

### 10.4 Jobs ~ transfers (the hardest mapping; do not invent)
- **What exists:**
  - A real admission queue with aggregate counts: `/api/status.queue = {inflight, queued, max_concurrent, max_queue}`, `ucompute_inflight` and `ucompute_queue_depth` metrics, and `ucompute_requests_rejected_total`.
  - Remote requests appear as `remote_request` audit events.
  - Streaming generations are cancelled server-side when the client disconnects (`openai.rs` CancellationToken on drop). For remote targets, a `Cancel{request_id}` is sent to the peer (`ucompute-network/src/remote.rs`).
- **What does not exist:**
  - A per-job registry or list endpoint, job IDs visible to the UI, or a cancel endpoint for jobs started by other clients.
  - Any reorder or priority control.
  - A wired peer `Status{queue_depth, active_sessions}`. The message is defined in the proto but I found it used only in a round-trip test.
- **Therefore, without G2, the Jobs page shows:**
  - The aggregate queue gauge (inflight/max, queued/max, rejected).
  - Requests started from this app session as rows with real timings. These come from the `Timings`/`Usage` returned to the client. Their Cancel action is just an abort of the app's own stream, which is a real server-side cancel.
  - Recent remote requests from the audit log.
  - **Never** a "#" queue-position column, Move up/down, Force start, or Cancel on rows the app did not start.
- **State vocabulary, adapted from §3:**
  - Queued (waiting for an admission slot), Running, Stalled (running but no token for N s; only if measurable from the stream), Completed, Failed: <code>, Cancelled, Rejected (queue full / quota / policy).
  - The **[F]-style marker** for anything that bypasses normal routing is the honest-routing badge: show the execution location on every row (Local GPU / Local CPU / Device X). That is UCompute's equivalent of an explicit queue bypass.

### 10.5 Connection indicator (status bar), adapted from §7
Use the same pessimistic default: never green without evidence.

| UCompute state | Condition (real fields) |
|---|---|
| Mesh off | `/api/network.enabled == false` (neutral grey, not red: off is a choice) |
| Not reachable | enabled but `listen_addr` null, or the bind failed, or `dial_addr` null (0.0.0.0 without a route) |
| Listening, no devices connected | enabled, `connections == 0`. On Windows, add the firewall status from the in-flight Tauri command (blocked → explicit "Windows Firewall is blocking inbound pairing" with an Allow action). |
| Connected (n) | `connections > 0`, with the per-device health in the tooltip |

A **discovered mDNS record must never raise the state**. Discovered entries feed the "Discovered (not paired)" filter only, with `seen_utc` age. A trusted device counts as connected only when `health == connected`. Unhealthy and reconnecting peers get their own colours, because they follow the real keep-alive state machine (1 s keep-alive, 4 s idle, backoff 1–30 s; `ucompute-network/src/health.rs`).

### 10.6 Limits and the "alternative speed" toggle
- **Turtle → Serving peers toggle.** One click switches `serve_peers` (`PUT /api/network`). It is a pressed/unpressed button whose tooltip says what a click will do. The change goes to Activity. This is a permission change, so it must be keyboard-reachable and must never be scheduled automatically. Skip the qBittorrent-style scheduler (YAGNI; no backend).
- **`rate [limit] (session)` display →** show `inflight/max_concurrent` and peer quota `requests/min` next to live usage. Show ∞ only when a quota is genuinely 0/unlimited in config.
- **Per-torrent limits "capped by global" →** per-device policy is group-based today (`allow_remote_inference`). Do not invent per-device limits.

### 10.7 Settings, adapted from §8
- **Page list:**
  - General/Behavior: confirmations, table options, start minimised, notifications.
  - Network: enable mesh, mDNS, listen/dial address, Windows firewall status/allow.
  - Execution: device auto/cpu/gpu, context, concurrency as read-only where config-only.
  - Storage: `UCOMPUTE_HOME`, disk free, model store.
  - Privacy & permissions: privacy mode, groups and policies, serve peers, quotas read-only.
  - Security: this device's key fingerprint, revoked devices, pairing.
  - Advanced: raw config view, read-only.
- **Rules carried over:**
  - Checkbox-gated sub-groups, with dependent controls disabled (not hidden) and a reason given.
  - Restart-required flag, mirroring the status bar warning.
  - Contributor/credit controls **absent** until a backend exists (owner directive).

### 10.8 Notifications, adapted from §9
- **Notify only on:**
  - Pairing succeeded or failed.
  - A trusted device lost, but only after the reconnect backoff fails, not on every `unhealthy` blip.
  - Model download completed or failed.
  - A remote request rejected by policy or quota.
  - Daemon stopped unexpectedly.
- Everything else goes to Activity. Each class gets its own off switch. Notifications never contain prompt content.

### 10.9 Keyboard map, adapted from §2
- Ctrl/Cmd+1..7 for the sections; Ctrl/Cmd+F filter; Esc clear/close; Enter default action; Delete for Remove (with confirmation; Revoke is never on a bare key).
- F2 rename only if a rename API exists (none today); Ctrl/Cmd+C copies the selected IDs or addresses.
- Use Cmd on macOS and Ctrl on Windows, following each platform's conventions.

---

## 11. Smallest backend gaps the UI would justify (proposed; none implemented)

| ID | Need | Why the UI needs it | Minimal shape |
|---|---|---|---|
| G1 | Peer capabilities in `/api/peers` | Device inspector Hardware/Models tabs and the "On devices" column. The data is already received and signature-checked (`Connection.capabilities`). | Add `capabilities: {hardware summary, free_memory_bytes, engines[], cached_model_sha256[], issued_at} \| null` per connected peer. `null` means "not reported". |
| G2 | Job list | Jobs table beyond this app session | In-memory ring `GET /api/jobs` (id, model, target, origin, state, queued_at/started/finished, ttft, tok/s, tokens, error code). Optional `POST /api/jobs/{id}/cancel` reusing the existing CancellationToken. No persistence, no reorder. |
| G3 | Disconnect | Devices toolbar | There is no route today. Either add `POST /api/peers/{id}/disconnect` (close the connection and stop redialing until Connect), or omit the action. Never fake it with DELETE (that unpairs). |
| G4 | Peer busy state | "busy" device state | Wire the existing proto `Status{free_memory_bytes, active_sessions, queue_depth}`. Until then, show Busy as Unknown. |

Each gap needs a documented contract (openapi.json) and tests, per the owner directive. Without them, the corresponding UI shows "Not available".

---

## 12. Principles that do NOT transfer

1. **Swarm/piece semantics.** Pieces bars, availability per piece, seeds/leechers, ratio, popularity, wasted bytes and super-seeding have no equivalent. UCompute runs whole requests on one device, and no model partitioning exists, so do not draw piece/shard bars.
2. **Share/seeding limits and ratio actions** (stop/remove at ratio). There is no seeding concept, and credit-like ratio framing must stay absent until a credits backend exists (D-019).
3. **User-ordered queue** (Top/Up/Down/Bottom, # column, Force start, [F] bypass as an action). The admission queue is FIFO with no reorder API. Show counts only.
4. **Bandwidth limits, alternative-limit scheduler, µTP/overhead/LAN scope.** The only analogues are config-file quotas. No time-based scheduler.
5. **Trackers/DHT/PeX/LSD pseudo-rows and peer discovery sources.** UCompute has mDNS plus manual address only. A single "Discovered via mDNS / manual" field is enough.
6. **Peer flags letters, client strings, country flags/GeoIP, hostname resolution, IP filter/ban list.** Devices are explicitly paired identities, so trust/revoke replaces ban. GeoIP would leak data and adds nothing.
7. **Per-file priority and content checkboxes.** Models are single artifacts. Selecting a quant is a different model, and the UI must never substitute one silently.
8. **Search engine, RSS, torrent creator, magnet/.torrent file associations, watched folders.** Out of scope; the model catalog replaces discovery.
9. **On downloads done → shutdown/suspend/reboot, email on completion, run an external program on completion.** These are system-power and arbitrary-exec features. They fall in the security-sensitive category and nothing in the spec asks for them.
10. **WebUI remote control (and its auth surface).** The daemon API is loopback-only by default. The desktop app is the UI. Never expose the API to the network to mimic a WebUI.
11. **UI password lock.** Not required. OS account security covers it, and adding it means storing a secret (skip unless the owner asks).
12. **"Firewalled = no inbound seen" heuristic as-is.** In Mesh, either side may dial, so no inbound is not by itself a fault. Use real signals: bind result, dial_addr, the Windows firewall status command, `last_error` codes.
13. **38 columns and a µTorrent-like look.** Carry the column-chooser mechanism, not the column count or the visuals. The defaults stay small, and the visual identity is UCompute's own (charcoal, deep green, lime).
14. **Row-colour-only state signalling.** qBittorrent can turn state colours on and off; UCompute always pairs colour + icon + word (a11y). Do not copy the pure-red/green palette. Pick contrast-checked tokens in the UCompute palette.
15. **Global speed graph series (payload/overhead/DHT/tracker).** The analogue is decode tok/s, TTFT, inflight and queue depth over time. The daemon keeps no history, so the graph covers "since the app opened" (client-side sampling of `/metrics`) and must say so. The 1 min–24 h periods are only honest once the app has actually been running that long.

---

## Sources
- [S1] qBittorrent official site, features and latest version: https://www.qbittorrent.org/
- [S2] Official screenshots page (Linux main window, collapsed/expanded General tab, Options dialog, search): https://www.qbittorrent.org/screenshots
- [S3] Official news/changelog (v5.0.0 2024-09-29 … v5.2.4 2026-09-28): https://www.qbittorrent.org/news
- [S4] Wiki FAQ (hotkeys, state colour table, peer flags, seeding queue note): https://github.com/qbittorrent/qBittorrent/wiki/Frequently-Asked-Questions
- [S5] Wiki, Explanation of Options: https://github.com/qbittorrent/qBittorrent/wiki/Explanation-of-Options-in-qBittorrent
- [S6] Wiki, WebUI API (state enum and descriptions, filters, connection_status, alt-speed mode): https://github.com/qbittorrent/qBittorrent/wiki/WebUI-API-(qBittorrent-4.1)
- [S7] README and COPYING (Qt + libtorrent; GPLv2+ source, GPLv3+ binaries due to GPLv3+ assets): https://github.com/qbittorrent/qBittorrent/blob/master/README.md , https://github.com/qbittorrent/qBittorrent/blob/master/COPYING
- [S8] src/gui/mainwindow.ui (menus, toolbar): https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/mainwindow.ui
- [S9] src/gui/mainwindow.cpp (tabs, filter box, shortcuts, tray, lock, title): https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/mainwindow.cpp
- [S10] src/gui/statusbar.cpp: https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/statusbar.cpp
- [S11] src/gui/transferlistmodel.cpp (columns, status text, icons, value formatting): https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/transferlistmodel.cpp
- [S12] src/gui/transferlistwidget.cpp (header menu, selection, context menu, delete, shortcuts): https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/transferlistwidget.cpp
- [S13] src/gui/transferlistsortmodel.cpp (sorting and filter combination): https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/transferlistsortmodel.cpp
- [S14] src/gui/transferlistfilters/ (status, categories, tags, trackers, tracker status): https://github.com/qbittorrent/qBittorrent/tree/master/src/gui/transferlistfilters
- [S15] src/gui/properties/propertieswidget.cpp/.ui: https://github.com/qbittorrent/qBittorrent/tree/master/src/gui/properties
- [S16] src/gui/properties/speedwidget.cpp: https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/properties/speedwidget.cpp
- [S17] src/gui/properties/peerlistwidget.cpp: https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/properties/peerlistwidget.cpp
- [S18] src/gui/trackerlist/trackerlistmodel.cpp: https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/trackerlist/trackerlistmodel.cpp
- [S19] src/gui/torrentcontentwidget.cpp: https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/torrentcontentwidget.cpp
- [S20] src/gui/optionsdialog.ui (pages, defaults: queueing, slow-torrent thresholds, alt limits, schedule, seeding limits, WebUI): https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/optionsdialog.ui
- [S21] src/gui/statsdialog.ui, speedlimitdialog.ui, torrentoptionsdialog.ui: https://github.com/qbittorrent/qBittorrent/tree/master/src/gui
- [S22] src/gui/executionlogwidget.cpp: https://github.com/qbittorrent/qBittorrent/blob/master/src/gui/executionlogwidget.cpp
- [S23] src/app/application.cpp (notifications, completion actions): https://github.com/qbittorrent/qBittorrent/blob/master/src/app/application.cpp
- [S24] libtorrent manual, queuing (auto-managed, active limits, slow torrents, seed ranking): https://www.libtorrent.org/manual-ref.html

**Verification limits.**
- I took the menu, toolbar and properties-tab layout from source and the wiki. I did not visually inspect the screenshots, so pixel layout is not described, deliberately.
- I did not confirm the status bar connection-icon click target from source (the click handler is outside statusbar.cpp).
- The UCompute facts come from the working tree, which other workflows are editing. Re-check `client.ts` and the `mesh.rs` routes before implementing.
