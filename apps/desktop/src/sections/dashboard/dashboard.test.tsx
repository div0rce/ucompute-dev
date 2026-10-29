// The Dashboard (plan §2.1, owner A): attention with inline fixes, first run, Computers, Running now, Recent activity.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AuditEvent, Job, Loaded } from "../../api/client";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { pushAttention } from "../../shell/attention";
import { ownStreams, pairing, pulls, refresh } from "../../state/live";
import { chatHistory, daemon, lastError, privacy, settings } from "../../state/stores";
import { installFakeDaemon, JOBS, LOADED, MODELS, NETWORK_ON, PEER, PEERS_E1, PRIVACY } from "../../test/mocks";

// The app shell's Windows Firewall command (src-tauri firewall.rs), as in firewall.test.tsx.
const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const w = window as unknown as Record<string, unknown>;

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
  lastError.set(null);
  privacy.set({ data: null, at: null, error: null });
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
});
afterEach(() => {
  delete w.__TAURI_INTERNALS__;
  vi.unstubAllGlobals();
});

async function open(over: Parameters<typeof installFakeDaemon>[0] = {}, loaded: Loaded | null = LOADED) {
  const fake = installFakeDaemon(over);
  fake.loaded = loaded;
  await refreshStatus();
  window.location.hash = "#/dashboard";
  render(<App poll={false} />);
  await screen.findByRole("grid", { name: "Computers" });
  return fake;
}
const grid = (name: string) => screen.getByRole("grid", { name });
const dataRows = (name: string) => within(grid(name)).getAllByRole("row").slice(1);
/** The cell under `header` in `row`. */
function cell(row: HTMLElement, header: string): HTMLElement {
  const headers = within(row.closest("table")!).getAllByRole("columnheader");
  return within(row).getAllByRole("gridcell")[headers.findIndex((h) => h.textContent === header || h.textContent?.startsWith(header))];
}
/** A cell's visible text (its hidden tooltip removed). */
function text(el: HTMLElement): string {
  const c = el.cloneNode(true) as HTMLElement;
  c.querySelectorAll('[role="tooltip"]').forEach((t) => t.remove());
  return c.textContent ?? "";
}
const PROMPTS = [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }];

describe("Computers", () => {
  it("lists this computer first, then paired devices (never nearby or revoked); a row opens Devices with it selected", async () => {
    const user = userEvent.setup();
    await open({ network: NETWORK_ON, peers: PEERS_E1.peers, discovered: PEERS_E1.discovered, revocations: PEERS_E1.revocations });
    const rows = dataRows("Computers");
    // The Devices table's own columns (B's table.tsx); nearby and revoked rows stay in Devices.
    expect(rows.map((r) => text(cell(r, "Name")))).toEqual([expect.stringContaining("DESKTOP-7QK2M1"), expect.stringContaining("Laptop"), expect.stringContaining("Studio Mac")]);
    expect(text(cell(rows[2], "State"))).toBe("Connected");
    // Its last dial was lost and it is not announced nearby: not "Offline", not a warning.
    expect(text(cell(rows[1], "State"))).toBe("Not seen nearby");
    expect(cell(rows[1], "State").querySelector(".tone-warn")).toBeNull();
    expect(text(cell(rows[1], "Free memory"))).toBe("—Not reported live");
    expect(text(cell(rows[2], "Last seen"))).toBe("Now");
    await user.dblClick(cell(rows[2], "Name"));
    expect(window.location.hash).toBe(`#/devices/${PEER.id}`);
  });

  // Replaces the Phase 0 "RunOn on Home" case (privacy.test.tsx): the landing screen says where prompts can run.
  it.each([
    ["trusted_group", "Can send", /Its own permissions/],
    ["local_only", "Not permitted here", null],
  ] as const)("says where prompts can run (privacy %s): %s", async (mode, word, note) => {
    await open({ network: NETWORK_ON, peers: [{ ...PEER, health: "connected" }], groups: PROMPTS, privacy: { ...PRIVACY, mode } });
    await waitFor(() => expect(text(cell(dataRows("Computers")[1], "Can run there"))).toMatch(new RegExp(`^${word}`)));
    const c = cell(dataRows("Computers")[1], "Can run there");
    if (note) expect(text(c)).toMatch(note);
    else expect(document.getElementById(c.getAttribute("aria-describedby")!)!.textContent).toMatch(/allow trusted devices under Privacy/);
  });

  it("with nothing paired, offers pairing", async () => {
    await open();
    expect(dataRows("Computers")).toHaveLength(1);
    expect(screen.getByRole("link", { name: "Pair a device…" }).getAttribute("href")).toBe("#/devices?pair=1");
  });
});

describe("Running now", () => {
  it("active jobs first, then the last 5 finished; This app via client_ref; Stop only on this app's own active rows", async () => {
    const user = userEvent.setup();
    const recent: Job[] = Array.from({ length: 7 }, (_, i) => ({ ...JOBS.recent[0], id: `chatcmpl-r${i}`, client_ref: null }));
    const stop = vi.fn();
    ownStreams.set({ [JOBS.active[0].client_ref!]: { stop } });
    await open({ network: NETWORK_ON, peers: PEERS_E1.peers, jobs: { active: JOBS.active, recent } });
    await waitFor(() => expect(dataRows("Running now")).toHaveLength(4 + 5));
    const rows = dataRows("Running now");
    // The Jobs table's own columns (C's useJobs), Wait and TTFT hidden here.
    expect(rows.slice(0, 4).map((r) => [text(cell(r, "State")), text(cell(r, "Requested by"))])).toEqual([
      ["Running", "This app"],
      ["Running", "Studio Mac"],
      ["Waiting for a slot", "This computer (another API client or not matched)"],
      ["Connecting to the device", "This computer (another API client or not matched)"],
    ]);
    expect(within(grid("Running now")).queryByRole("columnheader", { name: "TTFT" })).toBeNull();
    expect(rows.slice(4).map((r) => text(cell(r, "State")))).toEqual(Array(5).fill("Done"));
    expect(screen.queryByText("Nothing is running. Recently finished:")).toBeNull();
    expect(text(cell(rows[0], "Tokens"))).toBe("37so far");

    const stopBtn = screen.getByRole("button", { name: "Stop" });
    expect(stopBtn.getAttribute("aria-disabled")).toBe("true");
    await user.click(cell(rows[2], "Model"));
    expect(document.getElementById(stopBtn.getAttribute("aria-describedby")!.split(" ").pop()!)!.textContent).toMatch(/^Only requests started in this app can be stopped/);
    await user.click(stopBtn);
    expect(stop).not.toHaveBeenCalled();
    await user.click(cell(rows[0], "Model"));
    const ready = screen.getByRole("button", { name: "Stop" });
    expect(ready.getAttribute("aria-disabled")).toBeNull();
    await user.click(ready);
    expect(stop).toHaveBeenCalledTimes(1);
  });

  it("without E3 says the job list is not reported, with the slot counts", async () => {
    await open({ jobs: "unsupported" });
    expect(await screen.findByText("Job list not reported by this service version. In use 0/1 · Waiting 0.")).toBeTruthy();
    expect(screen.queryByRole("grid", { name: "Running now" })).toBeNull();
  });

  it("only finished rows: says nothing is running above them, so the title does not claim they run", async () => {
    await open({ jobs: { active: [], recent: JOBS.recent } });
    await waitFor(() => expect(dataRows("Running now").length).toBeGreaterThan(0));
    expect(screen.getByText("Nothing is running. Recently finished:")).toBeTruthy();
  });

  it("nothing running says where requests come from", async () => {
    await open();
    expect(screen.getByText(/^Nothing is running\. Requests from Chat, other programs on this computer and paired devices appear here\.$/)).toBeTruthy();
  });
});

describe("Recent activity", () => {
  const T = (s: number) => new Date(Date.UTC(2026, 9, 22, 10, 0, s)).toISOString();

  it("shows at most 8 rows, newest first, and opens Activity at the row", async () => {
    const user = userEvent.setup();
    const audit: AuditEvent[] = Array.from({ length: 12 }, (_, i) => ({ ts_utc: T(i), kind: "pairing_started", fields: { window_secs: 120 } }));
    await open({ audit });
    await waitFor(() => expect(dataRows("Recent activity")).toHaveLength(8));
    await user.dblClick(cell(dataRows("Recent activity")[0], "Event"));
    expect(window.location.hash).toBe(`#/activity/${encodeURIComponent(`${T(11)}|pairing_started|`)}`);
  });
});

describe("Needs attention", () => {
  it("an unreachable paired device offers Connect to <name>, which dials it", async () => {
    const user = userEvent.setup();
    const fake = await open({
      network: NETWORK_ON,
      peers: [{ ...PEER, connected: false, health: "disconnected", last_error: { addr: "192.168.1.77:8193", code: "peer_unauthorized", message: "the device did not prove it is 0f1e-2d3c-4b5a", hint: null, at_utc: T0 } }],
      answersAt: { [PEER.id]: "192.168.1.77:8193" },
    });
    const list = screen.getByRole("region", { name: "Needs attention" });
    expect(within(list).getByText(/^Studio Mac is unreachable: /)).toBeTruthy();
    await user.click(within(list).getByRole("button", { name: "Connect to Studio Mac" }));
    await waitFor(() => expect(fake.fetchMock.mock.calls.some(([u, i]) => String(u).endsWith(`/api/peers/${PEER.id}/connect`) && (i as RequestInit)?.method === "POST")).toBe(true));
    await waitFor(() => expect(screen.queryByRole("region", { name: "Needs attention" })).toBeNull());
  });

  it("pairing open resumes the Pair dialog; event entries and the last problem can be dismissed", async () => {
    const user = userEvent.setup();
    const fake = await open();
    // Open on the service too, or the Pair dialog's poller ends the session.
    fake.pairing.session = { code: "482913", port: 51234, pair_addr: "192.168.1.10:51234", qr_payload: "ucompute-pair:v1:x", expires_at: Math.floor(Date.now() / 1000) + 90 };
    act(() => {
      pairing.set({ code: "482913", addr: null, payload: "ucompute-pair:v1:x", expiresAt: Date.now() + 90_000 });
      pushAttention({ id: "pull:m", tone: "danger", text: "Download failed: m" });
      lastError.set({ code: "insufficient_memory", title: "Needs more memory.", next_step: "Choose a smaller model.", detail: "need 9 GiB", hint: null });
    });
    const list = screen.getByRole("region", { name: "Needs attention" });
    expect(within(list).getByText("Last problem")).toBeTruthy();
    const failed = within(list).getByText("Download failed: m").closest("li")!;
    await user.click(within(failed).getByRole("button", { name: "Dismiss" }));
    expect(within(list).queryByText("Download failed: m")).toBeNull();
    await user.click(within(screen.getByText("Last problem").closest("li")!).getByRole("button", { name: "Dismiss" }));
    expect(screen.queryByText("Last problem")).toBeNull();
    await user.click(within(list).getByRole("button", { name: "Resume pairing" }));
    expect(window.location.hash).toBe("#/devices?pair=1");
  });

  it("Windows with no firewall rule found: the §2.2 wording and Allow, in warn, never in the status bar", async () => {
    w.__TAURI_INTERNALS__ = {};
    invoke.mockImplementation(async (cmd: string) => (cmd === "firewall_status" ? { supported: true, allowed: false, rule_program: null, public_networks: [] } : undefined));
    await open({ network: NETWORK_ON });
    const list = await screen.findByRole("region", { name: "Needs attention" });
    expect(within(list).getByText("No Windows Firewall rule for the service was found; other devices may be unable to connect.").closest(".notice")!.className).toContain("tone-warn");
    expect(within(list).getByRole("button", { name: "Allow on private networks" })).toBeTruthy();
    expect(screen.getByRole("contentinfo", { name: "Status" }).textContent).not.toMatch(/firewall/i);
  });
});

describe("Get started", () => {
  it("with nothing downloaded: the recommended model that fits, and Download opens the Models download dialog", async () => {
    await open({ models: MODELS.map((m) => ({ ...m, downloaded: false })) }, null);
    const block = await screen.findByRole("region", { name: "Get started" });
    expect(within(block).getByText(MODELS[0].name)).toBeTruthy();
    expect(within(block).getByText(/Apache-2.0 · Fits · .* headroom/)).toBeTruthy();
    expect(within(block).getByRole("link", { name: "Download…" }).getAttribute("href")).toBe(`#/models?confirm=${MODELS[0].id}`);
  });

  it("after downloading it here: Run loads it and opens Chat; hidden once a model was already downloaded", async () => {
    const user = userEvent.setup();
    const fake = await open({ models: MODELS.map((m) => ({ ...m, downloaded: m.id === MODELS[0].id })) });
    expect(screen.queryByRole("region", { name: "Get started" })).toBeNull();
    fake.loaded = null;
    await act(async () => {
      await refreshStatus();
      pulls.set({ [MODELS[0].id]: { state: "done", ev: null, startedAt: Date.now() } });
      await refresh("models");
    });
    const block = await screen.findByRole("region", { name: "Get started" });
    await user.click(within(block).getByRole("button", { name: "Run" }));
    await waitFor(() => expect(window.location.hash).toBe("#/chat"));
    expect(fake.fetchMock.mock.calls.some(([u, i]) => String(u).endsWith(`/api/models/${MODELS[0].id}/load`) && (i as RequestInit).method === "POST")).toBe(true);
  });
});

const T0 = "2026-10-22T10:00:00Z";
