// Defects seen with the installed apps on a Mac and a Windows laptop (docs/evidence/desktop-two-machine-2026-10-19.md)
// and the Devices section's discovery and connection state. Unit level against the fake daemon: no test here claims
// two devices; the physical two-machine checks are the integrator's. Migrated from the Peers view (plan §5.4): same
// assertions, selectors by role and column header. Deliberate changes: "Offline" reads "Not seen nearby"; a device
// whose last dial found nothing and that is not announced nearby reads "Not seen nearby" with the reason in its
// inspector (plan §2.2 ★7); Connect dials from the toolbar and the optional address lives in the Connection tab;
// the device name is edited in Settings › Network (D), so its field is no longer asserted here.
import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { bodies, cellText, inspector, renderDevices, rowOf, selectionActions, toolbar } from "../sections/devices/fixtures";
import { setTable } from "../state/ui";
import { chatHistory, daemon, lastError, privacy, runTarget, settings } from "../state/stores";
import { installFakeDaemon, NETWORK, NETWORK_ON, PEER, PRIVACY, STRANGER } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  chatHistory.set({ messages: [] });
  lastError.set(null);
  privacy.set(null);
  runTarget.set("local");
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
});
afterEach(() => vi.unstubAllGlobals());

const TRUSTED = { ...PRIVACY, mode: "trusted_group" as const };
const posts = (fake: ReturnType<typeof installFakeDaemon>, path: string) => bodies(fake, "POST", path);
const tab = async (user: ReturnType<typeof userEvent.setup>, name: string) => user.click(await within(inspector()).findByRole("tab", { name }));
const pick = async (user: ReturnType<typeof userEvent.setup>, name: string) => user.click(await rowOf(name));

describe("E1: no macOS wording on other systems", () => {
  it("pairs under the daemon's device name unless the user set one", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    // This computer is listed under the daemon's own name, and the dialog says which name other devices will see.
    expect(await cellText(NETWORK.device_name!, "Name")).toBe(`${NETWORK.device_name}This computer`);
    await user.click(within(toolbar()).getByRole("button", { name: "Pair a device…" }));
    const d = await screen.findByRole("dialog", { name: "Pair a device" });
    expect(d.textContent).toContain(`Other devices will see this computer as ${NETWORK.device_name}`);

    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    expect(posts(fake, "/api/pair/start")).toEqual([{}]);

    fake.completePairing();
    await within(d).findByText("Paired with Studio Mac.");
    // A name chosen in Settings › Network (the same store) is what the next window announces.
    act(() => settings.set({ ...settings.get(), device_name: "Kitchen PC" }));
    expect(await cellText("Kitchen PC", "Name")).toBe("Kitchen PCThis computer");
    expect(d.textContent).toContain("Other devices will see this computer as Kitchen PC");
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    await waitFor(() => expect(posts(fake, "/api/pair/start")).toEqual([{}, { name: "Kitchen PC" }]));
  });

  it("forgets the old hard-coded default but keeps a name the user chose", async () => {
    for (const [stored, expected] of [
      ["This Mac", ""],
      ["Kitchen PC", "Kitchen PC"],
    ]) {
      vi.resetModules();
      localStorage.setItem("common.settings", JSON.stringify({ device: "gpu", n_ctx: 4096, device_name: stored }));
      const fresh = await import("../state/stores");
      expect(fresh.settings.get()).toEqual({ device: "gpu", n_ctx: 4096, device_name: expected });
    }
  });
});

describe("Devices: discovery and connection state", () => {
  const OLD = { ...PEER, id: "cd".repeat(32), short_label: "cdcd-cdcd-cdcd", name: "Old laptop", connected: false, health: "disconnected" as const, last_addr: null };
  const AWAY = {
    ...PEER,
    id: "ee".repeat(32),
    short_label: "eeee-eeee-eeee",
    name: "Laptop",
    connected: false,
    health: "disconnected" as const,
    last_addr: "172.20.10.6:8193",
    last_error: { addr: "172.20.10.6:8193", code: "peer_lost", message: "no answer from 172.20.10.6:8193 within 4 s", hint: "the device may be asleep or on another network, or a firewall may block UDP port 8193", at_utc: "2026-10-19T03:40:00Z" },
  };
  const REASON = /^Last try at .+: No answer from 172\.20\.10\.6:8193 within 4 s\. The device may be asleep or on another network, or a firewall may block UDP port 8193\.$/;

  it("shows where each paired device is: seen nearby, last known, or not known; and why it is not connected", async () => {
    const user = userEvent.setup();
    setTable("devices", { columns: { addr: true } });
    installFakeDaemon({
      network: NETWORK_ON,
      peers: [{ ...PEER, health: "connected" }, AWAY, OLD],
      discovered: [{ ...STRANGER, id: PEER.id, trusted: true, pairing: false, addr: "192.168.1.77:8193", addrs: ["192.168.1.77:8193", "10.0.0.9:8193"] }],
    });
    renderDevices();
    expect(await cellText("Studio Mac", "Address")).toBe("192.168.1.77:8193seen nearby");
    expect(await cellText("Laptop", "Address")).toBe("172.20.10.6:8193last known");
    expect(await cellText("Old laptop", "Address")).toBe("—not known");
    // Every advertised address is in the Connection tab.
    await pick(user, "Studio Mac");
    await tab(user, "Connection");
    expect(within(inspector()).getByText(/^192\.168\.1\.77:8193, 10\.0\.0\.9:8193/)).toBeTruthy();
    // The reason and what to check, only while not connected.
    expect(await cellText("Laptop", "State")).toBe("Not seen nearby");
    await pick(user, "Laptop");
    await tab(user, "Overview");
    expect(within(inspector()).getByText(REASON)).toBeTruthy();
    expect(await cellText("Studio Mac", "State")).toBe("Connected");
    // Paired devices are not offered for pairing among nearby strangers.
    expect(within(screen.getByRole("group", { name: "Show devices" })).getByRole("button", { name: /^Nearby/ }).textContent).toBe("Nearby 0");
    expect(selectionActions().some((a) => a.startsWith("Pair with"))).toBe(false);
    // This computer's own address, for the other device to type.
    expect(await cellText(NETWORK.device_name!, "Address")).toBe("192.168.1.10:8193this computer");
  });

  it("reconnects by address after the device moved networks; the typed address is only a hint, the key decides", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [AWAY], answersAt: { [AWAY.id]: "10.12.0.44:8193" }, impostors: ["10.12.0.45:8193"] });
    renderDevices();
    await pick(user, "Laptop");
    // Connected devices and strangers get no enabled Connect; this one does.
    await user.click(await within(toolbar()).findByRole("button", { name: "Connect to Laptop" }));
    // Connect shows the device's Connection tab, where the outcome and the address field are.
    const panel = await screen.findByRole("form", { name: "Connect to Laptop" });
    const addr = within(panel).getByLabelText("Address (optional)");

    // 1. Retry with what the daemon knows: the old address does not answer.
    expect((await within(panel).findByRole("alert")).textContent).toContain("The connection to the other device was lost.");
    expect(within(panel).getByRole("alert").textContent).toContain("firewall may block UDP port 8193");

    // 2. Another Common device answers at the typed address: refused, nothing changes.
    await user.type(addr, "10.12.0.45:8193{Enter}");
    // Never "not authorized, pair it": the address proved nothing, and the device itself may be fine.
    await waitFor(() =>
      expect(within(panel).getByRole("alert").textContent).toContain(
        "The device that answered did not prove it is Laptop, or Laptop no longer trusts this computer.Nothing was trusted or saved. Check the address shown on Laptop under Devices › This computer › Connection.peer_unauthorized: the device at 10.12.0.45:8193 did not prove it is",
      ),
    );
    expect(fake.peers[0]).toMatchObject({ connected: false, last_addr: "172.20.10.6:8193" });

    // 3. The device's new address: connected, and that address becomes the pinned hint.
    await user.clear(addr);
    await user.type(addr, "10.12.0.44:8193{Enter}");
    await screen.findByText("Connected to Laptop.");
    expect(screen.queryByRole("form", { name: "Connect to Laptop" })).toBeNull();
    // The form and its Connect button are gone; focus lands on the outcome, not the top of the page.
    await waitFor(() => expect(document.activeElement?.textContent).toBe("Connected to Laptop."));
    expect(await cellText("Laptop", "State")).toBe("Connected");
    expect(within(inspector()).getByText("10.12.0.44:8193", { exact: false })).toBeTruthy();
    expect(posts(fake, `/api/peers/${AWAY.id}/connect`)).toEqual([{}, { addr: "10.12.0.45:8193" }, { addr: "10.12.0.44:8193" }]);
    // Connected: Connect stays visible but is not possible, with the reason.
    expect(within(toolbar()).getByRole("button", { name: "Connect to Laptop" }).getAttribute("aria-disabled")).toBe("true");
  });

  it("names each state the daemon can back; a cached nearby record never makes a device connected or ready", async () => {
    const user = userEvent.setup();
    const dev = (n: number, extra: Partial<typeof PEER>) => ({ ...PEER, id: String(n).repeat(64), short_label: `${n}${n}${n}${n}-x`, name: `Device ${n}`, last_addr: null, ...extra });
    const near = (n: number) => ({ ...STRANGER, id: String(n).repeat(64), trusted: true, pairing: false, addrs: [STRANGER.addr] });
    // Through the service: Devices reads /api/privacy on mount, so a store set here would be overwritten.
    installFakeDaemon({
      network: NETWORK_ON,
      privacy: TRUSTED,
      peers: [
        dev(1, { connected: true, health: "connected" }),
        dev(2, { connected: true, health: "unhealthy" }),
        dev(3, { connected: false, health: "reconnecting" }),
        dev(4, { connected: false, health: "disconnected" }),
        dev(5, { connected: false }),
        dev(6, { connected: false, health: "disconnected", last_error: AWAY.last_error }),
        // Lost, and the daemon's redial since then failed: health keeps saying reconnecting until one succeeds.
        dev(7, { connected: false, health: "reconnecting", last_error: AWAY.last_error }),
      ],
      // 4 and 6 announce themselves nearby (signed records the daemon may still hold after they left); so does a stranger.
      discovered: [near(4), near(6), STRANGER],
      groups: [
        { name: "home", members: ["1", "4", "6"].map((n) => n.repeat(64)), policy: { allow_remote_inference: true, allow_pooled_inference: false } },
        { name: "work", members: ["2".repeat(64)], policy: { allow_remote_inference: false, allow_pooled_inference: false } },
      ],
    });
    renderDevices();
    const state = (n: number) => cellText(`Device ${n}`, "State");
    const run = (n: number) => cellText(`Device ${n}`, "Can run there");
    /** The readiness sentence in the device's Overview. */
    const prompts = async (n: number) => {
      await pick(user, `Device ${n}`);
      return (await within(inspector()).findByText(/^(Can|Cannot) run prompts from this computer/)).textContent;
    };
    expect(await state(1)).toBe("Connected");
    expect(await run(1)).toBe("Can sendIts own permissions, serving setting and load are not reported.");
    expect(await prompts(1)).toBe("Can run prompts from this computer.");
    expect(await state(2)).toBe("Not responding");
    // Not responding is not live: nothing can be sent there now.
    expect(await run(2)).toBe("—");
    expect(await prompts(2)).toBe('Cannot run prompts from this computer yet: turn on "Members may run prompts for each other" in its group.');
    expect(await state(3)).toBe("Reconnecting");
    expect(await prompts(3)).toBe("Cannot run prompts from this computer yet: connect it, add it to a group that allows prompts.");
    // Seen nearby and in a group that allows prompts, but no open connection: neither connected nor ready.
    expect(await state(4)).toBe("Not connected");
    expect(await run(4)).toBe("—");
    expect(await prompts(4)).toBe("Cannot run prompts from this computer yet: connect it.");
    expect(await state(5)).toBe("Not seen nearby");
    // Seen nearby, yet the dial failed (a firewall, say): the failure wins, with its reason.
    expect(await state(6)).toBe("Unreachable");
    expect(await prompts(6)).toBe("Cannot run prompts from this computer yet: connect it.");
    expect(within(inspector()).getAllByText(REASON).length).toBeGreaterThan(0);
    // Not announced and the last dial found nothing: not seen nearby, with the reason (never "Unreachable" there).
    expect(await state(7)).toBe("Not seen nearby");
    // Discovered, not paired: listed under Nearby, untrusted, never as a paired device.
    await user.click(within(screen.getByRole("group", { name: "Show devices" })).getByRole("button", { name: /^Nearby/ }));
    const stranger = await rowOf(STRANGER.id.slice(0, 4));
    expect(stranger.textContent).toContain("Untrusted (nearby)");
    expect(stranger.textContent).toContain("Not paired");
    expect(within(stranger.closest("table")!).queryByRole("row", { name: (n) => n.includes("Device 4") })).toBeNull();
  });

  it("calls a device not seen nearby only where this computer looks for devices, and says when Privacy blocks prompts", async () => {
    const user = userEvent.setup();
    installFakeDaemon({
      network: { ...NETWORK_ON, mdns: false },
      peers: [{ ...PEER, connected: false, last_addr: "192.168.1.77:8193" }],
      groups: [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }],
    });
    renderDevices();
    expect(await cellText("Studio Mac", "State")).toBe("Not connected");
    await pick(user, "Studio Mac");
    expect(await within(inspector()).findByText("Cannot run prompts from this computer yet: connect it, allow trusted devices under Privacy.")).toBeTruthy();
    expect(within(inspector()).getByText("home")).toBeTruthy();
  });

  it("a malformed address is an invalid_request, shown in the Connection tab; it is checked here, so nothing is sent", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [AWAY] });
    renderDevices(`#/devices/${AWAY.id}?tab=connection`);
    const field = await screen.findByLabelText("Address (optional)");
    await user.type(field, "laptop.local{Enter}");
    expect((await screen.findByRole("alert")).textContent).toContain("invalid_request");
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(posts(fake, `/api/peers/${AWAY.id}/connect`)).toEqual([]);
  });

  it("offers Connect only while networking is on", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK, peers: [AWAY] });
    renderDevices();
    await pick(user, "Laptop");
    const connect = await within(toolbar()).findByRole("button", { name: "Connect to Laptop" });
    expect(connect.getAttribute("aria-disabled")).toBe("true");
    await user.click(connect);
    await tab(user, "Connection");
    expect(screen.queryByRole("form", { name: "Connect to Laptop" })).toBeNull();
    expect(within(inspector()).getByText("Networking is off: turn it on to connect.")).toBeTruthy();
  });

  it("says why no device shows up nearby: discovery off here, or a network that blocks it", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK_ON, mdns: false } });
    const nearby = async () => user.click(within(await screen.findByRole("group", { name: "Show devices" })).getByRole("button", { name: /^Nearby/ }));
    const { unmount } = renderDevices();
    await nearby();
    expect(await screen.findByText(/This computer is not looking for devices on the network\. Pair by address instead/)).toBeTruthy();
    unmount();
    fake.network = NETWORK_ON;
    renderDevices();
    await nearby();
    expect(await screen.findByText(/campus and office Wi-Fi networks, stop devices from finding each other; pair by address instead/)).toBeTruthy();
  });

  it("the keyboard reaches Connect: arrow to the row, then the toolbar", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [AWAY] });
    renderDevices();
    const self = await rowOf(NETWORK.device_name!);
    within(self).getAllByRole("gridcell")[0].focus();
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect((document.activeElement as HTMLElement).closest("tr")?.textContent).toContain("Laptop"));
    const btn = await within(toolbar()).findByRole("button", { name: "Connect to Laptop" });
    // The toolbar is one Tab stop; arrow keys move between its buttons.
    for (let i = 0; i < 15 && !toolbar().contains(document.activeElement); i++) await user.tab({ shift: true });
    for (let i = 0; i < 5 && document.activeElement !== btn; i++) await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(btn);
    await user.keyboard("{Enter}");
    await waitFor(() => expect(posts(fake, `/api/peers/${AWAY.id}/connect`)).toEqual([{}]));
  });

  it("while one device is connecting, the others' Connect waits, so neither outcome is lost", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [AWAY, OLD] });
    // Hold the dial, as a silent address does for up to 4 s per try.
    const real = fake.fetchMock.getMockImplementation() as typeof fetch;
    let release = () => {};
    const held = new Promise<void>((r) => (release = r));
    fake.fetchMock.mockImplementation(async (u: RequestInfo | URL, init?: RequestInit) => {
      if (String(u).endsWith("/connect")) await held;
      return real(u, init);
    });
    renderDevices();
    await pick(user, "Laptop");
    await user.click(await within(toolbar()).findByRole("button", { name: "Connect to Laptop" }));
    await within(await screen.findByRole("form", { name: "Connect to Laptop" })).findByRole("button", { name: "Connecting" });
    expect(await cellText("Laptop", "State")).toBe("Connecting");
    await pick(user, "Old laptop");
    expect((await within(toolbar()).findByRole("button", { name: "Connect to Old laptop" })).getAttribute("aria-disabled")).toBe("true");
    release();
    await waitFor(async () => expect(await cellText("Laptop", "State")).toBe("Not seen nearby"));
    // The other device can be tried now; Laptop's failure is still shown in its own Connection tab.
    await waitFor(() => expect(within(toolbar()).getByRole("button", { name: "Connect to Old laptop" }).getAttribute("aria-disabled")).toBeNull());
    await pick(user, "Laptop");
    await tab(user, "Connection");
    const panel = screen.getByRole("form", { name: "Connect to Laptop" });
    expect((await within(panel).findByRole("alert")).textContent).toContain("The connection to the other device was lost.");
  });
});
