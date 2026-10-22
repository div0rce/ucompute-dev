// F-10-11 for Settings (owner D): every page, and Privacy (#/privacy is its alias) with the disclosure open.
import { act, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { privacy } from "../../state/stores";
import { resetStores, scan, scenario } from "../../test/axe";
import { PRIVACY } from "../../test/mocks";
import { PAGES } from "./Settings";

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/settings (every page) has no axe violations", async () => {
    const { container } = await scenario("settings/execution");
    expect(await screen.findByRole("heading", { level: 1, name: "Settings" })).toBeTruthy();
    expect(screen.getByRole("navigation", { name: "Settings pages" })).toBeTruthy();
    await screen.findByLabelText("Context length");
    await scan(container);
    for (const p of PAGES) {
      act(() => void (window.location.hash = `#/settings/${p.id}`));
      await screen.findByRole("heading", { level: 2, name: p.label });
      expect(screen.getAllByRole("heading", { level: 1 })).toHaveLength(1);
      await scan(container);
    }
  }, 30000);

  // Privacy in trusted-devices mode with the disclosure open (the acknowledge checkbox visible).
  it("#/privacy has no axe violations", async () => {
    const { fake, container } = await scenario("privacy");
    expect(await screen.findByRole("heading", { level: 2, name: "Privacy" })).toBeTruthy();
    // The disclosure only appears when leaving local-only, so start there and pick trusted devices.
    fake.privacy = { ...PRIVACY };
    privacy.set({ ...PRIVACY });
    (await screen.findByRole("radio", { name: /Trusted devices/ })).click();
    await screen.findByRole("checkbox", { name: /I understand/ });
    await scan(container);
  }, 20000);
});
