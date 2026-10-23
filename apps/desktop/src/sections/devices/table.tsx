// The Devices table (plan §2.2): columns, the row context menu and the row actions. The Dashboard's "Computers"
// block shows the same table and menu, so everything here works outside the Devices section too.
import { api, shortLabel, type Hardware, type Model, type PeerCapabilities, type Peer, type PeerRecord } from "../../api/client";
import { explain, mapError, type MappedError } from "../../api/errors";
import { ago, byteRate, clock, humanBytes, num } from "../../format";
import { go, parseHash, select } from "../../router";
import { dialErrorText, live, TRUST_WORD, type DeviceRow, type Tone, type Trust, type Word } from "../../state/devices";
import { dialing, hardware, mesh, models, netRate, network, refreshMany, samples, type ConnSample } from "../../state/live";
import { createStore } from "../../state/store";
import { daemon } from "../../state/stores";
import { setInspector } from "../../state/ui";
import type { MenuItem } from "../../ui/contextMenu";
import type { Column } from "../../ui/DataTable";
import { Icon, type IconName } from "../../ui/icons";
import { StateBadge } from "../../ui/primitives";
import { frameState, type FrameState } from "../../ui/SectionFrame";

/** 64 hex as 8 groups of 8, the form the pairing confirmation compares. */
export const grouped = (hex: string) => (hex.match(/.{1,8}/g) ?? [hex]).join(" ");
const nameOf = (p: PeerRecord & { short_label?: string }) => p.name || p.short_label || shortLabel(p.id);

/** Why a paired device's capabilities (E1) are missing; null when they are there. */
export function capsGap(caps: PeerCapabilities | null | undefined): string | null {
  if (caps === undefined) return "Not reported by this service version";
  if (caps === null) return "Not reported while not connected";
  return null;
}
/** The same for its hardware report, which can also be unreadable. */
export const hardwareGap = (caps: PeerCapabilities | null | undefined) => capsGap(caps) ?? (caps!.hardware === null ? "Sent in a format this version cannot read" : null);

const TRUST: Record<Trust, { icon: IconName; tone: Tone }> = {
  self: { icon: "monitor", tone: "neutral" },
  trusted: { icon: "shield-check", tone: "ok" },
  untrusted: { icon: "shield-off", tone: "neutral" },
  revoked: { icon: "shield-x", tone: "danger" },
};
const TONE_ICON: Record<Tone, IconName> = { ok: "circle-check", active: "loader", warn: "circle-alert", danger: "circle-x", neutral: "unplug" };
const stateIcon = (r: DeviceRow): IconName => (r.trust === "untrusted" ? "radio" : r.trust === "self" && r.state.tone === "neutral" ? "network" : TONE_ICON[r.state.tone]);
const runIcon = (w: Word): IconName => (w.word === "Can send" ? "upload" : w.word === "Not permitted here" ? "shield-off" : TONE_ICON[w.tone]);

export function TrustIcon({ trust }: { trust: Trust }) {
  return (
    <span className={`trust tone-${TRUST[trust].tone}`}>
      <Icon name={TRUST[trust].icon} />
    </span>
  );
}
export const StateCell = ({ r }: { r: DeviceRow }) => <StateBadge tone={r.state.tone} icon={stateIcon(r)} label={r.state.word} />;
export const RunCell = ({ w }: { w: Word | null }) => (!w || w.word === "—" ? <>—</> : <StateBadge tone={w.tone} icon={runIcon(w)} label={w.word} />);

/** Sort ranks (plan §2.2 table columns); anything unlisted sorts after the listed words. */
const STATE_ORDER = ["Connected", "Not responding", "Connecting", "Reconnecting", "Unreachable", "Not seen nearby", "Not connected", "Disconnected by", "Disconnected by you", "Networking off", "Not paired", "Revoked"];
const RUN_ORDER = ["Available", "Can send", "Busy", "Loading", "Refused", "Not permitted", "Not running"];
const rank = (order: string[], w: string) => {
  const exact = order.indexOf(w);
  if (exact >= 0) return exact;
  const i = order.findIndex((o) => w.startsWith(o));
  return i >= 0 ? i : order.length;
};

/** This connection's byte rate between the last two samples (same connection id), or null (a gap or no samples). */
export function connRate(conn: ConnSample[], id: number | undefined, now = Date.now()) {
  if (id == null) return null;
  const k = String(id);
  return netRate(conn.slice(-2).map((s) => ({ at: s.at, conns: s.conns[k] ? { [k]: s.conns[k] } : {} })), now);
}

interface Ctx {
  hw: Hardware | null;
  hwAt: number | null;
  models: Model[] | null;
  conn: ConnSample[];
  now: number;
}

const dash = (reason: string | null) => ({ text: "—", note: reason });

function hardwareCell(r: DeviceRow, c: Ctx): { text: string; note: string | null; bytes: number | null } {
  if (r.trust === "self") return c.hw ? { text: `${c.hw.host.chip} · ${humanBytes(c.hw.memory.total_bytes)}`, note: null, bytes: c.hw.memory.total_bytes } : { ...dash("Not reported yet"), bytes: null };
  if (r.trust !== "trusted") return { ...dash(null), bytes: null };
  const gap = hardwareGap(r.caps);
  const h = r.caps?.hardware;
  return gap || !h ? { ...dash(gap), bytes: null } : { text: `${h.host.chip} · ${humanBytes(h.memory.total_bytes)}`, note: null, bytes: h.memory.total_bytes };
}

function freeMemory(r: DeviceRow, c: Ctx): { text: string; note: string | null; bytes: number | null } {
  if (r.trust === "self") return c.hw ? { text: humanBytes(c.hw.memory.available_bytes), note: `probed ${ago(c.hwAt, c.now)}`, bytes: c.hw.memory.available_bytes } : { ...dash(null), bytes: null };
  // The start-time snapshot a peer sent is in its Hardware tab, with its caveat; never in a column that reads as live.
  if (r.trust === "trusted") return { ...dash("Not reported live"), bytes: null };
  return { ...dash(null), bytes: null };
}

function modelCount(r: DeviceRow, c: Ctx): number | null {
  if (r.trust === "self") return c.models ? c.models.filter((m) => m.downloaded).length : null;
  return r.caps ? r.caps.models.length : null;
}

/** Last seen: "Now" while live (`last_seen_utc` does not advance while connected); nearby rows by their record. */
function lastSeen(r: DeviceRow, now: number): { text: string; at: number | null } {
  if (r.peer && live(r.peer)) return { text: "Now", at: now };
  const t = r.peer ? r.peer.last_seen_utc : (r.discovered?.seen_utc ?? null);
  if (r.trust === "self" || !t) return { text: r.peer ? "Never" : "—", at: null };
  return { text: ago(t, now), at: Date.parse(t) };
}

const PROVENANCE_NOTE: Record<string, string> = { "this computer": "this computer", connected: "connected", "seen nearby": "seen nearby", "last known": "last known" };

/** The §2.2 columns, given the stores the self row and the rate columns read. */
export function deviceColumns(c: Ctx): Column<DeviceRow>[] {
  return [
    {
      id: "name",
      header: "Name",
      width: 160,
      cell: (r) => (
        <span className="device-name">
          <TrustIcon trust={r.trust} />
          {r.trust !== "self" && <span className="visually-hidden">{TRUST_WORD[r.trust]}: </span>}
          {r.name}
        </span>
      ),
      text: (r) => r.name,
      note: (r) => (r.trust === "self" ? "This computer" : r.trust === "untrusted" ? "Name is shared at pairing" : null),
      sort: (r) => r.name,
    },
    { id: "state", header: "State", width: 140, live: true, cell: (r) => <StateCell r={r} />, text: (r) => r.state.word, note: (r) => r.state.note ?? null, detail: (r) => r.state.detail ?? null, sort: (r) => (r.trust === "self" ? -1 : rank(STATE_ORDER, r.state.word)) },
    {
      id: "run",
      header: "Can run there",
      width: 130,
      live: true,
      cell: (r) => <RunCell w={r.runThere} />,
      text: (r) => r.runThere?.word ?? "—",
      note: (r) => r.runThere?.note ?? null,
      detail: (r) => r.runThere?.detail ?? null,
      sort: (r) => (!r.runThere || r.runThere.word === "—" ? null : rank(RUN_ORDER, r.runThere.word)),
    },
    { id: "use", header: "Can use this computer", width: 110, cell: (r) => r.useHere?.word ?? "—", note: (r) => r.useHere?.note ?? null, sort: (r) => (r.useHere ? (r.useHere.word === "Yes" ? 0 : 1) : null) },
    { id: "hardware", header: "Hardware", width: 140, cell: (r) => hardwareCell(r, c).text, note: (r) => hardwareCell(r, c).note, sort: (r) => hardwareCell(r, c).bytes },
    { id: "free", header: "Free memory", width: 90, align: "end", live: true, cell: (r) => freeMemory(r, c).text, note: (r) => freeMemory(r, c).note, sort: (r) => freeMemory(r, c).bytes },
    {
      id: "models",
      header: "Models",
      width: 50,
      align: "end",
      cell: (r) => num(modelCount(r, c)),
      detail: (r) => (r.trust === "trusted" && !r.caps ? capsGap(r.caps) : r.trust === "trusted" ? "Reported when this connection opened" : null),
      sort: (r) => modelCount(r, c),
    },
    { id: "seen", header: "Last seen", width: 80, live: true, cell: (r) => lastSeen(r, c.now).text, sort: (r) => lastSeen(r, c.now).at },
    { id: "trust", header: "Trust", hidden: true, width: 110, cell: (r) => TRUST_WORD[r.trust], sort: (r) => ["self", "trusted", "untrusted", "revoked"].indexOf(r.trust) },
    { id: "addr", header: "Address", hidden: true, width: 150, cell: (r) => <span className="mono">{r.addr?.value ?? "—"}</span>, text: (r) => r.addr?.value ?? "", note: (r) => (r.addr ? PROVENANCE_NOTE[r.addr.provenance] : "not known"), sort: (r) => r.addr?.value ?? null },
    { id: "jobs", header: "Jobs", hidden: true, width: 50, align: "end", live: true, cell: (r) => num(r.activeJobs), detail: (r) => (r.activeJobs == null && (r.trust === "self" || r.trust === "trusted") ? "Jobs are not reported by this service version" : null), sort: (r) => r.activeJobs },
    { id: "rtt", header: "RTT", hidden: true, width: 70, align: "end", live: true, cell: (r) => (r.link ? `${num(r.link.rtt_ms, 1)} ms` : "—"), sort: (r) => r.link?.rtt_ms ?? null },
    {
      id: "rate",
      header: "↓ / ↑",
      hidden: true,
      width: 150,
      live: true,
      cell: (r) => {
        const rate = connRate(c.conn, r.link?.id, c.now);
        return rate ? `↓ ${byteRate(rate.rx)} ↑ ${byteRate(rate.tx)}` : "—";
      },
      detail: (r) => (r.link ? "Includes 1 s keep-alives." : null),
      sort: (r) => connRate(c.conn, r.link?.id, c.now)?.rx ?? null,
    },
    { id: "groups", header: "Groups", hidden: true, width: 120, cell: (r) => (r.peer ? r.peer.groups.join(", ") || "None" : "—"), sort: (r) => r.peer?.groups.join(", ") ?? null },
    {
      id: "error",
      header: "Last error",
      hidden: true,
      width: 160,
      cell: (r) => (r.peer?.last_error ? mapError(r.peer.last_error.code, r.peer.last_error.message).title : "—"),
      detail: (r) => (r.peer?.last_error ? `${dialErrorText(r.peer.last_error)} (${r.peer.last_error.addr})` : null),
      sort: (r) => r.peer?.last_error?.code ?? null,
    },
    { id: "paired", header: "Paired", hidden: true, width: 110, cell: (r) => (r.peer ? new Date(r.peer.paired_at_utc).toLocaleDateString() : "—"), sort: (r) => (r.peer ? Date.parse(r.peer.paired_at_utc) : null) },
    { id: "id", header: "Device ID", hidden: true, width: 200, cell: (r) => <span className="mono">{r.id || "—"}</span>, text: (r) => r.id, sort: (r) => r.id || null },
    { id: "label", header: "Short label", hidden: true, width: 120, cell: (r) => <span className="mono">{r.shortLabel || "—"}</span>, text: (r) => r.shortLabel, sort: (r) => r.shortLabel || null },
    { id: "version", header: "Version", hidden: true, width: 80, cell: (r) => r.caps?.common_version ?? "—", sort: (r) => r.caps?.common_version ?? null },
  ];
}

/** The columns over the live stores (Devices and the Dashboard's "Computers" block). */
export function useDeviceColumns(): Column<DeviceRow>[] {
  const hw = hardware.use();
  const mo = models.use().data;
  const conn = samples.use().conn;
  return deviceColumns({ hw: hw.data, hwAt: hw.at, models: mo, conn, now: Date.now() });
}

// Row actions -----------------------------------------------------------------------------------------------------

/** The outcome of the last connect this app started, per device (shown in its Connection tab). */
export const connectOutcome = createStore<Record<string, { ok: string } | { error: MappedError }>>({});
/** A failed row action (disconnect, remove, revoke, networking), shown above the Devices table. */
export const deviceError = createStore<MappedError | null>(null);

/**
 * Why row actions wait (plan §2 shared states): the service is busy or a refresh failed, so the rows are not live.
 * null while they are. The toolbar gets the same reason from SectionFrame.
 */
export function frozenWhy(s: FrameState): string | null {
  if (typeof s !== "object") return null;
  if ("failed" in s) return `Not read from the service: ${s.failed.title}`;
  return "busy" in s ? `Service busy; not updated since ${clock(s.busy.since)}` : `Not updated since ${clock(s.stale.since)}`;
}
/** The same from the stores now, for the native row menu (built when it opens). */
export const frozenNow = () => frozenWhy(frameState(daemon.get(), network.get(), mesh.get()));

/** Focuses the Devices table (after a confirmed Remove or Revoke the confirmation and its row are gone; plan §2). */
export function focusDevices(): void {
  setTimeout(() => {
    const g = document.querySelector<HTMLElement>('[role="grid"][aria-label="Devices"]');
    (g?.querySelector<HTMLElement>('[data-active="true"]') ?? g)?.focus();
  }, 0);
}

/**
 * Selects `id` in Devices with `params` (in place), or goes there from another section (Dashboard menu). A `tab`
 * means the outcome is shown there (a connect error, the Revoke confirmation): the inspector is expanded for it.
 */
export function openDevice(id: string | null, params: Record<string, string> = {}): void {
  if (params.tab) setInspector("devices", { collapsed: false });
  if (parseHash(window.location.hash).section === "devices") select(id, params);
  else go("devices", id, params);
}

/**
 * A refused connect. An address proves nothing, so the daemon reports `peer_unauthorized` both for whatever answered
 * without proving it is the paired device (another device, or an alert sent before any key: common-network
 * transport.rs `unproven`) and for the paired device itself refusing this computer. Name both; never tell the user to
 * re-pair a device that may be fine.
 */
export function connectError(err: unknown, name: string): MappedError {
  const m = explain(err);
  if (m.code !== "peer_unauthorized") return m;
  return {
    ...m,
    title: `The device that answered did not prove it is ${name}, or ${name} no longer trusts this computer.`,
    next_step: `Nothing was trusted or saved. Check the address shown on ${name} under Devices › This computer › Connection.`,
  };
}

/** Why Connect is not possible now, or null. One dial at a time: another's outcome would be lost. */
export function connectBlocked(p: Peer, name: string): string | null {
  if (!network.get().data?.enabled) return "Networking is off";
  if (p.connected) return `${name} is connected`;
  const d = dialing.get();
  if (d === p.id) return `Connecting to ${name}…`;
  if (d) return "Wait for the other connection attempt to finish";
  return null;
}

/** Dials a paired device now: `addr` first when given, else the addresses the daemon knows. True when connected. */
export async function connectTo(p: Peer, addr?: string): Promise<boolean> {
  if (dialing.get()) return false;
  const name = nameOf(p);
  dialing.set(p.id);
  connectOutcome.set(({ [p.id]: _, ...rest }) => rest);
  let ok = false;
  try {
    const rec = await api.connectPeer(p.id, addr);
    connectOutcome.set((o) => ({ ...o, [p.id]: { ok: `Connected to ${nameOf(rec)}.` } }));
    ok = true;
  } catch (err) {
    connectOutcome.set((o) => ({ ...o, [p.id]: { error: connectError(err, name) } }));
  } finally {
    dialing.set(null);
  }
  await refreshMany(["mesh", "network"], true);
  return ok;
}

/** Toolbar and menu Connect: shows the device's Connection tab (where the outcome appears) and dials. */
export function connectFromRow(p: Peer): void {
  openDevice(p.id, { tab: "connection" });
  void connectTo(p);
}

async function run(action: () => Promise<unknown>, keys: Parameters<typeof refreshMany>[0] = ["mesh"]) {
  try {
    await action();
    deviceError.set(null);
  } catch (e) {
    deviceError.set(explain(e));
  }
  await refreshMany(keys, true);
}

/**
 * `PUT /api/network`. Turning networking off also stops serving paired devices (the service's rule). The PUT carries
 * both fields, so it is refused until the current state is read: a guess would switch sharing off.
 */
export async function setNetworking(patch: { enabled?: boolean; serve_peers?: boolean }): Promise<MappedError | null> {
  const cur = network.get().data;
  if (!cur) return { code: "network_not_read", title: "The network state has not been read yet.", next_step: "Wait a moment, then try again.", detail: "nothing was changed", hint: null };
  const enabled = patch.enabled ?? cur.enabled;
  try {
    const next = await api.setNetwork({ enabled, serve_peers: enabled ? (patch.serve_peers ?? cur.serve_peers) : false });
    network.set({ data: next, at: Date.now(), error: null });
    void refreshMany(["mesh"], true);
    return null;
  } catch (e) {
    return explain(e);
  }
}

/** E5: closes every connection to the device and holds it until Connect, a networking restart or a service restart. */
export const disconnect = (p: Peer) => run(() => api.disconnectPeer(p.id));
export const removeDevice = (p: Peer) => run(() => api.removePeer(p.id), ["mesh", "groups"]);
export const revokeDevice = (p: Peer) => run(() => api.revokePeer(p.id), ["mesh", "groups"]);

export function copyId(id: string): void {
  void navigator.clipboard?.writeText(id).catch(() => undefined);
}

/** A nearby (or revoked but announcing) device whose pairing window can be joined from here. */
export const canPairWith = (r: DeviceRow) => (r.trust === "untrusted" || r.trust === "revoked") && !!r.discovered;

/**
 * The row's native context menu (Tauri only). Revoke is here and in the Overview tab's Danger group, never in the
 * toolbar or on a key; Remove and Revoke open their confirmation first. While the rows are not live (busy or stale)
 * every action on the service waits; Copy ID does not.
 */
export function deviceMenu(r: DeviceRow): MenuItem[] {
  const items: MenuItem[] = [];
  const ok = !frozenNow();
  const p = r.peer;
  if (p) {
    items.push({ label: `Connect to ${r.name}`, enabled: ok && !connectBlocked(p, r.name), action: () => connectFromRow(p) });
    if (p.held !== undefined) items.push({ label: `Disconnect ${r.name}`, enabled: ok && p.connected, action: () => void disconnect(p) });
  }
  // Any nearby row (plan §2.2): the pairing-window announcement can lag a window just opened there.
  if (canPairWith(r)) items.push({ label: `Pair with ${r.name}…`, enabled: ok, action: () => openDevice(r.key, { pair: "join" }) });
  if (r.id) items.push({ label: "Copy ID", action: () => copyId(r.id) });
  if (p)
    items.push(
      { separator: true },
      { label: `Remove ${r.name}…`, enabled: ok, action: () => openDevice(r.key, { confirm: "remove" }) },
      { label: `Revoke ${r.name}…`, enabled: ok, action: () => openDevice(r.key, { tab: "overview", confirm: "revoke" }) },
    );
  return items;
}

/**
 * The `ip:port` shape the service accepts for Connect (IPv4, or IPv6 in brackets; port 1-65535), checked before the
 * request. The service still refuses 0.0.0.0, :: and multicast. null when it fits.
 */
export function dialAddrError(s: string): MappedError | null {
  const a = s.trim();
  const m = /^(?:(\d{1,3}(?:\.\d{1,3}){3})|\[[0-9a-fA-F:.]+\]):(\d{1,5})$/.exec(a);
  const port = Number(m?.[2]);
  if (m && (!m[1] || m[1].split(".").every((o) => Number(o) <= 255)) && port >= 1 && port <= 65535) return null;
  return { ...mapError("invalid_request", `"${a}" is not ip:port`), title: "That is not an address with a port.", next_step: "Enter it as ip:port, such as 192.168.1.20:8193, or leave it empty." };
}

/** "10:32:07 (3 min. ago)". */
export const when = (t: string | number | null | undefined, now = Date.now()) => (t ? `${clock(t)} (${ago(t, now)})` : "—");
