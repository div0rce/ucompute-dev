// Jobs behaviour (plan §2.4, §5.4 C): rows from E3, the All default scope, Requested by, live "so far" values from two
// polls, Stop only on this app's own streams, no queue controls, the unsupported state and the phase bar.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Job, JobsData } from "../../api/client";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { ownStreams, refresh } from "../../state/live";
import { chatHistory } from "../../state/stores";
import { resetStores } from "../../test/axe";
import { installFakeDaemon, JOBS, LOADED, NETWORK_ON, PEER, type FakeDaemon } from "../../test/mocks";
import { isActive, jobState } from "./Jobs";

beforeEach(resetStores);
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

const RUNNING = JOBS.active[0]; // local chat, client_ref set
const QUEUED = JOBS.active[1]; // local completion, no client_ref
const SERVED = JOBS.active[2]; // served for PEER
const CONNECTING = JOBS.active[3]; // chat to PEER, being dialled
const SENT = JOBS.recent[1]; // done on PEER

function cell(row: HTMLElement, header: string) {
  const grid = row.closest<HTMLElement>('[role="grid"]')!;
  const i = within(grid)
    .getAllByRole("columnheader")
    .findIndex((h) => h.textContent?.trim().startsWith(header));
  // Visible text only: a cell's Tip text stays in the DOM (hidden).
  const c = within(row).getAllByRole("gridcell")[i].cloneNode(true) as HTMLElement;
  c.querySelectorAll('[role="tooltip"]').forEach((t) => t.remove());
  return c.textContent;
}
const grid = () => screen.findByRole("grid", { name: "Jobs" });
const rowOf = (g: HTMLElement, j: Job) => within(g).getAllByRole("row").find((r) => r.getAttribute("data-copy") === j.id)!;

async function open(fake: FakeDaemon, hash = "#/jobs") {
  void fake;
  await refreshStatus();
  window.location.hash = hash;
  return render(<App poll={false} />);
}

describe("jobState", () => {
  it("uses only real E3 states, with errors.ts titles", () => {
    expect(jobState({ ...QUEUED }).word).toBe("Waiting for a slot");
    expect(jobState(CONNECTING)).toMatchObject({ word: "Connecting to the device", tone: "active" });
    // A service from before `connecting` reports a request to another device as queued while it is dialled: no slot here.
    expect(jobState({ ...CONNECTING, state: "queued" })).toMatchObject({ word: "Connecting to the device", tone: "active" });
    // A state from a newer service is shown as reported, not guessed.
    expect(jobState({ ...QUEUED, state: "paused" as Job["state"] }).word).toBe("State not known to this app: paused");
    expect(jobState({ ...QUEUED, state: "loading" }).word).toBe("Loading model");
    expect(jobState(RUNNING).word).toBe("Running");
    expect(jobState(JOBS.recent[2])).toMatchObject({ word: "Failed: Common is busy with other requests.", tone: "danger" });
    expect(jobState({ ...SENT, state: "cancelled", error_code: "client_gone" }).word).toBe("Requester disconnected");
    expect(jobState({ ...SENT, state: "cancelled", error_code: "cancelled" }).word).toBe("Stopped");
    expect(jobState(SENT)).toMatchObject({ word: "Done", tone: "ok", detail: "Finish reason: stop" });
  });

  it("never files a state it does not know under finished", () => {
    // Unknown is not an outcome: it stays with the active jobs (and its Stop says why it is unavailable).
    expect(isActive({ ...QUEUED, state: "paused" as Job["state"] })).toBe(true);
    for (const state of ["queued", "connecting", "loading", "running"] as const) expect(isActive({ ...QUEUED, state })).toBe(true);
    for (const state of ["done", "error", "cancelled"] as const) expect(isActive({ ...QUEUED, state })).toBe(false);
  });
});

describe("Jobs section", () => {
  it("lists every E3 row under All (the default): active first, Requested by and Runs on resolved", async () => {
    chatHistory.set({ messages: [{ role: "assistant", content: "x", client_ref: RUNNING.client_ref! }] });
    await open(installFakeDaemon({ jobs: JOBS, network: NETWORK_ON, peers: [PEER] }));
    const g = await grid();
    await waitFor(() => expect(within(g).getAllByRole("row")).toHaveLength(8));
    expect(within(screen.getByRole("group", { name: "Show jobs" })).getByRole("button", { name: /^All/ }).getAttribute("aria-pressed")).toBe("true");
    const states = within(g)
      .getAllByRole("row")
      .slice(1)
      .map((r) => cell(r, "State"));
    // Active rows by state, then finished rows newest first whatever their outcome: the 10:29 failure is older than
    // both replies, so it comes last (a state sort would lift it above them).
    expect(states).toEqual(["Running", "Running", "Waiting for a slot", "Connecting to the device", "Done", "Done", "Failed: Common is busy with other requests."]);
    expect(within(g).getAllByRole("columnheader").some((h) => h.hasAttribute("aria-sort"))).toBe(false);

    expect(cell(rowOf(g, RUNNING), "Requested by")).toBe("This app");
    expect(cell(rowOf(g, RUNNING), "Runs on")).toBe("This computer · Metal0 (Apple M2)");
    expect(cell(rowOf(g, QUEUED), "Requested by")).toBe("This computer (another API client or not matched)");
    expect(cell(rowOf(g, SERVED), "Requested by")).toBe("Studio Mac");
    expect(cell(rowOf(g, SENT), "Runs on")).toBe("Studio MacGPU/CPU not reported");
    expect(cell(rowOf(g, QUEUED), "Duration")).toBe("—not started");
    expect(cell(rowOf(g, SENT), "tok/s")).toBe("30.7");
  });

  it("a queued request this app sent reads This app through its client_ref", async () => {
    const ref = "0b7c6f5e-2a7d-4e1b-9c3f-1d2e3f4a5b6c";
    ownStreams.set({ [ref]: { stop: () => undefined } });
    const jobs: JobsData = { active: [{ ...QUEUED, id: "job-9", client_ref: ref }], recent: [] };
    await open(installFakeDaemon({ jobs }));
    const g = await grid();
    await waitFor(() => expect(cell(within(g).getAllByRole("row")[1], "Requested by")).toBe("This app"));
    expect(cell(within(g).getAllByRole("row")[1], "State")).toBe("Waiting for a slot");
  });

  it("active rows count tokens and tok/s 'so far' between two polls", async () => {
    let n = 37;
    const fake = installFakeDaemon({ routes: { "GET /api/jobs": () => new Response(JSON.stringify({ active: [{ ...RUNNING, completion_tokens: n }], recent: [] })) } });
    await open(fake);
    const g = await grid();
    await waitFor(() => expect(cell(within(g).getAllByRole("row")[1], "Tokens")).toBe("37so far"));
    expect(cell(within(g).getAllByRole("row")[1], "tok/s")).toBe("—so far");
    const t = Date.now() + 10_000;
    const now = vi.spyOn(Date, "now").mockReturnValue(t);
    await act(() => refresh("jobs"));
    n = 57;
    now.mockReturnValue(t + 2000);
    await act(() => refresh("jobs"));
    now.mockRestore();
    const r = within(g).getAllByRole("row")[1];
    expect(cell(r, "Tokens")).toBe("57so far");
    expect(cell(r, "tok/s")).toBe("10so far");
  });

  it("Stop works only on this app's own active streams; no queue position, reorder or cancel controls exist", async () => {
    const user = userEvent.setup();
    const stop = vi.fn();
    ownStreams.set({ [RUNNING.client_ref!]: { stop } });
    await open(installFakeDaemon({ jobs: JOBS, network: NETWORK_ON, peers: [PEER] }));
    const g = await grid();
    await waitFor(() => expect(within(g).getAllByRole("row")).toHaveLength(8));

    await user.click(rowOf(g, SERVED));
    const btn = screen.getByRole("button", { name: "Stop" });
    expect(btn.getAttribute("aria-disabled")).toBe("true");
    expect(document.getElementById(btn.getAttribute("aria-describedby")!.split(" ").pop()!)?.textContent).toBe("Only requests started in this app can be stopped");
    await user.click(btn);
    await user.click(rowOf(g, QUEUED));
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(stop).not.toHaveBeenCalled();

    await user.click(rowOf(g, RUNNING));
    expect(screen.getByRole("button", { name: "Stop" }).getAttribute("aria-disabled")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(stop).toHaveBeenCalledTimes(1);

    expect(screen.queryAllByRole("button", { name: /move|priority|position|reorder|force|cancel|resume|pause/i })).toEqual([]);
    expect(within(g).queryAllByRole("columnheader", { name: /position|priority|^#$|queue/i })).toEqual([]);
  });

  it("a reply streaming from Chat is This app's row and Jobs' Stop ends it (Chat → Jobs → Stop)", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ chat: { chunks: Array.from({ length: 200 }, (_, i) => `w${i} `), delayMs: 10 } });
    fake.loaded = LOADED;
    await open(fake, "#/chat");
    await user.type(await screen.findByRole("textbox", { name: "Message" }), "Hi{Enter}");
    await screen.findByText(/w2 /);
    act(() => void (window.location.hash = "#/jobs"));
    const g = await grid();
    const mine = await waitFor(() => within(g).getAllByRole("row").find((r) => r.getAttribute("aria-rowindex") !== "1" && cell(r, "Requested by") === "This app")!);
    expect(cell(mine, "State")).toBe("Running");
    await user.click(mine);
    await user.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(chatHistory.get().messages[1]?.stopped).toBe(true));
    const chatCall = fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))!;
    expect((chatCall[1] as RequestInit).signal?.aborted).toBe(true);
    expect(chatHistory.get().messages[1].content.length).toBeGreaterThan(0);
  });

  it("Show device and Open in Chat follow the row", async () => {
    const user = userEvent.setup();
    chatHistory.set({ messages: [{ role: "assistant", content: "x", job_id: JOBS.recent[0].id }] });
    await open(installFakeDaemon({ jobs: JOBS, network: NETWORK_ON, peers: [PEER] }));
    const g = await grid();
    await waitFor(() => expect(within(g).getAllByRole("row")).toHaveLength(8));
    await user.click(rowOf(g, JOBS.recent[0]));
    expect(screen.getByRole("button", { name: "Show device" }).getAttribute("aria-disabled")).toBe("true");
    await user.click(screen.getByRole("button", { name: "Open in Chat" }));
    expect(window.location.hash).toBe(`#/chat?reply=${JOBS.recent[0].id}`);
    act(() => void (window.location.hash = `#/jobs/${SENT.id}`));
    await user.click(await screen.findByRole("button", { name: "Show device" }));
    expect(window.location.hash).toBe(`#/devices/${PEER.id}`);
  });

  it("scopes come from the URL (the status bar links ?scope=served) and filter by model, device and id", async () => {
    const user = userEvent.setup();
    await open(installFakeDaemon({ jobs: JOBS, network: NETWORK_ON, peers: [PEER] }), "#/jobs?scope=served");
    const g = await grid();
    await waitFor(() => expect(within(g).getAllByRole("row")).toHaveLength(2));
    expect(cell(within(g).getAllByRole("row")[1], "Requested by")).toBe("Studio Mac");
    const scopes = screen.getByRole("group", { name: "Show jobs" });
    expect(within(scopes).getByRole("button", { name: /^Served for devices/ }).getAttribute("aria-pressed")).toBe("true");
    await user.click(within(scopes).getByRole("button", { name: /^Failed/ }));
    expect(window.location.hash).toBe("#/jobs?scope=failed");
    expect(within(await grid()).getAllByRole("row")).toHaveLength(2);
    await user.click(within(scopes).getByRole("button", { name: /^All/ }));
    await user.type(screen.getByRole("searchbox", { name: "Filter jobs" }), "studio");
    expect(within(await grid()).getAllByRole("row")).toHaveLength(4);
  });

  it("a service without /api/jobs shows the slot strip and says so; no table, no sparkline", async () => {
    await open(installFakeDaemon({ jobs: "unsupported" }));
    expect(await screen.findByText("This service version does not report jobs")).toBeTruthy();
    expect(screen.getByText(/^Slots 0 of 1 in use/)).toBeTruthy();
    expect(screen.queryByRole("grid")).toBeNull();
    expect(screen.queryByRole("img", { name: /Tokens per second/ })).toBeNull();
  });

  it("with E3 the strip reads slots, device slots, waiting and rejections, and the sparkline is there", async () => {
    await open(installFakeDaemon({ jobs: JOBS, network: NETWORK_ON }));
    await grid();
    expect(await screen.findByRole("img", { name: /^Tokens per second/ })).toBeTruthy();
    await waitFor(() => expect(screen.getByText(/^Slots 0 of 1 in use · devices may use up to 3 · waiting 0 of 4/)).toBeTruthy());
    // The fake's /metrics has no rejection counters: unknown, not 0.
    expect(screen.getByText(/rejected since service start: not reported/)).toBeTruthy();
  });

  it("the phase bar draws measured phases only; a job no longer held says so", async () => {
    await open(installFakeDaemon({ jobs: JOBS }), `#/jobs/${RUNNING.id}?tab=timing`);
    const bar = await screen.findByRole("img", { name: /^Phases:/ });
    expect(bar.getAttribute("aria-label")).toBe("Phases: Wait 3 ms, Load 0 ms, Prompt 180 ms");
    expect(bar.querySelector(".phase-decode")).toBeNull();
    act(() => void (window.location.hash = "#/jobs/chatcmpl-gone"));
    expect(await screen.findByText("No longer held by the service (cleared at restart or past 256)")).toBeTruthy();
  });
});

describe("Tokens/s caption", () => {
  it("says what was sampled: every jobs read while the app runs (jobs is always polled), not only while a view is open", async () => {
    await open(installFakeDaemon({ jobs: JOBS }));
    await grid();
    const caption = document.querySelector(".jobs-rate figcaption")!.textContent!;
    expect(caption).toMatch(/sampled about every 5 s while this app was open and the service answering, last ≤ 1 h\./);
    expect(caption).not.toMatch(/was open, last|Dashboard, Jobs or Activity/);
  });
});

describe("TTFT says what it measured", () => {
  it("a job's TTFT is from arrival here (wait and load included), in its column tip and the inspector", async () => {
    const done = JOBS.recent.find((j) => j.ttft_ms != null)!;
    await open(installFakeDaemon({ jobs: JOBS, network: NETWORK_ON, peers: [PEER] }), `#/jobs/${done.id}`);
    const g = await grid();
    const basis = "From arrival here: includes waiting, loading and, for another device, the network.";
    const ttft = within(rowOf(g, done)).getAllByRole("gridcell")[within(g).getAllByRole("columnheader").findIndex((h) => h.textContent === "TTFT")];
    expect(within(ttft).getByRole("tooltip", { hidden: true }).textContent).toBe(basis);
    const panel = screen.getByRole("region", { name: "Job details" });
    expect(within(panel).getByText("Time to first token").parentElement!.textContent).toContain(basis);
  });
});
