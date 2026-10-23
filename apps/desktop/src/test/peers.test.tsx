// F-10-05: pairing in both directions, honest failures, revocation with confirmation, strangers never actionable.
// Migrated to the Devices section (plan §5.4): same assertions, selectors by role and column header. Deliberate
// wording change: a paired device that is neither connected nor announced nearby reads "Not seen nearby", never
// "Offline" (plan §2.2 ★10).
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { pairingAddr, shortLabel } from "../api/client";
import { bodies, calls, cellText, dialog, grid, inspector, renderDevices, rowOf, selectionActions, toolbar } from "../sections/devices/fixtures";
import { pairPoll } from "../sections/devices/PairDialog";
import { grouped } from "../sections/devices/table";
import { daemon, privacy } from "../state/stores";
import { installFakeDaemon, JOIN_CODE, NETWORK, NETWORK_ON, PAIR_ADDR, PEER, QR_PAYLOAD, SELF_ID, STRANGER } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  privacy.set({ data: null, at: null, error: null });
});
afterEach(() => vi.unstubAllGlobals());

const openPair = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(await within(await screen.findByRole("toolbar", { name: "Devices actions" })).findByRole("button", { name: "Pair a device…" }));
  return screen.findByRole("dialog", { name: "Pair a device" });
};
const openJoin = async (user: ReturnType<typeof userEvent.setup>) => {
  const d = await openPair(user);
  await user.click(within(d).getByRole("tab", { name: "Enter a code" }));
  return d;
};

describe("grouped", () => {
  it("shows a 64-hex id as 8 groups of 8", () => {
    expect(grouped(SELF_ID).split(" ")).toHaveLength(8);
    expect(grouped(SELF_ID).replace(/ /g, "")).toBe(SELF_ID);
  });
});

describe("Devices: pairing and trust", () => {
  it("starts with networking off and explains it; pairing is unavailable until it is on", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    renderDevices();
    const sw = await screen.findByRole("switch", { name: "Networking" });
    expect(sw).toHaveProperty("checked", false);
    expect(screen.getByText(/Off by default\. When on, Common listens/)).toBeTruthy();
    // This computer is preselected: its identity is in the inspector.
    await waitFor(() => expect(within(inspector()).getByText(NETWORK.short_label)).toBeTruthy());
    expect(within(inspector()).getByText(grouped(SELF_ID))).toBeTruthy();
    // The dialog opens, but its first step is turning networking on: no code can be shown yet.
    const d = await openPair(user);
    expect(within(d).getByRole("button", { name: "Turn on networking" })).toBeTruthy();
    expect(within(d).queryByRole("button", { name: "Pair this device" })).toBeNull();
    expect(calls(fake, "PUT", "/api/network")).toHaveLength(0);
  });

  it("pairs this device: start -> code shown -> status polled -> paired list updated", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    renderDevices();

    await user.click(await screen.findByRole("switch", { name: "Networking" }));
    await waitFor(() => expect(fake.network.enabled).toBe(true));
    const d = await openPair(user);
    await user.click(await within(d).findByRole("button", { name: "Pair this device" }));

    // The six digits are shown large and spaced; the QR payload is available as text.
    const group = await screen.findByRole("group", { name: "Pairing code" });
    expect(within(group).getByText("482 913")).toBeTruthy();
    expect(within(group).getByText(/Expires in \d+ s/)).toBeTruthy();
    await user.click(within(group).getByText("Show as text for a QR app"));
    expect(within(group).getByText(QR_PAYLOAD)).toBeTruthy();
    expect(within(group).getByRole("status").textContent).toContain("Waiting for the other device");

    // The other device confirms; polling notices and the list updates.
    await waitFor(() => expect(calls(fake, "GET", "/api/pair/status").length).toBeGreaterThan(0));
    fake.completePairing();
    await within(d).findByText("Paired with Studio Mac.");
    expect(screen.queryByRole("group", { name: "Pairing code" })).toBeNull();
    await user.click(within(d).getByRole("button", { name: "Close" }));
    await rowOf("Studio Mac");
    // Pairing opens no workload connection; the fake does not announce the device on this network either.
    expect(await cellText("Studio Mac", "State")).toBe("Not seen nearby");
  });

  it("a window the other device fails closes with the daemon's typed error", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    const d = await openPair(user);
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    fake.failPairing();
    const alert = await within(d).findByRole("alert");
    expect(alert.textContent).toContain("Pairing did not complete.");
    expect(screen.queryByRole("group", { name: "Pairing code" })).toBeNull();
    expect(fake.peers).toHaveLength(0);
    // A new window can be opened straight away; the stale result is not mistaken for this one.
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
  });

  it("join with a wrong code shows the pairing_failed explanation; a locked peer shows pairing_locked", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    const d = await openJoin(user);

    await user.type(within(d).getByLabelText("Address"), "192.168.1.77:51234");
    await user.type(within(d).getByLabelText("Code"), "000 000");
    await user.click(within(d).getByRole("button", { name: "Join" }));
    const alert = await within(d).findByRole("alert");
    expect(alert.textContent).toContain("Pairing did not complete.");
    expect(alert.textContent).toContain("Check the code and that both devices are on the same network");
    expect(alert.textContent).toContain("pairing_failed");
    expect(fake.peers).toHaveLength(0);

    fake.pairLocked = true;
    await user.click(within(d).getByRole("button", { name: "Join" }));
    await waitFor(() => expect(within(d).getByRole("alert").textContent).toContain("Too many failed pairing attempts."));
    expect(within(d).getByRole("alert").textContent).toContain("Wait a minute, then start pairing again with a fresh code.");

    fake.pairLocked = false;
    await user.clear(within(d).getByLabelText("Code"));
    await user.type(within(d).getByLabelText("Code"), JOIN_CODE);
    await user.click(within(d).getByRole("button", { name: "Join" }));
    await within(d).findByText("Paired with Studio Mac.");
    expect(fake.peers.map((p) => p.id)).toEqual([PEER.id]);
  });

  it("join accepts the pasted QR text without an address", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    const d = await openJoin(user);
    const join = within(d).getByRole("button", { name: "Join" });
    expect(join).toHaveProperty("disabled", true);
    await user.click(within(d).getByLabelText("Code"));
    await user.paste(QR_PAYLOAD);
    expect(join).toHaveProperty("disabled", false);
    await user.click(join);
    await within(d).findByText("Paired with Studio Mac.");
    // No name set: the daemon announces this computer's own name, so none is sent.
    expect(bodies(fake, "POST", "/api/pair/join")).toEqual([{ code: QR_PAYLOAD }]);
  });

  it("lists strangers under Nearby with Pair as the only action, which prefills the join form", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, discovered: [STRANGER, { ...STRANGER, id: "ab".repeat(32), addr: "192.168.1.78:1", pairing: false, pair_port: null }] });
    renderDevices();
    await user.click(within(await screen.findByRole("group", { name: "Show devices" })).getByRole("button", { name: /^Nearby/ }));
    await user.click(await rowOf(shortLabel(STRANGER.id)));
    await waitFor(() => expect(selectionActions()).toEqual([`Pair with ${shortLabel(STRANGER.id)}`]));
    // A device not announcing a pairing window: Pair is still offered (plan §2.2, any nearby row: the announcement
    // can lag a window just opened there), and the inspector says what was announced.
    await user.click(await rowOf("abab-abab-abab"));
    const idle = await within(toolbar()).findByRole("button", { name: "Pair with abab-abab-abab" });
    expect(idle.getAttribute("aria-disabled")).toBeNull();
    expect(selectionActions()).toEqual(["Pair with abab-abab-abab"]);
    expect(within(inspector()).getByText("Not in pairing mode")).toBeTruthy();

    // The join form gets the pairing window's address (host + pair_port), not the workload port.
    await user.click(await rowOf(shortLabel(STRANGER.id)));
    await user.click(within(toolbar()).getByRole("button", { name: `Pair with ${shortLabel(STRANGER.id)}` }));
    expect(pairingAddr(STRANGER)).toBe("192.168.1.77:51234");
    const d = await screen.findByRole("dialog", { name: "Pair a device" });
    expect(within(d).getByRole("tab", { name: "Enter a code" }).getAttribute("aria-selected")).toBe("true");
    expect(within(d).getByLabelText("Address")).toHaveProperty("value", "192.168.1.77:51234");
    await waitFor(() => expect(document.activeElement).toBe(within(d).getByLabelText("Code")));
  });

  it("the address shown under the code goes into the other side's join form verbatim; the code pastes with its space", async () => {
    // One jsdom page and one fake daemon play both sides; this checks the form, not two devices.
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    const d = await openPair(user);
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    const group = await screen.findByRole("group", { name: "Pairing code" });
    const shown = within(group).getByText(PAIR_ADDR).textContent!;
    expect(shown).toBe(PAIR_ADDR);

    await user.click(within(d).getByRole("tab", { name: "Enter a code" }));
    await user.click(within(d).getByLabelText("Address"));
    await user.paste(shown);
    await user.click(within(d).getByLabelText("Code"));
    await user.paste(" 123 456 ");
    await user.click(within(d).getByRole("button", { name: "Join" }));
    await within(d).findByText("Paired with Studio Mac.");
    expect(bodies(fake, "POST", "/api/pair/join")).toEqual([{ addr: PAIR_ADDR, code: JOIN_CODE }]);
  });

  it("an older daemon without pair_addr still shows the port", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    const start = fake.fetchMock.getMockImplementation() as typeof fetch;
    fake.fetchMock.mockImplementation(async (input: RequestInfo | URL, init?: RequestInit) => {
      const res = await start(input, init);
      if (!String(input).endsWith("/api/pair/start")) return res;
      const { pair_addr: _, ...old } = await res.json();
      return new Response(JSON.stringify(old), { headers: { "content-type": "application/json" } });
    });
    renderDevices();
    const d = await openPair(user);
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    const group = await screen.findByRole("group", { name: "Pairing code" });
    expect(group.textContent).toContain("Port 51234.");
  });

  it("closing the dialog keeps the window: reopening shows the same code and countdown with no second pair/start", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    let d = await openPair(user);
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    await within(d).findByText("482 913");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Pair a device" })).toBeNull());
    expect(window.location.hash).toBe("#/devices");
    // The toolbar says the window is still open, with its countdown, and reopens it. The countdown is not part of the
    // name: focus returns here on Esc, and a name that changes every second would be announced again and again.
    const resume = await within(toolbar()).findByRole("button", { name: "Pairing open" });
    expect(resume.textContent).toMatch(/^Pairing open · \d:\d\d$/);
    await user.click(resume);
    d = await screen.findByRole("dialog", { name: "Pair a device" });
    expect(within(within(d).getByRole("group", { name: "Pairing code" })).getByText("482 913")).toBeTruthy();
    expect(calls(fake, "POST", "/api/pair/start")).toHaveLength(1);
    // The status bar's "Resume pairing" lands on #/devices?pair=1 and opens the same window.
    await user.click(within(d).getByRole("button", { name: "Close" }));
    window.location.hash = "#/devices?pair=1";
    expect(await within(await screen.findByRole("dialog", { name: "Pair a device" })).findByText("482 913")).toBeTruthy();
    expect(calls(fake, "POST", "/api/pair/start")).toHaveLength(1);
  });

  it("shows heartbeat health in plain words and falls back to connected without it", async () => {
    const peer = (n: number, extra: Partial<typeof PEER>) => ({ ...PEER, id: String(n).repeat(64), short_label: `${n}${n}${n}${n}-x`, name: `Device ${n}`, ...extra });
    installFakeDaemon({
      network: NETWORK_ON,
      peers: [
        peer(1, { health: "connected" }),
        peer(2, { health: "unhealthy" }),
        peer(3, { health: "reconnecting", connected: false }),
        peer(4, { health: "disconnected", connected: false }),
        peer(5, { connected: false }),
      ],
    });
    renderDevices();
    expect(await cellText("Device 1", "State")).toBe("Connected");
    expect(await cellText("Device 2", "State")).toBe("Not responding");
    expect(await cellText("Device 3", "State")).toBe("Reconnecting");
    // Not connected and not announcing itself on this network: never "Offline".
    expect(await cellText("Device 4", "State")).toBe("Not seen nearby");
    expect(await cellText("Device 5", "State")).toBe("Not seen nearby");
    expect((await grid()).textContent).not.toMatch(/Offline/);
  });

  it("never lists this device among nearby devices, even when the daemon reports its own advert", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, discovered: [{ ...STRANGER, id: SELF_ID, addr: "192.168.1.10:8193" }, STRANGER] });
    renderDevices();
    const scopes = await screen.findByRole("group", { name: "Show devices" });
    await user.click(within(scopes).getByRole("button", { name: /^Nearby/ }));
    expect(within(scopes).getByRole("button", { name: /^Nearby/ }).textContent).toBe("Nearby 1");
    await rowOf(shortLabel(STRANGER.id));
    expect(within(await grid()).queryByRole("row", { name: (n) => n.includes(shortLabel(SELF_ID)) })).toBeNull();
    expect(within(await grid()).getAllByRole("row")).toHaveLength(2);
  });

  it("revoke asks for confirmation and then removes the device; remove does too; Revoke is never in the toolbar", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER, { ...PEER, id: "cd".repeat(32), short_label: "cdcd-cdcd-cdcd", name: "Old laptop", connected: false }] });
    renderDevices();
    // Without heartbeat health the daemon's `connected` decides the column.
    expect(await cellText("Studio Mac", "State")).toBe("Connected");
    expect(await cellText("Old laptop", "State")).toBe("Not seen nearby");

    await user.click(await rowOf("Studio Mac"));
    await waitFor(() => expect(selectionActions()).toEqual(["Connect to Studio Mac", "Remove Studio Mac"]));
    await user.click(within(inspector()).getByRole("button", { name: "Revoke Studio Mac" }));
    expect(fake.peers).toHaveLength(2);
    const confirm = await screen.findByRole("group", { name: "Confirm revoking Studio Mac" });
    expect(confirm.textContent).toContain("cannot reconnect until it is paired again");
    expect(document.activeElement?.textContent).toBe("Cancel");
    await user.click(within(confirm).getByRole("button", { name: "Revoke" }));
    await waitFor(async () => expect(within(await grid()).queryByRole("row", { name: (n) => n.includes("Studio Mac") })).toBeNull());
    expect(calls(fake, "POST", `/api/peers/${PEER.id}/revoke`)).toHaveLength(1);

    await user.click(await rowOf("Old laptop"));
    await user.click(await within(toolbar()).findByRole("button", { name: "Remove Old laptop" }));
    const c2 = await screen.findByRole("group", { name: "Confirm removing Old laptop" });
    expect(c2.textContent).toContain("you can pair it again later");
    expect(document.activeElement?.textContent).toBe("Cancel");
    await user.click(within(c2).getByRole("button", { name: "Cancel" }));
    expect(await rowOf("Old laptop")).toBeTruthy();
    // Focus returns to the button that opened the confirmation.
    await waitFor(() => expect(document.activeElement).toBe(within(toolbar()).getByRole("button", { name: "Remove Old laptop" })));
    expect(fake.peers).toHaveLength(1);

    // Delete (Windows) opens the same confirmation; Delete then Enter cancels (focus is on Cancel); confirming removes it.
    Object.defineProperty(navigator, "platform", { value: "Win32", configurable: true });
    within(await rowOf("Old laptop")).getAllByRole("gridcell")[0].focus();
    await user.keyboard("{Delete}");
    await screen.findByRole("group", { name: "Confirm removing Old laptop" });
    await user.keyboard("{Enter}");
    await waitFor(() => expect(screen.queryByRole("group", { name: "Confirm removing Old laptop" })).toBeNull());
    expect(fake.peers).toHaveLength(1);
    within(await rowOf("Old laptop")).getAllByRole("gridcell")[0].focus();
    await user.keyboard("{Delete}");
    await user.click(within(await screen.findByRole("group", { name: "Confirm removing Old laptop" })).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(fake.peers).toHaveLength(0));
    expect(calls(fake, "DELETE", `/api/peers/${"cd".repeat(32)}`)).toHaveLength(1);
  });

  it("edits a device's groups and their prompt switch through PUT /api/groups, one group per request", async () => {
    const user = userEvent.setup();
    const work = { name: "work", members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } };
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: [work, { ...work, name: "home" }] });
    renderDevices(`#/devices/${PEER.id}?tab=permissions`);
    await user.click(await within(inspector()).findByRole("checkbox", { name: "In home" }));
    await waitFor(() => expect(fake.groups[1].members).toEqual([PEER.id]));
    expect(bodies(fake, "PUT", "/api/groups")[0]).toEqual({ name: "home", members: [PEER.id], policy: { allow_remote_inference: false, allow_pooled_inference: false } });
    await user.click(await within(inspector()).findByRole("checkbox", { name: "Members of home may run prompts for each other" }));
    await waitFor(() => expect(fake.groups[1].policy.allow_remote_inference).toBe(true));
    expect(fake.groups[0]).toEqual(work);
    expect(bodies(fake, "PUT", "/api/groups").map((g) => g.name)).toEqual(["home", "home"]);
  });

  it("keyboard only: turn networking on, start pairing, see the code, see the paired device", async () => {
    pairPoll.ms = 20;
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    const tabTo = async (el: Element) => {
      for (let i = 0; i < 60 && document.activeElement !== el; i++) await user.tab();
      expect(document.activeElement).toBe(el);
    };
    window.location.hash = "#/peers";
    render(<App poll={false} />);
    await screen.findByRole("heading", { level: 1, name: "Devices" });

    const sw = await screen.findByRole("switch", { name: "Networking" });
    await waitFor(() => expect(sw).toHaveProperty("disabled", false));
    await tabTo(sw);
    await user.keyboard(" ");
    await waitFor(() => expect(fake.network.enabled).toBe(true));
    await tabTo(within(toolbar()).getByRole("button", { name: "Pair a device…" }));
    await user.keyboard("{Enter}");
    // The safe primary control has focus when the dialog opens.
    await waitFor(() => expect(document.activeElement).toBe(within(dialog()).getByRole("button", { name: "Pair this device" })));
    await user.keyboard("{Enter}");
    const group = await screen.findByRole("group", { name: "Pairing code" });
    expect(within(group).getByText("482 913")).toBeTruthy();
    fake.completePairing();
    await within(dialog()).findByText("Paired with Studio Mac.", {}, { timeout: 4000 });
    await user.keyboard("{Escape}");
    await rowOf("Studio Mac");
  }, 15000);
});
