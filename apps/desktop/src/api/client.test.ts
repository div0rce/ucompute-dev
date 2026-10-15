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

describe("api.pull", () => {
  it("reports progress until done", async () => {
    installFakeDaemon();
    const seen: number[] = [];
    await api.pull("stories15M-q4_0", (e) => seen.push(e.downloaded_bytes));
    expect(seen.length).toBe(4);
    expect(seen[3]).toBe(19077344);
  });
});
