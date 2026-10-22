// F-10-11 for Models (owner C): the section, its download dialog, an inline confirmation and the inspector tabs.
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { installFakeDaemon, LOADED, PEERS_E1 } from "../../test/mocks";
import { resetStores, scan, scenario } from "../../test/axe";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/models has no serious or critical violations", async () => {
    const { container } = await scenario("models");
    expect(await screen.findByRole("heading", { level: 1, name: "Models" })).toBeTruthy();
    await screen.findByText(/TinyStories 15M/);
    await scan(container);
  }, 20000);

  it("models view with the download dialog open (?confirm=)", async () => {
    installFakeDaemon();
    await refreshStatus();
    window.location.hash = "#/models?confirm=qwen2.5-1.5b-instruct-q4_k_m";
    const { container } = render(<App poll={false} />);
    const dialog = await screen.findByRole("dialog", { name: /Confirm download of Qwen2.5/ });
    expect(dialog.textContent).toContain("License: Apache-2.0");
    // The confirm control has the initial focus; the ?confirm param is consumed (selection stays).
    expect(document.activeElement?.textContent).toMatch(/^Download 1\.0 GB/);
    expect(window.location.hash).toBe("#/models/qwen2.5-1.5b-instruct-q4_k_m");
    await scan(container);
  }, 20000);

  it("a selected loaded model with E1 peers, every inspector tab and a delete confirmation", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ peers: PEERS_E1.peers });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/models/qwen2.5-1.5b-instruct-q4_k_m";
    fake.models[0].downloaded = true;
    const { container } = render(<App poll={false} />);
    const grid = await screen.findByRole("grid", { name: "Models" });
    await within(grid).findByText(/Loaded · Metal/);
    for (const tab of ["Overview", "Memory", "Locations", "Download"]) {
      await user.click(screen.getByRole("tab", { name: tab }));
      await scan(container);
    }
    await user.click(screen.getByRole("button", { name: "Delete…" }));
    await screen.findByRole("group", { name: /Confirm deletion of Qwen2.5/ });
    await act(async () => {});
    await scan(container);
  }, 30000);
});
