// F-10-11 for Devices (owner B): the section with realistic data, every inspector tab of a connected E1 device, the
// Pair dialog (both tabs) and a failed connect. Moved from src/test/a11y.test.tsx in Phase 0; the old Hardware view is
// now #/devices/self?tab=hardware (the #/hardware alias) and the old Peers view is #/devices (the #/peers alias).
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { resetStores, scan, scenario } from "../../test/axe";
import { AUDIT_E4, installFakeDaemon, JOBS, LOADED, NETWORK_ON, PEER, PEERS_E1, PRIVACY, STRANGER } from "../../test/mocks";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/hardware (this computer's Hardware tab) has no serious or critical violations", async () => {
    const { container } = await scenario("hardware");
    expect(await screen.findByRole("heading", { level: 1, name: "Devices" })).toBeTruthy();
    expect((await screen.findByRole("tab", { name: "Hardware" })).getAttribute("aria-selected")).toBe("true");
    await screen.findByText("Apple M2", { selector: "dd" });
    await scan(container);
  }, 20000);

  // Networking on, a paired device, a group, a stranger nearby and a pairing code showing.
  it("#/peers with the Pair dialog showing a code has no serious or critical violations", async () => {
    const user = userEvent.setup();
    const { container } = await scenario("peers");
    expect(await screen.findByRole("heading", { level: 1, name: "Devices" })).toBeTruthy();
    await within(await screen.findByRole("grid", { name: "Devices" })).findByRole("row", { name: /Studio Mac/ });
    await scan(container);
    await user.click(screen.getByRole("button", { name: "Pair a device…" }));
    await user.click(await screen.findByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    await scan(container);
    await user.click(screen.getByRole("tab", { name: "Enter a code" }));
    await screen.findByRole("button", { name: "Join" });
    await scan(container);
  }, 20000);

  it("a device seen nearby, one that failed to answer and its Connection tab showing the error", async () => {
    const user = userEvent.setup();
    const away = { ...PEER, id: "ee".repeat(32), short_label: "eeee-eeee-eeee", name: "Laptop", connected: false, health: "disconnected" as const, last_addr: "172.20.10.6:8193", last_error: { addr: "172.20.10.6:8193", code: "peer_lost", message: "no answer from 172.20.10.6:8193 within 4 s", hint: "the device may be asleep or on another network, or a firewall may block UDP port 8193", at_utc: "2026-10-19T03:40:00Z" } };
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER, away], discovered: [{ ...STRANGER, id: PEER.id, trusted: true, addrs: [STRANGER.addr] }, STRANGER] });
    await refreshStatus();
    window.location.hash = `#/devices/${away.id}?tab=connection`;
    const { container } = render(<App poll={false} />);
    const form = await screen.findByRole("form", { name: "Connect to Laptop" });
    await user.click(within(form).getByRole("button", { name: "Connect" }));
    await within(form).findByText("The connection to the other device was lost.");
    await scan(container);
  }, 20000);

  it("every inspector tab of a connected E1 device, and the Revoked scope", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({
      network: { ...NETWORK_ON, serve_peers: true },
      peers: PEERS_E1.peers,
      discovered: PEERS_E1.discovered,
      revocations: PEERS_E1.revocations,
      jobs: JOBS,
      audit: AUDIT_E4,
      groups: [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }],
      privacy: { ...PRIVACY, mode: "trusted_group" },
    });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = `#/devices/${PEER.id}`;
    const { container } = render(<App poll={false} />);
    await screen.findByRole("tab", { name: "Overview" });
    for (const tab of ["Overview", "Hardware", "Models", "Performance", "Connection", "Permissions", "Activity"]) {
      await user.click(screen.getByRole("tab", { name: tab }));
      await waitFor(() => expect(screen.getByRole("tab", { name: tab }).getAttribute("aria-selected")).toBe("true"));
      await scan(container);
    }
    await user.click(within(screen.getByRole("group", { name: "Show devices" })).getByRole("button", { name: /^Revoked/ }));
    await within(screen.getByRole("grid", { name: "Devices" })).findByRole("row", { name: /cdcd-cdcd-cdcd/ });
    await scan(container);
  }, 30000);
});
