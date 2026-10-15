// F-10-05: pairing in both directions, honest failures, revocation with confirmation, strangers never actionable.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { daemon, privacy } from "../state/stores";
import { pairingAddr, shortLabel } from "../api/client";
import { grouped, Peers } from "../views/Peers";
import { installFakeDaemon, JOIN_CODE, NETWORK, NETWORK_ON, PAIR_ADDR, PEER, QR_PAYLOAD, SELF_ID, STRANGER } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  privacy.set(null);
});
afterEach(() => vi.unstubAllGlobals());

const calls = (fake: ReturnType<typeof installFakeDaemon>, method: string, path: string) =>
  fake.fetchMock.mock.calls.filter(([u, init]) => String(u).endsWith(path) && ((init as RequestInit)?.method ?? "GET") === method);

describe("grouped", () => {
  it("shows a 64-hex id as 8 groups of 8", () => {
    expect(grouped(SELF_ID).split(" ")).toHaveLength(8);
    expect(grouped(SELF_ID).replace(/ /g, "")).toBe(SELF_ID);
  });
});

describe("Peers view", () => {
  it("starts with networking off and explains it; pairing is unavailable until it is on", async () => {
    const fake = installFakeDaemon();
    render(<Peers pollMs={20} />);
    const sw = await screen.findByRole("switch", { name: /Let this Mac talk to paired devices/ });
    expect(sw).toHaveProperty("checked", false);
    expect(screen.getByText(/Off by default\. When on, Common listens/)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Pair this device" })).toHaveProperty("disabled", true);
    expect(screen.getByText(NETWORK.short_label)).toBeTruthy();
    expect(screen.getByText(grouped(SELF_ID))).toBeTruthy();
    expect(calls(fake, "PUT", "/api/network")).toHaveLength(0);
  });

  it("pairs this device: start -> code shown -> status polled -> paired list updated", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    render(<Peers pollMs={20} />);

    await user.click(await screen.findByRole("switch", { name: /Let this Mac talk/ }));
    expect(fake.network.enabled).toBe(true);
    const pair = await screen.findByRole("button", { name: "Pair this device" });
    await waitFor(() => expect(pair).toHaveProperty("disabled", false));
    await user.click(pair);

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
    await screen.findByText("Paired with Studio Mac.");
    expect(screen.queryByRole("group", { name: "Pairing code" })).toBeNull();
    await screen.findByText("Studio Mac", { selector: ".model-name" });
    // Pairing opens no workload connection; the daemon connects when a request first targets the device.
    expect(screen.getByText("Not connected")).toBeTruthy();
  });

  it("a window the other device fails closes with the daemon's typed error", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={20} />);
    await user.click(await screen.findByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    fake.failPairing();
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Pairing did not complete.");
    expect(screen.queryByRole("group", { name: "Pairing code" })).toBeNull();
    expect(fake.peers).toHaveLength(0);
    // A new window can be opened straight away; the stale result is not mistaken for this one.
    await user.click(screen.getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
  });

  it("join with a wrong code shows the pairing_failed explanation; a locked peer shows pairing_locked", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={20} />);

    await user.type(await screen.findByLabelText("Address"), "192.168.1.77:51234");
    await user.type(screen.getByLabelText("Code"), "000 000");
    await user.click(screen.getByRole("button", { name: "Join" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Pairing did not complete.");
    expect(alert.textContent).toContain("Check the code and that both devices are on the same network");
    expect(alert.textContent).toContain("pairing_failed");
    expect(fake.peers).toHaveLength(0);

    fake.pairLocked = true;
    await user.click(screen.getByRole("button", { name: "Join" }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Too many failed pairing attempts."));
    expect(screen.getByRole("alert").textContent).toContain("Wait a minute, then start pairing again with a fresh code.");

    fake.pairLocked = false;
    await user.clear(screen.getByLabelText("Code"));
    await user.type(screen.getByLabelText("Code"), JOIN_CODE);
    await user.click(screen.getByRole("button", { name: "Join" }));
    await screen.findByText("Paired with Studio Mac.");
    expect(fake.peers.map((p) => p.id)).toEqual([PEER.id]);
  });

  it("join accepts the pasted QR text without an address", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={20} />);
    const join = await screen.findByRole("button", { name: "Join" });
    expect(join).toHaveProperty("disabled", true);
    await user.click(screen.getByLabelText("Code"));
    await user.paste(QR_PAYLOAD);
    expect(join).toHaveProperty("disabled", false);
    await user.click(join);
    await screen.findByText("Paired with Studio Mac.");
    const body = JSON.parse(String((calls(fake, "POST", "/api/pair/join")[0][1] as RequestInit).body));
    expect(body).toEqual({ code: QR_PAYLOAD, name: "This Mac" });
  });

  it("lists strangers separately with Pair as the only action, which prefills the join form", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, discovered: [STRANGER, { ...STRANGER, id: "ab".repeat(32), addr: "192.168.1.78:1", pairing: false, pair_port: null }] });
    render(<Peers pollMs={20} />);
    const row = (await screen.findByText(shortLabel(STRANGER.id))).closest("tr")!;
    expect(within(row).getAllByRole("button").map((b) => b.textContent)).toEqual(["Pair"]);
    const idle = screen.getByText("abab-abab-abab").closest("tr")!;
    expect(within(idle).queryAllByRole("button")).toHaveLength(0);
    expect(within(idle).getByText("Not in pairing mode")).toBeTruthy();

    // The join form gets the pairing window's address (host + pair_port), not the workload port.
    await user.click(within(row).getByRole("button", { name: `Pair with ${shortLabel(STRANGER.id)}` }));
    expect(pairingAddr(STRANGER)).toBe("192.168.1.77:51234");
    expect(screen.getByLabelText("Address")).toHaveProperty("value", "192.168.1.77:51234");
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Code")));
  });

  it("the address shown under the code goes into the other side's join form verbatim; the code pastes with its space", async () => {
    // One jsdom page and one fake daemon play both sides; this checks the form, not two devices.
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={20} />);
    await user.click(await screen.findByRole("button", { name: "Pair this device" }));
    const group = await screen.findByRole("group", { name: "Pairing code" });
    const shown = within(group).getByText(PAIR_ADDR).textContent!;
    expect(shown).toBe(PAIR_ADDR);

    await user.click(screen.getByLabelText("Address"));
    await user.paste(shown);
    await user.click(screen.getByLabelText("Code"));
    await user.paste(" 123 456 ");
    await user.click(screen.getByRole("button", { name: "Join" }));
    await screen.findByText("Paired with Studio Mac.");
    const body = JSON.parse(String((calls(fake, "POST", "/api/pair/join")[0][1] as RequestInit).body));
    expect(body).toEqual({ addr: PAIR_ADDR, code: JOIN_CODE, name: "This Mac" });
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
    render(<Peers pollMs={20} />);
    await user.click(await screen.findByRole("button", { name: "Pair this device" }));
    const group = await screen.findByRole("group", { name: "Pairing code" });
    expect(group.textContent).toContain("Port 51234.");
  });

  it("shows heartbeat health in plain words and falls back to connected without it", async () => {
    const peer = (n: number, extra: Partial<typeof PEER>) => ({ ...PEER, id: String(n).repeat(64), short_label: `${n}${n}${n}${n}-x`, name: `Device ${n}`, ...extra });
    installFakeDaemon({
      network: NETWORK_ON,
      peers: [
        peer(1, { health: "connected" }),
        peer(2, { health: "unhealthy" }),
        peer(3, { health: "reconnecting", connected: true }),
        peer(4, { health: "disconnected" }),
        peer(5, { connected: false }),
      ],
    });
    render(<Peers pollMs={20} />);
    const health = async (name: string) => within((await screen.findByText(name, { selector: ".model-name" })).closest("tr")!).getAllByRole("cell")[1].textContent;
    expect(await health("Device 1")).toBe("Connected");
    expect(await health("Device 2")).toBe("Not responding");
    expect(await health("Device 3")).toBe("Reconnecting");
    expect(await health("Device 4")).toBe("Not connected");
    expect(await health("Device 5")).toBe("Not connected");
  });

  it("never lists this device among nearby devices, even when the daemon reports its own advert", async () => {
    installFakeDaemon({ network: NETWORK_ON, discovered: [{ ...STRANGER, id: SELF_ID, addr: "192.168.1.10:8193" }, STRANGER] });
    render(<Peers pollMs={20} />);
    const nearby = (await screen.findByRole("heading", { name: "Nearby, not paired" })).closest(".section") as HTMLElement;
    await within(nearby).findByText(shortLabel(STRANGER.id));
    expect(within(nearby).queryByText(shortLabel(SELF_ID))).toBeNull();
    expect(within(nearby).getAllByRole("button", { name: /^Pair with/ })).toHaveLength(1);
  });

  it("revoke asks for confirmation and then removes the device; remove does too", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER, { ...PEER, id: "cd".repeat(32), short_label: "cdcd-cdcd-cdcd", name: "Old laptop", connected: false }] });
    render(<Peers pollMs={20} />);
    // Without heartbeat health the daemon's `connected` decides the column.
    expect(await screen.findByText("Connected")).toBeTruthy();
    expect(screen.getByText("Not connected")).toBeTruthy();

    await user.click(await screen.findByRole("button", { name: "Revoke Studio Mac" }));
    expect(fake.peers).toHaveLength(2);
    const confirm = await screen.findByRole("group", { name: "Confirm revoking Studio Mac" });
    expect(confirm.textContent).toContain("cannot reconnect until it is paired again");
    expect(document.activeElement?.textContent).toBe("Revoke");
    await user.click(within(confirm).getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(screen.queryByText("Studio Mac", { selector: ".model-name" })).toBeNull());
    expect(calls(fake, "POST", `/api/peers/${PEER.id}/revoke`)).toHaveLength(1);

    await user.click(screen.getByRole("button", { name: "Remove Old laptop" }));
    const c2 = await screen.findByRole("group", { name: "Confirm removing Old laptop" });
    await user.click(within(c2).getByRole("button", { name: "Cancel" }));
    expect(screen.getByText("Old laptop", { selector: ".model-name" })).toBeTruthy();
    expect(fake.peers).toHaveLength(1);
  });

  it("edits group membership and policy through PUT /api/groups, one group per request", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: [{ name: "work", members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } }] });
    render(<Peers pollMs={20} />);
    await user.type(await screen.findByLabelText("New group"), "home");
    await user.click(screen.getByRole("button", { name: "Create group" }));
    await screen.findByRole("group", { name: /home/ });
    expect(fake.groups.map((g) => g.name)).toEqual(["work", "home"]);
    expect(JSON.parse(String((calls(fake, "PUT", "/api/groups")[0][1] as RequestInit).body))).toEqual({ name: "home", members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } });

    await user.selectOptions(screen.getByLabelText("Add a device to home"), PEER.id);
    await waitFor(() => expect(fake.groups[1].members).toEqual([PEER.id]));
    const home = screen.getByRole("group", { name: /home/ });
    await user.click(within(home).getByRole("checkbox", { name: "Members may run prompts for each other" }));
    await waitFor(() => expect(fake.groups[1].policy.allow_remote_inference).toBe(true));
    expect(fake.groups[0]).toEqual({ name: "work", members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } });
    await waitFor(() => expect(screen.getByText("home", { selector: "td" })).toBeTruthy());
  });

  it("keyboard only: turn networking on, start pairing, see the code, see the paired device", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    const tabTo = async (el: Element) => {
      for (let i = 0; i < 60 && document.activeElement !== el; i++) await user.tab();
      expect(document.activeElement).toBe(el);
    };
    window.location.hash = "#/peers";
    render(<App poll={false} />);
    await screen.findByRole("heading", { level: 1, name: "Peers" });

    const sw = await screen.findByRole("switch", { name: /Let this Mac talk/ });
    await tabTo(sw);
    await user.keyboard(" ");
    await waitFor(() => expect(fake.network.enabled).toBe(true));
    const pair = screen.getByRole("button", { name: "Pair this device" });
    await waitFor(() => expect(pair).toHaveProperty("disabled", false));
    await tabTo(pair);
    await user.keyboard("{Enter}");
    const group = await screen.findByRole("group", { name: "Pairing code" });
    expect(within(group).getByText("482 913")).toBeTruthy();
    fake.completePairing();
    await screen.findByText("Paired with Studio Mac.", {}, { timeout: 4000 });
    await screen.findByText("Studio Mac", { selector: ".model-name" });
  }, 15000);
});
