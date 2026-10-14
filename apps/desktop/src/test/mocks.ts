// Fake daemon for tests: an in-memory catalog plus a fetch() that serves the API contract.
import { vi } from "vitest";
import type { Hardware, Loaded, Model, Status } from "../api/client";

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
  fetchMock: ReturnType<typeof vi.fn>;
}

/** Installs a fetch mock serving the daemon contract from mutable in-memory state. */
export function installFakeDaemon(overrides: Partial<Pick<FakeDaemon, "models" | "loaded" | "chat">> = {}): FakeDaemon {
  const fake: FakeDaemon = {
    models: structuredClone(MODELS),
    loaded: null,
    chat: { chunks: ["Hello", ", ", "world", "."], delayMs: 0 },
    ...overrides,
    fetchMock: vi.fn(),
  };
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
    if (path === "/v1/chat/completions" && method === "POST") {
      if (!fake.loaded) return apiError("model_not_downloaded", "no model loaded", 404);
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
      });
      return sse(chunks, { delayMs: fake.chat.delayMs, signal: init?.signal ?? undefined });
    }
    return apiError("not_implemented", `no route for ${method} ${path}`, 501);
  });
  vi.stubGlobal("fetch", fake.fetchMock);
  return fake;
}
