// F-10-11 for Activity (owner D; #/diagnostics is its alias): the table with E4 audit events and E3 jobs, a selected
// row's inspector, and the Error codes dialog.
import { act, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { refresh } from "../../state/live";
import { resetStores, scan, scenario } from "../../test/axe";
import { AUDIT_E4, JOBS } from "../../test/mocks";

const menus = vi.hoisted(() => ({ items: [] as { label?: string; action?: () => void }[] }));
vi.mock("../../ui/contextMenu", async (orig) => ({
  ...(await orig<typeof import("../../ui/contextMenu")>()),
  popupMenu: async (items: { label?: string; action?: () => void }[]) => ((menus.items = items), true),
}));

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

describe("axe accessibility scan", () => {
  it("#/diagnostics has no axe violations", async () => {
    const user = userEvent.setup();
    const { fake, container } = await scenario("diagnostics");
    expect(await screen.findByRole("heading", { level: 1, name: "Activity" })).toBeTruthy();
    const grid = await screen.findByRole("grid", { name: "Activity" });
    fake.audit.push(...AUDIT_E4);
    fake.jobs = structuredClone(JOBS);
    await act(() => Promise.all([refresh("audit"), refresh("jobs")]));
    await within(grid).findByText(/Lost connection to Laptop|Lost connection to eeee-eeee-eeee/);
    await scan(container);

    await user.click(within(grid).getAllByRole("row")[1]);
    await screen.findByText("Raw JSON");
    await scan(container);

    await user.click(screen.getByRole("button", { name: "More" }));
    act(() => menus.items.find((i) => i.label === "Error codes…")!.action!());
    const dialog = await screen.findByRole("dialog", { name: "Error codes" });
    expect(within(dialog).getByText("insufficient_memory")).toBeTruthy();
    await scan(container);
  }, 20000);
});
