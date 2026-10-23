// The Windows Firewall finding (2026-10-19): pairing started on the Windows app failed because the firewall dropped
// the pairing port. The app offers one consent step (src-tauri firewall.rs, Tauri invoke mocked here); the joiner's
// "nothing answered" error points at that firewall instead of the code. Unit level against the fake daemon.
// Migrated to the Devices section (plan §5.4). Deliberate wording change (plan §2.2 Firewall): the check cannot prove
// a block, so the notice says no rule was found and that other devices may be unable to connect.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderDevices, toolbar } from "../sections/devices/fixtures";
import { daemon, privacy, settings } from "../state/stores";
import { apiError, installFakeDaemon, JOIN_CODE, NETWORK, NETWORK_ON } from "./mocks";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const BLOCKED = { supported: true, allowed: false, rule_program: null, public_networks: [] as string[] };
const PUBLIC_NOTE =
  "Windows treats eduroam as Public, where this rule does not apply, so other devices cannot reach this computer there. If you trust a network, set it to Private in Windows Settings > Network & internet.";
const ALLOW = "Allow on private networks";
/** The Windows app: `firewall_status` answers `status`, `allow_through_firewall` runs `allow`. */
function windowsApp(status: object = BLOCKED, allow: () => Promise<void> = async () => {}) {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  invoke.mockImplementation(async (cmd: string) => (cmd === "firewall_status" ? status : cmd === "allow_through_firewall" ? allow() : undefined));
}
const allowCalls = () => invoke.mock.calls.filter(([c]) => c === "allow_through_firewall").length;
const openPair = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(await within(await screen.findByRole("toolbar", { name: "Devices actions" })).findByRole("button", { name: "Pair a device…" }));
  return screen.findByRole("dialog", { name: "Pair a device" });
};

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  privacy.set({ data: null, at: null, error: null });
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
  invoke.mockReset();
});
afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  vi.unstubAllGlobals();
});

describe("Windows Firewall consent step", () => {
  it("offers one rule when networking is on and none was found; only the click asks Windows, and success says so", async () => {
    const user = userEvent.setup();
    windowsApp();
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    const button = await screen.findByRole("button", { name: ALLOW });
    expect(screen.getByText("No Windows Firewall rule for the service was found; other devices may be unable to connect.")).toBeTruthy();
    expect(screen.getByText("Adds a rule for Common's background service only, on networks Windows treats as Private. Windows will ask you to confirm.")).toBeTruthy();
    // Never silently: reading the state is all that happened so far.
    expect(allowCalls()).toBe(0);

    await user.click(button);
    const done = await screen.findByText("Allowed on private networks.");
    expect(done.getAttribute("role")).toBe("status");
    expect(allowCalls()).toBe(1);
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
    // The focused button is gone: the outcome takes focus instead of the document.
    await waitFor(() => expect(document.activeElement).toBe(done));
  });

  it("on a network Windows treats as Public the rule does not help, so the notice says so instead of going away", async () => {
    const user = userEvent.setup();
    // The rule exists: no offer, but the Public network is named with what the user can change.
    windowsApp({ ...BLOCKED, allowed: true, rule_program: "C:\\Common\\common.exe", public_networks: ["eduroam"] });
    installFakeDaemon({ network: NETWORK_ON });
    const { unmount } = renderDevices();
    expect(await screen.findByText(PUBLIC_NOTE)).toBeTruthy();
    expect(screen.getByText("Windows Firewall still blocks other devices on this network.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
    unmount();

    // No rule yet on a Public network: the offer, the note, and after Allow the note stays.
    invoke.mockReset();
    windowsApp({ ...BLOCKED, public_networks: ["eduroam"] });
    renderDevices();
    await user.click(await screen.findByRole("button", { name: ALLOW }));
    await screen.findByText("Allowed on private networks.");
    expect(screen.getByText(PUBLIC_NOTE)).toBeTruthy();
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
  });

  it("a cancelled Windows prompt or a failed netsh is explained, changes nothing and can be retried", async () => {
    const user = userEvent.setup();
    let outcome = "cancelled";
    windowsApp(BLOCKED, () => Promise.reject(outcome));
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    await user.click(await screen.findByRole("button", { name: ALLOW }));
    expect((await screen.findByRole("alert")).textContent).toBe("Nothing changed: the Windows confirmation was cancelled. Other devices may still be unable to connect.");

    outcome = "netsh could not add the rule (exit code 1)";
    await user.click(screen.getByRole("button", { name: ALLOW }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Windows did not add the rule (netsh could not add the rule (exit code 1))."));
    expect(screen.queryByText("Allowed on private networks.")).toBeNull();
    expect(allowCalls()).toBe(2);
  });

  it("a service this app did not start is not checked and no rule is offered for the bundled program instead", async () => {
    windowsApp({ ...BLOCKED, adopted: true });
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    expect(await screen.findByText("The running background service was not started by this app, so the app cannot check or add its Windows Firewall rule.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
    expect(screen.queryByText(/No Windows Firewall rule/)).toBeNull();
  });

  it("a domain network is named too, pointing at the administrator rather than at Private", async () => {
    windowsApp({ ...BLOCKED, allowed: true, rule_program: "C:\\Common\\common.exe", domain_networks: ["corp.example"] });
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    expect(await screen.findByText("Windows Firewall may still block other devices on this network.")).toBeTruthy();
    expect(screen.getByText(/^Windows treats corp\.example as a domain network, where this rule does not apply either/).textContent).toMatch(/Ask your administrator/);
    expect(screen.queryByText(/set it to Private/)).toBeNull();
  });

  it("a firewall read that failed claims nothing: no warning, no offer", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockImplementation(async (cmd: string) => {
      if (cmd === "firewall_status") throw "could not read the firewall rules: blocked";
    });
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    await screen.findByRole("grid", { name: "Devices" });
    await waitFor(() => expect(invoke).toHaveBeenCalledWith("firewall_status"));
    expect(screen.queryByText(/No Windows Firewall rule/)).toBeNull();
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
  });

  it("stays out of the way with networking off, with the rule in place, off Windows and outside the app", async () => {
    for (const [status, network] of [
      [BLOCKED, NETWORK],
      [{ ...BLOCKED, allowed: true, rule_program: "C:\\Common\\common.exe" }, NETWORK_ON],
      [{ supported: false, allowed: false, rule_program: null }, NETWORK_ON],
    ] as const) {
      windowsApp(status);
      installFakeDaemon({ network });
      const { unmount } = renderDevices();
      await screen.findByRole("grid", { name: "Devices" });
      await waitFor(() => expect(invoke).toHaveBeenCalledWith("firewall_status"));
      expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
      unmount();
      invoke.mockReset();
    }
    // A browser tab on the dev server: no Tauri, so nothing is asked of it.
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    await screen.findByRole("grid", { name: "Devices" });
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("the notice, its failure and its place by the pairing code have no axe violations", async () => {
    const user = userEvent.setup();
    windowsApp(BLOCKED, () => Promise.reject("cancelled"));
    installFakeDaemon({ network: NETWORK_ON });
    const { container } = renderDevices();
    const serious = async () =>
      (await axe.run(container, { rules: { "color-contrast": { enabled: false } } })).violations
        .filter((v) => v.impact === "serious" || v.impact === "critical")
        .map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
    await user.click(await screen.findByRole("button", { name: ALLOW }));
    await screen.findByRole("alert");
    expect(await serious()).toEqual([]);
    const d = await openPair(user);
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    expect(await serious()).toEqual([]);
  });

  it("sits next to the pairing code while pairing started here is waiting", async () => {
    const user = userEvent.setup();
    windowsApp();
    installFakeDaemon({ network: NETWORK_ON });
    renderDevices();
    await screen.findByRole("button", { name: ALLOW });
    const d = await openPair(user);
    await user.click(within(d).getByRole("button", { name: "Pair this device" }));
    const code = await screen.findByRole("group", { name: "Pairing code" });
    expect(await within(code).findByRole("button", { name: ALLOW })).toBeTruthy();
    // One notice, not two: the banner above the table steps aside while the dialog is open.
    expect(screen.getAllByRole("button", { name: ALLOW })).toHaveLength(1);
    expect(within(toolbar()).queryByRole("button", { name: ALLOW })).toBeNull();
  });
});

describe("joining a pairing window where nothing answers", () => {
  it("points at the other device's firewall instead of the code; a wrong code still says check the code", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    const real = fake.fetchMock.getMockImplementation() as typeof fetch;
    // The hint is the service's (E0 wording, common-network pairing.rs `no_answer`).
    const hint = "a firewall on the other device may block its pairing port (UDP 51999); on Windows, allow Common through the firewall on that device (Devices in its desktop app)";
    fake.fetchMock.mockImplementation(async (u: RequestInfo | URL, init?: RequestInit) =>
      String(u).endsWith("/api/pair/join") && String(init?.body).includes("51999") ? apiError("pairing_failed", "nothing answered at 192.168.1.20:51999", 401, hint) : real(u, init),
    );
    renderDevices();
    const d = await openPair(user);
    await user.click(within(d).getByRole("tab", { name: "Enter a code" }));
    await user.type(within(d).getByLabelText("Address"), "192.168.1.20:51999");
    await user.type(within(d).getByLabelText("Code"), `${JOIN_CODE}{Enter}`);
    const alert = await within(d).findByRole("alert");
    expect(alert.textContent).toContain("Pairing did not complete.");
    expect(alert.textContent).toContain("A firewall on the other device may block its pairing port (UDP 51999); on Windows, allow Common through the firewall on that device (Devices in its desktop app).");
    expect(alert.textContent).toContain("nothing answered at 192.168.1.20:51999");
    expect(alert.textContent).not.toContain("Check the code");

    await user.clear(within(d).getByLabelText("Address"));
    await user.type(within(d).getByLabelText("Address"), "192.168.1.20:51234");
    await user.clear(within(d).getByLabelText("Code"));
    await user.type(within(d).getByLabelText("Code"), "999999{Enter}");
    await waitFor(() => expect(within(d).getByRole("alert").textContent).toContain("Check the code"));
  });
});
