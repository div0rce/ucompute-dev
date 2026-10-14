// F-10-05: pairing in both directions, honest failures, revocation with confirmation, strangers never actionable.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { daemon, privacy } from "../state/stores";
import { grouped, Peers } from "../views/Peers";
import { installFakeDaemon, JOIN_CODE, NETWORK, PEER, SELF_ID, STRANGER } from "./mocks";

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
    expect(within(group).getByText(/^common-pair:v1:/)).toBeTruthy();
    expect(within(group).getByRole("status").textContent).toContain("Waiting for the other device");

    // The other device confirms; polling notices and the list updates.
    await waitFor(() => expect(calls(fake, "GET", "/api/pair/status").length).toBeGreaterThan(0));
    fake.completePairing();
    await screen.findByText("Paired with Studio Mac.");
    expect(screen.queryByRole("group", { name: "Pairing code" })).toBeNull();
    await screen.findByText("Studio Mac", { selector: ".model-name" });
    expect(screen.getByText("Connected")).toBeTruthy();
  });

  it("join with a wrong code shows the pairing_failed explanation; a locked peer shows pairing_locked", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK, enabled: true } });
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

  it("lists strangers separately with Pair as the only action, which prefills the join form", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: { ...NETWORK, enabled: true }, discovered: [STRANGER, { ...STRANGER, id: "ab".repeat(32), short_label: "abab-abab-abab", addr: "192.168.1.78:1", pairing: false }] });
    render(<Peers pollMs={20} />);
    const row = (await screen.findByText(STRANGER.short_label)).closest("tr")!;
    expect(within(row).getAllByRole("button").map((b) => b.textContent)).toEqual(["Pair"]);
    const idle = screen.getByText("abab-abab-abab").closest("tr")!;
    expect(within(idle).queryAllByRole("button")).toHaveLength(0);
    expect(within(idle).getByText("Not in pairing mode")).toBeTruthy();

    await user.click(within(row).getByRole("button", { name: `Pair with ${STRANGER.short_label}` }));
    expect(screen.getByLabelText("Address")).toHaveProperty("value", STRANGER.addr);
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Code")));
  });

  it("revoke asks for confirmation and then removes the device; remove does too", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK, enabled: true }, peers: [PEER, { ...PEER, id: "cd".repeat(32), short_label: "cdcd-cdcd-cdcd", name: "Old laptop", health: "disconnected", connected: false }] });
    render(<Peers pollMs={20} />);

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

  it("edits group membership and policy through PUT /api/groups", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK, enabled: true }, peers: [PEER] });
    render(<Peers pollMs={20} />);
    await user.type(await screen.findByLabelText("New group"), "home");
    await user.click(screen.getByRole("button", { name: "Create group" }));
    await screen.findByRole("group", { name: /home/ });
    expect(fake.groups).toEqual([{ name: "home", members: [], policy: { allow_remote_inference: false, allow_pooled_inference: false } }]);

    await user.selectOptions(screen.getByLabelText("Add a device to home"), PEER.id);
    await waitFor(() => expect(fake.groups[0].members).toEqual([PEER.id]));
    await user.click(screen.getByRole("checkbox", { name: "Members may run prompts for each other" }));
    await waitFor(() => expect(fake.groups[0].policy.allow_remote_inference).toBe(true));
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
