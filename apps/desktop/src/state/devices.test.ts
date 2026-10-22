// deviceRows invariants (created by A in Phase 0; B owns this file and extends it to every row of the plan §2.2
// Connection, State, Can run there and Can use this computer tables).
import { describe, expect, it } from "vitest";
import type { AuditEvent, Peer } from "../api/client";
import { NETWORK, NETWORK_ON, PEER, PEER_E1, PEERS_E1, PEERS_E5, STATUS_E2, STRANGER } from "../test/mocks";
import { deviceRows, live, type DeviceInput } from "./devices";
import type { DaemonState } from "./stores";
import { mapError } from "../api/errors";

const UP: DaemonState = { reachable: true, responding: true, version: "0.1.0", status: null, spawned: false };
const input = (over: Partial<DeviceInput> = {}): DeviceInput => ({
  daemon: UP,
  network: NETWORK_ON,
  mesh: { peers: [], discovered: [], revocations: [] },
  groups: [],
  models: null,
  hardware: null,
  deviceName: "",
  localOnly: false,
  jobs: null,
  audit: null,
  dialing: null,
  now: Date.parse("2026-10-22T10:30:17Z"),
  ...over,
});
const row = (rows: ReturnType<typeof deviceRows>, key: string) => rows.find((r) => r.key === key)!;
const home = [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }];

describe("deviceRows", () => {
  it("this computer is first with the key self; it is never listed as nearby", () => {
    const rows = deviceRows(input({ mesh: { peers: [], discovered: [{ ...STRANGER, id: NETWORK_ON.peer_id }], revocations: [] } }));
    expect(rows.map((r) => r.key)).toEqual(["self"]);
    expect(rows[0]).toMatchObject({ trust: "self", name: NETWORK_ON.device_name, state: { word: "Listening · 0 connected" } });
  });

  it("a cached mDNS record never makes a device connected; a missing one never says offline", () => {
    const away: Peer = { ...PEER, connected: false, health: "disconnected", last_error: null };
    const seen = deviceRows(input({ mesh: { peers: [away], discovered: [{ ...STRANGER, id: PEER.id, trusted: true }], revocations: [] } }));
    expect(row(seen, PEER.id).state).toMatchObject({ word: "Not connected", tone: "neutral" });
    const unseen = deviceRows(input({ mesh: { peers: [away], discovered: [], revocations: [] } }));
    expect(row(unseen, PEER.id).state).toMatchObject({ word: "Not seen nearby", tone: "neutral" });
    expect(JSON.stringify(unseen)).not.toMatch(/Offline/);
  });

  it("peer_lost and not announced is Not seen nearby (neutral); announced but nothing answered is Unreachable (warn)", () => {
    const lost = PEERS_E1.peers[1];
    expect(row(deviceRows(input({ mesh: { peers: [lost], discovered: [], revocations: [] } })), lost.id).state).toMatchObject({ word: "Not seen nearby", tone: "neutral" });
    const announced = { ...STRANGER, id: lost.id, trusted: true };
    expect(row(deviceRows(input({ mesh: { peers: [lost], discovered: [announced], revocations: [] } })), lost.id).state).toMatchObject({ word: "Unreachable", tone: "warn" });
  });

  it("E5: held here is Disconnected by you; held there (peer_held) is Disconnected by <name>", () => {
    const rows = deviceRows(input({ mesh: PEERS_E5 }));
    expect(row(rows, PEERS_E5.peers[0].id).state.word).toBe("Disconnected by you");
    expect(row(rows, PEERS_E5.peers[1].id).state).toMatchObject({ word: "Disconnected by Laptop", detail: "It must reconnect from its side." });
  });

  it("networking off overrides every peer state; unhealthy is Not responding and not live", () => {
    const sick = { ...PEER, health: "unhealthy" as const };
    expect(live(sick)).toBe(false);
    expect(row(deviceRows(input({ mesh: { peers: [sick], discovered: [], revocations: [] } })), PEER.id).state).toMatchObject({ word: "Not responding", tone: "warn" });
    expect(row(deviceRows(input({ network: NETWORK, mesh: { peers: [sick], discovered: [], revocations: [] } })), PEER.id).state.word).toBe("Networking off");
    expect(row(deviceRows(input({ mesh: { peers: [sick], discovered: [], revocations: [] }, groups: home })), PEER.id).runThere?.word).toBe("—");
  });

  it("Can run there: Can send (neutral) until a real outcome on this connection; done is ok, a refusal is warn", () => {
    const peer = PEER_E1;
    const at = (ts: string, outcome: string): AuditEvent => ({ ts_utc: ts, kind: "remote_request", peer_id: peer.id, fields: { role: "client", phase: "finished", outcome } });
    const run = (audit: AuditEvent[]) => row(deviceRows(input({ mesh: { peers: [peer], discovered: [], revocations: [] }, groups: home, audit })), peer.id).runThere;
    expect(run([])).toMatchObject({ word: "Can send", tone: "neutral" });
    // Before this connection opened (10:30:00): ignored.
    expect(run([at("2026-10-22T10:29:00Z", "done")])).toMatchObject({ word: "Can send" });
    expect(run([at("2026-10-22T10:30:10Z", "done")])).toMatchObject({ word: "Can send · last request done", tone: "ok" });
    expect(run([at("2026-10-22T10:30:10Z", "policy_denied")])).toMatchObject({ word: "Refused last request (its permissions)", tone: "warn" });
    expect(run([at("2026-10-22T10:30:10Z", "overloaded")])).toMatchObject({ word: "Refused last request (busy or over quota)" });
    expect(JSON.stringify(run([]))).not.toMatch(/Available|Busy/);
  });

  it("a revoked id that still announces is a Revoked row, not Nearby; a re-pair supersedes the revocation", () => {
    const rev = PEERS_E1.revocations[0];
    const rows = deviceRows(input({ mesh: { peers: [], discovered: [{ ...STRANGER, id: rev.peer_id }], revocations: [rev] } }));
    expect(rows.map((r) => [r.key, r.trust])).toEqual([
      ["self", "self"],
      [rev.peer_id, "revoked"],
    ]);
    expect(rows[1].state.word).toMatch(/^Revoked · announced nearby /);
    const repaired = deviceRows(input({ mesh: { peers: [{ ...PEER, id: rev.peer_id }], discovered: [], revocations: [rev] } }));
    expect(repaired.map((r) => r.trust)).toEqual(["self", "trusted"]);
  });

  it("Can use this computer mirrors may_serve; capabilities undefined (before E1) vs null (not connected)", () => {
    const rows = deviceRows(input({ network: { ...NETWORK_ON, serve_peers: true }, mesh: PEERS_E1, groups: home }));
    expect(row(rows, PEER.id)).toMatchObject({ useHere: { word: "Yes" }, link: { id: 7 }, addr: { provenance: "connected" } });
    expect(row(rows, PEERS_E1.peers[1].id)).toMatchObject({ useHere: { word: "No · not in a prompt group" }, caps: null });
    expect(row(deviceRows(input({ mesh: { peers: [PEER], discovered: [], revocations: [] } })), PEER.id).caps).toBeUndefined();
  });
});

// Every row of the plan §2.2 tables (B, Phase 1). One case per rule; the first matching rule wins.
describe("plan §2.2 tables, rule by rule", () => {
  const ID = PEER.id;
  const err = (code: string) => ({ addr: "10.0.0.5:8193", code, message: "no answer from 10.0.0.5:8193 within 4 s", hint: "the device may be asleep", at_utc: "2026-10-22T10:29:00Z" });
  const announced = { ...STRANGER, id: ID, trusted: true };
  const state = (p: Partial<Peer>, over: Partial<DeviceInput> = {}, discovered = [] as (typeof STRANGER)[]) =>
    row(deviceRows(input({ mesh: { peers: [{ ...PEER, ...p }], discovered, revocations: [] }, ...over })), ID).state;

  it.each([
    ["1 networking off", { connected: true }, { network: NETWORK }, [], "Networking off", "neutral"],
    ["2 held here (E5)", { connected: false, held: true }, {}, [], "Disconnected by you", "neutral"],
    ["2 held but still connected is not held", { connected: true, held: true }, {}, [], "Connected", "ok"],
    ["3 connected, missing keep-alives", { connected: true, health: "unhealthy" }, {}, [], "Not responding", "warn"],
    ["4 connected", { connected: true, health: "connected" }, {}, [], "Connected", "ok"],
    ["5 this app is dialling", { connected: false, last_error: err("peer_lost") }, { dialing: ID }, [], "Connecting", "active"],
    ["6 the other device held this one", { connected: false, last_error: err("peer_held") }, {}, [], "Disconnected by Studio Mac", "neutral"],
    ["7 nothing answered, not announced", { connected: false, last_error: err("peer_lost") }, {}, [], "Not seen nearby", "neutral"],
    ["8 nothing answered, but announced", { connected: false, last_error: err("peer_lost") }, {}, [announced], "Unreachable", "warn"],
    ["8 refused (peer_unauthorized)", { connected: false, last_error: err("peer_unauthorized") }, {}, [], "Unreachable", "warn"],
    ["8 peer_lost where mDNS is off", { connected: false, last_error: err("peer_lost") }, { network: { ...NETWORK_ON, mdns: false } }, [], "Unreachable", "warn"],
    ["9 reconnecting", { connected: false, health: "reconnecting" }, {}, [announced], "Reconnecting", "warn"],
    ["10 not announced where mDNS runs", { connected: false, health: "disconnected" }, {}, [], "Not seen nearby", "neutral"],
    ["11 announced but not connected", { connected: false, health: "disconnected" }, {}, [announced], "Not connected", "neutral"],
    ["11 mDNS off", { connected: false }, { network: { ...NETWORK_ON, mdns: false } }, [], "Not connected", "neutral"],
  ] as const)("Connection %s", (_, p, over, discovered, word, tone) => {
    expect(state(p as Partial<Peer>, over as Partial<DeviceInput>, [...discovered])).toMatchObject({ word, tone });
  });

  it("each state carries the reason it can give: a dial error's message and hint, or what to do", () => {
    expect(state({ connected: false, last_error: err("peer_unauthorized") }).detail).toMatch(/^Last try at \d\d:\d\d:\d\d: No answer from 10\.0\.0\.5:8193 within 4 s\. The device may be asleep\.$/);
    expect(state({ connected: false, last_error: err("peer_lost") }).detail).toMatch(/^Nothing answered at 10\.0\.0\.5:8193, \d\d:\d\d:\d\d; not announced nearby\.$/);
    expect(state({ connected: false, held: true }).detail).toBe("Until you connect again, turn networking off and on, or the service restarts.");
  });

  it("State of this computer, nearby and revoked rows", () => {
    const self = (network: DeviceInput["network"], peers: Peer[] = []) => deviceRows(input({ network, mesh: { peers, discovered: [], revocations: [] } }))[0].state;
    expect(self(NETWORK)).toMatchObject({ word: "Networking off", tone: "neutral" });
    expect(self({ ...NETWORK_ON, listen_addr: null })).toMatchObject({ word: "Not listening" });
    // Connected counts only live devices: a device missing keep-alives is apart.
    expect(self(NETWORK_ON, [PEER, { ...PEER, id: "ab".repeat(32), health: "unhealthy" }])).toMatchObject({ word: "Listening · 1 connected", tone: "ok" });
    const now = Date.parse(STRANGER.seen_utc) + 12_000;
    const nearby = deviceRows(input({ mesh: { peers: [], discovered: [STRANGER], revocations: [] }, now }))[1];
    expect(nearby).toMatchObject({ trust: "untrusted", runThere: null, useHere: null });
    expect(nearby.state.word).toMatch(/^Not paired · announced 12 sec/);
    const rev = PEERS_E1.revocations[0];
    expect(deviceRows(input({ mesh: { peers: [], discovered: [], revocations: [rev] } }))[1].state).toMatchObject({ word: "Revoked", tone: "danger" });
  });

  it("Can run there for this computer, rule by rule", () => {
    const q = (inflight: number, queued: number) => ({ inflight, queued, max_concurrent: 2, max_queue: 8 });
    const self = (daemon: Partial<DaemonState>) => deviceRows(input({ daemon: { ...UP, ...daemon } }))[0].runThere;
    expect(self({ reachable: false })).toMatchObject({ word: "Not running", tone: "danger" });
    expect(self({ responding: false })).toMatchObject({ word: "Busy (not answering)", tone: "warn" });
    expect(self({ status: { ...STATUS_E2, engine: { state: "loading", loading_model_id: "m", loading_for_ms: 900 } } })).toMatchObject({ word: "Loading model", tone: "active" });
    expect(self({ status: { ...STATUS_E2, queue: q(2, 0) } })).toMatchObject({ word: "Busy · 2/2 in use, 0 waiting", tone: "warn" });
    expect(self({ status: { ...STATUS_E2, queue: q(1, 3) } })).toMatchObject({ word: "Busy · 1/2 in use, 3 waiting", tone: "warn" });
    expect(self({ status: { ...STATUS_E2, queue: q(1, 0) } })).toEqual({ word: "Available", tone: "ok", note: undefined });
    expect(self({ status: { ...STATUS_E2, loaded: null, queue: q(0, 0) } })).toMatchObject({ word: "Available", note: "No model loaded" });
    // A status poll answered with an HTTP error: the kept queue is not current.
    expect(self({ status: { ...STATUS_E2, queue: q(0, 0) }, statusError: mapError("internal", "500") })).toMatchObject({ word: "Not known", tone: "neutral" });
  });

  it("groups or privacy not read yet: what depends on them is unknown, never No or Not permitted", () => {
    const one = (over: Partial<DeviceInput>) => row(deviceRows(input({ network: { ...NETWORK_ON, serve_peers: true }, mesh: { peers: [PEER_E1], discovered: [], revocations: [] }, groups: home, ...over })), PEER_E1.id);
    expect(one({ groups: null }).useHere).toEqual({ word: "—", note: "groups not read yet" });
    expect(one({ groups: null }).runThere).toMatchObject({ word: "—", note: "groups or privacy not read yet" });
    expect(one({ localOnly: null }).runThere).toMatchObject({ word: "—", note: "groups or privacy not read yet" });
    // Known negatives still come first: networking or sharing off does not depend on groups.
    expect(one({ groups: null, network: NETWORK_ON }).useHere).toEqual({ word: "No · sharing off" });
  });

  it("Can run there for a paired device, rule by rule; Busy and Available are never said of a peer", () => {
    const p = PEER_E1; // connected; this connection opened 10:30:00
    const finished = (outcome: string, ts = "2026-10-22T10:30:10Z"): AuditEvent => ({ ts_utc: ts, kind: "remote_request", peer_id: p.id, fields: { role: "client", phase: "finished", outcome } });
    const run = (over: Partial<DeviceInput>, peer: Peer = p) => row(deviceRows(input({ mesh: { peers: [peer], discovered: [], revocations: [] }, groups: home, ...over })), p.id).runThere!;
    expect(run({}, { ...p, connected: false, connection: null })).toMatchObject({ word: "—" });
    expect(run({ groups: [] })).toMatchObject({ word: "Not permitted here", tone: "neutral", detail: 'Cannot run prompts from this computer yet: turn on "Members may run prompts for each other" in its group.' });
    expect(run({ localOnly: true })).toMatchObject({ word: "Not permitted here", detail: "Cannot run prompts from this computer yet: allow trusted devices under Privacy." });
    for (const code of ["policy_denied", "peer_unauthorized", "peer_revoked"]) expect(run({ audit: [finished(code)] })).toMatchObject({ word: "Refused last request (its permissions)", tone: "warn" });
    expect(run({ audit: [finished("overloaded")] })).toMatchObject({ word: "Refused last request (busy or over quota)", tone: "warn" });
    expect(run({ audit: [finished("done")] })).toMatchObject({ word: "Can send · last request done", tone: "ok" });
    expect(run({ audit: [finished("timeout")] })).toMatchObject({ word: "Can send · last request failed: The model took too long to respond.", tone: "warn" });
    expect(run({ audit: [] })).toMatchObject({ word: "Can send", tone: "neutral", note: "Its own permissions, serving setting and load are not reported." });
    // A sent (not finished) request and a served one are not outcomes of this computer's requests.
    const sent: AuditEvent = { ts_utc: "2026-10-22T10:30:20Z", kind: "remote_request", peer_id: p.id, fields: { role: "client", phase: "sent" } };
    const served: AuditEvent = { ts_utc: "2026-10-22T10:30:21Z", kind: "remote_request", peer_id: p.id, fields: { outcome: "policy_denied" } };
    expect(run({ audit: [finished("done"), sent, served] })).toMatchObject({ word: "Can send · last request done" });
    // Fractional seconds from the service still compare as instants (10:30:00.5 is after the 10:30:00 open).
    expect(run({ audit: [finished("done", "2026-10-22T10:30:00.500Z")] })).toMatchObject({ word: "Can send · last request done" });
    // Before E1 the connection start is the newest peer_connected entry.
    const old = { ...PEER, connected: true, health: "connected" as const };
    const opened: AuditEvent = { ts_utc: "2026-10-22T10:31:00Z", kind: "peer_connected", peer_id: p.id, fields: {} };
    expect(run({ audit: [finished("done"), opened] }, old)).toMatchObject({ word: "Can send" });
    expect(run({ audit: [opened, finished("done", "2026-10-22T10:31:05Z")] }, old)).toMatchObject({ word: "Can send · last request done" });
    // Neither: when this connection started is not known, so no outcome is known to be from it (an older one is not).
    expect(run({ audit: [finished("done")] }, old)).toMatchObject({ word: "Can send", tone: "neutral" });
    expect(run({ audit: [finished("policy_denied")] }, old)).toMatchObject({ word: "Can send" });
  });

  it("network state or status not read yet: unknown, never Networking off, No · networking off or Available", () => {
    const rows = deviceRows(input({ network: null, mesh: { peers: [{ ...PEER, connected: false }, { ...PEER_E1, id: "ab".repeat(32) }], discovered: [], revocations: [] }, groups: home }));
    expect(rows[0].state).toMatchObject({ word: "Not known", tone: "neutral" });
    expect(row(rows, PEER.id).state).toMatchObject({ word: "Not known", tone: "neutral" });
    expect(row(rows, PEER.id).useHere).toEqual({ word: "—", note: "network state not read yet" });
    // An open connection is a fact from /api/peers whatever the network read says.
    expect(row(rows, "ab".repeat(32)).state).toMatchObject({ word: "Connected" });
    expect(JSON.stringify(rows)).not.toMatch(/networking off/i);
    expect(rows[0].runThere).toMatchObject({ word: "Not known", tone: "neutral" });
  });

  it("Can use this computer, rule by rule; null for this computer, nearby and revoked rows", () => {
    const use = (network: DeviceInput["network"], groups = home) => row(deviceRows(input({ network, mesh: { peers: [PEER], discovered: [STRANGER], revocations: [] }, groups })), PEER.id).useHere;
    expect(use(NETWORK)).toEqual({ word: "No · networking off" });
    expect(use(NETWORK_ON)).toEqual({ word: "No · sharing off" });
    expect(use({ ...NETWORK_ON, serve_peers: true }, [{ ...home[0], policy: { allow_remote_inference: false, allow_pooled_inference: false } }])).toEqual({ word: "No · not in a prompt group" });
    expect(use({ ...NETWORK_ON, serve_peers: true })).toEqual({ word: "Yes", note: "Quotas and free slots still apply." });
    const rows = deviceRows(input({ network: { ...NETWORK_ON, serve_peers: true }, mesh: { peers: [], discovered: [STRANGER], revocations: PEERS_E1.revocations }, groups: home }));
    expect(rows.map((r) => r.useHere)).toEqual([null, null, null]);
  });

  it("addresses carry where they came from: connected, seen nearby, last known", () => {
    const addr = (p: Partial<Peer>, discovered = [] as (typeof STRANGER)[]) => row(deviceRows(input({ mesh: { peers: [{ ...PEER, ...p }], discovered, revocations: [] } })), ID).addr;
    expect(addr({ connection: PEER_E1.connection })).toEqual({ value: "192.168.1.77:8193", provenance: "connected" });
    expect(addr({ connected: false }, [{ ...announced, addrs: ["10.0.0.9:8193"] }])).toEqual({ value: "10.0.0.9:8193", provenance: "seen nearby" });
    expect(addr({ connected: false })).toEqual({ value: PEER.last_addr, provenance: "last known" });
    expect(addr({ connected: false, last_addr: null })).toBeNull();
  });
});
