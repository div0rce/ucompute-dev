// App-level live data. Each polled store keeps its last good data and when it was fetched (`at`), plus the latest
// failure (`error`): a failed refresh never empties a table, it only marks it stale (plan §0.2, §5.3).
import { useEffect } from "react";
import { api, type AuditEvent, type Group, type Hardware, type Job, type JobsData, type Mesh, type Model, type NetworkState, type PullEvent, type ReqOpts } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { createStore } from "./store";
import { daemon, privacy } from "./stores";

export interface Live<T> {
  data: T | null;
  at: number | null;
  error: MappedError | null;
}
const blank = <T>(): Live<T> => ({ data: null, at: null, error: null });

export const network = createStore<Live<NetworkState>>(blank());
export const mesh = createStore<Live<Mesh>>(blank());
export const groups = createStore<Live<Group[]>>(blank());
export const models = createStore<Live<Model[]>>(blank());
export const hardware = createStore<Live<Hardware>>(blank());
/** `"unsupported"`: the service has no `/api/jobs` (before E3). */
export const jobs = createStore<Live<JobsData | "unsupported">>(blank());
/** `GET /api/audit?n=1000`, oldest first as the service returns it. */
export const audit = createStore<Live<AuditEvent[]>>(blank());
/** `/metrics` parsed (`name{labels}` -> value); polled only while needed (Jobs). */
export const metrics = createStore<Live<Record<string, number>>>(blank());
/** Before E2: consecutive `/ready` 503s saying "a model is loading" (one miss can be a request briefly holding the slot). */
export const ready = createStore<{ loadingStreak: number }>({ loadingStreak: 0 });

/** Per-connection byte counters (E1) at one mesh refresh. Keyed by `String(connection.id)`. */
export interface ConnSample {
  at: number;
  conns: Record<string, { peer: string; rx: number; tx: number }>;
}
/** Cumulative completion tokens this app saw generated (E3 live counts), by where they ran. Monotonic. */
export interface TokenSample {
  at: number;
  local: number;
  sent: number;
  served: number;
}
/** Rings of 720 samples, at least ~5 s apart (about an hour each). */
export const samples = createStore<{ conn: ConnSample[]; tokens: TokenSample[] }>({ conn: [], tokens: [] });

/** Downloads this app started (C's `startPull` writes it); survives navigation, not a reload (N4). */
export interface Pull {
  state: "downloading" | "done" | "error";
  ev: PullEvent | null;
  error?: MappedError;
  startedAt: number;
}
export const pulls = createStore<Record<string, Pull>>({});

/** The open pairing window started from this app (B's PairDialog writes it); `expiresAt` is epoch ms. */
export interface PairingState {
  code: string;
  addr: string | null;
  payload: string;
  expiresAt: number;
}
export const pairing = createStore<PairingState | null>(null);

/** Dismissable event entries shown in Attention (shell/attention.ts adds and clears them). */
export interface AttentionEntry {
  id: string;
  tone: "warn" | "danger";
  text: string;
  action?: { label: string; run: () => void };
}
export const attention = createStore<AttentionEntry[]>([]);

/** The paired device this app is dialling now (one dial at a time); null when none. */
export const dialing = createStore<string | null>(null);
/** Streams this app started, by client ref, registered before the request is sent; `stop` aborts the fetch. */
export const ownStreams = createStore<Record<string, { stop: () => void }>>({});

export type Key = "network" | "mesh" | "groups" | "models" | "hardware" | "jobs" | "audit" | "metrics" | "privacy" | "ready";

type Store<T> = { get: () => Live<T>; set: (v: Live<T> | ((p: Live<T>) => Live<T>)) => void };
async function into<T>(store: Store<T>, p: Promise<T>): Promise<T | null> {
  try {
    const data = await p;
    store.set({ data, at: Date.now(), error: null });
    return data;
  } catch (err) {
    store.set((s) => ({ ...s, error: explain(err) }));
    return null;
  }
}

export const SAMPLE_MS = 5000;
const RING = 720;
const push = <T extends { at: number }>(ring: T[], s: T) => (ring.length && s.at - ring[ring.length - 1].at < SAMPLE_MS * 0.9 ? ring : [...ring.slice(-(RING - 1)), s]);

function sampleConns(m: Mesh, at: number) {
  // Without E1 no peer carries `connection`, and a rate would be invented: record nothing.
  if (!m.peers.some((p) => "connection" in p)) return;
  const conns: ConnSample["conns"] = {};
  for (const p of m.peers) if (p.connection) conns[String(p.connection.id)] = { peer: p.id, rx: p.connection.rx_bytes, tx: p.connection.tx_bytes };
  samples.set((s) => ({ ...s, conn: push(s.conn, { at, conns }) }));
}

const jobCategory = (j: Job): "local" | "sent" | "served" => (j.kind === "served_for_peer" ? "served" : j.target === "local" ? "local" : "sent");
let tokenSeen = new Map<string, number>();
let tokenTotals = { local: 0, sent: 0, served: 0 };
let tokenPrimed = false;
function sampleTokens(j: JobsData, at: number) {
  const rows = [...j.active, ...j.recent];
  const next = new Map<string, number>();
  for (const r of rows) {
    const n = r.completion_tokens ?? 0;
    const prev = tokenSeen.get(r.id) ?? 0;
    // The first poll only primes: tokens generated before the app looked are not a rate.
    if (tokenPrimed && n > prev) tokenTotals = { ...tokenTotals, [jobCategory(r)]: tokenTotals[jobCategory(r)] + (n - prev) };
    next.set(r.id, Math.max(prev, n));
  }
  tokenSeen = next;
  tokenPrimed = true;
  samples.set((s) => ({ ...s, tokens: push(s.tokens, { at, ...tokenTotals }) }));
}

const AUDIT_N = 1000;
const fetchers: Record<Key, (o: ReqOpts) => Promise<unknown>> = {
  network: (o) => into(network, api.network(o)),
  mesh: async (o) => {
    const m = await into(mesh, api.peers(o));
    if (m) sampleConns(m, Date.now());
  },
  groups: (o) => into(groups, api.groups(o)),
  models: (o) => into(models, api.models(o)),
  hardware: (o) => into(hardware, api.hardware(o)),
  jobs: async (o) => {
    const j = await into(jobs, api.jobs(o));
    if (j && j !== "unsupported") sampleTokens(j, Date.now());
  },
  audit: (o) => into(audit, api.audit(AUDIT_N, o)),
  metrics: (o) => into(metrics, api.metrics(o)),
  privacy: (o) => into(privacy, api.privacy(o)),
  ready: async (o) => {
    const r = await api.ready(o);
    ready.set((s) => ({ loadingStreak: r.status === "not_ready" && r.reason === "a model is loading" ? s.loadingStreak + 1 : 0 }));
  },
};

let defaultTimeoutMs = 5000;
/** Tests inject a short timeout; the default is 5 s. */
export function setRefreshTimeout(ms: number): void {
  defaultTimeoutMs = ms;
}

const inflight = new Map<Key, Promise<void>>();
/** Refreshes one store. Single-flight: a call while one runs returns that one. Never rejects (failures land in `error`). */
export function refresh(key: Key, timeoutMs = defaultTimeoutMs): Promise<void> {
  const running = inflight.get(key);
  if (running) return running;
  const p = fetchers[key]({ timeoutMs })
    .then(
      () => undefined,
      () => undefined,
    )
    .finally(() => inflight.delete(key));
  inflight.set(key, p);
  return p;
}
/**
 * A read that starts after this call: waits out one already in flight (it may have been answered before the action
 * the caller just finished), then reads again. Use after an action, so the result is never the pre-action state.
 */
export async function refreshFresh(key: Key, timeoutMs = defaultTimeoutMs): Promise<void> {
  const running = inflight.get(key);
  if (running) await running;
  return refresh(key, timeoutMs);
}
/** Reads `keys` at once; `fresh` after an action (see `refreshFresh`), else a read in flight answers (polls, reconnect). */
export const refreshMany = (keys: readonly Key[], fresh = false) => Promise.all(keys.map((k) => (fresh ? refreshFresh(k) : refresh(k)))).then(() => undefined);

const needs = new Map<Key, number>();
/** Keys some mounted view needs now. */
export const neededKeys = (): Key[] => [...needs].filter(([, n]) => n > 0).map(([k]) => k);
/** True while a mounted view declared it needs `key` (faster cadence for jobs, metrics, audit). */
export const isNeeded = (key: Key) => (needs.get(key) ?? 0) > 0;
/**
 * Declares the data a mounted view needs: refreshed at once, then polled at the "needed" cadence (poll.ts). Until the
 * service answers, the app may not know its address yet (it asks the shell first, or a dev `?daemon=` is still being
 * tried): then nothing is fetched here, and App refreshes every needed key when the service becomes reachable.
 */
export function useNeed(keys: readonly Key[]): void {
  const sig = keys.join(",");
  useEffect(() => {
    const ks = (sig ? sig.split(",") : []) as Key[];
    for (const k of ks) {
      needs.set(k, (needs.get(k) ?? 0) + 1);
      if (daemon.get().reachable === true) void refresh(k);
    }
    return () => {
      for (const k of ks) needs.set(k, Math.max(0, (needs.get(k) ?? 1) - 1));
    };
  }, [sig]);
}

/** Network rate summed over connections present in the last two samples with the same id; null = unknown (a gap). */
export function netRate(conn: ConnSample[], now = Date.now(), interval = SAMPLE_MS): { rx: number; tx: number } | null {
  const [a, b] = conn.slice(-2);
  if (!a || !b || b.at - a.at > 1.5 * interval || now - b.at > 1.5 * interval) return null;
  const dt = (b.at - a.at) / 1000;
  let rx = 0;
  let tx = 0;
  let n = 0;
  for (const [id, c] of Object.entries(b.conns)) {
    const p = a.conns[id];
    if (!p || dt <= 0) continue;
    rx += Math.max(0, c.rx - p.rx);
    tx += Math.max(0, c.tx - p.tx);
    n++;
  }
  return n ? { rx: rx / dt, tx: tx / dt } : null;
}

/** Tokens/s per category between consecutive samples; null marks a gap (missed samples). */
export function tokenRates(s: TokenSample[], interval = SAMPLE_MS): { local: (number | null)[]; sent: (number | null)[]; served: (number | null)[] } {
  const out = { local: [] as (number | null)[], sent: [] as (number | null)[], served: [] as (number | null)[] };
  for (let i = 1; i < s.length; i++) {
    const dt = s[i].at - s[i - 1].at;
    const gap = dt <= 0 || dt > 1.5 * interval;
    for (const k of ["local", "sent", "served"] as const) out[k].push(gap ? null : ((s[i][k] - s[i - 1][k]) * 1000) / dt);
  }
  return out;
}

/** Test helper: empties every live store and the refresh bookkeeping. */
export function resetLive(): void {
  for (const s of [network, mesh, groups, models, hardware, jobs, audit, metrics] as Store<unknown>[]) s.set(blank());
  ready.set({ loadingStreak: 0 });
  samples.set({ conn: [], tokens: [] });
  pulls.set({});
  pairing.set(null);
  attention.set([]);
  dialing.set(null);
  ownStreams.set({});
  inflight.clear();
  needs.clear();
  tokenSeen = new Map();
  tokenTotals = { local: 0, sent: 0, served: 0 };
  tokenPrimed = false;
  defaultTimeoutMs = 5000;
}
