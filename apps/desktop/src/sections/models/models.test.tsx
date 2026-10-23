// Models behaviour (plan §2.3, §5.4 C): state words, downloads that survive navigation without a second pull,
// load/unload/delete confirmations, "Load (replaces …)" with the sharing caveat, import path validation, E1 "On devices".
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Model, Status } from "../../api/client";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { pulls } from "../../state/live";
import { daemon, settings } from "../../state/stores";
import { resetStores } from "../../test/axe";
import { apiError, installFakeDaemon, JOBS, LOADED, MODELS, NETWORK_ON, PEER_ID, PEERS_E1, sse, STATUS_E2, type FakeDaemon } from "../../test/mocks";
import type { MenuItem } from "../../ui/contextMenu";
import { isLoaded, modelState, onDevices } from "./Models";
import { startPull } from "./pulls";

// The native menu (More ▾, row menus) only exists in Tauri: capture its items instead.
const menus = vi.hoisted(() => ({ last: [] as unknown[] }));
vi.mock("../../ui/contextMenu", async (orig) => ({
  ...(await orig<typeof import("../../ui/contextMenu")>()),
  popupMenu: vi.fn(async (items: unknown[]) => {
    menus.last = items;
    return true;
  }),
}));

beforeEach(resetStores);
afterEach(() => vi.unstubAllGlobals());

const QWEN = MODELS[0];
const STORIES = MODELS[1];

/** The visible text of `row`'s cell under the column headed `header`. */
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
const row = async (name: RegExp) => within(await screen.findByRole("grid", { name: "Models" })).findByRole("row", { name });

async function open(fake: FakeDaemon, hash = "#/models") {
  void fake;
  await refreshStatus();
  window.location.hash = hash;
  return render(<App poll={false} />);
}

describe("modelState (plan §2.3, first match)", () => {
  const status = (s: Partial<Status>): Status => ({ ...STATUS_E2, ...s });
  it("downloading wins, with percent and the resumed size", () => {
    const pull = { state: "downloading" as const, ev: { downloaded_bytes: 42, total_bytes: 100, network_bytes: 30, resumed_from: 12, done: false }, startedAt: 0 };
    const s = modelState(QWEN, { pull, status: null, ownLoad: null, peers: [] });
    expect(s.word).toBe("Downloading 42%");
    expect(s.tone).toBe("active");
    expect(s.note).toContain("resumed from 12 B");
  });
  it("E2 loading names the seconds; a load from this app without E2 reads Loading", () => {
    const loading = status({ loaded: null, engine: { state: "loading", loading_model_id: STORIES.id, loading_for_ms: 12400 } });
    expect(modelState(STORIES, { status: loading, ownLoad: null, peers: [] }).word).toBe("Loading · 12 s");
    expect(modelState(STORIES, { status: { ...loading, engine: undefined }, ownLoad: STORIES.id, peers: [] }).word).toBe("Loading");
  });
  it("loaded shows the device, a CPU fallback warning and the device a load was for", () => {
    const st = status({ loaded: { ...STATUS_E2.loaded!, model_id: QWEN.id, fallback_reason: "Metal unavailable", loaded_for: `peer:${PEER_ID}` } });
    const s = modelState({ ...QWEN, downloaded: true }, { status: st, ownLoad: null, peers: PEERS_E1.peers });
    expect(s.word).toBe("Loaded · Metal0 (Apple M2)");
    expect(s.tone).toBe("ok");
    expect(s.warn).toBe("CPU fallback: Metal unavailable");
    expect(s.note).toBe("for a request from Studio Mac");
  });
  it("the status poll decides loaded over a stale catalog read", () => {
    expect(isLoaded({ ...QWEN, loaded: true }, status({ loaded: null }))).toBe(false);
    expect(isLoaded({ ...QWEN, loaded: true }, null)).toBe(true);
  });
  it("a failed download, downloaded, not downloaded", () => {
    const pull = { state: "error" as const, ev: null, startedAt: 0, error: { code: "download_failed", title: "The download did not complete.", next_step: "", detail: "", hint: null } };
    expect(modelState(QWEN, { pull, status: null, ownLoad: null, peers: [] })).toMatchObject({ word: "Download failed: The download did not complete.", tone: "danger" });
    expect(modelState(STORIES, { status: null, ownLoad: null, peers: [] }).word).toBe("Downloaded");
    expect(modelState(QWEN, { status: null, ownLoad: null, peers: [] }).word).toBe("Not downloaded");
  });
  it("On devices counts reported models only on connected devices, the rest as not reported", () => {
    expect(onDevices(QWEN, PEERS_E1.peers)).toEqual({ names: ["Studio Mac"], fromPeers: 1, unreported: 1 });
    expect(onDevices(STORIES, PEERS_E1.peers)).toEqual({ names: ["This computer"], fromPeers: 0, unreported: 1 });
    // Without E1 (no `capabilities` key) a connected device is not reported either.
    const { capabilities: _c, connection: _k, ...old } = PEERS_E1.peers[0];
    expect(onDevices(QWEN, [old as never])).toEqual({ names: [], fromPeers: 0, unreported: 1 });
  });
});

describe("Models section", () => {
  it("shows E1 availability with its caveat in the On devices column", async () => {
    await open(installFakeDaemon({ network: NETWORK_ON, peers: PEERS_E1.peers }));
    const r = await row(/Qwen2\.5/);
    await waitFor(() => expect(cell(r, "On devices")).toBe("Studio Macreported at connect · +1 not reported"));
    expect(cell(await row(/TinyStories/), "On devices")).toBe("This computer+1 not reported");
    expect(cell(r, "Fit")).toBe("Fits · 2.6 GB headroomest.");
  });

  it("a download survives navigation and never starts a second pull", async () => {
    const user = userEvent.setup();
    let pullsStarted = 0;
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const total = QWEN.bytes;
    const enc = new TextEncoder();
    const ev = (p: number) => enc.encode(`data: ${JSON.stringify({ downloaded_bytes: total * p, total_bytes: total, network_bytes: total * p, resumed_from: 0, done: p === 1 })}\n\n`);
    const fake = installFakeDaemon({
      routes: {
        // Half the file, then held until the test releases the rest.
        "POST /api/models/qwen2.5-1.5b-instruct-q4_k_m/pull": ({ fake: f }) => {
          pullsStarted++;
          const body = new ReadableStream<Uint8Array>({
            async start(c) {
              c.enqueue(ev(0.5));
              await gate;
              f.models[0].downloaded = true;
              c.enqueue(ev(1));
              c.close();
            },
          });
          return new Response(body, { headers: { "content-type": "text/event-stream" } });
        },
      },
    });
    await open(fake);
    await user.click(await row(/Qwen2\.5/));
    await user.click(screen.getByRole("button", { name: "Download…" }));
    const dialog = await screen.findByRole("dialog", { name: /Confirm download of Qwen2\.5/ });
    expect(dialog.textContent).toContain("Downloads can't be cancelled");
    await user.click(within(dialog).getByRole("button", { name: /^Download 1\.0 GB/ }));
    await waitFor(() => expect(cell(screen.getByRole("row", { name: /Qwen2\.5/ }), "State")).toMatch(/^Downloading 50%/));

    // Away and back: the progress is the app's, not the view's.
    act(() => void (window.location.hash = "#/jobs"));
    await screen.findByRole("heading", { level: 1, name: "Jobs" });
    act(() => void (window.location.hash = "#/models/qwen2.5-1.5b-instruct-q4_k_m"));
    await waitFor(() => expect(cell(screen.getByRole("row", { name: /Qwen2\.5/ }), "State")).toMatch(/^Downloading 50%/));
    const dl = screen.getByRole("button", { name: "Download…" });
    expect(dl.getAttribute("aria-disabled")).toBe("true");
    await user.click(dl);
    expect(screen.queryByRole("dialog", { name: /Confirm download/ })).toBeNull();
    await startPull(QWEN.id);
    expect(pullsStarted).toBe(1);

    release();
    await waitFor(() => expect(cell(screen.getByRole("row", { name: /Qwen2\.5/ }), "State")).toBe("Downloaded"));
    expect(pulls.get()[QWEN.id].state).toBe("done");
    expect(pullsStarted).toBe(1);
  });

  it("a failure inside the download stream reads Download failed, not done", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({
      routes: { "POST /api/models/qwen2.5-1.5b-instruct-q4_k_m/pull": () => sse([{ error: { code: "download_failed", message: "connection reset" } }], { done: false }) },
    });
    await open(fake, "#/models/qwen2.5-1.5b-instruct-q4_k_m");
    await row(/Qwen2\.5/);
    await user.click(screen.getByRole("button", { name: "Download…" }));
    await user.click(await screen.findByRole("button", { name: /^Download 1\.0 GB/ }));
    await waitFor(() => expect(cell(screen.getByRole("row", { name: /Qwen2\.5/ }), "State")).toBe("Download failed: The download did not complete."));
  });

  it("Load (replaces …) asks first, with the sharing caveat, then loads", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK_ON, serve_peers: true } });
    fake.loaded = LOADED;
    await open(fake, `#/models/${STORIES.id}`);
    await row(/TinyStories/);
    const load = await screen.findByRole("button", { name: "Load (replaces Qwen2.5 1.5B Instruct (Q4_K_M))" });
    await user.click(load);
    const group = await screen.findByRole("group", { name: "Confirm loading TinyStories 15M (Q4_0)" });
    expect(group.textContent).toContain("unloads Qwen2.5 1.5B Instruct (Q4_K_M)");
    expect(group.textContent).toContain("Paired devices' requests can load a different model here.");
    expect(document.activeElement?.textContent).toBe("Cancel");
    expect(fake.loaded?.model_id).toBe(QWEN.id);
    await user.click(within(group).getByRole("button", { name: "Load TinyStories 15M (Q4_0)" }));
    await waitFor(() => expect(fake.loaded?.model_id).toBe(STORIES.id));
    await waitFor(() => expect(cell(screen.getByRole("row", { name: /TinyStories/ }), "State")).toBe("Loaded · Metal"));
    // Focus moves on to the next step.
    await waitFor(() => expect(document.activeElement?.textContent).toBe("Open in Chat"));
  });

  it("the Locations tab loads once with its own device and context; the settings defaults stay", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    await open(fake, `#/models/${STORIES.id}?tab=locations`);
    await row(/TinyStories/);
    await user.selectOptions(await screen.findByRole("combobox", { name: "Device" }), "cpu");
    await user.type(screen.getByRole("spinbutton", { name: "Context length" }), "2048");
    await user.click(screen.getByRole("button", { name: "Load once with these settings" }));
    await waitFor(() => expect(fake.loaded).toMatchObject({ model_id: STORIES.id, device_requested: "cpu", n_ctx: 2048 }));
    expect(settings.get()).toMatchObject({ device: "auto", n_ctx: null });
  });

  it("the inspector's live values are not refreshed while busy; an unread status never reads Downloaded", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    fake.loaded = LOADED;
    await open(fake, `#/models/${QWEN.id}`);
    await row(/Qwen2\.5/);
    const details = () => screen.getByRole("region", { name: "Model details" });
    const kv = (k: string) => [...details().querySelectorAll("dt")].find((d) => d.textContent === k)?.nextElementSibling?.textContent;
    expect(kv("State")).toBe("Loaded · Metal");
    // Busy (usually a load of another model): the table blanks its State column, and so does the inspector.
    act(() => daemon.set((d) => ({ ...d, responding: false })));
    expect(kv("State")).toBe("— not refreshed");
    await user.click(within(details()).getByRole("tab", { name: "Locations" }));
    expect(kv("State")).toBe("— not refreshed");
    expect(kv("Requests using it")).toBeUndefined();
    // No status read (a 500): loaded per the catalog, as Overview says; the engine details are not known.
    act(() => daemon.set((d) => ({ ...d, responding: true, status: null })));
    expect(kv("State")).toBe("Loaded");
    expect(kv("Device")).toBe("— status not read yet");
    expect(within(details()).queryByRole("button", { name: "Load once with these settings" })).toBeNull();
  });

  it("no caveat while sharing is off; Esc cancels the confirmation", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    fake.loaded = LOADED;
    await open(fake, `#/models/${STORIES.id}`);
    await user.click(await screen.findByRole("button", { name: /^Load \(replaces/ }));
    const group = await screen.findByRole("group", { name: /Confirm loading/ });
    expect(group.textContent).not.toContain("Paired devices");
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("group", { name: /Confirm loading/ })).toBeNull();
    expect(fake.loaded?.model_id).toBe(QWEN.id);
  });

  it("Unload; while requests use the model it says how many", async () => {
    const user = userEvent.setup();
    let refuse = true;
    const fake = installFakeDaemon({
      jobs: JOBS,
      routes: {
        "POST /api/unload": ({ fake: f }) => {
          if (refuse) return apiError("overloaded", "the engine is in use", 429);
          f.loaded = null;
          return new Response(null, { status: 204 });
        },
      },
    });
    fake.loaded = LOADED;
    await open(fake, `#/models/${QWEN.id}`);
    await row(/Qwen2\.5/);
    await user.click(screen.getByRole("button", { name: "Unload" }));
    // The fake reports in_flight = its running jobs (2 in the E3 fixture).
    expect((await screen.findByRole("alert")).textContent).toContain("In use by 2 requests; try when they finish.");
    refuse = false;
    await user.click(screen.getByRole("button", { name: "Unload" }));
    await waitFor(() => expect(fake.loaded).toBeNull());
    await waitFor(() => expect(screen.getByRole("button", { name: "Unload" }).getAttribute("aria-disabled")).toBe("true"));
  });

  it("Delete asks first with Cancel focused, then removes the file only", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    await open(fake, `#/models/${STORIES.id}`);
    await row(/TinyStories/);
    await user.click(screen.getByRole("button", { name: "Delete…" }));
    const group = await screen.findByRole("group", { name: "Confirm deletion of TinyStories 15M (Q4_0)" });
    expect(group.textContent).toContain("the catalog entry stays");
    expect(document.activeElement?.textContent).toBe("Cancel");
    const deletes = () => fake.fetchMock.mock.calls.filter(([, i]) => (i as RequestInit | undefined)?.method === "DELETE").length;
    expect(deletes()).toBe(0);
    await user.click(within(group).getByRole("button", { name: "Delete file" }));
    await waitFor(() => expect(deletes()).toBe(1));
    await waitFor(() => expect(cell(screen.getByRole("row", { name: /TinyStories/ }), "State")).toBe("Not downloaded"));
  });

  it("Import GGUF… (More menu) validates the path before anything is sent", async () => {
    const user = userEvent.setup();
    const bodies: unknown[] = [];
    const fake = installFakeDaemon({
      routes: {
        "POST /api/models/import": ({ body, fake: f }) => {
          bodies.push(body);
          f.models.push({ ...(STORIES as Model), id: "my-model", name: "my-model", downloaded: true });
          return new Response(JSON.stringify({ id: "my-model" }), { status: 200, headers: { "content-type": "application/json" } });
        },
      },
    });
    await open(fake);
    await row(/TinyStories/);
    await user.click(screen.getByRole("button", { name: "More" }));
    const item = (menus.last as MenuItem[]).find((i): i is Extract<MenuItem, { action: () => void }> => "label" in i && i.label === "Import GGUF…")!;
    expect(item.enabled).toBe(true);
    act(() => item.action());
    const dialog = await screen.findByRole("dialog", { name: "Import a GGUF file" });
    const path = within(dialog).getByRole("textbox", { name: "File path" });
    await user.type(path, "models/x.gguf");
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    expect(within(dialog).getByText(/Enter the full path/)).toBeTruthy();
    expect(path.getAttribute("aria-invalid")).toBe("true");
    await user.type(within(dialog).getByRole("textbox", { name: "Model id (optional)" }), "bad id!");
    await user.clear(path);
    await user.type(path, "C:\\models\\x.gguf");
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    expect(within(dialog).getByText(/may only contain letters/)).toBeTruthy();
    expect(bodies).toEqual([]);
    await user.clear(within(dialog).getByRole("textbox", { name: "Model id (optional)" }));
    await user.click(within(dialog).getByRole("button", { name: "Import" }));
    await waitFor(() => expect(bodies).toEqual([{ path: "C:\\models\\x.gguf" }]));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Import a GGUF file" })).toBeNull());
    await waitFor(() => expect(window.location.hash).toBe("#/models/my-model"));
  });

  it("an empty catalog offers import", async () => {
    const user = userEvent.setup();
    await open(installFakeDaemon({ models: [] }));
    expect(await screen.findByText("No catalog entries; import a GGUF file")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Import GGUF…" }));
    expect(await screen.findByRole("dialog", { name: "Import a GGUF file" })).toBeTruthy();
  });

  it("filter matches name, id, family, quantization, licence and tags; scopes count", async () => {
    const user = userEvent.setup();
    await open(installFakeDaemon());
    await row(/TinyStories/);
    const box = screen.getByRole("searchbox", { name: "Filter models" });
    await user.type(box, "apache");
    expect(screen.getAllByRole("row")).toHaveLength(2);
    await user.clear(box);
    await user.type(box, "fixture");
    expect(screen.getByRole("row", { name: /TinyStories/ })).toBeTruthy();
    await user.clear(box);
    await user.type(box, "zzz");
    expect(screen.getByText('No models match "zzz"')).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Clear filter" }));
    const scopes = screen.getByRole("group", { name: "Show models" });
    expect(within(scopes).getByRole("button", { name: /Too large/ }).textContent).toBe("Too large 1");
    await user.click(within(scopes).getByRole("button", { name: /Downloaded/ }));
    expect(screen.getAllByRole("row").map((r) => r.textContent)).toEqual([expect.stringContaining("Name"), expect.stringContaining("TinyStories")]);
  });
});
