// The Windows Firewall finding (2026-10-19): pairing started on the Windows app failed because the firewall dropped
// the pairing port. The app offers one consent step (src-tauri firewall.rs, Tauri invoke mocked here); the joiner's
// "nothing answered" error points at that firewall instead of the code. Unit level against the fake daemon.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemon, privacy, settings } from "../state/stores";
import { Peers } from "../views/Peers";
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

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  privacy.set(null);
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
  invoke.mockReset();
});
afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  vi.unstubAllGlobals();
});

describe("Windows Firewall consent step", () => {
  it("offers one rule when networking is on and none exists; only the click asks Windows, and success says so", async () => {
    const user = userEvent.setup();
    windowsApp();
    installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={1000} />);
    const button = await screen.findByRole("button", { name: ALLOW });
    expect(screen.getByText("Windows Firewall blocks other devices from reaching this computer, so pairing started here and incoming connections fail.")).toBeTruthy();
    expect(screen.getByText("Adds a rule for Common's background service only, on networks Windows treats as Private. Windows will ask you to confirm.")).toBeTruthy();
    // Never silently: reading the state is all that happened so far.
    expect(allowCalls()).toBe(0);

    await user.click(button);
    expect((await screen.findByText("Allowed on private networks.")).getAttribute("role")).toBe("status");
    expect(allowCalls()).toBe(1);
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
  });

  it("on a network Windows treats as Public the rule does not help, so the notice says so instead of going away", async () => {
    const user = userEvent.setup();
    // The rule exists: no offer, but the Public network is named with what the user can change.
    windowsApp({ ...BLOCKED, allowed: true, rule_program: "C:\\Common\\common.exe", public_networks: ["eduroam"] });
    installFakeDaemon({ network: NETWORK_ON });
    const { unmount } = render(<Peers pollMs={1000} />);
    expect(await screen.findByText(PUBLIC_NOTE)).toBeTruthy();
    expect(screen.getByText("Windows Firewall still blocks other devices on this network.")).toBeTruthy();
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
    unmount();

    // No rule yet on a Public network: the offer, the note, and after Allow the note stays.
    invoke.mockReset();
    windowsApp({ ...BLOCKED, public_networks: ["eduroam"] });
    render(<Peers pollMs={1000} />);
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
    render(<Peers pollMs={1000} />);
    await user.click(await screen.findByRole("button", { name: ALLOW }));
    expect((await screen.findByRole("alert")).textContent).toBe("Nothing changed: the Windows confirmation was cancelled. Other devices still cannot reach this computer.");

    outcome = "netsh could not add the rule (exit code 1)";
    await user.click(screen.getByRole("button", { name: ALLOW }));
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Windows did not add the rule (netsh could not add the rule (exit code 1))."));
    expect(screen.queryByText("Allowed on private networks.")).toBeNull();
    expect(allowCalls()).toBe(2);
  });

  it("stays out of the way with networking off, with the rule in place, off Windows and outside the app", async () => {
    for (const [status, network] of [
      [BLOCKED, NETWORK],
      [{ ...BLOCKED, allowed: true, rule_program: "C:\\Common\\common.exe" }, NETWORK_ON],
      [{ supported: false, allowed: false, rule_program: null }, NETWORK_ON],
    ] as const) {
      windowsApp(status);
      installFakeDaemon({ network });
      const { unmount } = render(<Peers pollMs={1000} />);
      await screen.findByText("This device");
      await waitFor(() => expect(invoke).toHaveBeenCalledWith("firewall_status"));
      expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
      unmount();
      invoke.mockReset();
    }
    // A browser tab on the dev server: no Tauri, so nothing is asked of it.
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={1000} />);
    await screen.findByText("This device");
    expect(screen.queryByRole("button", { name: ALLOW })).toBeNull();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("the notice, its failure and its place by the pairing code have no serious or critical axe violations", async () => {
    const user = userEvent.setup();
    windowsApp(BLOCKED, () => Promise.reject("cancelled"));
    installFakeDaemon({ network: NETWORK_ON });
    const { container } = render(<Peers pollMs={1000} />);
    const serious = async () =>
      (await axe.run(container, { rules: { "color-contrast": { enabled: false } } })).violations
        .filter((v) => v.impact === "serious" || v.impact === "critical")
        .map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`);
    await user.click(await screen.findByRole("button", { name: ALLOW }));
    await screen.findByRole("alert");
    expect(await serious()).toEqual([]);
    await user.click(screen.getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    expect(await serious()).toEqual([]);
  });

  it("sits next to the pairing code while pairing started here is waiting", async () => {
    const user = userEvent.setup();
    windowsApp();
    installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={1000} />);
    await screen.findByRole("button", { name: ALLOW });
    await user.click(screen.getByRole("button", { name: "Pair this device" }));
    const code = await screen.findByRole("group", { name: "Pairing code" });
    expect(within(code).getByRole("button", { name: ALLOW })).toBeTruthy();
    // One notice, not two.
    expect(screen.getAllByRole("button", { name: ALLOW })).toHaveLength(1);
  });
});

describe("joining a pairing window where nothing answers", () => {
  it("points at the other device's firewall instead of the code; a wrong code still says check the code", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    const real = fake.fetchMock.getMockImplementation() as typeof fetch;
    fake.fetchMock.mockImplementation(async (u: RequestInfo | URL, init?: RequestInit) =>
      String(u).endsWith("/api/pair/join") && String(init?.body).includes("51999")
        ? apiError("pairing_failed", "nothing answered at 192.168.1.20:51999", 401, "a firewall on the other device may block its pairing port (UDP 51999); on Windows, allow Common under Peers on that device")
        : real(u, init),
    );
    render(<Peers pollMs={1000} />);
    await user.type(await screen.findByLabelText("Address"), "192.168.1.20:51999");
    await user.type(screen.getByLabelText("Code"), `${JOIN_CODE}{Enter}`);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Pairing did not complete.");
    expect(alert.textContent).toContain("A firewall on the other device may block its pairing port (UDP 51999); on Windows, allow Common under Peers on that device.");
    expect(alert.textContent).toContain("nothing answered at 192.168.1.20:51999");
    expect(alert.textContent).not.toContain("Check the code");

    await user.clear(screen.getByLabelText("Address"));
    await user.type(screen.getByLabelText("Address"), "192.168.1.20:51234");
    await user.clear(screen.getByLabelText("Code"));
    await user.type(screen.getByLabelText("Code"), "999999{Enter}");
    await waitFor(() => expect(screen.getByRole("alert").textContent).toContain("Check the code"));
  });
});
