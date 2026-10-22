// Test helpers for the Devices section (owner B): render it and read the grid by column header, not by cell index.
import { render, screen, within } from "@testing-library/react";
import type { installFakeDaemon } from "../../test/mocks";
import Devices from "./Devices";
import { pairPoll } from "./PairDialog";

/** Renders the section at `#/devices…` (the router reads the hash); pairing status is polled every 20 ms. */
export function renderDevices(hash = "#/devices") {
  pairPoll.ms = 20;
  window.location.hash = hash;
  return render(<Devices />);
}

export const grid = () => screen.findByRole("grid", { name: "Devices" });
/** The row whose text contains `name`. */
export const rowOf = async (name: string) => within(await grid()).findByRole("row", { name: (n) => n.includes(name) });
/** Text a sighted user sees in `el`: tooltips (hidden until hover or focus) and screen-reader-only text removed. */
export function seen(el: Element): string {
  const c = el.cloneNode(true) as Element;
  c.querySelectorAll('[role="tooltip"], .visually-hidden').forEach((n) => n.remove());
  return c.textContent ?? "";
}
/** The cell of `name`'s row under the column headed `header`. */
export async function cell(name: string, header: string): Promise<HTMLElement> {
  const g = await grid();
  const col = within(g)
    .getAllByRole("columnheader")
    .findIndex((h) => h.textContent === header);
  if (col < 0) throw new Error(`no column ${header}`);
  return within(await rowOf(name)).getAllByRole("gridcell")[col];
}
export const cellText = async (name: string, header: string) => seen(await cell(name, header));
export const inspector = () => screen.getByRole("region", { name: "Device details" });
export const toolbar = () => screen.getByRole("toolbar", { name: "Devices actions" });
/** The selection actions' names (the section action "Pair a device…" left out). */
export const selectionActions = () =>
  within(toolbar())
    .getAllByRole("button")
    .map((b) => b.textContent ?? "")
    .filter((t) => t !== "Pair a device…" && !t.startsWith("Pairing open"));
export const dialog = () => screen.getByRole("dialog", { name: "Pair a device" });

export const calls = (fake: ReturnType<typeof installFakeDaemon>, method: string, path: string) =>
  fake.fetchMock.mock.calls.filter(([u, init]) => String(u).endsWith(path) && ((init as RequestInit)?.method ?? "GET") === method);
export const bodies = (fake: ReturnType<typeof installFakeDaemon>, method: string, path: string) =>
  calls(fake, method, path).map(([, init]) => JSON.parse(String((init as RequestInit).body ?? "{}")));
