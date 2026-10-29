// The shell (owner A): router aliases and selection, rail, status bar, attention, announcer, shortcuts, copy, and
// cases moved here in Phase 0 (the "this Mac" sweep from devices.test.tsx). The Phase 0 "RunOn on Home" case
// (from privacy.test.tsx) is replaced by the Dashboard case "says where prompts can run" in dashboard.test.tsx.
import { act, render, renderHook, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { go, href, parseHash, select, useLocation } from "../router";
import { announce, Announcer, THROTTLE_MS } from "../shell/announcer";
import { dismissAttention, startWatchers } from "../shell/attention";
import { installCopyHandler } from "../shell/copy";
import { Rail } from "../shell/Rail";
import { installShortcuts, onShortcut } from "../shell/shortcuts";
import { StatusBar } from "../shell/StatusBar";
import { jobs, mesh, network, pairing, pulls, refresh, samples, type Live } from "../state/live";
import { chatHistory, daemon, lastError, privacy, runTarget, settings } from "../state/stores";
import { applyTheme } from "../state/ui";
import { mapError } from "../api/errors";
import { hang, installFakeDaemon, JOBS, LOADED, MODELS, NETWORK_ON, PEER, PEERS_E1, PRIVACY, STATUS, STRANGER } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  chatHistory.set({ messages: [] });
  lastError.set(null);
  privacy.set({ data: null, at: null, error: null });
  runTarget.set("local");
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

const live = <T,>(data: T): Live<T> => ({ data, at: Date.now(), error: null });
const bar = () => screen.getByRole("contentinfo", { name: "Status" });
const item = (text: RegExp | string) => within(bar()).getByText(text).closest(".sb-item") as HTMLElement;
const setPlatform = (p: string) => Object.defineProperty(navigator, "platform", { value: p, configurable: true });

const TRUSTED = { ...PRIVACY, mode: "trusted_group" as const };

describe("E1: no macOS wording on other systems (moved from devices.test.tsx)", () => {
  it.each(["home", "models", "chat", "hardware", "settings", "peers", "privacy"])("#/%s never says this Mac", async (route) => {
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: route === "privacy" ? PRIVACY : TRUSTED });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = `#/${route}`;
    const { container } = render(<App poll={false} />);
    await screen.findByRole("heading", { level: 1 });
    await new Promise((r) => setTimeout(r, 100)); // every fetch of the view answered
    const placeholders = [...container.querySelectorAll("[placeholder]")].map((e) => e.getAttribute("placeholder")).join(" ");
    expect(`${container.textContent} ${placeholders}`).not.toMatch(/this Mac/i);
  });
});

describe("router", () => {
  it.each([
    ["", "dashboard", null, ""],
    ["#/home", "dashboard", null, ""],
    ["#/cluster", "dashboard", null, ""],
    ["#/contributions", "dashboard", null, ""],
    ["#/nonsense", "dashboard", null, ""],
    ["#/peers", "devices", null, ""],
    ["#/hardware", "devices", "self", "tab=hardware"],
    ["#/performance", "jobs", null, ""],
    ["#/diagnostics", "activity", null, ""],
    ["#/privacy", "settings", "privacy", ""],
    ["#/models?confirm=qwen", "models", null, "confirm=qwen"],
    ["#/devices/self?tab=hardware", "devices", "self", "tab=hardware"],
  ])("%s -> %s", (hash, section, id, params) => {
    const loc = parseHash(hash);
    expect([loc.section, loc.id, loc.params.toString()]).toEqual([section, id, params]);
  });

  it("ids round-trip through href (activity keys hold | and :)", () => {
    const key = "2026-10-22T10:30:00Z|peer_lost|" + PEER.id;
    expect(parseHash(href("activity", key)).id).toBe(key);
  });

  it("a section change pushes history; a selection replaces it and re-renders", () => {
    window.location.hash = "#/devices";
    const { result } = renderHook(() => useLocation());
    const n = history.length;
    act(() => select(PEER.id, { tab: "hardware" }));
    expect(history.length).toBe(n);
    expect(window.location.hash).toBe(`#/devices/${PEER.id}?tab=hardware`);
    expect(result.current).toMatchObject({ section: "devices", id: PEER.id });
    act(() => go("models"));
    expect(history.length).toBe(n + 1);
  });
});

describe("status bar", () => {
  it("shows each item from the service: running, engine, slots, mesh, privacy link, disk", async () => {
    const fake = installFakeDaemon({ privacy: { ...PRIVACY, mode: "trusted_group" } });
    fake.loaded = LOADED;
    await refreshStatus();
    render(<App poll={false} />);
    await within(bar()).findByText("Disk 16.0 GB free");
    expect(item("Running").className).toContain("tone-ok");
    expect(within(bar()).getByText(`${MODELS[0].name} · Metal`)).toBeTruthy();
    expect(within(bar()).getByText("In use 0/1 · Waiting 0")).toBeTruthy();
    expect(within(bar()).getByText("Networking off")).toBeTruthy();
    expect(within(bar()).getByRole("link", { name: "Privacy mode: Trusted devices" }).getAttribute("href")).toBe("#/settings/privacy");
    expect(bar().querySelector("[aria-live]")).toBeNull();
  });

  it("a status timeout with /health answering reads Busy (not answering), not Not running", async () => {
    const fake = installFakeDaemon();
    await refreshStatus();
    fake.routes["GET /api/status"] = ({ init }) => hang(init);
    await refreshStatus(50);
    render(<StatusBar />);
    expect(item("Busy (not answering)").className).toContain("tone-warn");
    expect(within(bar()).queryByText("Not running")).toBeNull();
    expect(within(bar()).getByRole("button", { name: "Service busy (not answering)" })).toBeTruthy();
  });

  it("counts distinct live devices: two connections to one device is 1; not responding is apart, in warn", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    network.set(live({ ...NETWORK_ON, connections: 2 }));
    mesh.set(live({ peers: [{ ...PEER, health: "connected" }], discovered: [], revocations: [] }));
    const { rerender } = render(<StatusBar />);
    expect(item("1 device connected").className).toContain("tone-ok");
    const sick = { ...PEER, id: "ee".repeat(32), name: "Laptop", health: "unhealthy" as const };
    act(() => mesh.set(live({ peers: [{ ...PEER, health: "connected" }, sick], discovered: [], revocations: [] })));
    rerender(<StatusBar />);
    expect(item("1 device connected · 1 not responding").className).toContain("tone-warn");
  });

  it("is never ok from discovered entries alone (a cached mDNS record is not a connection)", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    network.set(live(NETWORK_ON));
    mesh.set(live({ peers: [{ ...PEER, connected: false, health: "disconnected" }], discovered: [{ ...STRANGER, id: PEER.id, trusted: true }, STRANGER], revocations: [] }));
    render(<StatusBar />);
    expect(item("Listening · no devices connected").className).toContain("tone-neutral");
  });

  it("a device list not read yet never reads as no devices connected", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    network.set(live(NETWORK_ON));
    mesh.set({ data: null, at: null, error: null });
    render(<StatusBar />);
    expect(item("Listening · devices —").className).toContain("tone-neutral");
    expect(bar().textContent).not.toMatch(/no devices/);
  });

  it("privacy mode: only while the service runs, and marked not updated after a failed read", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    privacy.set(live(TRUSTED));
    render(<StatusBar />);
    const link = (name: string) => within(bar()).getByRole("link", { name });
    expect(link("Privacy mode: Trusted devices").textContent).toBe("Trusted devices");
    act(() => privacy.set((p) => ({ ...p, error: mapError("internal", "store unreadable") })));
    expect(link("Privacy mode: Trusted devices").textContent).toBe("Trusted devicesnot updated");
    act(() => daemon.set((d) => ({ ...d, reachable: false })));
    expect(link("Privacy mode: unknown").textContent).toBe("Privacy mode unknown");
  });

  it("an item whose tip comes and goes is not remounted: the focused privacy link keeps focus", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    privacy.set(live(TRUSTED));
    render(<StatusBar />);
    const link = within(bar()).getByRole("link", { name: "Privacy mode: Trusted devices" });
    link.focus();
    act(() => privacy.set((p) => ({ ...p, error: mapError("internal", "store unreadable") })));
    expect(link.isConnected && document.activeElement === link).toBe(true);
    act(() => privacy.set(live(TRUSTED)));
    act(() => daemon.set((d) => ({ ...d, reachable: false })));
    expect(link.isConnected && document.activeElement === link).toBe(true);
  });

  it("focusable text items have stable names: Service by its state, Network rate without the changing rates", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    render(<StatusBar />);
    const service = within(bar()).getByRole("group", { name: "Service: Running" });
    expect(service.tabIndex).toBe(0);
    expect(document.getElementById(service.getAttribute("aria-describedby")!)!.textContent).toMatch(/^Service 0\.1\.0, up/);
    expect(within(bar()).getByRole("group", { name: "Network rate" }).tabIndex).toBe(0);
  });

  it("network rate: < 1 KB/s for keep-alive traffic, unknown across a connection id change, — without E1", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    const now = Date.now();
    // Samples kept from a 2 s poll are about 6 s apart, not 5: the tip gives the real window.
    samples.set({ conn: [{ at: now - 6000, conns: { "7": { peer: PEER.id, rx: 1000, tx: 1000 } } }, { at: now, conns: { "7": { peer: PEER.id, rx: 1720, tx: 1600 } } }], tokens: [] });
    const { rerender } = render(<StatusBar />);
    expect(item("Received").textContent).toBe("Received < 1 KB/s Sent < 1 KB/s");
    // The tip gives the exact rate and when it was measured.
    expect(within(bar()).getByText(/^Received 120 B\/s, sent 100 B\/s, over the 6 s to \d\d:\d\d:\d\d\. Includes 1 s keep-alives\.$/)).toBeTruthy();
    act(() => samples.set({ conn: [{ at: now - 5000, conns: { "7": { peer: PEER.id, rx: 0, tx: 0 } } }, { at: now, conns: { "9": { peer: PEER.id, rx: 900000, tx: 0 } } }], tokens: [] }));
    rerender(<StatusBar />);
    expect(item("Received").textContent).toBe("Received — Sent —");
  });

  it("sharing reads Serving 1 request for <device> from an active served row", async () => {
    installFakeDaemon({ network: { ...NETWORK_ON, serve_peers: true }, peers: PEERS_E1.peers, jobs: JOBS });
    await refreshStatus();
    render(<App poll={false} />);
    const serving = await within(bar()).findByText("Serving 1 request for Studio Mac");
    expect(serving.closest("a")!.getAttribute("href")).toBe("#/jobs?scope=served");
    // The firewall half moved to dashboard.test.tsx, where the Windows app's firewall command is mocked: the Dashboard
    // re-reads it on mount, which outside the app reads "unsupported".
  });

  it("droppable items carry their drop order; Attention, Service, Mesh and Privacy never drop", async () => {
    installFakeDaemon({ network: NETWORK_ON });
    await refreshStatus();
    render(<App poll={false} />);
    await within(bar()).findByText(/^Disk/);
    const drop = (el: HTMLElement) => [...el.classList].find((c) => c.startsWith("sb-drop-")) ?? null;
    expect(drop(item(/^Disk/))).toBe("sb-drop-1");
    expect(drop(item("Received"))).toBe("sb-drop-2");
    expect(drop(item("Not sharing"))).toBe("sb-drop-3");
    expect(drop(item(/^In use/))).toBe("sb-drop-4");
    for (const t of ["Running", "Listening · no devices connected", "Local only"]) expect(drop(item(t)), t).toBeNull();
  });

  it("engine: loading with its model and time (E2); before E2 two loading answers in a row say Loading a model", async () => {
    const fake = installFakeDaemon();
    fake.loading = { model_id: MODELS[0].id, since: Date.now() - 12000 };
    await refreshStatus();
    await refresh("models");
    const { unmount } = render(<StatusBar />);
    expect(item(`Loading ${MODELS[0].name}`).textContent).toBe(`Loading ${MODELS[0].name} · 12 s`);
    // The seconds change every poll: hidden from the link's name, given in its tip.
    const link = within(bar()).getByRole("link", { name: `Loading ${MODELS[0].name}` });
    expect(document.getElementById(link.getAttribute("aria-describedby")!)!.textContent).toBe("Loading for 12 s.");
    unmount();
    fake.e2 = false;
    await refreshStatus();
    await refresh("ready");
    render(<StatusBar />);
    expect(within(bar()).getByText("No model loaded")).toBeTruthy();
    await act(() => refresh("ready"));
    expect(within(bar()).getByText("Loading a model")).toBeTruthy();
  });
});

describe("status bar never shows more than the service reported", () => {
  const withData = () => {
    network.set(live({ ...NETWORK_ON, connections: 1 }));
    mesh.set(live({ peers: [{ ...PEER, health: "connected" }], discovered: [], revocations: [] }));
    const now = Date.now();
    samples.set({ conn: [{ at: now - 5000, conns: { "7": { peer: PEER.id, rx: 0, tx: 0 } } }, { at: now, conns: { "7": { peer: PEER.id, rx: 50000, tx: 50000 } } }], tokens: [] });
  };

  it("while the service is not running: no device count, no engine, no rate; Starting is not known either", () => {
    withData();
    daemon.set({ reachable: false, responding: true, version: null, status: null, spawned: false });
    const { rerender } = render(<StatusBar />);
    expect(within(bar()).queryByText(/device connected/)).toBeNull();
    expect(item("Networking —").className).toContain("tone-neutral");
    expect(within(bar()).getByText("Engine —")).toBeTruthy();
    // Engine, Mesh and Privacy all say why.
    expect(within(bar()).getAllByText("Not known while the service is not running.")).toHaveLength(3);
    expect(item("Received").textContent).toBe("Received — Sent —");
    act(() => daemon.set({ reachable: null, responding: true, version: null, status: null, spawned: false }));
    rerender(<StatusBar />);
    expect(item("Starting").className).toContain("tone-active");
    expect(within(bar()).getByText("Engine —")).toBeTruthy();
    expect(within(bar()).getAllByText("Not known until the service answers.")).toHaveLength(3);
  });

  it("a failed refresh keeps the last text, drops its colour and says it is not updated, with the time", () => {
    withData();
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: { ...STATUS, loaded: LOADED }, spawned: false });
    const at = Date.parse("2026-10-22T10:30:00Z");
    mesh.set({ data: { peers: [{ ...PEER, health: "connected" }], discovered: [], revocations: [] }, at, error: { code: "slow", title: "t", next_step: "n", detail: "", hint: null } });
    const { rerender } = render(<StatusBar />);
    const mesh1 = item("1 device connected");
    expect(mesh1.className).toContain("tone-neutral");
    expect(mesh1.textContent).toBe("1 device connected" + "not updated");
    expect(document.getElementById(mesh1.getAttribute("aria-describedby")!)!.textContent).toMatch(/^Not updated since \d\d:\d\d:\d\d\. 1 connection\./);
    // A status poll that timed out: the engine and slots are the last reading, marked the same way.
    act(() => daemon.set((d) => ({ ...d, responding: false, at })));
    rerender(<StatusBar />);
    expect(item(`${LOADED.model_id} · Metal`).textContent).toContain("not updated");
    expect(item(/^In use/).className).toContain("tone-neutral");
    // So is a status poll answered with an HTTP error: the service runs, but the reading is the last good one.
    act(() => daemon.set((d) => ({ ...d, responding: true, statusError: mapError("internal", "500") })));
    rerender(<StatusBar />);
    expect(item(`${LOADED.model_id} · Metal`).textContent).toContain("not updated");
    expect(item(/^In use/).textContent).toContain("not updated");
    expect(item("Running").className).toContain("tone-ok");
  });

  it("the disk tip gives the exact free bytes and when they were measured", async () => {
    installFakeDaemon();
    await refreshStatus();
    await refresh("hardware");
    render(<StatusBar />);
    expect(within(bar()).getByText(/^17,179,869,184 bytes free, measured at \d\d:\d\d:\d\d\.$/)).toBeTruthy();
  });
});

describe("webview context menu", () => {
  it("is suppressed on chrome and tables, kept in text fields and on selected text", () => {
    const off = installShortcuts();
    render(
      <>
        <a href="#/devices">Devices</a>
        <input aria-label="Filter" />
      </>,
    );
    const fire = (el: Element) => {
      const e = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      el.dispatchEvent(e);
      return e.defaultPrevented;
    };
    expect(fire(screen.getByRole("link"))).toBe(true);
    expect(fire(screen.getByRole("textbox"))).toBe(false);
    vi.spyOn(document, "getSelection").mockReturnValue({ rangeCount: 1, getRangeAt: () => ({ collapsed: false }) } as unknown as Selection);
    expect(fire(screen.getByRole("link"))).toBe(false);
    vi.restoreAllMocks();
    off();
  });
});

describe("attention and the announcer", () => {
  it("an open pairing window shows its countdown and reopens the Pair dialog", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    // The window is open on the (fake) service too: the Pair dialog's poller ends the session when the service says so.
    const fake = installFakeDaemon();
    fake.pairing.session = { code: "482913", port: 51234, pair_addr: "192.168.1.10:51234", qr_payload: "ucompute-pair:v1:x", expires_at: Math.floor(Date.now() / 1000) + 102 };
    pairing.set({ code: "482913", addr: "192.168.1.10:51234", payload: "ucompute-pair:v1:x", expiresAt: Date.now() + 102_000 });
    render(<StatusBar />);
    // The countdown is visible but not part of the button's name, which would otherwise change every second.
    const btn = within(bar()).getByRole("button", { name: "Pairing open" });
    expect(btn.textContent).toMatch(/^Pairing open · 1:4\d$/);
    act(() => btn.click());
    expect(window.location.hash).toBe("#/devices?pair=1");
  });

  it("a failed download adds a dismissable entry and is announced; Lost connection too, until it reconnects", async () => {
    vi.useFakeTimers();
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: STATUS, spawned: false });
    network.set(live(NETWORK_ON));
    const stop = startWatchers();
    render(
      <>
        <StatusBar />
        <Announcer />
      </>,
    );
    act(() => pulls.set({ m: { state: "downloading", ev: null, startedAt: Date.now() } }));
    act(() => pulls.set({ m: { state: "error", ev: null, startedAt: Date.now() } }));
    expect(within(bar()).getByRole("button", { name: "Download failed: m" })).toBeTruthy();
    expect(screen.getByTestId("announcer").textContent).toBe("Download failed: m");
    act(() => dismissAttention("pull:m"));
    expect(within(bar()).queryByText("Download failed: m")).toBeNull();

    act(() => mesh.set(live({ peers: [PEER], discovered: [], revocations: [] })));
    act(() => mesh.set(live({ peers: [{ ...PEER, connected: false, health: "reconnecting" }], discovered: [], revocations: [] })));
    expect(within(bar()).getByRole("button", { name: "Lost connection to Studio Mac" })).toBeTruthy();
    await act(() => vi.advanceTimersByTimeAsync(THROTTLE_MS));
    expect(screen.getByTestId("announcer").textContent).toBe("Lost connection to Studio Mac");
    act(() => mesh.set(live({ peers: [PEER], discovered: [], revocations: [] })));
    expect(within(bar()).queryByText("Lost connection to Studio Mac")).toBeNull();
    stop();
  });

  it("a loss seen through Not responding still counts; a Not responding blip that recovers announces nothing", async () => {
    vi.useFakeTimers();
    network.set(live(NETWORK_ON));
    const stop = startWatchers();
    render(
      <>
        <StatusBar />
        <Announcer />
      </>,
    );
    const at = (p: typeof PEER) => act(() => mesh.set(live({ peers: [p], discovered: [], revocations: [] })));
    at(PEER);
    at({ ...PEER, health: "unhealthy" });
    at(PEER);
    await act(() => vi.advanceTimersByTimeAsync(THROTTLE_MS));
    expect(screen.getByTestId("announcer").textContent).not.toMatch(/Connected to/);
    at({ ...PEER, health: "unhealthy" });
    at({ ...PEER, connected: false, health: "reconnecting" });
    expect(within(bar()).getByRole("button", { name: "Lost connection to Studio Mac" })).toBeTruthy();
    await act(() => vi.advanceTimersByTimeAsync(THROTTLE_MS));
    expect(screen.getByTestId("announcer").textContent).toBe("Lost connection to Studio Mac");
    stop();
  });

  it("a device the user disconnected or a networking switch-off is not a loss", () => {
    network.set(live(NETWORK_ON));
    const stop = startWatchers();
    render(<StatusBar />);
    act(() => mesh.set(live({ peers: [PEER], discovered: [], revocations: [] })));
    act(() => mesh.set(live({ peers: [{ ...PEER, connected: false, held: true }], discovered: [], revocations: [] })));
    expect(within(bar()).queryByText(/Lost connection/)).toBeNull();
    stop();
  });

  it("announces at most once every 2 s, in order", async () => {
    vi.useFakeTimers();
    render(<Announcer />);
    act(() => {
      announce("Service stopped");
      announce("Service started");
    });
    expect(screen.getByTestId("announcer").textContent).toBe("Service stopped");
    await act(() => vi.advanceTimersByTimeAsync(THROTTLE_MS - 100));
    expect(screen.getByTestId("announcer").textContent).toBe("Service stopped");
    await act(() => vi.advanceTimersByTimeAsync(200));
    expect(screen.getByTestId("announcer").textContent).toBe("Service started");
  });
});

describe("rail", () => {
  it("brand UCompute, seven sections, the current one marked; badges from live stores with their meaning described", () => {
    daemon.set({ reachable: true, responding: true, version: "0.1.0", status: { ...STATUS, queue: { ...STATUS.queue, inflight: 1, queued: 2 } }, spawned: false });
    network.set(live(NETWORK_ON));
    mesh.set(live(PEERS_E1));
    pulls.set({ m: { state: "downloading", ev: null, startedAt: 0 } });
    render(<Rail current="devices" />);
    const nav = screen.getByRole("navigation", { name: "Sections" });
    expect(within(nav).getByText("UCompute")).toBeTruthy();
    // The name is the visible label (no aria-label: with a badge showing it would not match the visible text, WCAG 2.5.3).
    const links = within(nav).getAllByRole("link");
    expect(links.map((a) => a.querySelector(".rail-label")!.textContent)).toEqual(["Dashboard", "Devices", "Models", "Jobs", "Chat", "Activity", "Settings"]);
    expect(links.filter((a) => a.hasAttribute("aria-label"))).toEqual([]);
    const devices = within(nav).getByRole("link", { name: "Devices" });
    expect(devices.getAttribute("aria-current")).toBe("page");
    expect(devices.textContent).toBe("Devices1/2");
    // The other device's last dial failed and it is not announced nearby: Not seen nearby, so no warn dot.
    expect(document.getElementById(devices.getAttribute("aria-describedby")!)!.textContent).toBe("1 of 2 paired devices connected");
    expect(within(nav).getByRole("link", { name: "Jobs" }).textContent).toBe("Jobs3");
    expect(within(nav).getByRole("link", { name: "Models" }).textContent).toBe("Models↓1");
  });

  it.each([
    ["not running", { reachable: false }, null],
    ["busy", { reachable: true, responding: false }, null],
    ["after a failed mesh read", { reachable: true, responding: true }, "mesh"],
  ] as const)("no device count while the service is %s (last good data is not live)", (_, state, failing) => {
    daemon.set({ version: "0.1.0", status: null, spawned: false, ...state });
    network.set(live(NETWORK_ON));
    mesh.set({ ...live(PEERS_E1), ...(failing ? { error: mapError("slow", "") } : {}) });
    render(<Rail current="dashboard" />);
    const devices = screen.getByRole("link", { name: "Devices" });
    expect(devices.textContent).toBe("Devices");
    expect(devices.getAttribute("aria-describedby")).toBeNull();
  });

  it("a warn dot only for Not responding or Unreachable", () => {
    network.set(live(NETWORK_ON));
    mesh.set(live({ ...PEERS_E1, discovered: [...PEERS_E1.discovered, { ...STRANGER, id: PEERS_E1.peers[1].id, trusted: true }] }));
    render(<Rail current="dashboard" />);
    const devices = screen.getByRole("link", { name: "Devices" });
    expect(devices.querySelector(".rail-badge.warn")).toBeTruthy();
    expect(document.getElementById(devices.getAttribute("aria-describedby")!)!.textContent).toMatch(/not responding or unreachable/);
  });
});

describe("shortcuts", () => {
  const key = (init: KeyboardEventInit) => act(() => onShortcut(new KeyboardEvent("keydown", { cancelable: true, ...init })));
  const original = navigator.platform;
  afterEach(() => setPlatform(original));

  it("Mod is ⌘ on macOS (Ctrl does nothing) and Ctrl on Windows (⌘ does nothing); one key press is one navigation", () => {
    setPlatform("MacIntel");
    window.location.hash = "#/dashboard";
    const n = history.length;
    key({ key: "2", ctrlKey: true });
    expect(window.location.hash).toBe("#/dashboard");
    key({ key: "2", metaKey: true });
    expect(window.location.hash).toBe("#/devices");
    expect(history.length).toBe(n + 1);
    setPlatform("Win32");
    key({ key: "7", metaKey: true });
    expect(window.location.hash).toBe("#/devices");
    key({ key: "7", ctrlKey: true });
    expect(window.location.hash).toBe("#/settings");
    key({ key: "4", ctrlKey: true });
    expect(window.location.hash).toBe("#/jobs");
    key({ key: ",", ctrlKey: true });
    expect(window.location.hash).toBe("#/settings");
  });

  it("F6 / Shift+F6 cycle focus through the regions", async () => {
    installFakeDaemon();
    await refreshStatus();
    window.location.hash = "#/dashboard";
    render(<App poll={false} />);
    await screen.findByRole("grid", { name: "Computers" });
    const rail = screen.getByRole("navigation", { name: "Sections" });
    within(rail).getByRole("link", { name: "Dashboard" }).focus();
    const region = () => (document.activeElement as HTMLElement).closest("[data-region]")?.getAttribute("data-region");
    const seen: (string | null | undefined)[] = [];
    for (let i = 0; i < 4; i++) {
      key({ key: "F6" });
      seen.push(region());
    }
    // The Dashboard has no scope row and no inspector.
    expect(seen).toEqual(["toolbar", "table", "statusbar", "rail"]);
    key({ key: "F6", shiftKey: true });
    expect(region()).toBe("statusbar");
  });

  it("F6 lands on the current item: the section's rail link, the inspector's selected tab", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER] });
    await refreshStatus();
    window.location.hash = `#/devices/${PEER.id}`;
    render(<App poll={false} />);
    const overview = await screen.findByRole("tab", { name: "Overview" });
    (document.activeElement as HTMLElement | null)?.blur();
    key({ key: "F6" });
    expect(document.activeElement).toBe(within(screen.getByRole("navigation", { name: "Sections" })).getByRole("link", { name: "Devices" }));
    for (let i = 0; i < 6 && !overview.closest("[data-region]")!.contains(document.activeElement); i++) key({ key: "F6" });
    expect(document.activeElement).toBe(overview);
  });

  it("Mod+F focuses the filter box when the section has one", () => {
    setPlatform("Win32");
    render(<input data-filter="" aria-label="Filter" />);
    key({ key: "f", ctrlKey: true });
    expect(document.activeElement).toBe(screen.getByRole("textbox", { name: "Filter" }));
  });
});

describe("theme", () => {
  it("light/dark set data-theme; system removes it so the OS preference applies", async () => {
    await applyTheme("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    await applyTheme("system");
    expect(document.documentElement.dataset.theme).toBeUndefined();
  });
});

describe("copy", () => {
  it("with a grid cell focused and no text selected, copies the row id once; with text selected, leaves the browser copy alone", () => {
    const off = installCopyHandler();
    render(
      <table role="grid" aria-label="Devices">
        <tbody>
          <tr data-copy={PEER.id}>
            <td role="gridcell" tabIndex={0}>
              Studio Mac
            </td>
          </tr>
        </tbody>
      </table>,
    );
    screen.getByRole("gridcell").focus();
    const copy = () => {
      const setData = vi.fn();
      const e = new Event("copy", { bubbles: true, cancelable: true });
      Object.defineProperty(e, "clipboardData", { value: { setData } });
      document.activeElement!.dispatchEvent(e);
      return { setData, prevented: e.defaultPrevented };
    };
    const a = copy();
    expect(a.setData).toHaveBeenCalledTimes(1);
    expect(a.setData).toHaveBeenCalledWith("text/plain", PEER.id);
    expect(a.prevented).toBe(true);
    // jsdom's Selection does not report a selected range as non-collapsed; stand in for the browser's.
    vi.spyOn(document, "getSelection").mockReturnValue({ rangeCount: 1, getRangeAt: () => ({ collapsed: false }) } as unknown as Selection);
    const b = copy();
    expect(b.setData).not.toHaveBeenCalled();
    expect(b.prevented).toBe(false);
    vi.restoreAllMocks();
    off();
  });
});

describe("jobs badge source", () => {
  it("polled jobs land in the store without touching the rail count (which is status.queue)", async () => {
    installFakeDaemon({ jobs: JOBS });
    await refresh("jobs");
    expect(jobs.get().data).toEqual(JOBS);
  });
});
