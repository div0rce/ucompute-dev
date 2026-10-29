import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { mapError } from "../api/errors";
import { groups, mesh, network, type Live } from "../state/live";
import type { DaemonState } from "../state/stores";
import { installFakeDaemon, NETWORK_ON, PEER } from "../test/mocks";
import { DataTable } from "./DataTable";
import { Dialog } from "./Dialog";
import { FirewallNotice, firewall } from "./FirewallNotice";
import { GroupMembership, GroupsEditor } from "./Groups";
import { Icon } from "./icons";
import { ScopeBar, ToolButton } from "./primitives";
import { frameState, SectionFrame } from "./SectionFrame";
import { Sparkline } from "./Sparkline";
import { Tip } from "./Tip";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
afterEach(() => {
  vi.unstubAllGlobals();
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  invoke.mockReset();
});

const UP: DaemonState = { reachable: true, responding: true, version: "0.1.0", status: null, spawned: false, at: 1000 };
const live = <T,>(data: T | null, at: number | null = 2000, error: Live<T>["error"] = null): Live<T> => ({ data, at, error });

describe("Tip", () => {
  it("shows on hover and on keyboard focus, hides on Esc, and is the element's description", async () => {
    const user = userEvent.setup();
    render(
      <Tip text="Version 0.1.0">
        <button type="button">Running</button>
      </Tip>,
    );
    const btn = screen.getByRole("button", { name: "Running" });
    const tip = document.getElementById(btn.getAttribute("aria-describedby")!)!;
    expect(tip.getAttribute("role")).toBe("tooltip");
    expect(tip.hidden).toBe(true);
    await user.hover(btn);
    expect(tip.hidden).toBe(false);
    await user.unhover(btn);
    await user.tab();
    expect(tip.hidden).toBe(false);
    await user.keyboard("{Escape}");
    expect(tip.hidden).toBe(true);
    expect(btn.hasAttribute("title")).toBe(false);
  });
});

describe("ToolButton and ScopeBar", () => {
  it("a disabled action stays focusable, says why, and does nothing", async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<ToolButton icon="plug" label="Connect" onClick={onClick} disabledReason="Networking is off" />);
    const btn = screen.getByRole("button", { name: "Connect" });
    expect(btn.getAttribute("aria-disabled")).toBe("true");
    expect(document.getElementById(btn.getAttribute("aria-describedby")!)!.textContent).toBe("Networking is off");
    await user.tab();
    expect(document.activeElement).toBe(btn);
    await user.keyboard("{Enter}");
    await user.click(btn);
    expect(onClick).not.toHaveBeenCalled();
  });

  it("keeps the same focused button when it becomes busy, disabled or enabled again", () => {
    const { rerender } = render(<ToolButton icon="play" label="Load" onClick={() => undefined} />);
    const btn = screen.getByRole("button", { name: "Load" });
    btn.focus();
    rerender(<ToolButton icon="play" label="Load" onClick={() => undefined} busy disabledReason="Loading" />);
    expect(screen.getByRole("button", { name: "Load" })).toBe(btn);
    rerender(<ToolButton icon="play" label="Load" onClick={() => undefined} />);
    expect(screen.getByRole("button", { name: "Load" })).toBe(btn);
    expect(document.activeElement).toBe(btn);
    expect(btn.hasAttribute("aria-describedby")).toBe(false);
  });

  it("the scope bar is a labelled group of pressed buttons with counts", async () => {
    const user = userEvent.setup();
    function S() {
      const [v, setV] = useState("all");
      return <ScopeBar label="Show" value={v} onChange={setV} items={[{ id: "all", label: "All", count: 5 }, { id: "connected", label: "Connected", count: 2 }]} />;
    }
    render(<S />);
    const group = screen.getByRole("group", { name: "Show" });
    await user.click(within(group).getByRole("button", { name: "Connected 2" }));
    expect(within(group).getByRole("button", { name: "Connected 2" }).getAttribute("aria-pressed")).toBe("true");
    expect(within(group).getByRole("button", { name: "All 5" }).getAttribute("aria-pressed")).toBe("false");
  });
});

describe("Icon", () => {
  it("Stop is a filled square (the outline square alone reads as an empty checkbox on an icon-only button)", () => {
    const { container } = render(<Icon name="stop" />);
    expect(container.querySelector("rect")?.getAttribute("fill")).toBe("currentColor");
    const outline = render(<Icon name="square" />).container;
    expect(outline.querySelector("rect")?.getAttribute("fill")).toBeNull();
  });
});

describe("Dialog", () => {
  it("opens modally with focus on the data-autofocus control, closes on Esc and returns focus to the opener", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    function D() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" onClick={() => setOpen(true)}>
            Download…
          </button>
          <Dialog
            open={open}
            label="Download Qwen"
            onClose={() => {
              onClose();
              setOpen(false);
            }}
          >
            <button type="button">View licence</button>
            <button type="button" data-autofocus="">
              Confirm
            </button>
          </Dialog>
        </>
      );
    }
    render(<D />);
    const opener = screen.getByRole("button", { name: "Download…" });
    await user.click(opener);
    expect(screen.getByRole("dialog", { name: "Download Qwen" })).toBeTruthy();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Confirm" }));
    await user.keyboard("{Escape}");
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });
});

describe("Sparkline", () => {
  it("draws no line across a gap and names last/min/max per series", () => {
    const { container } = render(<Sparkline label="Tokens/s" series={[{ label: "On this computer", values: [1, 2, null, 4, 5] }, { label: "Served for devices", values: [null, null] }]} />);
    expect(container.querySelectorAll("polyline")).toHaveLength(2);
    expect(screen.getByRole("img").getAttribute("aria-label")).toBe("Tokens/s. On this computer: last 5, min 1, max 5; Served for devices: no data");
  });
});

describe("frameState and SectionFrame", () => {
  it("only a service that does not answer at all is down; a timeout with /health answering is busy", () => {
    expect(frameState({ ...UP, reachable: null })).toBe("connecting");
    expect(frameState({ ...UP, reachable: false })).toBe("down");
    expect(frameState({ ...UP, responding: false }, live([1], 1500))).toEqual({ busy: { since: 1500 } });
    // Busy before a store was ever read: nothing to keep, so its empty state ("Nothing is running") is not shown.
    expect(frameState({ ...UP, responding: false }, live([1], 1500), live(null, null))).toBe("loading");
    expect(frameState({ ...UP, responding: false }, live(null, null, mapError("slow", "no answer")))).toBe("loading");
    const err = mapError("slow", "no answer");
    expect(frameState(UP, live([1], 1500, err))).toEqual({ stale: { since: 1500, error: err } });
    expect(frameState(UP, live(null, null))).toBe("loading");
    expect(frameState(UP, live([1]))).toBe("ok");
  });

  it("a first read that failed is failed, not stale: there is no data to show as empty", () => {
    const err = mapError("slow", "no answer");
    expect(frameState(UP, live(null, null, err))).toEqual({ failed: err });
    // One store never read wins over another's stale data: its "nothing here" text would be a guess.
    expect(frameState(UP, live([1], 1500, err), live(null, null, err))).toEqual({ failed: err });
    render(
      <SectionFrame title="Models" state={{ failed: err }} inspector={<p>inspector</p>}>
        <p>No catalog entries; import a GGUF file</p>
      </SectionFrame>,
    );
    expect(screen.queryByText("No catalog entries; import a GGUF file")).toBeNull();
    expect(screen.queryByText("inspector")).toBeNull();
    expect(screen.getByText(`Not read from the service: ${err.title}`)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });

  const Table = () => (
    <DataTable
      id="m"
      label="Models"
      columns={[
        { id: "name", header: "Name", cell: (r: { id: string }) => r.id, sort: (r) => r.id },
        { id: "state", header: "State", cell: () => "Loaded", live: true },
      ]}
      rows={[{ id: "qwen" }]}
      rowKey={(r) => r.id}
      selected="qwen"
      onSelect={() => undefined}
      empty="none"
    />
  );

  it("stale data keeps the rows, dimmed, with the banner and its details; selection actions are disabled with the reason", () => {
    const err = mapError("slow", "No answer within 5000 ms");
    render(
      <SectionFrame title="Models" state={{ stale: { since: Date.parse("2026-10-22T10:32:05"), error: err } }} selectionActions={<ToolButton icon="play" label="Load" onClick={() => undefined} />} actions={<ToolButton icon="download" label="Import" onClick={() => undefined} />}>
        <Table />
      </SectionFrame>,
    );
    expect(screen.getByRole("heading", { level: 1, name: "Models" })).toBeTruthy();
    expect(screen.getByText(/^Not updated since 10:32:05: UCompute's background service is running but did not answer in time\.$/)).toBeTruthy();
    const row = screen.getAllByRole("row")[1];
    expect(row.className).toContain("stale");
    expect(within(row).getAllByRole("gridcell")[1].textContent).toBe("Unknown (not refreshed)");
    expect(screen.getByRole("button", { name: "Load" }).getAttribute("aria-disabled")).toBe("true");
    expect(screen.getByRole("button", { name: "Import" }).hasAttribute("aria-disabled")).toBe(false);
    expect(screen.getByRole("toolbar", { name: "Models actions" })).toBeTruthy();
  });

  it("busy says so with the time; down replaces the table with the reason and Try again", () => {
    const { rerender } = render(
      <SectionFrame title="Models" state={{ busy: { since: Date.parse("2026-10-22T10:32:05") } }}>
        <Table />
      </SectionFrame>,
    );
    expect(screen.getByText("Service running, busy (likely loading a model). Not updated since 10:32:05.")).toBeTruthy();
    expect(screen.getByRole("grid", { name: "Models" })).toBeTruthy();
    rerender(
      <SectionFrame title="Models" state="down">
        <Table />
      </SectionFrame>,
    );
    expect(screen.queryByRole("grid")).toBeNull();
    expect(screen.getByText("The background service is not running")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Try again" })).toBeTruthy();
  });

  it("the toolbar is one Tab stop with arrow-key roving; the filter is its own stop and Esc clears it", async () => {
    const user = userEvent.setup();
    function F() {
      const [q, setQ] = useState("qw");
      return (
        <SectionFrame title="Models" state="ok" actions={<ToolButton icon="download" label="Import" onClick={() => undefined} />} selectionActions={<ToolButton icon="play" label="Load" onClick={() => undefined} />} filter={{ value: q, onChange: setQ }}>
          <p>body</p>
        </SectionFrame>
      );
    }
    render(<F />);
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Import" }));
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Load" }));
    await user.tab();
    const box = screen.getByRole("searchbox", { name: "Filter" });
    expect(document.activeElement).toBe(box);
    await user.keyboard("{Escape}");
    expect((box as HTMLInputElement).value).toBe("");
  });
});

describe("Groups", () => {
  const G = { name: "home", members: [] as string[], policy: { allow_remote_inference: false, allow_pooled_inference: true } };

  it("never shows the pooled flag and sends it back unchanged", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: [G] });
    groups.set(live([G]));
    mesh.set(live({ peers: [PEER], discovered: [], revocations: [] }));
    render(<GroupsEditor />);
    expect(screen.queryByText(/pool/i)).toBeNull();
    await user.click(screen.getByRole("checkbox", { name: "Members may run prompts for each other" }));
    await waitFor(() => expect(fake.groups[0].policy).toEqual({ allow_remote_inference: true, allow_pooled_inference: true }));
    expect(groups.get().data![0].policy.allow_remote_inference).toBe(true);
  });

  it("membership for one device: join a group, then allow prompts there", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: [G] });
    groups.set(live([G]));
    mesh.set(live({ peers: [PEER], discovered: [], revocations: [] }));
    render(<GroupMembership peerId={PEER.id} />);
    await user.click(screen.getByRole("checkbox", { name: "In home" }));
    await waitFor(() => expect(fake.groups[0].members).toEqual([PEER.id]));
    await user.click(await screen.findByRole("checkbox", { name: "Members of home may run prompts for each other" }));
    await waitFor(() => expect(fake.groups[0].policy.allow_remote_inference).toBe(true));
  });
});

describe("FirewallNotice", () => {
  const windows = (status: object, allow: () => Promise<void> = async () => {}) => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    invoke.mockImplementation(async (cmd: string) => (cmd === "firewall_status" ? status : cmd === "allow_through_firewall" ? allow() : undefined));
  };

  it("says what was not found (it cannot prove a block), and Allow asks Windows only on click", async () => {
    const user = userEvent.setup();
    windows({ supported: true, allowed: false, rule_program: null, public_networks: [] });
    network.set(live(NETWORK_ON));
    render(<FirewallNotice />);
    expect(await screen.findByText("No Windows Firewall rule for the service was found; other devices may be unable to connect.")).toBeTruthy();
    expect(invoke.mock.calls.filter(([c]) => c === "allow_through_firewall")).toHaveLength(0);
    await user.click(screen.getByRole("button", { name: "Allow on private networks" }));
    expect((await screen.findByText("Allowed on private networks.")).getAttribute("role")).toBe("status");
    expect(firewall.get()?.allowed).toBe(true);
  });

  it("stays away off Windows, outside the app and with networking off", async () => {
    network.set(live(NETWORK_ON));
    const { container, unmount } = render(<FirewallNotice />);
    await act(async () => undefined);
    expect(container.textContent).toBe("");
    unmount();
    windows({ supported: true, allowed: false, rule_program: null });
    network.set(live({ ...NETWORK_ON, enabled: false }));
    const off = render(<FirewallNotice />);
    await act(async () => undefined);
    expect(off.container.textContent).toBe("");
  });
});
