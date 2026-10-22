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
  /** Why the planner capped or reduced the context or KV cache; null when it did not. */
  context_note: string | null;
  load_ms: number;
  pid: number | null;
  /** E2: streams using the engine now. Absent on older services. */
  in_flight?: number;
  /** E2: who caused this load: this computer ("local") or a paired device's request ("peer:<hex>"). */
  loaded_for?: Target;
  /** E2: when the load finished. */
  loaded_at_utc?: string;
}

/** E2: the engine slot read without waiting on it, so a load in progress is visible. */
export interface EngineState {
  state: "idle" | "loading" | "ready";
  loading_model_id: string | null;
  loading_for_ms: number | null;
}

export interface Status {
  version: string;
  uptime_seconds: number;
  loaded: Loaded | null;
  queue: { inflight: number; queued: number; max_concurrent: number; max_queue: number };
  /** E2; absent on older services (then `/ready` is the only load signal). */
  engine?: EngineState;
}

export interface Hardware {
  host: { os: string; os_version: string; arch: string; chip: string; memory_bytes: number };
  cpu: { model: string; physical_cores: number; logical_cores: number; arch: string; features: string[] };
  memory: { total_bytes: number; available_bytes: number; unified: boolean };
  /** `memory_bytes` and `cores` are `Option` on the wire. */
  gpus: { name: string; vendor: string; backend: string; memory_bytes?: number | null; cores?: number | null }[];
  backends: { name: string; available: boolean; reason?: string | null }[];
  disk_free_bytes?: number | null;
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
  /** What the memory estimate is based on: the GGUF header, or only the file size (not downloaded). */
  estimate_basis: "gguf" | "file_size";
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

/** Every field is nullable on the wire; null renders as "—", never as 0. */
export interface Timings {
  time_to_first_token_ms: number | null;
  prompt_ms?: number | null;
  prompt_tokens_per_second: number | null;
  decode_ms?: number | null;
  decode_tokens_per_second: number | null;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface ChatResult {
  /** The response id from the first chunk (`chatcmpl-…`); null if no chunk arrived. Matches `Job.id` (E3). */
  id: string | null;
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
  /** This computer's name (hostname); the name other devices see unless the user set one. Older daemons omit it. */
  device_name?: string;
  /** `ip:port` other devices should dial to reach this one by address; null while networking is off. */
  dial_addr?: string | null;
  /** What paired devices may use here (service configuration; read-only, applies after restart). */
  peer_quota: { requests_per_minute: number; tokens_per_minute: number; max_concurrent: number };
  /** Engine slots paired devices may use at once. */
  peer_slots: number;
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

/** The most recent failed dial to a paired device since its last successful connection. */
export interface DialError {
  addr: string;
  code: string;
  message: string;
  hint?: string | null;
  at_utc: string;
}

export type PeerHealth = "connected" | "unhealthy" | "reconnecting" | "disconnected";

/** A pinned device as `GET /api/peers` lists it: the record plus live state. */
export interface Peer extends PeerRecord {
  short_label: string;
  groups: string[];
  connected: boolean;
  /** Heartbeat health; not emitted by every daemon version, so readers fall back to `connected`. */
  health?: PeerHealth;
  /** Why the last dial failed; null or absent while connected or before any failure. */
  last_error?: DialError | null;
  /** E1: what the device reported (signed) when this connection opened; null while not connected; absent on older services. */
  capabilities?: PeerCapabilities | null;
  /** E1: the open connection; null while not connected; absent on older services. */
  connection?: PeerConnectionInfo | null;
  /** E5: disconnected by the user here (until Connect or a restart); absent on services without Disconnect. */
  held?: boolean;
}

/**
 * E1. `hardware` and `free_memory_bytes` were measured when the other device's networking started (time not
 * reported); `issued_at_utc` is when the record was signed, at this connection's handshake. Not independently verified.
 */
export interface PeerCapabilities {
  common_version: string;
  protocol_minor: number;
  engines: string[];
  issued_at_utc: string;
  free_memory_bytes: number;
  /** null when the device sent a hardware report this version cannot read. */
  hardware: Hardware | null;
  /** Model files it holds; `id` is null when the hash is not in this computer's catalog. */
  models: { sha256: string; id: string | null }[];
}

/** E1. `id` is stable for one QUIC connection: byte-rate deltas are only taken within the same id. */
export interface PeerConnectionInfo {
  id: number;
  opened_at_utc: string;
  remote_addr: string;
  direction: "outbound" | "inbound";
  rtt_ms: number;
  tx_bytes: number;
  rx_bytes: number;
  lost_packets: number;
}

/** A signed revocation (`GET /api/peers` `revocations[]`). A revoked device is unpinned and listed only here. */
export interface Revocation {
  seq: number;
  peer_id: string;
  revoked_by: string;
  at_utc: string;
  signature: string;
}

/** `GET /api/peers`. */
export interface Mesh {
  peers: Peer[];
  discovered: Discovered[];
  revocations: Revocation[];
}

export type JobState = "queued" | "loading" | "running" | "done" | "error" | "cancelled";
export type JobKind = "chat" | "completion" | "embeddings" | "served_for_peer";

/** E3 `GET /api/jobs` row. Ids, counts and durations only, never prompt or reply text. */
export interface Job {
  /** Response id (`chatcmpl-…`) once known; a registry id before; the peer's `request_id` for served rows. */
  id: string;
  /** From `X-Common-Client-Ref`; matches `ownStreams` and chat history for requests this app sent. */
  client_ref?: string | null;
  kind: JobKind;
  model_id: string;
  target: Target;
  origin: Target;
  state: JobState;
  created_at_utc: string;
  started_at_utc?: string | null;
  first_token_at_utc?: string | null;
  finished_at_utc?: string | null;
  queue_ms?: number | null;
  load_ms?: number | null;
  ttft_ms?: number | null;
  prompt_ms?: number | null;
  decode_ms?: number | null;
  duration_ms?: number | null;
  prompt_tokens?: number | null;
  /** Live on active rows (counted per chunk), final on finished rows. */
  completion_tokens?: number | null;
  decode_tokens_per_second?: number | null;
  finish_reason?: string | null;
  error_code?: string | null;
  /** Remote runs; joins the audit `remote_request` entries. */
  request_id?: string | null;
  /** Local runs, from the loaded engine at admit time. */
  device_actual?: string | null;
}

export interface JobsData {
  active: Job[];
  /** In memory only, newest first, at most 256; cleared when the service restarts. */
  recent: Job[];
}

/** `GET /ready`: 503 bodies are answers too. */
export interface Ready {
  status: "ready" | "not_ready";
  model?: string | null;
  reason?: string;
}

/** Seen through mDNS but not pinned. Never actionable except pairing. */
export interface Discovered {
  id: string;
  /** Workload endpoint `ip:port`, the best of `addrs`. */
  addr: string;
  /** Every advertised address the daemon will dial, best first (`addrs[0] === addr`). Older daemons omit it. */
  addrs?: string[];
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
  /** `ip:port` the other device enters under "Join a device" (LAN address when bound on every interface); older daemons omit it. */
  pair_addr?: string;
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
/** Code used only by the desktop app when a request with a timeout got no answer in time (daemon busy). */
export const SLOW = "slow";

/** Per-call options for reads that pollers make. Actions (load, connect, pull, chat) never time out here. */
export interface ReqOpts {
  /** Abort and throw `ApiError(SLOW)` when no answer arrived within this many ms. */
  timeoutMs?: number;
}

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

const isTimeout = (err: unknown) => (err as Error)?.name === "TimeoutError";
const slow = (ms?: number) => new ApiError(SLOW, `No answer from the daemon at ${baseUrl} within ${ms ?? "?"} ms`);

/** Fetches without throwing on HTTP status. Network failure -> `unreachable`, timeout -> `slow`, abort passes through. */
async function request(path: string, init: RequestInit = {}, o: ReqOpts = {}): Promise<Response> {
  const signals = [init.signal, o.timeoutMs ? AbortSignal.timeout(o.timeoutMs) : null].filter((x): x is AbortSignal => !!x);
  const signal = signals.length > 1 ? AbortSignal.any(signals) : signals[0];
  try {
    return await fetch(baseUrl + path, { ...init, signal, headers: { ...headers(init.body != null), ...(init.headers ?? {}) } });
  } catch (err) {
    if (isTimeout(err)) throw slow(o.timeoutMs);
    if ((err as Error)?.name === "AbortError") throw err;
    throw new ApiError(UNREACHABLE, `Could not reach the daemon at ${baseUrl}`);
  }
}

async function send(path: string, init: RequestInit = {}, o: ReqOpts = {}): Promise<Response> {
  const res = await request(path, init, o);
  if (!res.ok) throw await toApiError(res);
  return res;
}

/** Reads a body; a timeout that fires while the body streams is `slow` too. */
async function body<T>(res: Response, read: (r: Response) => Promise<T>, o: ReqOpts): Promise<T> {
  try {
    return await read(res);
  } catch (err) {
    if (isTimeout(err)) throw slow(o.timeoutMs);
    throw err;
  }
}

async function json<T>(path: string, init?: RequestInit, o: ReqOpts = {}): Promise<T> {
  return body(await send(path, init, o), (r) => r.json() as Promise<T>, o);
}

const get = <T>(path: string, o?: ReqOpts) => json<T>(path, undefined, o);

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
  health: (o?: ReqOpts) => get<{ status: string; version: string }>("/health", o),
  /** Before E2 the only non-blocking load signal: 503 `reason: "a model is loading"` (or a request briefly holding the slot). */
  ready: async (o: ReqOpts = {}): Promise<Ready> => {
    const res = await request("/ready", {}, o);
    if (res.ok || res.status === 503) return body(res, (r) => r.json() as Promise<Ready>, o);
    throw await toApiError(res);
  },
  status: (o?: ReqOpts) => get<Status>("/api/status", o),
  hardware: (o?: ReqOpts) => get<Hardware>("/api/hardware", o),
  models: async (o?: ReqOpts) => (await get<{ models: Model[] }>("/api/models", o)).models,
  load: (id: string, opts: { device?: Device; n_ctx?: number } = {}) =>
    json<Loaded>(`/api/models/${encodeURIComponent(id)}/load`, { method: "POST", body: JSON.stringify(opts) }),
  unload: async () => {
    await send("/api/unload", { method: "POST" });
  },
  remove: async (id: string) => {
    await send(`/api/models/${encodeURIComponent(id)}`, { method: "DELETE" });
  },
  /** `path` is absolute on this computer; `id` and `license` are optional overrides. */
  importModel: (path: string, extra: { id?: string; license?: string } = {}) =>
    json<Record<string, unknown>>("/api/models/import", { method: "POST", body: JSON.stringify({ path, ...extra }) }),
  metrics: async (o: ReqOpts = {}) => parseMetrics(await body(await send("/metrics", {}, o), (r) => r.text(), o)),
  /** E3 job registry; `"unsupported"` when the service has no such route (404). */
  jobs: async (o?: ReqOpts): Promise<JobsData | "unsupported"> => {
    try {
      return await get<JobsData>("/api/jobs", o);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) return "unsupported";
      throw err;
    }
  },

  /**
   * Downloads a model; `onEvent` receives every progress event. Resolves after the `done` event. A failure after the
   * stream opened arrives as `{"error": {...}}` (admin.rs `pull`) and rejects with its code, as does a stream that
   * ends without `done` (for example at service shutdown).
   */
  async pull(id: string, onEvent: (e: PullEvent) => void, signal?: AbortSignal): Promise<void> {
    const res = await send(`/api/models/${encodeURIComponent(id)}/pull`, { method: "POST", signal });
    for await (const data of sseData(res)) {
      const ev = JSON.parse(data) as PullEvent & { error?: { code?: string; message?: string; hint?: string | null } };
      if (ev.error) throw new ApiError(ev.error.code ?? "download_failed", ev.error.message ?? "the download stream ended with an error", ev.error.hint ?? null);
      onEvent(ev);
      if (ev.done) return;
    }
    throw new ApiError("download_failed", "the download stream ended before the download finished");
  },

  /**
   * Streams a chat completion. `onDelta` receives each new text fragment. Abort via `signal` to stop.
   * `opts.clientRef` is sent as `X-Common-Client-Ref` (E3 matches it to a job row). The caller sends it only to a
   * service that reports `status.engine` (E2 and later allow the header in CORS; an older one would refuse the preflight).
   */
  async chat(
    req: { model: string; messages: ChatMessage[]; max_tokens?: number; temperature?: number; target?: Target },
    onDelta: (text: string) => void,
    signal?: AbortSignal,
    opts: { clientRef?: string } = {},
  ): Promise<ChatResult> {
    const { target = "local", ...rest } = req;
    const result: ChatResult = { id: null, content: "", finish_reason: null, usage: null, timings: null, stopped: false, target };
    let res: Response;
    try {
      res = await send("/v1/chat/completions", {
        method: "POST",
        // stream_options.include_usage makes the daemon append a final chunk with usage + timings (OpenAI semantics).
        // `target` is only sent for a peer so local requests keep the M1 wire shape.
        body: JSON.stringify({ ...rest, ...(target !== "local" ? { target } : {}), stream: true, stream_options: { include_usage: true } }),
        ...(opts.clientRef ? { headers: { "X-Common-Client-Ref": opts.clientRef } } : {}),
        signal,
      });
    } catch (err) {
      if ((err as Error)?.name === "AbortError") return { ...result, stopped: true };
      throw err;
    }
    try {
      for await (const data of sseData(res)) {
        const chunk = JSON.parse(data) as {
          id?: string;
          choices?: { delta?: { content?: string }; finish_reason?: string | null }[];
          usage?: Usage;
          timings?: Timings;
          target?: Target;
          error?: { code?: string; message?: string; hint?: string | null };
        };
        // A failure after the stream opened (the peer lacks the model, the engine stopped) arrives as an
        // OpenAI error object in the stream; it must not read as an empty reply.
        if (chunk.error) throw new ApiError(chunk.error.code ?? "internal", chunk.error.message ?? "the stream ended with an error", chunk.error.hint ?? null);
        if (chunk.id && !result.id) result.id = chunk.id;
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
  network: (o?: ReqOpts) => get<NetworkState>("/api/network", o),
  setNetwork: (body: { enabled: boolean; serve_peers: boolean }) =>
    json<NetworkState>("/api/network", { method: "PUT", body: JSON.stringify(body) }),
  peers: async (o?: ReqOpts): Promise<Mesh> => {
    const m = await get<Partial<Mesh> & Pick<Mesh, "peers" | "discovered">>("/api/peers", o);
    return { peers: m.peers, discovered: m.discovered, revocations: m.revocations ?? [] };
  },
  removePeer: async (id: string) => {
    await send(`/api/peers/${encodeURIComponent(id)}`, { method: "DELETE" });
  },
  revokePeer: async (id: string) => {
    await send(`/api/peers/${encodeURIComponent(id)}/revoke`, { method: "POST" });
  },
  /** Connects to a paired device now: `addr` first when given, then the addresses the daemon knows. The device
   *  must still present the key pinned at pairing; an address is only a hint. Resets the daemon's redial backoff.
   *  Also releases an E5 hold. */
  connectPeer: (id: string, addr?: string) =>
    json<PeerRecord>(`/api/peers/${encodeURIComponent(id)}/connect`, { method: "POST", body: JSON.stringify(addr ? { addr } : {}) }),
  /** E5: closes every connection to the device and holds it (both directions) until Connect or a restart. */
  disconnectPeer: async (id: string) => {
    await send(`/api/peers/${encodeURIComponent(id)}/disconnect`, { method: "POST" });
  },
  /** Without a name the daemon announces this computer's own name. */
  pairStart: (name?: string) => json<PairingSession>("/api/pair/start", { method: "POST", body: JSON.stringify(name ? { name } : {}) }),
  pairStatus: () => json<PairingStatus>("/api/pair/status"),
  /** `code` is the six digits (then `addr` is required) or a whole `common-pair:v1:...` payload. */
  pairJoin: ({ name, ...body }: { addr?: string; code: string; name?: string }) =>
    json<PeerRecord>("/api/pair/join", { method: "POST", body: JSON.stringify(name ? { ...body, name } : body) }),
  groups: async (o?: ReqOpts) => (await get<{ groups: Group[] }>("/api/groups", o)).groups,
  /** Inserts or replaces one group; the daemon answers with the full list. */
  putGroup: async (group: Group) => (await json<{ groups: Group[] }>("/api/groups", { method: "PUT", body: JSON.stringify(group) })).groups,
  privacy: (o?: ReqOpts) => get<Privacy>("/api/privacy", o),
  setPrivacy: (body: { mode: PrivacyMode; acknowledged: boolean }) =>
    json<Privacy>("/api/privacy", { method: "PUT", body: JSON.stringify(body) }),
  /** The last `n` (1..1000) events, oldest first. */
  audit: async (n = 100, o?: ReqOpts) => (await get<{ events: AuditEvent[] }>(`/api/audit?n=${n}`, o)).events,
  /**
   * Redacted diagnostics bundle (F-06-10). No service serves this route yet (N5): it answers 404 and callers fall
   * back to status + hardware.
   */
  diagnostics: () => json<Record<string, unknown>>("/api/diagnostics"),
};
