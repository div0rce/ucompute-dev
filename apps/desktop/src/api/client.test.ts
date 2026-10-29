import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, parseMetrics, SLOW, sseData, UNREACHABLE } from "./client";
import { apiError, hang, installFakeDaemon, JOBS, LOADED, NETWORK_ON, PEER, PEERS_E1, PEERS_E5, sse } from "../test/mocks";

afterEach(() => vi.unstubAllGlobals());

describe("parseMetrics", () => {
  it("reads name/value pairs and skips comments", () => {
    const m = parseMetrics("# HELP x\nfoo 1\nbar{le=\"0.5\"} 2.5\n\nbad line here\n");
    expect(m).toEqual({ foo: 1, 'bar{le="0.5"}': 2.5 });
  });
});

describe("sseData", () => {
  it("yields each data payload and stops at [DONE]", async () => {
    const out: string[] = [];
    for await (const d of sseData(sse([{ a: 1 }, { b: 2 }]))) out.push(d);
    expect(out).toEqual(['{"a":1}', '{"b":2}']);
  });
});

describe("api errors", () => {
  it("turns an error body into ApiError with the stable code", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => apiError("model_not_found", "no such model", 404, "ucompute models list")));
    const err = await api.status().catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.code).toBe("model_not_found");
    expect(err.hint).toBe("ucompute models list");
    expect(err.status).toBe(404);
  });
  it("reports an unreachable daemon when fetch itself fails", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => { throw new TypeError("fetch failed"); }));
    const err = await api.health().catch((e) => e);
    expect(err.code).toBe(UNREACHABLE);
  });
  it("sends a bearer token when configured", async () => {
    const { fetchMock } = installFakeDaemon();
    const { configure } = await import("./client");
    configure({ token: "secret" });
    await api.health();
    configure({ token: null });
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect((init.headers as Record<string, string>).Authorization).toBe("Bearer secret");
  });
});

describe("api.chat", () => {
  it("streams deltas and returns usage and timings", async () => {
    const fake = installFakeDaemon();
    fake.loaded = { model_id: "x" } as never;
    const deltas: string[] = [];
    const res = await api.chat({ model: "x", messages: [{ role: "user", content: "hi" }] }, (d) => deltas.push(d));
    expect(deltas.join("")).toBe("Hello, world.");
    expect(res.content).toBe("Hello, world.");
    expect(res.finish_reason).toBe("stop");
    expect(res.usage?.completion_tokens).toBe(4);
    expect(res.timings?.decode_tokens_per_second).toBe(42.5);
    expect(res.stopped).toBe(false);
  });
  it("returns stopped=true with partial content when aborted", async () => {
    const fake = installFakeDaemon({ chat: { chunks: Array.from({ length: 50 }, (_, i) => `t${i} `), delayMs: 5 } });
    fake.loaded = { model_id: "x" } as never;
    const ac = new AbortController();
    let seen = 0;
    const p = api.chat({ model: "x", messages: [] }, () => { if (++seen === 3) ac.abort(); }, ac.signal);
    const res = await p;
    expect(res.stopped).toBe(true);
    expect(seen).toBeLessThan(50);
    expect(res.content.startsWith("t0 t1 t2")).toBe(true);
  });
});

describe("api.chat target", () => {
  it("omits target for local runs and echoes the daemon's target for a peer", async () => {
    const fake = installFakeDaemon({ privacy: { mode: "trusted_group", disclosure: "" }, peers: [{ ...PEER, id: "ab".repeat(32), short_label: "abab-abab-abab", groups: [] }] });
    fake.loaded = { model_id: "x" } as never;
    const local = await api.chat({ model: "x", messages: [] }, () => undefined);
    expect(local.target).toBe("local");
    expect("target" in JSON.parse(String((fake.fetchMock.mock.calls.at(-1)![1] as RequestInit).body))).toBe(false);
    const remote = await api.chat({ model: "x", messages: [], target: `peer:${"ab".repeat(32)}` }, () => undefined);
    expect(remote.target).toBe(`peer:${"ab".repeat(32)}`);
    expect(JSON.parse(String((fake.fetchMock.mock.calls.at(-1)![1] as RequestInit).body)).target).toBe(`peer:${"ab".repeat(32)}`);
  });
});

describe("api.chat stream errors", () => {
  it("an error object inside the stream rejects with its typed code instead of an empty reply", async () => {
    // What openai.rs `stream_events` sends when the peer refuses after the stream opened.
    vi.stubGlobal("fetch", vi.fn(async () => sse([{ error: { message: "model x is not downloaded", type: "invalid_request_error", code: "model_not_downloaded", param: null, hint: "ucompute pull x" } }])));
    const err = await api.chat({ model: "x", messages: [], target: `peer:${"ab".repeat(32)}` }, () => undefined).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect([err.code, err.message, err.hint]).toEqual(["model_not_downloaded", "model x is not downloaded", "ucompute pull x"]);
  });
});

describe("peer connect and pairing bodies", () => {
  const bodyOf = (fetchMock: { mock: { calls: unknown[][] } }) => JSON.parse(String((fetchMock.mock.calls.at(-1)![1] as RequestInit).body));
  it("connectPeer posts {} or {addr}; pairing sends a name only when one is set", async () => {
    const fetchMock = vi.fn(async (_url: string, _init: RequestInit) => new Response("{}", { headers: { "content-type": "application/json" } }));
    vi.stubGlobal("fetch", fetchMock);
    await api.connectPeer("ab".repeat(32));
    expect(String(fetchMock.mock.calls.at(-1)![0])).toMatch(new RegExp(`/api/peers/${"ab".repeat(32)}/connect$`));
    expect(bodyOf(fetchMock)).toEqual({});
    await api.connectPeer("ab".repeat(32), "10.0.0.9:8193");
    expect(bodyOf(fetchMock)).toEqual({ addr: "10.0.0.9:8193" });
    await api.pairStart();
    expect(bodyOf(fetchMock)).toEqual({});
    await api.pairStart("Kitchen PC");
    expect(bodyOf(fetchMock)).toEqual({ name: "Kitchen PC" });
    await api.pairJoin({ addr: "10.0.0.9:51234", code: "123456" });
    expect(bodyOf(fetchMock)).toEqual({ addr: "10.0.0.9:51234", code: "123456" });
    await api.pairJoin({ code: "123456", name: "Kitchen PC" });
    expect(bodyOf(fetchMock)).toEqual({ code: "123456", name: "Kitchen PC" });
  });
});

describe("api.pull", () => {
  it("reports progress until done", async () => {
    installFakeDaemon();
    const seen: number[] = [];
    await api.pull("stories15M-q4_0", (e) => seen.push(e.downloaded_bytes));
    expect(seen.length).toBe(4);
    expect(seen[3]).toBe(19077344);
  });
  it("an error event inside the stream rejects with its code; a stream that ends without done rejects too", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => sse([{ downloaded_bytes: 1, total_bytes: 9, network_bytes: 1, resumed_from: 0, done: false }, { error: { message: "checksum mismatch", code: "download_failed", hint: null } }])));
    const seen: number[] = [];
    const err = await api.pull("x", (e) => seen.push(e.downloaded_bytes)).catch((e) => e);
    expect([err instanceof ApiError, err.code, err.message]).toEqual([true, "download_failed", "checksum mismatch"]);
    expect(seen).toEqual([1]);
    vi.stubGlobal("fetch", vi.fn(async () => sse([{ downloaded_bytes: 1, total_bytes: 9, network_bytes: 1, resumed_from: 0, done: false }])));
    expect((await api.pull("x", () => undefined).catch((e) => e)).code).toBe("download_failed");
  });
});

describe("timeouts (busy service)", () => {
  it("a read with timeoutMs that gets no answer throws slow, not unreachable", async () => {
    const f = vi.fn((_: unknown, init?: RequestInit) => hang(init));
    vi.stubGlobal("fetch", f);
    const t0 = performance.now();
    const err = await api.status({ timeoutMs: 50 }).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.code).toBe(SLOW);
    expect(performance.now() - t0).toBeLessThan(1000);
    // The request itself was aborted, so a blocked service gets no pile-up of open requests.
    expect((f.mock.calls[0][1] as RequestInit).signal?.aborted).toBe(true);
  });
  it("actions carry no timeout", async () => {
    const { fetchMock } = installFakeDaemon();
    await api.disconnectPeer(PEER.id).catch(() => undefined);
    await api.connectPeer(PEER.id).catch(() => undefined);
    for (const [, init] of fetchMock.mock.calls) expect((init as RequestInit).signal ?? null).toBeNull();
  });
  it("a caller's abort still passes through as AbortError", async () => {
    vi.stubGlobal("fetch", vi.fn((_: unknown, init?: RequestInit) => hang(init)));
    const ac = new AbortController();
    const p = api.pull("x", () => undefined, ac.signal).catch((e) => e);
    ac.abort();
    expect(((await p) as Error).name).toBe("AbortError");
  });
});

describe("new routes and fields (E1–E5)", () => {
  it("jobs: the registry, or unsupported on a service without the route", async () => {
    const fake = installFakeDaemon({ jobs: JOBS });
    expect(await api.jobs()).toEqual(JOBS);
    fake.jobs = "unsupported";
    expect(await api.jobs()).toBe("unsupported");
  });
  it("a jobs error other than 404 still throws", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => apiError("internal", "boom", 500)));
    expect((await api.jobs().catch((e) => e)).code).toBe("internal");
  });
  it("peers carries revocations (empty when an older service omits them) and the E1/E5 fields", async () => {
    installFakeDaemon({ peers: PEERS_E1.peers, revocations: PEERS_E1.revocations, network: NETWORK_ON });
    const m = await api.peers();
    expect(m.revocations).toEqual(PEERS_E1.revocations);
    expect(m.peers[0].capabilities?.hardware?.cpu.logical_cores).toBe(10);
    expect(m.peers[0].connection?.id).toBe(7);
    vi.stubGlobal("fetch", vi.fn(async () => new Response(JSON.stringify({ peers: [], discovered: [] }))));
    expect((await api.peers()).revocations).toEqual([]);
  });
  it("disconnect posts to the E5 route and holds the device; connect releases it", async () => {
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [{ ...PEERS_E5.peers[0], connected: true, health: "connected", held: false }], answersAt: { [PEER.id]: "192.168.1.77:8193" } });
    await api.disconnectPeer(PEER.id);
    expect(fake.fetchMock.mock.calls.at(-1)![0]).toMatch(new RegExp(`/api/peers/${PEER.id}/disconnect$`));
    expect(fake.peers[0]).toMatchObject({ connected: false, held: true });
    await api.connectPeer(PEER.id);
    expect(fake.peers[0]).toMatchObject({ connected: true, held: false });
  });
  it("disconnect on a service without E5 is a 404, never faked with DELETE", async () => {
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER] });
    expect((await api.disconnectPeer(PEER.id).catch((e) => e)).status).toBe(404);
    expect(fake.fetchMock.mock.calls.some(([, init]) => (init as RequestInit)?.method === "DELETE")).toBe(false);
  });
  it("audit passes n", async () => {
    const { fetchMock } = installFakeDaemon();
    await api.audit(1000);
    expect(String(fetchMock.mock.calls[0][0])).toMatch(/\/api\/audit\?n=1000$/);
  });
  it("ready reads a 503 answer instead of throwing", async () => {
    const fake = installFakeDaemon();
    fake.loading = { model_id: "m", since: Date.now() };
    expect(await api.ready()).toEqual({ status: "not_ready", reason: "a model is loading" });
  });
  it("chat sends X-UCompute-Client-Ref only when given and captures the response id", async () => {
    const fake = installFakeDaemon();
    fake.loaded = LOADED;
    const r1 = await api.chat({ model: "m", messages: [{ role: "user", content: "hi" }] }, () => undefined, undefined, { clientRef: "ref-1" });
    const r2 = await api.chat({ model: "m", messages: [{ role: "user", content: "hi" }] }, () => undefined);
    const [h1, h2] = fake.fetchMock.mock.calls.map(([, init]) => (init as RequestInit).headers as Record<string, string>);
    expect(h1["X-UCompute-Client-Ref"]).toBe("ref-1");
    expect(h2["X-UCompute-Client-Ref"]).toBeUndefined();
    expect(r1.id).toMatch(/^chatcmpl-/);
    expect(r2.id).not.toBe(r1.id);
    // The fake registry recorded the ref on the job, as E3 does (recent rows newest first).
    expect(fake.jobs !== "unsupported" && fake.jobs.recent.map((j) => j.client_ref)).toEqual([null, "ref-1"]);
  });
});
