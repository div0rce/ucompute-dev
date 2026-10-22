// Device derivations shared by Devices (owner B after Phase 0), the Dashboard, the rail, the status bar and Chat.
// Created by A in Phase 0 from views/Peers.tsx; `connState` now follows the plan §2.2 Connection table (★ rules).
import { shortLabel, type AuditEvent, type Discovered, type Group, type Hardware, type JobsData, type Model, type Mesh, type NetworkState, type Peer, type PeerCapabilities, type PeerConnectionInfo, type Revocation } from "../api/client";
import { mapError } from "../api/errors";
import { ago, clock } from "../format";
import { audit, dialing, hardware, jobs, mesh, models, network, groups as groupsStore } from "./live";
import { daemon, privacy, settings, type DaemonState } from "./stores";

export type Tone = "ok" | "active" | "warn" | "danger" | "neutral";

/** "Connected" everywhere means this: an authenticated connection that is not missing keep-alives. */
export const live = (p: Peer) => p.connected && p.health !== "unhealthy";

/**
 * Whether prompts sent from this computer can run on `p` now, and otherwise what is missing, from what this computer
 * knows: an open connection, one of its groups allowing prompts, and Privacy. The other device's own groups and
 * settings are not visible here.
 */
export function promptReadiness(p: Peer, groups: Group[], localOnly: boolean): { ok: boolean; text: string } {
  const steps = [
    ...(p.connected ? [] : ["connect it"]),
    ...(groups.some((g) => g.members.includes(p.id) && g.policy.allow_remote_inference)
      ? []
      : [p.groups.length ? `turn on "Members may run prompts for each other" in ${p.groups.length === 1 ? "its group" : "one of its groups"}` : "add it to a group that allows prompts"]),
    ...(localOnly ? ["allow trusted devices under Privacy"] : []),
  ];
  return steps.length ? { ok: false, text: `Cannot run prompts from this computer yet: ${steps.join(", ")}.` } : { ok: true, text: "Can run prompts from this computer." };
}

/** What this computer grants `p` (mirrors `PolicyEngine::may_serve`). `groups` null: not read yet, so not known. */
export function canUseThisComputer(p: Peer, network: NetworkState | null, groups: Group[] | null): { word: string; note?: string } {
  if (!network) return { word: "—", note: "network state not read yet" };
  if (!network.enabled) return { word: "No · networking off" };
  if (!network.serve_peers) return { word: "No · sharing off" };
  if (!groups) return { word: "—", note: "groups not read yet" };
  if (!groups.some((g) => g.members.includes(p.id) && g.policy.allow_remote_inference)) return { word: "No · not in a prompt group" };
  return { word: "Yes", note: "Quotas and free slots still apply." };
}

export interface Word {
  word: string;
  tone: Tone;
  /** Extra text for the tooltip / inspector. */
  detail?: string;
  /** Visible muted suffix (a caveat that changes the meaning). */
  note?: string;
}
export type Trust = "self" | "trusted" | "untrusted" | "revoked";

export interface DeviceRow {
  /** "self" for this computer, else the 64-hex id. */
  key: string;
  /** Full 64-hex id (this computer's own for the self row). */
  id: string;
  trust: Trust;
  name: string;
  shortLabel: string;
  peer?: Peer;
  discovered?: Discovered;
  revocation?: Revocation;
  state: Word;
  /** Can work sent from this computer run there now? null for nearby and revoked rows. */
  runThere: Word | null;
  /** What this computer grants that device; null for self, nearby and revoked rows. */
  useHere: { word: string; note?: string } | null;
  addr: { value: string; provenance: "this computer" | "connected" | "seen nearby" | "last known" } | null;
  /** undefined: the service does not report it (before E1); null: not connected. */
  caps: PeerCapabilities | null | undefined;
  link: PeerConnectionInfo | null | undefined;
  /** E3 active jobs this computer runs there (self: locally); null when jobs are not reported. */
  activeJobs: number | null;
}

export interface DeviceInput {
  daemon: DaemonState;
  network: NetworkState | null;
  mesh: Mesh | null;
  /** null: not read yet (permissions that depend on it are not known, never "No"). */
  groups: Group[] | null;
  models: Model[] | null;
  hardware: Hardware | null;
  /** The pairing name the user set here ("" = the daemon's). */
  deviceName: string;
  /** Privacy is local only; null: not read yet. */
  localOnly: boolean | null;
  jobs: JobsData | "unsupported" | null;
  audit: AuditEvent[] | null;
  dialing: string | null;
  now: number;
}

const OUTCOME_REFUSED = new Set(["policy_denied", "peer_unauthorized", "peer_revoked"]);

/** Trust as a word (the accessible text of the Name cell's icon). */
export const TRUST_WORD: Record<Trust, string> = { self: "This computer", trusted: "Trusted", untrusted: "Untrusted (nearby)", revoked: "Revoked" };

/** "Last try at 10:32:07: no answer from 1.2.3.4:8193 within 4 s. The device may be asleep." from a failed dial. */
export function dialErrorText(err: { message: string; hint?: string | null; at_utc: string }): string {
  return `Last try at ${clock(err.at_utc)}: ${sentence(err.message)}${err.hint ? ` ${sentence(err.hint)}` : ""}`;
}
/** Daemon messages and hints are lower-case clauses; this makes one a sentence without doubling its full stop. */
export const sentence = (s: string) => `${s.charAt(0).toUpperCase()}${s.slice(1).replace(/\.$/, "")}.`;

/**
 * The last finished `remote_request` this computer sent to `id` (newest wins), only from `since` (ISO) on when given.
 * Timestamps compare as instants: the service may write fractional seconds.
 */
export function lastOutcome(events: AuditEvent[] | null, id: string, since: string | null = null): { outcome: string; at: string } | null {
  if (!events) return null;
  const from = since ? Date.parse(since) : null;
  for (let i = events.length - 1; i >= 0; i--) {
    const e = events[i];
    if (from != null && Date.parse(e.ts_utc) < from) return null;
    const f = (e.fields ?? {}) as Record<string, unknown>;
    if (e.kind === "remote_request" && e.peer_id === id && f.role === "client" && f.phase === "finished" && typeof f.outcome === "string") return { outcome: f.outcome, at: e.ts_utc };
  }
  return null;
}

/** Start of the current connection: E1 `opened_at_utc`, else the newest `peer_connected` audit entry. */
function connectionStart(p: Peer, events: AuditEvent[] | null): string | null {
  if (p.connection) return p.connection.opened_at_utc;
  for (let i = (events?.length ?? 0) - 1; i >= 0; i--) if (events![i].kind === "peer_connected" && events![i].peer_id === p.id) return events![i].ts_utc;
  return null;
}

/**
 * A paired device's connection (plan §2.2, first match wins). Only an open, authenticated connection is Connected: a
 * signed mDNS record (`seen`) only says the device announced itself nearby, and the daemon may hold it after the
 * device left; a missing record never means "offline". `last_error` is what the daemon's last dial ran into.
 */
export function connState(p: Peer, i: Pick<DeviceInput, "network" | "dialing">, seen: Discovered | undefined): Word {
  const name = p.name || p.short_label;
  const err = p.last_error;
  const net = i.network;
  if (net && !net.enabled) return { word: "Networking off", tone: "neutral" };
  if (p.held === true && !p.connected) return { word: "Disconnected by you", tone: "neutral", detail: "Until you connect again, turn networking off and on, or the service restarts." };
  if (p.connected && p.health === "unhealthy") return { word: "Not responding", tone: "warn" };
  if (p.connected) return { word: "Connected", tone: "ok" };
  // Networking off and every rule below need the network state: unknown until it is read, never "Networking off".
  if (!net) return { word: "Not known", tone: "neutral", detail: "The network state has not been read yet." };
  if (i.dialing === p.id) return { word: "Connecting", tone: "active" };
  if (err?.code === "peer_held") return { word: `Disconnected by ${name}`, tone: "neutral", detail: "It must reconnect from its side." };
  if (err?.code === "peer_lost" && net.mdns && !seen)
    return { word: "Not seen nearby", tone: "neutral", detail: `Nothing answered at ${err.addr}, ${clock(err.at_utc)}; not announced nearby.` };
  if (err) return { word: "Unreachable", tone: "warn", detail: dialErrorText(err) };
  if (p.health === "reconnecting") return { word: "Reconnecting", tone: "warn" };
  if (net.mdns && !seen) return { word: "Not seen nearby", tone: "neutral" };
  return { word: "Not connected", tone: "neutral" };
}

function peerRunThere(p: Peer, i: DeviceInput): Word {
  if (!live(p)) return { word: "—", tone: "neutral" };
  // Unknown until both are read: Chat still treats an unknown privacy mode as local only.
  if (i.groups === null || i.localOnly === null) return { word: "—", tone: "neutral", note: "groups or privacy not read yet" };
  const ready = promptReadiness(p, i.groups, i.localOnly);
  if (!ready.ok) return { word: "Not permitted here", tone: "neutral", detail: ready.text };
  // No known start of this connection: no outcome is known to belong to it, so none is shown (an older one is not).
  const start = connectionStart(p, i.audit);
  const last = start ? lastOutcome(i.audit, p.id, start) : null;
  const quiet = "Its own permissions, serving setting and load are not reported.";
  if (!last) return { word: "Can send", tone: "neutral", note: quiet };
  const when = clock(last.at);
  if (OUTCOME_REFUSED.has(last.outcome)) return { word: "Refused last request (its permissions)", tone: "warn", detail: `${mapError(last.outcome, "").title} ${when}` };
  if (last.outcome === "overloaded") return { word: "Refused last request (busy or over quota)", tone: "warn", detail: when };
  if (last.outcome === "done") return { word: "Can send · last request done", tone: "ok", detail: when };
  return { word: `Can send · last request failed: ${mapError(last.outcome, "").title}`, tone: "warn", detail: when };
}

function selfRunThere(i: DeviceInput): Word {
  const d = i.daemon;
  const s = d.status;
  if (d.reachable !== true) return { word: "Not running", tone: "danger" };
  if (d.responding === false) return { word: "Busy (not answering)", tone: "warn" };
  // The queue below would be the last good status, not the current one.
  if (d.statusError) return { word: "Not known", tone: "neutral", detail: `The service's status was not read: ${d.statusError.title}` };
  if (s?.engine?.state === "loading") return { word: "Loading model", tone: "active" };
  if (s && (s.queue.inflight >= s.queue.max_concurrent || s.queue.queued > 0))
    return { word: `Busy · ${s.queue.inflight}/${s.queue.max_concurrent} in use, ${s.queue.queued} waiting`, tone: "warn" };
  if (!s) return { word: "Not known", tone: "neutral", detail: "The service's status has not been read yet." };
  return { word: "Available", tone: "ok", note: s.loaded ? undefined : "No model loaded" };
}

const activeOn = (j: DeviceInput["jobs"], target: string) => (j && j !== "unsupported" ? j.active.filter((x) => x.target === target).length : null);

/** Rows for the Devices table: this computer first, then paired, nearby (not paired) and revoked devices. */
export function deviceRows(i: DeviceInput): DeviceRow[] {
  const net = i.network;
  const m = i.mesh ?? { peers: [], discovered: [], revocations: [] };
  const paired = new Set(m.peers.map((p) => p.id));
  const revoked = new Set(m.revocations.filter((r) => !paired.has(r.peer_id)).map((r) => r.peer_id));
  const seen = (id: string) => m.discovered.find((d) => d.id === id);
  const liveCount = m.peers.filter(live).length;
  const selfId = net?.peer_id ?? "";

  const self: DeviceRow = {
    key: "self",
    id: selfId,
    trust: "self",
    name: i.deviceName || net?.device_name || "This computer",
    shortLabel: net?.short_label ?? "",
    state: !net
      ? { word: "Not known", tone: "neutral", detail: "The network state has not been read yet." }
      : !net.enabled
        ? { word: "Networking off", tone: "neutral" }
        : !net.listen_addr
          ? { word: "Not listening", tone: "warn" }
          : { word: `Listening · ${liveCount} connected`, tone: liveCount ? "ok" : "neutral" },
    runThere: selfRunThere(i),
    useHere: null,
    addr: net?.dial_addr ? { value: net.dial_addr, provenance: "this computer" } : null,
    caps: undefined,
    link: undefined,
    activeJobs: activeOn(i.jobs, "local"),
  };

  const peers: DeviceRow[] = m.peers.map((p) => {
    const d = seen(p.id);
    const addr = p.connection?.remote_addr
      ? { value: p.connection.remote_addr, provenance: "connected" as const }
      : d
        ? { value: d.addrs?.[0] ?? d.addr, provenance: "seen nearby" as const }
        : p.last_addr
          ? { value: p.last_addr, provenance: "last known" as const }
          : null;
    return {
      key: p.id,
      id: p.id,
      trust: "trusted",
      name: p.name || p.short_label,
      shortLabel: p.short_label,
      peer: p,
      discovered: d,
      state: connState(p, i, d),
      runThere: peerRunThere(p, i),
      useHere: canUseThisComputer(p, net, i.groups),
      addr,
      caps: p.capabilities,
      link: p.connection,
      activeJobs: activeOn(i.jobs, `peer:${p.id}`),
    };
  });

  const nearby: DeviceRow[] = m.discovered
    .filter((d) => !d.trusted && d.id !== selfId && !paired.has(d.id) && !m.revocations.some((r) => r.peer_id === d.id))
    .map((d) => ({
      key: d.id,
      id: d.id,
      trust: "untrusted",
      name: shortLabel(d.id),
      shortLabel: shortLabel(d.id),
      discovered: d,
      state: { word: `Not paired · announced ${ago(d.seen_utc, i.now)}`, tone: "neutral" },
      runThere: null,
      useHere: null,
      addr: { value: d.addrs?.[0] ?? d.addr, provenance: "seen nearby" },
      caps: undefined,
      link: undefined,
      activeJobs: null,
    }));

  const gone: DeviceRow[] = m.revocations
    .filter((r) => revoked.has(r.peer_id))
    .map((r) => {
      const d = seen(r.peer_id);
      return {
        key: r.peer_id,
        id: r.peer_id,
        trust: "revoked",
        name: shortLabel(r.peer_id),
        shortLabel: shortLabel(r.peer_id),
        revocation: r,
        discovered: d,
        state: { word: d ? `Revoked · announced nearby ${ago(d.seen_utc, i.now)}` : "Revoked", tone: "danger" },
        runThere: null,
        useHere: null,
        addr: d ? { value: d.addrs?.[0] ?? d.addr, provenance: "seen nearby" } : null,
        caps: undefined,
        link: undefined,
        activeJobs: null,
      };
    });

  return [self, ...peers, ...nearby, ...gone];
}

/** `deviceRows` over the live stores; re-renders when any of them changes. */
export function useDeviceRows(): DeviceRow[] {
  const d = daemon.use();
  const n = network.use();
  const m = mesh.use();
  const g = groupsStore.use();
  const mo = models.use();
  const h = hardware.use();
  const s = settings.use();
  const pr = privacy.use();
  const j = jobs.use();
  const a = audit.use();
  const dial = dialing.use();
  return deviceRows({
    daemon: d,
    network: n.data,
    mesh: m.data,
    groups: g.data,
    models: mo.data,
    hardware: h.data,
    deviceName: s.device_name,
    localOnly: pr ? pr.mode === "local_only" : null,
    jobs: j.data,
    audit: a.data,
    dialing: dial,
    now: Date.now(),
  });
}
