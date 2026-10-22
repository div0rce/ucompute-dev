// Activity (owner D, plan §2.6): scopes, the device select and the filter work client-side; collapsed runs show
// their count; Show device / Show job navigate; Copy diagnostics falls back to status and hardware and never holds a
// message; the table never shows chat text.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import type { AuditEvent } from "../../api/client";
import { refreshStatus } from "../../daemon";
import { chatHistory, daemon, lastError, privacy } from "../../state/stores";
import { AUDIT, AUDIT_E4, installFakeDaemon, JOBS, NETWORK_ON, PEER, PEERS_E1 } from "../../test/mocks";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  chatHistory.set({ messages: [{ role: "user", content: "canary-prompt-7f3a" }, { role: "assistant", content: "canary-reply-9b1c" }] });
  lastError.set(null);
  privacy.set(null);
});
afterEach(() => vi.unstubAllGlobals());

const REJECT: AuditEvent[] = [1, 2, 3].map((s) => ({ ts_utc: `2026-10-22T10:35:0${s}Z`, kind: "peer_rejected", peer_id: PEER.id, fields: { addr: "192.168.1.77:8193", reason: "peer_unauthorized", detail: "x", suppressed: 0 } }));

async function open() {
  const fake = installFakeDaemon({ network: NETWORK_ON, peers: PEERS_E1.peers, discovered: PEERS_E1.discovered, audit: [...AUDIT, ...AUDIT_E4, ...REJECT], jobs: JOBS });
  await refreshStatus();
  window.location.hash = "#/activity";
  render(<App poll={false} />);
  const grid = await screen.findByRole("grid", { name: "Activity" });
  await within(grid).findByText(/Lost connection to Laptop/);
  return { fake, grid };
}
const rowNames = (grid: HTMLElement) => within(grid).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("gridcell")[3].textContent);

describe("Activity", () => {
  it("shows audit events and local jobs newest first, collapsing the refusal flood into one row", async () => {
    const { grid } = await open();
    expect(rowNames(grid)).toEqual([
      "Refused Studio Mac: This computer is not authorized to run that on the selected device (192.168.1.77:8193)",
      "Refused Studio Mac: disconnected by you (192.168.1.77:8193)",
      "Disconnected Studio Mac: by you",
      "Lost connection to Laptop: timed out after 1 min",
      "Qwen2.5 1.5B Instruct (Q4_K_M) on this computer: done, 160 tokens in 4 s",
      "Qwen2.5 1.5B Instruct (Q4_K_M) on Studio Mac: done, 120 tokens in 4.6 s",
      "Connected to Laptop (outbound) at 172.20.10.6:8193",
      "Qwen2.5 1.5B Instruct (Q4_K_M) on this computer: failed: Common is busy with other requests, tokens unknown in 0 s",
      "Paired with Studio Mac",
      "Pairing window opened for 120 s",
    ]);
    expect(within(within(grid).getAllByRole("row")[1]).getByText("×3")).toBeTruthy();
    // Never a prompt or a reply.
    expect(screen.getByRole("main").textContent).not.toMatch(/canary/);
  });

  it("filters by scope, device and text, client-side", async () => {
    const user = userEvent.setup();
    const { fake, grid } = await open();
    const calls = fake.fetchMock.mock.calls.length;
    await user.click(screen.getByRole("button", { name: /^Warnings and errors/ }));
    expect(rowNames(grid)).toEqual([
      "Refused Studio Mac: This computer is not authorized to run that on the selected device (192.168.1.77:8193)",
      "Lost connection to Laptop: timed out after 1 min",
      "Qwen2.5 1.5B Instruct (Q4_K_M) on this computer: failed: Common is busy with other requests, tokens unknown in 0 s",
    ]);
    await user.click(screen.getByRole("button", { name: /^Execution/ }));
    expect(screen.getByText("Local jobs since the service started; kept in memory (last 256).")).toBeTruthy();
    // Local jobs, plus a job sent to a device that the audit does not cover (no matching request_id here).
    expect(rowNames(grid)).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: /^All/ }));
    await user.selectOptions(screen.getByRole("combobox", { name: "Device" }), "Laptop");
    expect(rowNames(grid)).toEqual(["Lost connection to Laptop: timed out after 1 min", "Connected to Laptop (outbound) at 172.20.10.6:8193"]);
    await user.selectOptions(screen.getByRole("combobox", { name: "Device" }), "");
    await user.type(screen.getByRole("searchbox", { name: "Filter activity" }), "pairing");
    // The filter matches every visible column, the category included.
    expect(rowNames(grid)).toEqual(["Paired with Studio Mac", "Pairing window opened for 120 s"]);
    await user.type(screen.getByRole("searchbox", { name: "Filter activity" }), " window");
    expect(rowNames(grid)).toEqual(["Pairing window opened for 120 s"]);
    expect(fake.fetchMock.mock.calls.length).toBe(calls);
  });

  it("Show device and Show job go to the selected event's device and job", async () => {
    const user = userEvent.setup();
    const { grid } = await open();
    expect(screen.getByRole("button", { name: "Show device" }).getAttribute("aria-disabled")).toBe("true");
    await user.click(within(grid).getByText("Disconnected Studio Mac: by you"));
    expect(window.location.hash).toMatch(/^#\/activity\/2026-10-22T10%3A32%3A00Z/);
    await user.click(screen.getByRole("button", { name: "Show device" }));
    await waitFor(() => expect(window.location.hash).toBe(`#/devices/${PEER.id}`));

    act(() => void (window.location.hash = "#/activity"));
    const g = await screen.findByRole("grid", { name: "Activity" });
    await user.click(within(g).getByText(/on this computer: done, 160 tokens/));
    await user.click(screen.getByRole("button", { name: "Show job" }));
    await waitFor(() => expect(window.location.hash).toBe("#/jobs/chatcmpl-1a2b3c4d"));
  });

  it("the inspector lists every field and the raw JSON of the selected event", async () => {
    const user = userEvent.setup();
    const { grid } = await open();
    await user.click(within(grid).getByText(/^Lost connection to Laptop/));
    const panel = screen.getByRole("region", { name: "Event details" });
    expect(within(panel).getByText("connected_s")).toBeTruthy();
    expect(within(panel).getAllByText("timed_out").length).toBe(2); // Code, and the `reason` field
    expect(within(panel).getByText("Raw JSON")).toBeTruthy();
  });

  it("Copy diagnostics falls back to status and hardware and never includes a message", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await open();
    await user.click(screen.getByRole("button", { name: "Copy diagnostics" }));
    await screen.findByText("Diagnostics copied. They contain status and hardware details, never your messages.");
    const copied = JSON.parse(writeText.mock.calls[0][0] as string);
    expect(Object.keys(copied)).toEqual(expect.arrayContaining(["service_url", "status", "hardware"]));
    expect(JSON.stringify(copied)).not.toMatch(/canary/);
  });
});

describe("Activity scope row at narrow widths", () => {
  // jsdom has no layout; the 1024 px screenshot is the visual check. This keeps the rule from being dropped: the shared
  // row scrolls (hidden scrollbar on macOS), so Activity's seven scopes wrap instead.
  it("wraps rather than scrolling the last scopes out of sight", async () => {
    const { readFileSync } = await import("node:fs");
    const { join } = await import("node:path");
    const css = readFileSync(join(__dirname, "activity.css"), "utf8");
    expect(css).toMatch(/\.frame-scope:has\(\.activity-device\) \{[^}]*height: auto;[^}]*flex-wrap: wrap;/);
    expect(css).toMatch(/\.frame-scope:has\(\.activity-device\) \.scope-bar \{[^}]*flex-wrap: wrap;[^}]*overflow-x: visible;/);
  });
});
