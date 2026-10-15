// The only module that talks to the daemon. Every endpoint, type and the SSE parser live here so an
// API-contract change is a one-file edit.

export type Device = "auto" | "cpu" | "gpu";

export interface Loaded {
  model_id: string;
  model_sha256: string;
  backend_name: string;
  backend_version: string;
  device_requested: string;
  device_actual: string;
  offloaded_layers: number;
  total_layers: number;
  n_ctx: number;
  threads: number;
  parallel_slots: number;
  fallback_reason: string | null;
  load_ms: number;
  pid: number;
}

export interface Status {
  version: string;
  uptime_seconds: number;
  loaded: Loaded | null;
  queue: { inflight: number; queued: number; max_concurrent: number; max_queue: number };
}

export interface Hardware {
  host: { os: string; os_version: string; arch: string; chip: string; memory_bytes: number };
  cpu: { model: string; physical_cores: number; logical_cores: number; arch: string; features: string[] };
  memory: { total_bytes: number; available_bytes: number; unified: boolean };
  gpus: { name: string; vendor: string; backend: string; memory_bytes: number; cores: number }[];
  backends: { name: string; available: boolean; reason: string | null }[];
  disk_free_bytes: number;
  probe_ms: number;
}

export type Fit =
  | { verdict: "fits"; headroom_bytes: number }
  | { verdict: "too_large"; needed_bytes: number; available_bytes: number; reason: string };

export interface Estimate {
  weights_bytes: number;
  kv_cache_bytes: number;
  compute_buffer_bytes: number;
  overhead_bytes: number;
  total_bytes: number;
  n_ctx: number;
}

export interface Model {
  id: string;
  name: string;
  bytes: number;
  license: string | null;
  license_url: string | null;
  source: string;
  family: string;
  parameters: string;
  quantization: string;
  chat: boolean;
  redistribution: string;
  tags: string[];
  downloaded: boolean;
  loaded: boolean;
  fit: Fit;
  estimate: Estimate;
}

export interface PullEvent {
  downloaded_bytes: number;
  total_bytes: number;
  network_bytes: number;
  resumed_from: number;
  done: boolean;
}

export interface ChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
}

export interface Timings {
  time_to_first_token_ms: number;
  prompt_tokens_per_second: number;
  decode_tokens_per_second: number;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface ChatResult {
  content: string;
  finish_reason: string | null;
  usage: Usage | null;
  timings: Timings | null;
  /** True when the client aborted the stream. */
  stopped: boolean;
  /** Echoed by the daemon when the request named a target; "local" otherwise. */
  target: Target;
}

/** Peer networking (ADR-0003). Ids are 64 lowercase hex; `short_label` is `xxxx-xxxx-xxxx`, display only. */
export interface NetworkState {
  /** True only while the endpoint is actually bound. */
  enabled: boolean;
  serve_peers: boolean;
  /** `ip:port` the daemon listens on; null while networking is off. */
  listen_addr: string | null;
  mdns: boolean;
  peer_id: string;
  short_label: string;
  connections: number;
}

/** A pinned device as the daemon stores it (`common_identity::PeerRecord`). */
export interface PeerRecord {
  id: string;
  name: string;
  paired_at_utc: string;
  last_seen_utc: string | null;
  /** Last known `ip:port`; advisory. */
  last_addr: string | null;
}

export type PeerHealth = "connected" | "unhealthy" | "reconnecting" | "disconnected";

/** A pinned device as `GET /api/peers` lists it: the record plus live state. */
export interface Peer extends PeerRecord {
  short_label: string;
  groups: string[];
  connected: boolean;
  /** Heartbeat health; not emitted by every daemon version, so readers fall back to `connected`. */
  health?: PeerHealth;
}

/** Seen through mDNS but not pinned. Never actionable except pairing. */
export interface Discovered {
  id: string;
  /** Workload endpoint `ip:port`. */
  addr: string;
  pairing: boolean;
  /** Port of the open pairing window on that host, when `pairing`. */
  pair_port: number | null;
  proto: string;
  trusted: boolean;
  seen_utc: string;
}

export interface PairingSession {
  code: string;
  port: number;
  qr_payload: string;
  /** Unix seconds (the daemon's form) or an ISO timestamp; see `expiresAtMs`. */
  expires_at: number | string;
}

/** `GET /api/pair/status`. `result` stays set after the window closes until the next start. */
export interface PairingStatus {
  active: boolean;
  result: null | { peer: PeerRecord } | { error: { code: string; message: string; hint?: string | null } };
}

export interface Group {
  name: string;
  members: string[];
  policy: { allow_remote_inference: boolean; allow_pooled_inference: boolean };
}

export type PrivacyMode = "local_only" | "trusted_group";

export interface Privacy {
  mode: PrivacyMode;
  /** What leaves this device in the non-local mode; shown before the user can switch. */
  disclosure: string;
}

export interface AuditEvent {
  ts_utc: string;
  kind: string;
  peer_id?: string;
  /** Structured, redaction-safe detail (ids, sizes, reasons); any JSON value, usually an object. */
  fields: unknown;
}

/** Where a request runs: this computer or a paired device. */
export type Target = "local" | `peer:${string}`;

/** `expires_at` as epoch milliseconds whichever form the daemon used. */
export function expiresAtMs(v: number | string): number {
  return typeof v === "number" ? v * 1000 : Date.parse(v);
}

/** `xxxx-xxxx-xxxx` from the first 12 hex of a peer id, the daemon's display label. Not proof of identity. */
export function shortLabel(id: string): string {
  return `${id.slice(0, 4)}-${id.slice(4, 8)}-${id.slice(8, 12)}`;
}

/** Address of the pairing window a discovered device advertises: its host with `pair_port`. */
export function pairingAddr(d: Pick<Discovered, "addr" | "pair_port">): string {
  return d.pair_port ? `${d.addr.replace(/:\d+$/, "")}:${d.pair_port}` : d.addr;
}

/** Error shape every daemon endpoint returns: {"error":{message,type,code,param,hint?}}. */
export class ApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public hint: string | null = null,
    public status = 0,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

/** Code used only by the desktop app when fetch itself fails (daemon not running). */
export const UNREACHABLE = "unreachable";

let baseUrl = "http://127.0.0.1:8192";
let token: string | null = null;

export function configure(opts: { baseUrl?: string; token?: string | null }): void {
  if (opts.baseUrl) baseUrl = opts.baseUrl.replace(/\/$/, "");
  if (opts.token !== undefined) token = opts.token;
}
export function getBaseUrl(): string {
  return baseUrl;
}

function headers(json: boolean): HeadersInit {
  const h: Record<string, string> = {};
  if (json) h["Content-Type"] = "application/json";
  if (token) h["Authorization"] = `Bearer ${token}`;
  return h;
}

async function toApiError(res: Response): Promise<ApiError> {
  let body: unknown = null;
  try {
    body = await res.json();
  } catch {
    /* non-JSON body */
  }
  const e = (body as { error?: { code?: string; message?: string; hint?: string | null } } | null)?.error;
  return new ApiError(e?.code ?? "internal", e?.message ?? `HTTP ${res.status}`, e?.hint ?? null, res.status);
}

async function send(path: string, init: RequestInit = {}): Promise<Response> {
  let res: Response;
  try {
    res = await fetch(baseUrl + path, { ...init, headers: { ...headers(init.body != null), ...(init.headers ?? {}) } });
  } catch (err) {
    if ((err as Error)?.name === "AbortError") throw err;
    throw new ApiError(UNREACHABLE, `Could not reach the daemon at ${baseUrl}`);
  }
  if (!res.ok) throw await toApiError(res);
  return res;
}

async function json<T>(path: string, init?: RequestInit): Promise<T> {
  return (await send(path, init)).json() as Promise<T>;
}

/** Parses an SSE body, yielding the payload of each `data:` line. Stops on `[DONE]`. */
export async function* sseData(res: Response): AsyncGenerator<string> {
  if (!res.body) return;
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    let nl: number;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl).replace(/\r$/, "");
      buf = buf.slice(nl + 1);
      if (!line.startsWith("data:")) continue;
      const data = line.slice(5).trim();
      if (data === "[DONE]") return;
      if (data) yield data;
    }
  }
}

/** Minimal Prometheus text parser: `name{labels} value` -> map keyed by `name{labels}`. */
export function parseMetrics(text: string): Record<string, number> {
  const out: Record<string, number> = {};
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const sp = line.lastIndexOf(" ");
    if (sp < 0) continue;
    const v = Number(line.slice(sp + 1));
    if (Number.isFinite(v)) out[line.slice(0, sp)] = v;
  }
  return out;
}

export const api = {
  health: () => json<{ status: string; version: string }>("/health"),
  status: () => json<Status>("/api/status"),
  hardware: () => json<Hardware>("/api/hardware"),
  models: async () => (await json<{ models: Model[] }>("/api/models")).models,
  load: (id: string, opts: { device?: Device; n_ctx?: number } = {}) =>
    json<Loaded>(`/api/models/${encodeURIComponent(id)}/load`, { method: "POST", body: JSON.stringify(opts) }),
  unload: async () => {
    await send("/api/unload", { method: "POST" });
  },
  remove: async (id: string) => {
    await send(`/api/models/${encodeURIComponent(id)}`, { method: "DELETE" });
  },
  importModel: (path: string) =>
    json<Record<string, unknown>>("/api/models/import", { method: "POST", body: JSON.stringify({ path }) }),
  metrics: async () => parseMetrics(await (await send("/metrics")).text()),

  /** Downloads a model; `onEvent` receives every progress event. Resolves after the `done` event. */
  async pull(id: string, onEvent: (e: PullEvent) => void, signal?: AbortSignal): Promise<void> {
    const res = await send(`/api/models/${encodeURIComponent(id)}/pull`, { method: "POST", signal });
    for await (const data of sseData(res)) {
      const ev = JSON.parse(data) as PullEvent;
      onEvent(ev);
      if (ev.done) return;
    }
  },

  /** Streams a chat completion. `onDelta` receives each new text fragment. Abort via `signal` to stop. */
  async chat(
    req: { model: string; messages: ChatMessage[]; max_tokens?: number; temperature?: number; target?: Target },
    onDelta: (text: string) => void,
    signal?: AbortSignal,
  ): Promise<ChatResult> {
    const { target = "local", ...rest } = req;
    const result: ChatResult = { content: "", finish_reason: null, usage: null, timings: null, stopped: false, target };
    let res: Response;
    try {
      res = await send("/v1/chat/completions", {
        method: "POST",
        // stream_options.include_usage makes the daemon append a final chunk with usage + timings (OpenAI semantics).
        // `target` is only sent for a peer so local requests keep the M1 wire shape.
        body: JSON.stringify({ ...rest, ...(target !== "local" ? { target } : {}), stream: true, stream_options: { include_usage: true } }),
        signal,
      });
    } catch (err) {
      if ((err as Error)?.name === "AbortError") return { ...result, stopped: true };
      throw err;
    }
    try {
      for await (const data of sseData(res)) {
        const chunk = JSON.parse(data) as {
          choices?: { delta?: { content?: string }; finish_reason?: string | null }[];
          usage?: Usage;
          timings?: Timings;
          target?: Target;
        };
        if (chunk.target) result.target = chunk.target;
        const choice = chunk.choices?.[0];
        const delta = choice?.delta?.content;
        if (delta) {
          result.content += delta;
          onDelta(delta);
        }
        if (choice?.finish_reason) result.finish_reason = choice.finish_reason;
        if (chunk.usage) result.usage = chunk.usage;
        if (chunk.timings) result.timings = chunk.timings;
      }
    } catch (err) {
      if ((err as Error)?.name === "AbortError" || signal?.aborted) return { ...result, stopped: true };
      throw err;
    }
    return result;
  },

  // Peer networking (ADR-0003). Every call below is refused by the daemon while networking is disabled,
  // except reading the state itself.
  network: () => json<NetworkState>("/api/network"),
  setNetwork: (body: { enabled: boolean; serve_peers: boolean }) =>
    json<NetworkState>("/api/network", { method: "PUT", body: JSON.stringify(body) }),
  peers: () => json<{ peers: Peer[]; discovered: Discovered[] }>("/api/peers"),
  removePeer: async (id: string) => {
    await send(`/api/peers/${encodeURIComponent(id)}`, { method: "DELETE" });
  },
  revokePeer: async (id: string) => {
    await send(`/api/peers/${encodeURIComponent(id)}/revoke`, { method: "POST" });
  },
  pairStart: (name: string) => json<PairingSession>("/api/pair/start", { method: "POST", body: JSON.stringify({ name }) }),
  pairStatus: () => json<PairingStatus>("/api/pair/status"),
  /** `code` is the six digits (then `addr` is required) or a whole `common-pair:v1:...` payload. */
  pairJoin: (body: { addr?: string; code: string; name: string }) =>
    json<PeerRecord>("/api/pair/join", { method: "POST", body: JSON.stringify(body) }),
  groups: async () => (await json<{ groups: Group[] }>("/api/groups")).groups,
  /** Inserts or replaces one group; the daemon answers with the full list. */
  putGroup: async (group: Group) => (await json<{ groups: Group[] }>("/api/groups", { method: "PUT", body: JSON.stringify(group) })).groups,
  privacy: () => json<Privacy>("/api/privacy"),
  setPrivacy: (body: { mode: PrivacyMode; acknowledged: boolean }) =>
    json<Privacy>("/api/privacy", { method: "PUT", body: JSON.stringify(body) }),
  /** The last 100 events, oldest first. */
  audit: async () => (await json<{ events: AuditEvent[] }>("/api/audit")).events,
  /** Redacted diagnostics bundle (F-06-10). Throws `not_implemented` on daemons without it. */
  diagnostics: () => json<Record<string, unknown>>("/api/diagnostics"),
};
