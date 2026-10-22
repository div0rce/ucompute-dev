import { act, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { onShortcut } from "../shell/shortcuts";
import { ui } from "../state/ui";
import { Inspector, openInspector } from "./Inspector";

const tabs = (spy = vi.fn(() => "hw body")) => [
  { id: "overview", label: "Overview", render: () => "overview body" },
  { id: "hardware", label: "Hardware", render: spy },
];

describe("Inspector", () => {
  it("APG tabs: arrow keys switch (automatic activation) and the tab goes in the URL without a history entry", async () => {
    const user = userEvent.setup();
    window.location.hash = "#/devices/self";
    const before = history.length;
    render(<Inspector id="devices" label="Device details" hasSelection empty="Select a device to see its details" tabs={tabs()} />);
    const overview = screen.getByRole("tab", { name: "Overview" });
    expect(overview.getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("tabpanel").textContent).toBe("overview body");
    overview.focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Hardware" }).getAttribute("aria-selected")).toBe("true");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Hardware" }));
    expect(screen.getByRole("tabpanel").textContent).toBe("hw body");
    expect(window.location.hash).toBe("#/devices/self?tab=hardware");
    expect(history.length).toBe(before);
    expect(ui.get().inspectors.devices.tab).toBe("hardware");
  });

  it("opens on the tab named in the URL", () => {
    window.location.hash = "#/devices/self?tab=hardware";
    render(<Inspector id="devices" label="Device details" hasSelection empty="" tabs={tabs()} />);
    expect(screen.getByRole("tab", { name: "Hardware" }).getAttribute("aria-selected")).toBe("true");
  });

  it("clicking the active tab does not collapse; the button and Mod+I do", async () => {
    const user = userEvent.setup();
    render(<Inspector id="models" label="Model details" hasSelection empty="" tabs={tabs()} />);
    await user.click(screen.getByRole("tab", { name: "Overview" }));
    expect(screen.getByRole("tabpanel")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Hide Model details" }));
    expect(screen.queryByRole("tabpanel")).toBeNull();
    expect(screen.getByRole("button", { name: "Show Model details" }).getAttribute("aria-expanded")).toBe("false");
    expect(ui.get().inspectors.models.collapsed).toBe(true);
    const ctrl = navigator.platform.includes("Mac") ? { metaKey: true } : { ctrlKey: true };
    act(() => onShortcut(new KeyboardEvent("keydown", { key: "i", ...ctrl, cancelable: true })));
    expect(screen.getByRole("tabpanel")).toBeTruthy();
  });

  it("with no selection shows the reason and renders no tab", () => {
    const spy = vi.fn(() => "hw body");
    render(<Inspector id="jobs" label="Job details" hasSelection={false} empty="Select a job to see its details" tabs={tabs(spy)} />);
    expect(screen.getByText("Select a job to see its details")).toBeTruthy();
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it("the splitter resizes with the keyboard (min 140 px) and the height persists", () => {
    render(<Inspector id="activity" label="Event details" hasSelection empty="" tabs={tabs()} />);
    const sep = screen.getByRole("separator", { name: "Resize Event details" });
    const h = Number(sep.getAttribute("aria-valuenow"));
    fireEvent.keyDown(sep, { key: "ArrowUp" });
    expect(Number(sep.getAttribute("aria-valuenow"))).toBe(h + 16);
    expect(ui.get().inspectors.activity.height).toBe(h + 16);
    for (let i = 0; i < 40; i++) fireEvent.keyDown(sep, { key: "ArrowDown" });
    expect(Number(sep.getAttribute("aria-valuenow"))).toBe(140);
  });

  it("openInspector expands it and focuses the selected tab", async () => {
    render(<Inspector id="devices" label="Device details" hasSelection empty="" tabs={tabs()} />);
    act(() => ui.set((u) => ({ ...u, inspectors: { devices: { collapsed: true } } })));
    act(() => openInspector());
    await vi.waitFor(() => expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Overview" })));
  });

  it('"fit" sizes to the table\'s rows plus one row (capped at half), not to the stretched main area', () => {
    // jsdom does no layout: the body is 700 px, the table's rows 100 px, the stretched main area 560 px.
    const px: Record<string, Partial<Record<"clientHeight" | "offsetHeight" | "scrollHeight", number>>> = { "frame-body": { clientHeight: 700 }, grid: { offsetHeight: 100 }, "frame-main": { scrollHeight: 560 } };
    const spies = (["clientHeight", "offsetHeight", "scrollHeight"] as const).map((prop) =>
      vi.spyOn(HTMLElement.prototype, prop, "get").mockImplementation(function (this: HTMLElement) {
        return px[this.className]?.[prop] ?? 0;
      }),
    );
    render(
      <div className="frame-body">
        <div className="frame-main">
          <table className="grid" />
        </div>
        <Inspector id="devices" label="Device details" hasSelection empty="" defaultHeight="fit" tabs={tabs()} />
      </div>,
    );
    // 700 - (100 + 28), not 700 - min(560 + 28, 350) = 350.
    expect(screen.getByRole("separator", { name: "Resize Device details" }).getAttribute("aria-valuenow")).toBe("572");
    spies.forEach((s) => s.mockRestore());
  });
});
