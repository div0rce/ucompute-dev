// Fake daemon for tests: an in-memory catalog plus a fetch() that serves the API contract.
import { vi } from "vitest";
import { shortLabel, type AuditEvent, type Discovered, type Group, type Hardware, type Loaded, type Model, type NetworkState, type PairingSession, type PairingStatus, type Peer, type PeerRecord, type Privacy, type Status } from "../api/client";

export const HARDWARE: Hardware = {
  host: { os: "macOS", os_version: "26.2", arch: "arm64", chip: "Apple M2", memory_bytes: 8589934592 },
  cpu: { model: "Apple M2", physical_cores: 8, logical_cores: 8, arch: "arm64", features: ["neon", "fp16", "dotprod"] },
  memory: { total_bytes: 8589934592, available_bytes: 4294967296, unified: true },
  gpus: [{ name: "Apple M2", vendor: "Apple", backend: "Metal", memory_bytes: 8589934592, cores: 10 }],
  backends: [
    { name: "Metal", available: true, reason: null },
    { name: "CUDA", available: false, reason: "No NVIDIA GPU in this computer." },
    { name: "CPU", available: true, reason: null },
  ],
  disk_free_bytes: 17179869184,
  probe_ms: 12,
};

export const MODELS: Model[] = [
  {
    id: "qwen2.5-1.5b-instruct-q4_k_m",
    name: "Qwen2.5 1.5B Instruct (Q4_K_M)",
    bytes: 1117320000,
    license: "Apache-2.0",
    license_url: "https://example.invalid/license",
    source: "Qwen/Qwen2.5-1.5B-Instruct-GGUF",
    family: "qwen2",
    parameters: "1.5B",
    quantization: "Q4_K_M",
    chat: true,
    redistribution: "allowed",
    tags: ["recommended"],
    downloaded: false,
    loaded: false,
    fit: { verdict: "fits", headroom_bytes: 2800000000 },
    estimate: { weights_bytes: 1117320000, kv_cache_bytes: 268435456, compute_buffer_bytes: 100000000, overhead_bytes: 50000000, total_bytes: 1535755456, n_ctx: 4096 },
  },
  {
    id: "stories15M-q4_0",
    name: "TinyStories 15M (Q4_0)",
    bytes: 19077344,
    license: null,
    license_url: null,
    source: "ggml-org/models-moved",
    family: "tinyllamas",
    parameters: "15M",
    quantization: "Q4_0",
    chat: false,
    redistribution: "unknown",
    tags: ["fixture"],
    downloaded: true,
    loaded: false,
    fit: { verdict: "fits", headroom_bytes: 4200000000 },
    estimate: { weights_bytes: 19077344, kv_cache_bytes: 1048576, compute_buffer_bytes: 4000000, overhead_bytes: 2000000, total_bytes: 26125920, n_ctx: 128 },
  },
  {
    id: "llama-3.1-70b-q4_k_m",
    name: "Llama 3.1 70B (Q4_K_M)",
    bytes: 42520000000,
    license: "Llama 3.1 Community License",
    license_url: "https://example.invalid/llama",
    source: "example",
    family: "llama",
    parameters: "70B",
    quantization: "Q4_K_M",
    chat: true,
    redistribution: "restricted",
    tags: [],
    downloaded: false,
    loaded: false,
    fit: { verdict: "too_large", needed_bytes: 45000000000, available_bytes: 4294967296, reason: "weights alone exceed available memory" },
    estimate: { weights_bytes: 42520000000, kv_cache_bytes: 1342177280, compute_buffer_bytes: 800000000, overhead_bytes: 300000000, total_bytes: 44962177280, n_ctx: 4096 },
  },
];

export const LOADED: Loaded = {
  model_id: "qwen2.5-1.5b-instruct-q4_k_m",
  model_sha256: "abc",
  backend_name: "llama.cpp",
  backend_version: "b11609",
  device_requested: "auto",
  device_actual: "Metal",
  offloaded_layers: 29,
  total_layers: 29,
  n_ctx: 4096,
  threads: 8,
  parallel_slots: 1,
  fallback_reason: null,
  load_ms: 1234,
  pid: 4242,
};

export const STATUS: Status = {
  version: "0.1.0",
  uptime_seconds: 125,
  loaded: null,
  queue: { inflight: 0, queued: 0, max_concurrent: 1, max_queue: 4 },
};

export const METRICS_TEXT = `# HELP common_tokens_generated_total Tokens generated.
# TYPE common_tokens_generated_total counter
common_tokens_generated_total 1234
# TYPE common_ttft_seconds histogram
common_ttft_seconds_sum 2.5
common_ttft_seconds_count 10
`;


// Peer networking fixtures. Shapes mirror crates/common-api/src/mesh.rs: ids are 64 hex, the daemon adds
// `short_label`/`connected`/`groups` to a stored PeerRecord, and discovered devices carry no label.
export const SELF_ID = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";
export const PEER_ID = "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c4b5a69788796a5b4c3d2e1f0";
export const STRANGER_ID = "ffeeddccbbaa99887766554433221100ffeeddccbbaa99887766554433221100";

export const NETWORK: NetworkState = { enabled: false, serve_peers: false, listen_addr: null, mdns: false, peer_id: SELF_ID, short_label: shortLabel(SELF_ID), connections: 0 };
/** What the daemon reports while the endpoint is bound. */
export const NETWORK_ON: NetworkState = { ...NETWORK, enabled: true, listen_addr: "0.0.0.0:8193", mdns: true };

export const PEER_RECORD: PeerRecord = { id: PEER_ID, name: "Studio Mac", paired_at_utc: "2026-10-14T11:59:30Z", last_seen_utc: "2026-10-14T12:00:00Z", last_addr: "192.168.1.77:8193" };
export const PEER: Peer = { ...PEER_RECORD, short_label: shortLabel(PEER_ID), groups: ["home"], connected: true };

export const STRANGER: Discovered = { id: STRANGER_ID, addr: "192.168.1.77:8193", pairing: true, pair_port: 51234, proto: "common/1", trusted: false, seen_utc: "2026-10-14T12:00:00Z" };
/** The QR text the fake's pairing window shows and `/api/pair/join` accepts in place of a code. */
export const QR_PAYLOAD = `common-pair:v1:${SELF_ID}:192.168.1.10:51234:00112233445566778899aabbccddeeff`;

export const PRIVACY: Privacy = {
  mode: "local_only",
  disclosure:
    "In trusted-devices mode a prompt you send to a paired device, and the reply, travel encrypted to that device, where it can read them. Nothing goes to any other device or service.",
};

/** Oldest first, as `GET /api/audit` returns them. */
export const AUDIT: AuditEvent[] = [
  { ts_utc: "2026-10-14T11:59:00Z", kind: "pairing_started", fields: { port: 51234, window_secs: 120 } },
  { ts_utc: "2026-10-14T12:00:00Z", kind: "pairing_succeeded", peer_id: PEER_ID, fields: { port: 51234 } },
];

/** The code the fake accepts on /api/pair/join. */
export const JOIN_CODE = "123456";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

export const apiError = (code: string, message: string, status: number, hint?: string) =>
  json({ error: { message, type: "server_error", code, param: null, hint: hint ?? null } }, status);

/** An SSE response. `delayMs` spaces events out; aborting `signal` errors the stream like a real fetch. */
export function sse(events: unknown[], { delayMs = 0, signal, done = true }: { delayMs?: number; signal?: AbortSignal; done?: boolean } = {}): Response {
  const enc = new TextEncoder();
  let i = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let ctrl: ReadableStreamDefaultController<Uint8Array> | undefined;
  const stream = new ReadableStream<Uint8Array>({
    start(c) {
      ctrl = c;
    },
    pull(c) {
      return new Promise<void>((resolve) => {
        timer = setTimeout(() => {
          try {
            if (i < events.length) c.enqueue(enc.encode(`data: ${JSON.stringify(events[i++])}\n\n`));
            else if (done) {
              c.enqueue(enc.encode("data: [DONE]\n\n"));
              c.close();
            } else c.close();
          } catch {
            /* stream already errored by abort */
          }
          resolve();
        }, delayMs);
      });
    },
    cancel() {
      clearTimeout(timer);
    },
  });
  signal?.addEventListener("abort", () => {
    clearTimeout(timer);
    try {
      ctrl?.error(new DOMException("The operation was aborted.", "AbortError"));
    } catch {
      /* already closed */
    }
  });
  return new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } });
}

export interface FakeDaemon {
  models: Model[];
  loaded: Loaded | null;
  /** Options for the next chat stream. */
  chat: { chunks: string[]; delayMs: number };
  network: NetworkState;
  peers: Peer[];
  discovered: Discovered[];
  groups: Group[];
  privacy: Privacy;
  audit: AuditEvent[];
  /** Pairing session opened by /api/pair/start; `result` is what /api/pair/status reports once set. */
  pairing: { session: PairingSession | null; result: PairingStatus["result"] };
  /** When true every join answers pairing_locked. */
  pairLocked: boolean;
  /** Redacted bundle served by /api/diagnostics; null means the endpoint does not exist (404 like the daemon). */
  diagnostics: Record<string, unknown> | null;
  /**
   * Simulates the other device finishing the pairing: pins `peer` and makes /api/pair/status report it. Like the
   * daemon, a just-paired device has no open connection (`connected: false`) until a request targets it.
   */
  completePairing: (peer?: Peer) => void;
  /** Simulates the other device failing the window (wrong code); the window closes with a typed error. */
  failPairing: (code?: string, message?: string) => void;
  fetchMock: ReturnType<typeof vi.fn>;
}

/** Installs a fetch mock serving the daemon contract from mutable in-memory state. */
export function installFakeDaemon(
  overrides: Partial<Pick<FakeDaemon, "models" | "loaded" | "chat" | "network" | "peers" | "discovered" | "groups" | "privacy" | "audit" | "diagnostics">> = {},
): FakeDaemon {
  const fake: FakeDaemon = {
    models: structuredClone(MODELS),
    loaded: null,
    chat: { chunks: ["Hello", ", ", "world", "."], delayMs: 0 },
    network: { ...NETWORK },
    peers: [],
    discovered: [],
    groups: [],
    privacy: { ...PRIVACY },
    audit: structuredClone(AUDIT),
    pairing: { session: null, result: null },
    pairLocked: false,
    diagnostics: null,
    completePairing: (peer = PEER) => {
      fake.peers.push({ ...peer, groups: [], connected: false });
      const { short_label: _l, groups: _g, connected: _c, health: _h, ...record } = peer;
      fake.pairing.result = { peer: record };
    },
    failPairing: (code = "pairing_failed", message = "confirmation MAC mismatch") => {
      fake.pairing.result = { error: { code, message, hint: null } };
    },
    ...overrides,
    fetchMock: vi.fn(),
  };
  const withGroups = (p: Peer): Peer => ({ ...p, groups: fake.groups.filter((g) => g.members.includes(p.id)).map((g) => g.name) });
  const disabled = () => apiError("policy_denied", "network disabled", 403, "enable networking in Peers");
  fake.fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
    const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
    const path = url.pathname;
    const method = (init?.method ?? "GET").toUpperCase();
    const withLoaded = (m: Model) => ({ ...m, loaded: fake.loaded?.model_id === m.id });
    let m: RegExpMatchArray | null;

    if (path === "/health") return json({ status: "ok", version: "0.1.0" });
    if (path === "/api/status") return json({ ...STATUS, loaded: fake.loaded });
    if (path === "/api/hardware") return json(HARDWARE);
    if (path === "/api/models" && method === "GET") return json({ models: fake.models.map(withLoaded) });
    if (path === "/metrics") return new Response(METRICS_TEXT, { headers: { "content-type": "text/plain" } });
    if (path === "/api/unload" && method === "POST") {
      fake.loaded = null;
      return new Response(null, { status: 204 });
    }
    if ((m = path.match(/^\/api\/models\/([^/]+)\/pull$/)) && method === "POST") {
      const model = fake.models.find((x) => x.id === decodeURIComponent(m![1]));
      if (!model) return apiError("model_not_found", "no such model", 404);
      const total = model.bytes;
      const events = [0.25, 0.5, 0.75, 1].map((f) => ({ downloaded_bytes: Math.round(total * f), total_bytes: total, network_bytes: Math.round(total * f), resumed_from: 0, done: f === 1 }));
      setTimeout(() => (model.downloaded = true), 0);
      return sse(events, { delayMs: 2, done: false });
    }
    if ((m = path.match(/^\/api\/models\/([^/]+)\/load$/)) && method === "POST") {
      const model = fake.models.find((x) => x.id === decodeURIComponent(m![1]));
      if (!model) return apiError("model_not_found", "no such model", 404);
      if (!model.downloaded) return apiError("model_not_downloaded", "weights missing", 404, "common pull " + model.id);
      const body = JSON.parse(String(init?.body ?? "{}")) as { device?: string; n_ctx?: number };
      fake.loaded = { ...LOADED, model_id: model.id, device_requested: body.device ?? "auto", n_ctx: body.n_ctx ?? LOADED.n_ctx };
      return json(fake.loaded);
    }
    if ((m = path.match(/^\/api\/models\/([^/]+)$/)) && method === "DELETE") {
      const model = fake.models.find((x) => x.id === decodeURIComponent(m![1]));
      if (model) model.downloaded = false;
      if (fake.loaded?.model_id === model?.id) fake.loaded = null;
      return new Response(null, { status: 204 });
    }

    // Peer networking (ADR-0003 / F-10-05 / F-10-10).
    if (path === "/api/network" && method === "GET") return json(fake.network);
    if (path === "/api/network" && method === "PUT") {
      const body = JSON.parse(String(init?.body ?? "{}")) as { enabled?: boolean; serve_peers?: boolean };
      const enabled = body.enabled ?? fake.network.enabled;
      fake.network = { ...(enabled ? NETWORK_ON : NETWORK), peer_id: fake.network.peer_id, short_label: fake.network.short_label, serve_peers: body.serve_peers ?? fake.network.serve_peers };
      return json(fake.network);
    }
    if (path === "/api/peers" && method === "GET") return json({ peers: fake.peers.map(withGroups), discovered: fake.network.enabled ? fake.discovered : [], revocations: [] });
    if ((m = path.match(/^\/api\/peers\/([^/]+)\/revoke$/)) && method === "POST") {
      const id = decodeURIComponent(m[1]);
      if (!fake.peers.some((p) => p.id === id)) return apiError("peer_unauthorized", "not a paired device", 404);
      fake.peers = fake.peers.filter((p) => p.id !== id);
      fake.groups = fake.groups.map((g) => ({ ...g, members: g.members.filter((x) => x !== id) }));
      fake.audit.unshift({ ts_utc: new Date().toISOString(), kind: "peer_revoked", peer_id: id, fields: {} });
      return new Response(null, { status: 204 });
    }
    if ((m = path.match(/^\/api\/peers\/([^/]+)$/)) && method === "DELETE") {
      const id = decodeURIComponent(m[1]);
      fake.peers = fake.peers.filter((p) => p.id !== id);
      fake.groups = fake.groups.map((g) => ({ ...g, members: g.members.filter((x) => x !== id) }));
      return new Response(null, { status: 204 });
    }
    if (path === "/api/pair/start" && method === "POST") {
      if (!fake.network.enabled) return disabled();
      if (fake.pairing.session && !fake.pairing.result) return apiError("overloaded", "a pairing session is already open; wait for it to finish or expire", 409);
      fake.pairing = { session: { code: "482913", port: 51234, qr_payload: QR_PAYLOAD, expires_at: Math.floor(Date.now() / 1000) + 120 }, result: null };
      return json(fake.pairing.session);
    }
    if (path === "/api/pair/status") {
      // Like the daemon, the outcome stays readable until the next /api/pair/start.
      const { session, result } = fake.pairing;
      if (!session) return json({ active: false, result: null });
      if (result) return json({ active: false, result });
      return json({ active: true, result: null, port: session.port, expires_at: session.expires_at });
    }
    if (path === "/api/pair/join" && method === "POST") {
      if (!fake.network.enabled) return disabled();
      if (fake.pairLocked) return apiError("pairing_locked", "too many failed confirmations in the last 60 s", 429);
      const body = JSON.parse(String(init?.body ?? "{}")) as { addr?: string; code: string; name: string };
      if (!body.code.startsWith("common-pair:") && !body.addr) return apiError("invalid_request", "addr (ip:port of the other device's pairing endpoint) is required with a code", 400);
      if (body.code !== JOIN_CODE && body.code !== QR_PAYLOAD) return apiError("pairing_failed", "confirmation MAC mismatch", 403);
      fake.peers.push({ ...PEER, groups: [], connected: false });
      fake.discovered = fake.discovered.filter((x) => x.id !== PEER.id);
      return json(PEER_RECORD);
    }
    if (path === "/api/groups" && method === "GET") return json({ groups: fake.groups });
    if (path === "/api/groups" && method === "PUT") {
      // Upsert of one group by name, as the daemon does.
      const g = JSON.parse(String(init?.body ?? "{}")) as Group;
      fake.groups = fake.groups.some((x) => x.name === g.name) ? fake.groups.map((x) => (x.name === g.name ? g : x)) : [...fake.groups, g];
      return json({ groups: fake.groups });
    }
    if (path === "/api/privacy" && method === "GET") return json(fake.privacy);
    if (path === "/api/privacy" && method === "PUT") {
      const body = JSON.parse(String(init?.body ?? "{}")) as { mode: Privacy["mode"]; acknowledged: boolean };
      if (body.mode !== "local_only" && !body.acknowledged) return apiError("policy_denied", "disclosure not acknowledged", 403);
      if (body.mode !== "local_only" && body.mode !== "trusted_group") return apiError("invalid_request", "unknown privacy mode", 400);
      fake.privacy = { ...fake.privacy, mode: body.mode };
      return json(fake.privacy);
    }
    if (path === "/api/audit") return json({ events: fake.audit });
    if (path === "/api/diagnostics" && fake.diagnostics) return json(fake.diagnostics);
    if (path === "/v1/chat/completions" && method === "POST") {
      if (!fake.loaded) return apiError("model_not_downloaded", "no model loaded", 404);
      const body = JSON.parse(String(init?.body ?? "{}")) as { target?: string };
      const target = body.target ?? "local";
      if (target !== "local") {
        if (fake.privacy.mode === "local_only") return apiError("policy_denied", "privacy mode is local_only", 403);
        const peer = fake.peers.find((p) => `peer:${p.id}` === target);
        if (!peer) return apiError("peer_unauthorized", "not a paired device", 403);
        // The daemon connects on demand (mesh.rs `peer_connection`); the connection then stays open.
        peer.connected = true;
      }
      const n = fake.chat.chunks.length;
      const chunks: unknown[] = fake.chat.chunks.map((c, i) => ({
        id: "chatcmpl-1",
        object: "chat.completion.chunk",
        choices: [{ index: 0, delta: i === 0 ? { role: "assistant", content: c } : { content: c }, finish_reason: null }],
      }));
      chunks.push({
        id: "chatcmpl-1",
        object: "chat.completion.chunk",
        choices: [{ index: 0, delta: {}, finish_reason: "stop" }],
        usage: { prompt_tokens: 7, completion_tokens: n, total_tokens: 7 + n },
        timings: { time_to_first_token_ms: 120, prompt_tokens_per_second: 300, decode_tokens_per_second: 42.5 },
        target,
      });
      return sse(chunks, { delayMs: fake.chat.delayMs, signal: init?.signal ?? undefined });
    }
    // The daemon's fallback for an unknown route (common-api lib.rs `not_found`).
    return apiError("invalid_request", "no such route", 404);
  });
  vi.stubGlobal("fetch", fake.fetchMock);
  return fake;
}
