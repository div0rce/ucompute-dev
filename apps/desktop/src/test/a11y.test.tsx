// F-10-11: the shell (rail, status bar, announcer, shortcuts sheet) has no serious/critical axe violations with
// realistic data. Per-section scans live in src/sections/<x>/<x>.a11y.test.tsx (each section's owner).
import { act, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { runAxeSweep } from "../shell/devAxe";
import { SHORTCUTS_EVENT } from "../shell/shortcuts";
import { refresh } from "../state/live";
import { resetStores, scan, scenario } from "./axe";
import { installFakeDaemon, JOBS, LOADED, NETWORK_ON, PEERS_E1, PRIVACY } from "./mocks";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan: shell", () => {
  it("rail and status bar with paired devices, a served request, E1 counters and a CPU fallback", async () => {
    const fake = installFakeDaemon({ network: { ...NETWORK_ON, serve_peers: true }, peers: PEERS_E1.peers, discovered: PEERS_E1.discovered, revocations: PEERS_E1.revocations, jobs: JOBS, privacy: { ...PRIVACY, mode: "trusted_group" } });
    fake.loaded = { ...LOADED, fallback_reason: "Metal could not allocate the KV cache" };
    await refreshStatus();
    window.location.hash = "#/dashboard";
    const { container } = render(<App poll={false} />);
    const bar = await screen.findByRole("contentinfo", { name: "Status" });
    await within(bar).findByText(/Serving 1 request for Studio Mac/);
    await within(bar).findByRole("button", { name: /Running on CPU: Metal could not allocate/ });
    expect(screen.getByRole("navigation", { name: "Sections" })).toBeTruthy();
    await scan(container);
  }, 20000);

  it("the service not running, then the keyboard shortcuts sheet open", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    await refreshStatus();
    window.location.hash = "#/devices";
    const { container } = render(<App poll={false} />);
    await within(screen.getByRole("contentinfo", { name: "Status" })).findByText("Not running");
    await scan(container);
    act(() => void window.dispatchEvent(new Event(SHORTCUTS_EVENT)));
    expect(await screen.findByRole("dialog", { name: "Keyboard shortcuts" })).toBeTruthy();
    await scan(container);
  }, 20000);

  it("the dev-only ?axe=1 sweep runs axe per section and theme, keeps the results and restores the theme", async () => {
    await scenario("dashboard");
    await screen.findByRole("grid", { name: "Computers" });
    const info = vi.spyOn(console, "info").mockImplementation(() => undefined);
    const results = await runAxeSweep({ sections: ["dashboard", "settings"], themes: ["dark"], settleMs: 0 });
    expect(results.map((r) => [r.section, r.theme])).toEqual([
      ["dashboard", "dark"],
      ["settings", "dark"],
    ]);
    expect(results[0].passes).toBeGreaterThan(5);
    expect((window as unknown as { __ucomputeAxe?: unknown }).__ucomputeAxe).toBe(results);
    expect(info).toHaveBeenCalledTimes(1);
    expect(document.documentElement.dataset.theme).toBeUndefined();
    info.mockRestore();
  }, 30000);

  it("every section renders inside the shell with one h1 and without violations", async () => {
    const { container } = await scenario("jobs");
    await refresh("jobs");
    for (const route of ["dashboard", "devices", "models", "jobs", "chat", "activity", "settings"]) {
      act(() => void (window.location.hash = `#/${route}`));
      // This section's own h1, not the previous one still on screen.
      await screen.findByRole("heading", { level: 1, name: route.charAt(0).toUpperCase() + route.slice(1) });
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
      await scan(container);
    }
  }, 60000);
});
