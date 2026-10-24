// F-08-04 / F-10-06 in the UI: the app shell restarts its own service when it dies and keeps its output in a capped
// log (src-tauri lib.rs `Daemon::after_exit`, `Daemon::restart`, `LogFile`; Tauri invoke mocked here). Unit level
// against the fake daemon: what the UI says while the shell waits to restart it, once it answers again, and once the
// shell gave up, and where the log is shown.
import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus, startDaemonPolling } from "../daemon";
import { Announcer, resetAnnouncer, THROTTLE_MS } from "../shell/announcer";
import { startWatchers } from "../shell/attention";
import { StatusBar } from "../shell/StatusBar";
import { attention } from "../state/live";
import { daemon } from "../state/stores";
import { installFakeDaemon } from "./mocks";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const LOG = "/Users/someone/Library/Logs/org.commoncompute.desktop/daemon.log";
/** What the shell's `daemon_info` answers. */
let shell: Record<string, unknown>;
const w = window as unknown as Record<string, unknown>;
const bar = () => screen.getByRole("contentinfo", { name: "Status" });
const said = () => screen.getByTestId("announcer").textContent;
const serviceDown = () => vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));

beforeEach(async () => {
  resetAnnouncer();
  attention.set([]);
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  shell = { base_url: "http://127.0.0.1:8192", spawned: true, restarts: 0, restarting: false, gave_up: false, log_path: LOG };
  w.__TAURI_INTERNALS__ = {};
  invoke.mockImplementation(async (cmd: string) => (cmd === "daemon_info" ? { ...shell } : undefined));
  installFakeDaemon();
  (await startDaemonPolling())(); // one reading, then polled by hand
});
afterEach(() => {
  delete w.__TAURI_INTERNALS__;
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

describe("the app restarts its service", () => {
  it("says it is restarting the service, then that it restarted it (not just started), and points at the log", async () => {
    vi.useFakeTimers();
    expect(daemon.get()).toMatchObject({ reachable: true, spawned: true, restarts: 0, logPath: LOG });
    const stop = startWatchers();
    render(
      <>
        <StatusBar />
        <Announcer />
      </>,
    );

    // The service died; the shell waits for its backoff.
    shell = { ...shell, restarting: true };
    serviceDown();
    await act(() => refreshStatus());
    expect(within(bar()).getByRole("button", { name: "Service stopped unexpectedly. Restarting it." })).toBeTruthy();
    expect(said()).toBe("Service stopped");

    // Restarted, and answering again.
    shell = { ...shell, restarting: false, restarts: 1 };
    installFakeDaemon();
    await act(() => refreshStatus());
    await act(() => vi.advanceTimersByTimeAsync(2 * THROTTLE_MS));
    expect(said()).toBe("Service restarted");
    const entry = within(bar()).getByRole("button", { name: "Service stopped unexpectedly and was restarted" });
    act(() => entry.click());
    expect(window.location.hash).toBe("#/settings/general");
    stop();
  });

  it("a restart no poll saw down is still said; the count a window opens with is not", async () => {
    vi.useFakeTimers();
    daemon.set((d) => ({ ...d, restarts: undefined }));
    const stop = startWatchers();
    render(<Announcer />);
    act(() => daemon.set((d) => ({ ...d, restarts: 2 })));
    expect(said()).toBe("");

    shell = { ...shell, restarts: 3 };
    await act(() => refreshStatus());
    expect(said()).toBe("Service restarted");
    stop();
  });

  it("once the shell gave up, says so and how to recover, and shows where the log is", async () => {
    shell = { ...shell, gave_up: true };
    serviceDown();
    await refreshStatus();
    render(<StatusBar />);
    const item = within(bar()).getByRole("button", { name: "Service stopped repeatedly and was not restarted. Quit and reopen the app." });
    act(() => item.click());
    expect(window.location.hash).toBe("#/settings/general");
  });
});

describe("the service log", () => {
  it("Settings › General shows the log file and the restarts", async () => {
    shell = { ...shell, restarts: 2 };
    await refreshStatus();
    window.location.hash = "#/settings/general";
    render(<App poll={false} />);
    await screen.findByRole("heading", { level: 2, name: "General" });
    expect(screen.getByText(LOG).tagName).toBe("CODE");
    expect(screen.getByText("2 times after it stopped unexpectedly")).toBeTruthy();
  });

  it("Copy diagnostics points at the log without copying its path (it holds the account name)", async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(async (_text: string) => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    shell = { ...shell, restarts: 1 };
    await refreshStatus();
    window.location.hash = "#/activity";
    render(<App poll={false} />);
    await user.click(await screen.findByRole("button", { name: "Copy diagnostics" }));
    await screen.findByText(`Diagnostics copied. They contain status and hardware details, never your messages. The service's own log is in ${LOG}.`);
    const copied = writeText.mock.calls[0][0] as string;
    expect(JSON.parse(copied)).toMatchObject({ service_restarts_by_app: 1, service_restarts_given_up: false });
    expect(copied).not.toContain("someone");
  });
});
