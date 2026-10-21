import { afterEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, parseMetrics, sseData, UNREACHABLE } from "./client";
import { apiError, installFakeDaemon, PEER, sse } from "../test/mocks";

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
    vi.stubGlobal("fetch", vi.fn(async () => apiError("model_not_found", "no such model", 404, "common models list")));
    const err = await api.status().catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err.code).toBe("model_not_found");
    expect(err.hint).toBe("common models list");
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
    vi.stubGlobal("fetch", vi.fn(async () => sse([{ error: { message: "model x is not downloaded", type: "invalid_request_error", code: "model_not_downloaded", param: null, hint: "common pull x" } }])));
    const err = await api.chat({ model: "x", messages: [], target: `peer:${"ab".repeat(32)}` }, () => undefined).catch((e) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect([err.code, err.message, err.hint]).toEqual(["model_not_downloaded", "model x is not downloaded", "common pull x"]);
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
});
