// The Devices section's own rules (plan §2.2, §5.4 B): columns never show more than the service reports, Disconnect
// only with E5, Revoke never in the toolbar, the Revoked scope, this computer pinned and preselected, stale rows.
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import type { Peer } from "../../api/client";
import { deviceRows, type DeviceInput } from "../../state/devices";
import { isNeeded, pairing, refresh } from "../../state/live";
import { daemon, privacy } from "../../state/stores";
import { setInspector, setTable, ui } from "../../state/ui";
import { popupMenu, type MenuItem } from "../../ui/contextMenu";
import { firewall } from "../../ui/FirewallNotice";
import { apiError, AUDIT_E4, hang, HARDWARE, installFakeDaemon, JOBS, JOIN_CODE, NETWORK, NETWORK_ON, PEER, PEER_E1, PEERS_E1, PEERS_E5, PRIVACY, QR_PAYLOAD, STRANGER } from "../../test/mocks";
import { calls, cellText, grid, inspector, renderDevices, rowOf, selectionActions, toolbar } from "./fixtures";
import { deviceColumns, dialAddrError, setNetworking } from "./table";

vi.mock("../../ui/contextMenu", async (orig) => ({ ...(await orig<typeof import("../../ui/contextMenu")>()), popupMenu: vi.fn(async () => true) }));

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  privacy.set(null);
});
afterEach(() => vi.unstubAllGlobals());

const UP = { reachable: true, responding: true, version: "0.1.0", status: null, spawned: false };
const NOW = Date.parse("2026-10-22T10:30:17Z");
const E1 = { network: NETWORK_ON, peers: PEERS_E1.peers, discovered: PEERS_E1.discovered, revocations: PEERS_E1.revocations };
const tab = async (user: ReturnType<typeof userEvent.setup>, name: string) => user.click(await within(inspector()).findByRole("tab", { name }));

describe("columns: unknown is '—' with its reason, never a guess", () => {
  const rows = (peers: Peer[]) =>
    deviceRows({ daemon: UP, network: NETWORK_ON, mesh: { peers, discovered: [], revocations: [] }, groups: [], models: null, hardware: HARDWARE, deviceName: "", localOnly: false, jobs: null, audit: null, dialing: null, now: NOW } as DeviceInput);
  const cols = deviceColumns({ hw: HARDWARE, hwAt: NOW - 60_000, models: null, conn: [], now: NOW });
  const col = (id: string) => cols.find((c) => c.id === id)!;
  const show = (id: string, r: ReturnType<typeof rows>[number]) => [col(id).cell(r), col(id).note?.(r) ?? null];

  it("Hardware: this computer's probe; a peer's E1 report; otherwise why not", () => {
    const [self, connected] = rows([PEER_E1]);
    expect(show("hardware", self)).toEqual(["Apple M2 · 8.0 GB", null]);
    expect(show("hardware", connected)).toEqual(["Apple M2 Pro · 16.0 GB", null]);
    expect(show("hardware", rows([{ ...PEER_E1, connected: false, capabilities: null, connection: null }])[1])).toEqual(["—", "Not reported while not connected"]);
    expect(show("hardware", rows([{ ...PEER_E1, capabilities: { ...PEER_E1.capabilities!, hardware: null } }])[1])).toEqual(["—", "Sent in a format this version cannot read"]);
    const { capabilities: _c, connection: _l, ...older } = PEER_E1;
    expect(show("hardware", rows([older])[1])).toEqual(["—", "Not reported by this service version"]);
  });

  it("Free memory: this computer with its probe age; a peer is '—', not reported live, even with a start-time snapshot", () => {
    const [self, peer] = rows([PEER_E1]);
    expect(show("free", self)).toEqual(["4.0 GB", "probed 1 min. ago"]);
    expect(show("free", peer)).toEqual(["—", "Not reported live (needs N2)"]);
    expect(col("free").live).toBe(true);
  });

  it("Models, Last seen and Version follow the same rules", () => {
    const [, peer] = rows([PEER_E1]);
    const [, gone] = rows([{ ...PEER_E1, connected: false, health: "disconnected", capabilities: null, connection: null }]);
    expect(col("models").cell(peer)).toBe("2");
    expect([col("models").cell(gone), col("models").detail!(gone)]).toEqual(["—", "Not reported while not connected"]);
    // last_seen_utc does not advance while connected: a live device reads "Now".
    expect(col("seen").cell(peer)).toBe("Now");
    expect(col("seen").cell(gone)).toMatch(/ago$/);
    expect([col("version").cell(peer), col("version").cell(gone)]).toEqual(["0.1.0", "—"]);
  });
});

describe("Devices section", () => {
  it("this computer is pinned first whatever the sort, and preselected when the URL names no device", async () => {
    const user = userEvent.setup();
    installFakeDaemon(E1);
    renderDevices();
    const names = async () => within(await grid()).getAllByRole("row").slice(1).map((r) => r.textContent);
    await rowOf("Studio Mac");
    expect((await names())[0]).toContain(NETWORK_ON.device_name);
    await user.click(within(await grid()).getByRole("button", { name: "Name" }));
    await waitFor(() => expect(within(screen.getByRole("grid", { name: "Devices" })).getByRole("columnheader", { name: /Name/ }).getAttribute("aria-sort")).toBe("descending"));
    expect((await names())[0]).toContain(NETWORK_ON.device_name);
    expect((await rowOf(NETWORK_ON.device_name!)).getAttribute("aria-selected")).toBe("true");
    expect(window.location.hash).toBe("#/devices");
    expect(within(inspector()).getByRole("tab", { name: "Overview" }).getAttribute("aria-selected")).toBe("true");
  });

  it("#/hardware selects this computer's Hardware tab", async () => {
    installFakeDaemon(E1);
    await refreshStatus();
    window.location.hash = "#/hardware";
    render(<App poll={false} />);
    expect((await screen.findByRole("tab", { name: "Hardware" })).getAttribute("aria-selected")).toBe("true");
    expect((await rowOf(NETWORK_ON.device_name!)).getAttribute("aria-selected")).toBe("true");
    expect(await within(inspector()).findByText("Raw JSON")).toBeTruthy();
  });

  it("Disconnect is absent without E5; with it, Disconnect holds the device and Connect releases it", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER] });
    const { unmount } = renderDevices(`#/devices/${PEER.id}`);
    await waitFor(() => expect(selectionActions()).toEqual(["Connect to Studio Mac", "Remove Studio Mac"]));
    unmount();

    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [{ ...PEER_E1, held: false }], answersAt: { [PEER.id]: PEER.last_addr! } });
    renderDevices(`#/devices/${PEER.id}?tab=connection`);
    // The hold is in memory on the service's network node: a networking restart ends it too, and the button says so.
    expect(await within(inspector()).findByText("Until you connect again, turn networking off and on, or the service restarts.")).toBeTruthy();
    await user.click(await within(toolbar()).findByRole("button", { name: "Disconnect Studio Mac" }));
    await waitFor(async () => expect(await cellText("Studio Mac", "State")).toBe("Disconnected by you"));
    expect(calls(fake, "POST", `/api/peers/${PEER.id}/disconnect`)).toHaveLength(1);
    expect(within(toolbar()).getByRole("button", { name: "Disconnect Studio Mac" }).getAttribute("aria-disabled")).toBe("true");
    await user.click(within(toolbar()).getByRole("button", { name: "Connect to Studio Mac" }));
    await waitFor(async () => expect(await cellText("Studio Mac", "State")).toBe("Connected"));
    expect(fake.peers[0].held).toBe(false);
  });

  it("the device the other side held reads Disconnected by <name> and says it must reconnect from its side", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: PEERS_E5.peers });
    renderDevices();
    expect(await cellText("Studio Mac", "State")).toBe("Disconnected by you");
    expect(await cellText("Laptop", "State")).toBe("Disconnected by Laptop");
    await user.click(await rowOf("Laptop"));
    expect(await within(inspector()).findByText("It must reconnect from its side.")).toBeTruthy();
  });

  it("Revoke is only in the row menu and the Overview's Danger group, never in the toolbar", async () => {
    const user = userEvent.setup();
    installFakeDaemon(E1);
    renderDevices(`#/devices/${PEER.id}`);
    await within(toolbar()).findByRole("button", { name: "Connect to Studio Mac" });
    expect(selectionActions().some((a) => /Revoke/.test(a))).toBe(false);
    const danger = within(inspector()).getByRole("region", { name: "Danger" });
    expect(within(danger).getByRole("button", { name: "Revoke Studio Mac" })).toBeTruthy();
    fireEvent.contextMenu(await rowOf("Studio Mac"));
    await waitFor(() => expect(popupMenu).toHaveBeenCalled());
    const labels = (vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label?: string }[]).map((i) => i.label ?? "—");
    expect(labels).toEqual(["Connect to Studio Mac", "Copy ID", "—", "Remove Studio Mac…", "Revoke Studio Mac…"]);
    // The menu's Revoke opens the same confirmation in the Overview tab.
    await user.click(await within(inspector()).findByRole("tab", { name: "Hardware" }));
    const revoke = (vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label?: string; action?: () => void }[]).find((i) => i.label === "Revoke Studio Mac…")!;
    act(() => revoke.action!());
    expect(await screen.findByRole("group", { name: "Confirm revoking Studio Mac" })).toBeTruthy();
    expect(within(inspector()).getByRole("tab", { name: "Overview" }).getAttribute("aria-selected")).toBe("true");
  });

  it("revoked devices are listed only under Revoked, with who revoked them; a nearby stranger only under All and Nearby", async () => {
    const user = userEvent.setup();
    installFakeDaemon(E1);
    renderDevices();
    const scopes = await screen.findByRole("group", { name: "Show devices" });
    await rowOf("Studio Mac");
    expect([...within(scopes).getAllByRole("button")].map((b) => b.textContent)).toEqual(["All 4", "Connected 1", "Not connected 1", "Nearby 1", "Revoked 1"]);
    expect(within(await grid()).queryByRole("row", { name: (n) => n.includes("cdcd-cdcd-cdcd") })).toBeNull();
    await user.click(within(scopes).getByRole("button", { name: /^Revoked/ }));
    await user.click(await rowOf("cdcd-cdcd-cdcd"));
    expect(await cellText("cdcd-cdcd-cdcd", "State")).toBe("Revoked");
    expect(within(inspector()).getAllByRole("tab").map((t) => t.textContent)).toEqual(["Overview", "Activity"]);
    expect(within(inspector()).getByText("This computer")).toBeTruthy();
    expect(selectionActions()).toEqual([]);
  });

  it("E1 tabs: hardware measured at its network start, models it reported, and both performance sources apart", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ ...E1, jobs: JOBS, audit: [...AUDIT_E4, { ts_utc: "2026-10-22T10:33:00Z", kind: "remote_request", peer_id: PEER.id, fields: { role: "client", phase: "finished", model_id: "qwen2.5-1.5b-instruct-q4_k_m", outcome: "done", completion_tokens: 100, duration_ms: 4000 } }] });
    renderDevices(`#/devices/${PEER.id}?tab=hardware`);
    const panel = await within(inspector()).findByText(/measured when Studio Mac's networking started \(time not reported\)/);
    expect(panel.textContent).toContain("Not independently verified.");
    expect(within(inspector()).getAllByText("at its network start").length).toBe(3);
    await tab(user, "Models");
    expect(await within(inspector()).findByText(/^Unknown model/)).toBeTruthy();
    expect(within(inspector()).getByText("Reported when this connection opened; models pulled or removed there since then are not shown.")).toBeTruthy();
    await tab(user, "Performance");
    const engine = (await within(inspector()).findByText("Measured by Studio Mac's engine; GPU/CPU not reported.")).closest("table")!;
    // A job's TTFT is timed here from arrival (network and that device's queue and load included): not that engine's.
    expect(within(engine).queryByRole("columnheader", { name: /TTFT/ })).toBeNull();
    expect(within(engine).getAllByRole("columnheader").map((h) => h.textContent)).toEqual(["Model", "Runs", "Median decode"]);
    expect(within(inspector()).getByText("End to end from this computer, including queue, load, prompt and network.")).toBeTruthy();
    expect(within(inspector()).getByText(/^No median TTFT here: it is timed on this computer from arrival/)).toBeTruthy();
    // 100 tokens in 4 s end to end, never mixed with the engine's 30.7 tok/s.
    expect(within(inspector()).getByText("25")).toBeTruthy();
    expect(within(inspector()).getByText("30.7 tok/s")).toBeTruthy();
  });

  it("a device not connected, or a service before E1, says so instead of showing hardware", async () => {
    const user = userEvent.setup();
    const laptop = PEERS_E1.peers[1];
    installFakeDaemon(E1);
    renderDevices(`#/devices/${laptop.id}?tab=hardware`);
    expect(await within(inspector()).findByText("Not reported while not connected.")).toBeTruthy();
    await tab(user, "Connection");
    expect(within(inspector()).getByText("Not connected")).toBeTruthy();
  });

  it("pairing again with a device revoked here asks first", async () => {
    const user = userEvent.setup();
    const rev = { ...PEERS_E1.revocations[0], peer_id: QR_PAYLOAD.split(":")[2] };
    const fake = installFakeDaemon({ network: NETWORK_ON, revocations: [rev] });
    renderDevices("#/devices?pair=join");
    const d = await screen.findByRole("dialog", { name: "Pair a device" });
    await user.click(within(d).getByLabelText("Code"));
    await user.paste(QR_PAYLOAD);
    await user.click(within(d).getByRole("button", { name: "Join" }));
    const ask = await within(d).findByRole("group", { name: "Confirm pairing a revoked device" });
    expect(ask.textContent).toMatch(/^You revoked this device on .+; pairing again trusts it again\./);
    expect(calls(fake, "POST", "/api/pair/join")).toHaveLength(0);
    await user.click(within(ask).getByRole("button", { name: "Trust it again" }));
    await within(d).findByText("Paired with Studio Mac.");
    expect(calls(fake, "POST", "/api/pair/join")).toHaveLength(1);
  });

  it("while the service is busy, rows stay dimmed, live columns read Unknown (not refreshed), and row actions wait", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [{ ...PEER, connected: false }] });
    renderDevices(`#/devices/${PEER.id}`);
    await rowOf("Studio Mac");
    act(() => daemon.set((d) => ({ ...d, responding: false, at: Date.now() })));
    expect(await screen.findByText(/Service running, busy \(likely loading a model\)/)).toBeTruthy();
    expect(await cellText("Studio Mac", "State")).toBe("Unknown (not refreshed)");
    expect((await rowOf("Studio Mac")).className).toContain("stale");
    expect(within(toolbar()).getByRole("button", { name: "Connect to Studio Mac" }).getAttribute("aria-disabled")).toBe("true");
    // The inspector's own actions wait too, with the same reason.
    expect((within(inspector()).getByRole("button", { name: "Revoke Studio Mac" }) as HTMLButtonElement).disabled).toBe(true);
    await userEvent.setup().click(within(inspector()).getByRole("tab", { name: "Connection" }));
    const form = await screen.findByRole("form", { name: "Connect to Studio Mac" });
    expect((within(form).getByRole("button", { name: "Connect" }) as HTMLButtonElement).disabled).toBe(true);
    expect(form.textContent).toMatch(/Service busy; not updated since/);
  });

  it("the Address column can be shown; the self row gives the address other devices dial", async () => {
    setTable("devices", { columns: { addr: true, id: true } });
    installFakeDaemon({ network: NETWORK_ON, discovered: [STRANGER] });
    renderDevices();
    expect(await cellText(NETWORK_ON.device_name!, "Address")).toBe("192.168.1.10:8193this computer");
    expect(await cellText(NETWORK_ON.device_name!, "Device ID")).toBe(NETWORK_ON.peer_id);
    expect(await cellText("ffee-ddcc-bbaa", "Name")).toBe("ffee-ddcc-bbaaName is shared at pairing");
  });

  it("Connect from the row menu outside Devices goes there and dials", async () => {
    vi.mocked(popupMenu).mockClear();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [{ ...PEER, connected: false }] });
    renderDevices();
    fireEvent.contextMenu(await rowOf("Studio Mac"));
    await waitFor(() => expect(popupMenu).toHaveBeenCalled());
    const connect = (vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label?: string; action?: () => void }[]).find((i) => i.label === "Connect to Studio Mac")!;
    window.history.replaceState(null, "", "#/dashboard");
    act(() => connect.action!());
    expect(window.location.hash).toBe(`#/devices/${PEER.id}?tab=connection`);
    await waitFor(() => expect(calls(fake, "POST", `/api/peers/${PEER.id}/connect`)).toHaveLength(1));
  });

  it("a first /api/peers read that failed never says No paired devices: the body names the failure", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], routes: { "GET /api/peers": () => apiError("internal", "store unreadable", 500) } });
    renderDevices();
    expect(await screen.findByText(/^Not read from the service:/)).toBeTruthy();
    expect(screen.queryByText("No paired devices.")).toBeNull();
    expect(screen.queryByRole("grid", { name: "Devices" })).toBeNull();
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });
});

// The Devices-vs-plan check (§2.2): outcomes are seen, actions wait while rows are not live, pairing survives a failed
// read, unknown is never "off", and focus and selection follow what the grid shows.
describe("Devices: outcomes, waiting actions, pairing reads and focus", () => {
  const AWAY: Peer = { ...PEER, connected: false, health: "disconnected" };
  const lastMenu = () => vi.mocked(popupMenu).mock.calls.at(-1)![0] as Extract<MenuItem, { label: string }>[];
  const menuOf = async (name: string) => {
    vi.mocked(popupMenu).mockClear();
    fireEvent.contextMenu(await rowOf(name));
    await waitFor(() => expect(popupMenu).toHaveBeenCalled());
    return lastMenu().filter((i) => i.label).map((i) => [i.label, i.enabled ?? true]);
  };
  const tick = () => act(() => new Promise((r) => setTimeout(r, 30)));

  it("Connect and the menu's Revoke expand a collapsed inspector, so the outcome and the confirmation are seen", async () => {
    const user = userEvent.setup();
    setInspector("devices", { collapsed: true });
    installFakeDaemon({ network: NETWORK_ON, peers: [AWAY] });
    renderDevices(`#/devices/${PEER.id}`);
    await rowOf("Studio Mac");
    expect(within(inspector()).queryByRole("tabpanel")).toBeNull();
    await user.click(await within(toolbar()).findByRole("button", { name: "Connect to Studio Mac" }));
    expect((await within(inspector()).findByRole("alert")).textContent).toContain("The connection to the other device was lost.");
    expect(ui.get().inspectors.devices?.collapsed).toBe(false);

    act(() => setInspector("devices", { collapsed: true }));
    await menuOf("Studio Mac");
    act(() => lastMenu().find((i) => i.label === "Revoke Studio Mac…")!.action!());
    expect(await within(inspector()).findByRole("group", { name: "Confirm revoking Studio Mac" })).toBeTruthy();
  });

  it("while busy, the row menu, the Delete key and a confirm URL all wait; Copy ID does not", async () => {
    const platform = navigator.platform;
    installFakeDaemon({ network: NETWORK_ON, peers: [{ ...AWAY, held: false }] });
    renderDevices(`#/devices/${PEER.id}`);
    await rowOf("Studio Mac");
    const all = (on: boolean) => [
      ["Connect to Studio Mac", on],
      ["Disconnect Studio Mac", false],
      ["Copy ID", true],
      ["Remove Studio Mac…", on],
      ["Revoke Studio Mac…", on],
    ];
    expect(await menuOf("Studio Mac")).toEqual(all(true));
    act(() => daemon.set((d) => ({ ...d, responding: false, at: Date.now() })));
    await screen.findByText(/Service running, busy/);
    expect(await menuOf("Studio Mac")).toEqual(all(false));
    try {
      Object.defineProperty(navigator, "platform", { value: "Win32", configurable: true });
      within(await rowOf("Studio Mac")).getAllByRole("gridcell")[0].focus();
      fireEvent.keyDown(document.activeElement!, { key: "Delete" });
      expect(window.location.hash).not.toContain("confirm=remove");
    } finally {
      Object.defineProperty(navigator, "platform", { value: platform, configurable: true });
    }
    act(() => void (window.location.hash = `#/devices/${PEER.id}?confirm=remove`));
    await tick();
    expect(screen.queryByRole("group", { name: "Confirm removing Studio Mac" })).toBeNull();
  });

  it("a failed pairing status read keeps the window and its code, says so and asks again; past expiry it ends", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices("#/devices?pair=1");
    const d = await screen.findByRole("dialog", { name: "Pair a device" });
    fake.routes["GET /api/pair/status"] = () => apiError("internal", "status unreadable", 500);
    await user.click(await within(d).findByRole("button", { name: "Pair this device" }));
    expect(await within(d).findByText(/^Status not read at \d\d:\d\d:\d\d: Something went wrong inside Common\. Trying again\.$/)).toBeTruthy();
    await tick();
    expect(within(within(d).getByRole("group", { name: "Pairing code" })).getByText("482 913")).toBeTruthy();
    delete fake.routes["GET /api/pair/status"];
    fake.completePairing();
    await within(d).findByText("Paired with Studio Mac.");

    // A window whose reads keep failing past its expiry ends, with the failure.
    fake.routes["GET /api/pair/status"] = () => apiError("internal", "status unreadable", 500);
    act(() => pairing.set({ code: "111222", addr: null, payload: QR_PAYLOAD, expiresAt: Date.now() - 1 }));
    await waitFor(() => expect(pairing.get()).toBeNull());
    expect(within(d).getByRole("alert").textContent).toContain("Something went wrong inside Common.");
  });

  it("the Pair dialog never reads a network state not read yet as off, and nothing PUTs a guess", async () => {
    const fake = installFakeDaemon({ network: NETWORK_ON, routes: { "GET /api/network": ({ init }) => hang(init) } });
    renderDevices("#/devices?pair=1");
    const d = await screen.findByRole("dialog", { name: "Pair a device" });
    expect(within(d).getByText("Reading the network state…")).toBeTruthy();
    expect(within(d).queryByRole("button", { name: "Turn on networking" })).toBeNull();
    expect(await setNetworking({ enabled: true })).toMatchObject({ code: "network_not_read" });
    expect(calls(fake, "PUT", "/api/network")).toHaveLength(0);
  });

  it("jobs and audit are polled fast while the inspector shows Performance or Activity, from the URL or the last tab", async () => {
    setInspector("devices", { tab: "performance" });
    installFakeDaemon(E1);
    renderDevices(`#/devices/${PEER.id}`);
    await waitFor(() => expect(within(inspector()).getByRole("tab", { name: "Performance" }).getAttribute("aria-selected")).toBe("true"));
    expect([isNeeded("jobs"), isNeeded("audit")]).toEqual([true, true]);
    // Collapsed, no tab is shown.
    act(() => setInspector("devices", { collapsed: true }));
    await waitFor(() => expect(isNeeded("jobs")).toBe(false));
  });

  it("a typed address that a revoked device announces asks before pairing it again", async () => {
    const user = userEvent.setup();
    const revoked = "cd".repeat(32);
    const fake = installFakeDaemon({
      network: NETWORK_ON,
      revocations: [{ ...PEERS_E1.revocations[0], peer_id: revoked }],
      discovered: [{ ...STRANGER, id: revoked, addr: "192.168.1.90:8193", pair_port: 51234 }],
    });
    renderDevices("#/devices?pair=join");
    await grid();
    const d = await screen.findByRole("dialog", { name: "Pair a device" });
    await user.type(within(d).getByLabelText("Address"), "192.168.1.90:51234");
    await user.type(within(d).getByLabelText("Code"), JOIN_CODE);
    await user.click(within(d).getByRole("button", { name: "Join" }));
    expect(await within(d).findByRole("group", { name: "Confirm pairing a revoked device" })).toBeTruthy();
    expect(calls(fake, "POST", "/api/pair/join")).toHaveLength(0);
  });

  it("Connect checks ip:port before sending: IPv4 or [IPv6], port 1-65535", () => {
    for (const ok of ["192.168.1.20:8193", " 10.0.0.5:1 ", "[fe80::1]:8193", "[::ffff:10.0.0.5]:65535"]) expect(dialAddrError(ok), ok).toBeNull();
    for (const bad of ["laptop.local", "laptop.local:8193", "10.0.0.300:8193", "10.0.0.5:0", "10.0.0.5:70000", "fe80::1:8193", "10.0.0.5"])
      expect(dialAddrError(bad), bad).toMatchObject({ code: "invalid_request", title: "That is not an address with a port." });
  });

  it("this computer's Connection tab says whether a Windows Firewall rule was found, or that it does not apply", async () => {
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices("#/devices/self?tab=connection");
    const value = async () => (await within(inspector()).findByText("Windows Firewall")).nextElementSibling!.textContent;
    await waitFor(async () => expect(await value()).toBe("Not applicable on this system"));
    act(() => firewall.set({ supported: true, allowed: false, rule_program: null }));
    expect(await value()).toBe("No rule for the service found");
    act(() => firewall.set({ supported: true, allowed: true, rule_program: "common" }));
    expect(await value()).toBe("Rule for the service found");
  });

  it("empty scopes offer the next step: Pair a device… with nothing paired, Turn on networking under Nearby", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK });
    renderDevices();
    const scopes = await screen.findByRole("group", { name: "Show devices" });
    await user.click(within(scopes).getByRole("button", { name: /^Connected/ }));
    const none = (await screen.findByText("No paired devices.")).closest(".empty-state") as HTMLElement;
    await user.click(within(none).getByRole("button", { name: "Pair a device…" }));
    expect(await screen.findByRole("dialog", { name: "Pair a device" })).toBeTruthy();
    await user.click(within(screen.getByRole("dialog", { name: "Pair a device" })).getByRole("button", { name: "Close" }));
    await user.click(within(scopes).getByRole("button", { name: /^Nearby/ }));
    const off = (await screen.findByText("Turn on networking to look for nearby devices.")).closest(".empty-state") as HTMLElement;
    await user.click(within(off).getByRole("button", { name: "Turn on networking" }));
    await waitFor(() => expect(calls(fake, "PUT", "/api/network").map(([, i]) => JSON.parse(String(i!.body)))).toEqual([{ enabled: true, serve_peers: false }]));
  });

  it("Overview: the address a refused dial tried, the last request beside the note, Can run here's reason, model ids", async () => {
    const user = userEvent.setup();
    const refused: Peer = { ...AWAY, last_error: { addr: "10.0.0.5:8193", code: "peer_unauthorized", message: "the device that answered did not prove it is the paired device", hint: null, at_utc: "2026-10-22T10:29:00Z" } };
    const home = [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }];
    // Finished before this connection opened (10:30:00): not this connection's outcome, but still the last answer.
    const earlier = { ts_utc: "2026-10-22T10:29:30Z", kind: "remote_request", peer_id: PEER.id, fields: { role: "client", phase: "finished", outcome: "done" } };
    installFakeDaemon({ network: NETWORK_ON, peers: [refused], discovered: [{ ...STRANGER, id: PEER.id, trusted: true }] });
    const { unmount } = renderDevices(`#/devices/${PEER.id}`);
    expect(await within(inspector()).findByText("Address tried:", { exact: false })).toBeTruthy();
    expect(within(inspector()).getByText("10.0.0.5:8193")).toBeTruthy();
    unmount();

    installFakeDaemon({ ...E1, groups: home, privacy: { ...PRIVACY, mode: "trusted_group" }, audit: [earlier] });
    const second = renderDevices(`#/devices/${PEER.id}`);
    await waitFor(() => expect(within(inspector()).getByText("Its own permissions, serving setting and load are not reported.")).toBeTruthy());
    // The audit log not read yet is not "none tried" (the app reads it every 30 s).
    expect(within(inspector()).getByText("Last request from here: not read yet.")).toBeTruthy();
    await act(() => refresh("audit"));
    expect(within(inspector()).getByText(/^Last request from here: done \(/)).toBeTruthy();
    second.unmount();

    // This computer: the reason behind its word, and model ids.
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices("#/devices/self");
    expect(await within(inspector()).findByText(/The service's status has not been read yet\./)).toBeTruthy();
    await user.click(within(inspector()).getByRole("tab", { name: "Models" }));
    expect(await within(inspector()).findByText("stories15M-q4_0")).toBeTruthy();
  });

  it("the inspector shows only a row the scope shows; a confirmed Remove or Revoke leaves focus in the table", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ ...E1, peers: [PEERS_E1.peers[0], { ...PEERS_E1.peers[1], connected: false }] });
    renderDevices();
    await rowOf("Studio Mac");
    const scopes = screen.getByRole("group", { name: "Show devices" });
    await user.click(within(scopes).getByRole("button", { name: /^Revoked/ }));
    expect(await within(inspector()).findByText("Select a device to see its details.")).toBeTruthy();
    await user.click(within(scopes).getByRole("button", { name: /^All/ }));

    await user.click(await rowOf("Laptop"));
    await user.click(await within(toolbar()).findByRole("button", { name: "Remove Laptop" }));
    await user.click(within(await screen.findByRole("group", { name: "Confirm removing Laptop" })).getByRole("button", { name: "Remove" }));
    await waitFor(async () => expect((await grid()).contains(document.activeElement)).toBe(true));

    await user.click(await rowOf("Studio Mac"));
    await user.click(await within(inspector()).findByRole("tab", { name: "Overview" }));
    await user.click(within(inspector()).getByRole("button", { name: "Revoke Studio Mac" }));
    await user.click(within(await screen.findByRole("group", { name: "Confirm revoking Studio Mac" })).getByRole("button", { name: "Revoke" }));
    await waitFor(async () => expect((await grid()).contains(document.activeElement)).toBe(true));
  });

  it("a failed re-probe is shown above the last report, which keeps its probe time", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices("#/devices/self?tab=hardware");
    await within(inspector()).findByText("Raw JSON");
    fake.routes["GET /api/hardware"] = () => apiError("internal", "probe failed", 500);
    await user.click(within(inspector()).getByRole("button", { name: "Re-probe" }));
    expect((await within(inspector()).findByRole("alert")).textContent).toContain("probe failed");
    expect(within(inspector()).getByText("Raw JSON")).toBeTruthy();
  });
});
