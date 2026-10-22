import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { ui } from "../state/ui";
import { popupMenu } from "./contextMenu";
import { DataTable, type Column, type DataTableProps } from "./DataTable";

vi.mock("./contextMenu", async (orig) => ({ ...(await orig<typeof import("./contextMenu")>()), popupMenu: vi.fn(async () => true) }));

type R = { id: string; name: string; mem: number | null; state: string; why?: string };
const ROWS: R[] = [
  { id: "a", name: "Device 10", mem: null, state: "Connected" },
  { id: "self", name: "This computer", mem: 8, state: "Listening" },
  { id: "b", name: "Device 2", mem: 16, state: "Connected", why: "seen at 10:32" },
  { id: "c", name: "device 3", mem: 4, state: "Not connected" },
];
const COLS: Column<R>[] = [
  { id: "name", header: "Name", cell: (r) => r.name, sort: (r) => r.name },
  { id: "mem", header: "Memory", cell: (r) => (r.mem == null ? "—" : `${r.mem} GB`), sort: (r) => r.mem, align: "end", live: true, note: (r) => (r.mem == null ? "Not reported while disconnected" : null) },
  { id: "state", header: "State", cell: (r) => r.state, sort: (r) => r.state, detail: (r) => r.why ?? null },
  { id: "id", header: "Device ID", cell: (r) => r.id, sort: (r) => r.id, hidden: true },
];

function Harness(p: Partial<DataTableProps<R>>) {
  const [sel, setSel] = useState<string | null>(null);
  return <DataTable id="t" label="Devices" columns={COLS} rows={ROWS} rowKey={(r) => r.id} selected={sel} onSelect={setSel} pinned={(r) => r.id === "self"} empty={<p>Nothing here</p>} {...p} />;
}
const names = () => screen.getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("gridcell")[0].textContent);
const header = (name: string) => screen.getByRole("columnheader", { name: new RegExp(`^${name}`) });

describe("DataTable", () => {
  it("sorts naturally with this computer pinned first; unknown values sort last in both directions", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByRole("grid", { name: "Devices" }).getAttribute("aria-rowcount")).toBe("5");
    expect(names()).toEqual(["This computer", "Device 2", "device 3", "Device 10"]);
    await user.click(within(header("Memory")).getByRole("button"));
    expect(names()).toEqual(["This computer", "device 3", "Device 2", "Device 10"]);
    await user.click(within(header("Memory")).getByRole("button"));
    expect(header("Memory").getAttribute("aria-sort")).toBe("descending");
    expect(names()).toEqual(["This computer", "Device 2", "device 3", "Device 10"]);
  });

  it("a new sort key demotes the old one to secondary; aria-sort is on the primary header only; the layout persists", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(within(header("State")).getByRole("button"));
    // Ties on State fall back to the default secondary key, Name.
    expect(names()).toEqual(["This computer", "Device 2", "Device 10", "device 3"]);
    await user.click(within(header("Memory")).getByRole("button"));
    expect(ui.get().tables.t.sort).toEqual([
      { id: "mem", desc: false },
      { id: "state", desc: false },
    ]);
    expect(header("Memory").getAttribute("aria-sort")).toBe("ascending");
    expect(header("State").hasAttribute("aria-sort")).toBe(false);
    expect(JSON.parse(localStorage.getItem("common.ui")!).tables.t.sort[0]).toEqual({ id: "mem", desc: false });
  });

  it("is one Tab stop; ↑↓ select keeping the column, ←→ move cells, ↑ from the first row reaches the header", async () => {
    const user = userEvent.setup();
    const onActivate = vi.fn();
    render(<Harness onActivate={onActivate} />);
    await user.tab();
    const first = document.activeElement as HTMLElement;
    expect(first.getAttribute("role")).toBe("gridcell");
    expect(first.textContent).toBe("This computer");
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement?.textContent).toBe("8 GB");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.textContent).toBe("16 GB");
    expect(screen.getAllByRole("row")[2].getAttribute("aria-selected")).toBe("true");
    await user.keyboard("{End}");
    expect(document.activeElement?.textContent).toBe("—Not reported while disconnected");
    await user.keyboard("{Enter}");
    expect(onActivate).toHaveBeenCalledWith(ROWS[0]);
    await user.keyboard("{Home}{ArrowUp}");
    expect(document.activeElement).toBe(within(header("Memory")).getByRole("button"));
    await user.keyboard("{Enter}");
    expect(header("Memory").getAttribute("aria-sort")).toBe("ascending");
    await user.tab();
    expect(screen.getByRole("grid").contains(document.activeElement)).toBe(false);
  });

  it("notes are part of the cell text; a detail is the focused cell's description", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const cell = within(screen.getAllByRole("row")[4]).getAllByRole("gridcell")[1];
    expect(cell.textContent).toBe("—Not reported while disconnected");
    await user.tab();
    await user.keyboard("{ArrowDown}{ArrowRight}{ArrowRight}");
    const focused = document.activeElement as HTMLElement;
    expect(focused.textContent).toContain("Connected");
    const tip = document.getElementById(focused.getAttribute("aria-describedby")!)!;
    expect(tip.getAttribute("role")).toBe("tooltip");
    expect(tip.textContent).toBe("seen at 10:32");
    expect(tip.hidden).toBe(false);
  });

  it("stale rows are dimmed and live columns say Unknown (not refreshed)", () => {
    render(<Harness stale />);
    const row = screen.getAllByRole("row")[2];
    expect(row.className).toContain("stale");
    expect(within(row).getAllByRole("gridcell")[1].textContent).toBe("Unknown (not refreshed)");
    expect(within(row).getAllByRole("gridcell")[0].textContent).toBe("Device 2");
  });

  it("keeps focus on the same row across a poll that reorders rows; when the row goes, focus moves to the table", async () => {
    const user = userEvent.setup();
    const { rerender } = render(<Harness rows={ROWS} />);
    await user.tab();
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.textContent).toBe("Device 2");
    rerender(<Harness rows={[...ROWS].reverse().map((r) => (r.id === "b" ? { ...r, name: "Device 2" } : r))} />);
    expect(document.activeElement?.textContent).toBe("Device 2");
    rerender(<Harness rows={ROWS.filter((r) => r.id !== "b")} />);
    expect(document.activeElement).toBe(screen.getByRole("grid"));
  });

  it("the filter matches visible text and notes; no row left shows the empty state", () => {
    const { rerender } = render(<Harness filter="device 1" />);
    expect(names()).toEqual(["Device 10"]);
    rerender(<Harness filter="not reported" />);
    expect(names()).toEqual(["Device 10"]);
    rerender(<Harness filter="zzz" />);
    expect(screen.queryByRole("grid")).toBeNull();
    expect(screen.getByText("Nothing here")).toBeTruthy();
  });

  it("the header context menu chooses columns; the last visible column cannot be hidden", async () => {
    render(<Harness />);
    expect(screen.queryByRole("columnheader", { name: "Device ID" })).toBeNull();
    fireEvent.contextMenu(header("Name"));
    const items = vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label: string; checked: boolean; enabled: boolean; action: () => void }[];
    expect(items.map((i) => [i.label, i.checked])).toEqual([
      ["Name", true],
      ["Memory", true],
      ["State", true],
      ["Device ID", false],
    ]);
    act(() => items[3].action());
    expect(screen.getByRole("columnheader", { name: "Device ID" })).toBeTruthy();
    act(() => ui.set((u) => ({ ...u, tables: { t: { columns: { mem: false, state: false, id: false } } } })));
    fireEvent.contextMenu(header("Name"));
    const only = vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label: string; enabled: boolean }[];
    expect(only.find((i) => i.label === "Name")!.enabled).toBe(false);
  });

  it("the column chooser opens from the keyboard too: Shift+F10 or the Menu key on a focused header", async () => {
    const user = userEvent.setup();
    render(<Harness menu={() => [{ label: "Copy ID", action: () => undefined }]} />);
    await user.tab();
    await user.keyboard("{ArrowUp}");
    expect(document.activeElement).toBe(within(header("Name")).getByRole("button"));
    vi.mocked(popupMenu).mockClear();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    const items = vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label: string; checked: boolean; action: () => void }[];
    expect(items.map((i) => i.label)).toEqual(["Name", "Memory", "State", "Device ID"]);
    act(() => items[3].action());
    expect(screen.getByRole("columnheader", { name: "Device ID" })).toBeTruthy();
    vi.mocked(popupMenu).mockClear();
    await user.keyboard("{ContextMenu}");
    expect((vi.mocked(popupMenu).mock.calls.at(-1)![0] as { label: string }[]).map((i) => i.label)).toContain("Device ID");
  });

  it("row context menu (Shift+F10) and delete key go to the row's handlers; rows carry their copy text", async () => {
    const user = userEvent.setup();
    const onDelete = vi.fn();
    const menu = vi.fn(() => [{ label: "Copy ID", action: () => undefined }]);
    render(<Harness menu={menu} onDelete={onDelete} onCopy={(r) => r.id} />);
    await user.tab();
    await user.keyboard("{Shift>}{F10}{/Shift}");
    expect(menu).toHaveBeenCalledWith(ROWS[1]);
    await user.keyboard(navigator.platform.includes("Mac") ? "{Meta>}{Backspace}{/Meta}" : "{Delete}");
    expect(onDelete).toHaveBeenCalledWith(ROWS[1]);
    expect(screen.getAllByRole("row")[1].getAttribute("data-copy")).toBe("self");
  });

  it("limit shows the first rows after sorting", () => {
    render(<Harness limit={2} />);
    expect(names()).toEqual(["This computer", "Device 2"]);
  });
});
