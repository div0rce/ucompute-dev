import { renderHook, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SLOW } from "../api/client";
import { refreshStatus } from "../daemon";
import { hang, installFakeDaemon, JOBS, MODELS, NETWORK_ON, PEER, PEER_E1 } from "../test/mocks";
import { jobs, mesh, models, neededKeys, netRate, network, ready, refresh, refreshMany, samples, setRefreshTimeout, tokenRates, useNeed, type ConnSample } from "./live";
import { intervalFor, startPoller } from "./poll";
import { daemon } from "./stores";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("daemon reachability (busy is not down)", () => {
  it("a status timeout with /health answering is busy: reachable, not responding, last status kept", async () => {
    const fake = installFakeDaemon();
    await refreshStatus();
    const before = daemon.get().status;
    fake.routes["GET /api/status"] = ({ init }) => hang(init);
    await refreshStatus(50);
    expect(daemon.get()).toMatchObject({ reachable: true, responding: false, status: before });
    delete fake.routes["GET /api/status"];
    await refreshStatus();
    expect(daemon.get().responding).toBe(true);
  });

  it("only a refused connection with no /health answer is down, and clears the status", async () => {
    installFakeDaemon();
    await refreshStatus();
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    await refreshStatus();
    expect(daemon.get()).toMatchObject({ reachable: false, status: null });
  });

  it("an HTTP error from /api/status means the service answered, and marks the kept status as not live", async () => {
    const fake = installFakeDaemon();
    await refreshStatus();
    const { status, at } = daemon.get();
    fake.routes["GET /api/status"] = () => new Response("{}", { status: 500 });
    await refreshStatus();
    expect(daemon.get()).toMatchObject({ reachable: true, responding: true, status, at });
    expect(daemon.get().statusError).toBeTruthy();
    delete fake.routes["GET /api/status"];
    await refreshStatus();
    expect(daemon.get().statusError).toBeNull();
  });
});

describe("refresh", () => {
  it("keeps the last good data when a refresh times out (injected 50 ms), with the error and the old time", async () => {
    const fake = installFakeDaemon();
    await refresh("models");
    const { at } = models.get();
    expect(models.get().data).toHaveLength(MODELS.length);
    fake.routes["GET /api/models"] = ({ init }) => hang(init);
    setRefreshTimeout(50);
    const t0 = performance.now();
    await refresh("models");
    expect(performance.now() - t0).toBeLessThan(1000);
    expect(models.get()).toMatchObject({ at, error: { code: SLOW } });
    expect(models.get().data).toHaveLength(MODELS.length);
  });

  it("is single-flight", async () => {
    const fake = installFakeDaemon();
    await Promise.all([refresh("network"), refresh("network"), refresh("network")]);
    expect(fake.fetchMock.mock.calls.filter(([u]) => String(u).endsWith("/api/network"))).toHaveLength(1);
    await refresh("network");
    expect(fake.fetchMock.mock.calls.filter(([u]) => String(u).endsWith("/api/network"))).toHaveLength(2);
  });

  it("after an action, refreshMany(keys, fresh) never answers with a read that started before it", async () => {
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER] });
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    let n = 0;
    fake.routes["GET /api/peers"] = async () => {
      // Each read answers with the peers as they were when it arrived; the first one is held back.
      const snapshot = fake.peers.map((p) => ({ ...p }));
      if (++n === 1) await gate;
      return new Response(JSON.stringify({ peers: snapshot, discovered: [], revocations: [] }));
    };
    const poll = refresh("mesh");
    fake.peers = []; // the action (a removal) lands while the poll is in flight
    const after = refreshMany(["mesh"], true);
    release();
    await Promise.all([poll, after]);
    expect(n).toBe(2);
    expect(mesh.get().data!.peers).toEqual([]);
  });

  it("jobs before E3 are unsupported, not empty", async () => {
    installFakeDaemon({ jobs: "unsupported" });
    await refresh("jobs");
    expect(jobs.get().data).toBe("unsupported");
  });

  it("before E2, two consecutive 'a model is loading' answers make a loading streak; one is not enough to say so", async () => {
    const fake = installFakeDaemon({ e2: false });
    fake.loading = { model_id: "qwen", since: Date.now() };
    await refresh("ready");
    expect(ready.get().loadingStreak).toBe(1);
    await refresh("ready");
    expect(ready.get().loadingStreak).toBe(2);
    fake.loading = null;
    await refresh("ready");
    expect(ready.get().loadingStreak).toBe(0);
  });
});

describe("poll cadence", () => {
  it("mesh 2 s while a device is connected, 5 s with networking on, else 30 s; jobs 2 s only when needed", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [{ ...PEER, connected: false }] });
    await Promise.all([refresh("network"), refresh("mesh")]);
    expect(intervalFor("mesh")).toBe(5000);
    mesh.set((m) => ({ ...m, data: { ...m.data!, peers: [PEER] } }));
    expect(intervalFor("mesh")).toBe(2000);
    network.set((n) => ({ ...n, data: { ...n.data!, enabled: false } }));
    mesh.set((m) => ({ ...m, data: { ...m.data!, peers: [] } }));
    expect(intervalFor("mesh")).toBe(30000);
    expect(intervalFor("jobs")).toBe(5000);
    expect(intervalFor("metrics")).toBeNull();
    expect(intervalFor("models")).toBeNull();
  });

  it("jobs: once a minute from a service without /api/jobs (before E3)", async () => {
    installFakeDaemon({ jobs: "unsupported" });
    await refresh("jobs");
    expect(jobs.get().data).toBe("unsupported");
    expect(intervalFor("jobs")).toBe(60000);
  });

  it("/ready is polled only while the service does not report engine state", async () => {
    const fake = installFakeDaemon();
    await refreshStatus();
    expect(intervalFor("ready")).toBeNull();
    fake.e2 = false;
    await refreshStatus();
    expect(intervalFor("ready")).toBe(2500);
  });

  it("polls nothing but status while the service is not reachable", async () => {
    vi.useFakeTimers();
    try {
      const refused = vi.fn(async () => Promise.reject(new TypeError("Failed to fetch")));
      vi.stubGlobal("fetch", refused);
      daemon.set((d) => ({ ...d, reachable: false }));
      const stop = startPoller({ tickMs: 100 });
      await vi.advanceTimersByTimeAsync(40000);
      stop();
      const paths = new Set(refused.mock.calls.map((c) => new URL(String((c as unknown[])[0])).pathname));
      expect(paths.has("/api/status")).toBe(true);
      expect([...paths].every((p) => p === "/api/status" || p === "/health")).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("useNeed", () => {
  it("fetches nothing until the service is reachable (its address may not be known yet), then at once", async () => {
    const fake = installFakeDaemon({ jobs: JOBS });
    for (const reachable of [null, false]) {
      daemon.set((d) => ({ ...d, reachable }));
      const { unmount } = renderHook(() => useNeed(["jobs"]));
      await new Promise((r) => setTimeout(r, 10));
      expect(fake.fetchMock).not.toHaveBeenCalled();
      expect(neededKeys()).toEqual(["jobs"]);
      unmount();
    }
    await refreshStatus();
    renderHook(() => useNeed(["jobs"]));
    await waitFor(() => expect(jobs.get().data).toEqual(JOBS));
  });
});

describe("samples", () => {
  const S = (at: number, conns: Record<string, [number, number]>): ConnSample => ({ at, conns: Object.fromEntries(Object.entries(conns).map(([id, [rx, tx]]) => [id, { peer: PEER.id, rx, tx }])) });

  it("rate = Δbytes / Δt within one connection id; a changed id or a long gap is unknown, never 0", () => {
    expect(netRate([S(0, { "7": [0, 0] }), S(5000, { "7": [61440, 10240] })], 5000)).toEqual({ rx: 12288, tx: 2048 });
    expect(netRate([S(0, { "7": [0, 0] }), S(5000, { "8": [61440, 10240] })], 5000)).toBeNull();
    expect(netRate([S(0, { "7": [0, 0] }), S(9000, { "7": [61440, 10240] })], 9000)).toBeNull();
    expect(netRate([S(0, { "7": [0, 0] }), S(5000, { "7": [61440, 10240] })], 20000)).toBeNull();
  });

  it("mesh refreshes record connection samples only when the service reports connections (E1)", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER] });
    await refresh("mesh");
    expect(samples.get().conn).toHaveLength(0);
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER_E1] });
    await refresh("mesh");
    expect(samples.get().conn[0].conns["7"]).toEqual({ peer: PEER.id, rx: 91822, tx: 48213 });
  });

  it("token counters only count tokens generated after the first poll, by where they ran", async () => {
    let now = 1_000_000;
    vi.spyOn(Date, "now").mockImplementation(() => now);
    const fake = installFakeDaemon({ jobs: JOBS });
    await refresh("jobs");
    expect(samples.get().tokens[0]).toMatchObject({ local: 0, sent: 0, served: 0 });
    const j = structuredClone(JOBS);
    j.active[0].completion_tokens! += 50; // local chat
    j.active[2].completion_tokens! += 20; // served for a peer
    fake.jobs = j;
    now += 5000;
    await refresh("jobs");
    expect(samples.get().tokens[1]).toMatchObject({ local: 50, sent: 0, served: 20 });
    expect(tokenRates(samples.get().tokens)).toEqual({ local: [10], sent: [0], served: [4] });
  });
});
