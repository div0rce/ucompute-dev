// Chat (owner D, plan §2.5 and the D list in §5.4): the session controller outlives the view and stops from Jobs; the
// client ref is registered before the request and sent only to an E2 service; Esc stops; the Destination picker uses
// `live` and the Can run there words; the peer's model list comes from what it reported (E1); loads and the
// conversation reset confirm first; a failed reply is an Attention entry until a reply succeeds.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import type { Peer } from "../../api/client";
import { refreshStatus } from "../../daemon";
import { attention, ownStreams, refresh } from "../../state/live";
import { chatHistory, daemon, lastError, privacy, runTarget, settings } from "../../state/stores";
import { apiError, AUDIT, installFakeDaemon, LOADED, NETWORK_ON, PEER, PEERS_E1, PRIVACY } from "../../test/mocks";
import { ui } from "../../state/ui";
import { TOGGLE_EVENT } from "../../ui/Inspector";
import type { Reply } from "./session";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  chatHistory.set({ messages: [] });
  lastError.set(null);
  privacy.set({ data: null, at: null, error: null });
  runTarget.set("local");
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
});
afterEach(() => vi.unstubAllGlobals());

const TRUSTED = { ...PRIVACY, mode: "trusted_group" as const };
const group = (...members: string[]) => [{ name: "home", members, policy: { allow_remote_inference: true, allow_pooled_inference: false } }];
const SLOW = { chunks: Array.from({ length: 200 }, (_, i) => `w${i} `), delayMs: 10 };
const chatCall = (fake: ReturnType<typeof installFakeDaemon>) => fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))!;
const replies = () => chatHistory.get().messages as Reply[];

async function open(fake: ReturnType<typeof installFakeDaemon>, hash = "#/chat") {
  await refreshStatus();
  window.location.hash = hash;
  render(<App poll={false} />);
  void fake;
  return screen.findByRole("textbox", { name: "Message" });
}

describe("before the first status read", () => {
  it("nothing says no model is loaded and no Load is offered", async () => {
    installFakeDaemon({ routes: { "GET /api/status": () => apiError("internal", "status unreadable", 500) } });
    await refreshStatus();
    expect(daemon.get()).toMatchObject({ reachable: true, status: null });
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const box = await screen.findByRole("textbox", { name: "Message" });
    expect(box.getAttribute("placeholder")).toBe("Waiting for the background service's status");
    expect(box).toHaveProperty("disabled", true);
    const panel = screen.getByRole("complementary", { name: "Run details" });
    expect(within(panel).getByText("Loaded model", { selector: "dt" }).nextElementSibling!.textContent).toBe("— status not read yet");
    expect(screen.queryByRole("button", { name: /^Load/ })).toBeNull();
  });

  it("a kept status is not shown as the loaded model while the service is busy or its status read failed", async () => {
    const fake = installFakeDaemon();
    fake.loaded = LOADED;
    await open(fake);
    const panel = screen.getByRole("complementary", { name: "Run details" });
    const value = () => within(panel).getByText("Loaded model", { selector: "dt" }).nextElementSibling!.textContent;
    await waitFor(() => expect(value()).toMatch(/ · Metal$/));
    // Busy is most often a load of another model: the one being replaced is not shown as current.
    act(() => daemon.set((d) => ({ ...d, responding: false })));
    expect(value()).toBe("— not refreshed");
    act(() => daemon.set((d) => ({ ...d, responding: true, statusError: { code: "internal", title: "t", next_step: "n", detail: "", hint: null } })));
    expect(value()).toBe("— not refreshed");
  });
});

describe("session controller", () => {
  it("keeps streaming on another section and Jobs' Stop (ownStreams) saves the partial reply within 300 ms", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ chat: SLOW });
    fake.loaded = LOADED;
    await user.type(await open(fake), "Hello{Enter}");
    await screen.findByText(/w2 /);

    act(() => void (window.location.hash = "#/jobs"));
    await waitFor(() => expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull());
    const refs = Object.keys(ownStreams.get());
    expect(refs).toHaveLength(1);
    const t0 = performance.now();
    act(() => ownStreams.get()[refs[0]].stop());
    await waitFor(() => expect(replies()[1]?.stopped).toBe(true));
    expect(performance.now() - t0).toBeLessThan(300);
    expect((chatCall(fake)[1] as RequestInit).signal?.aborted).toBe(true);
    expect(replies()[1].content.length).toBeGreaterThan(0);
    expect(replies()[1].content.length).toBeLessThan(200 * 5);
    expect(ownStreams.get()).toEqual({});
  });

  it("registers the client ref before the request, sends it to an E2 service, and links the reply to its job", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    fake.loaded = LOADED;
    const seen: { header?: string; registered?: string[] } = {};
    const serve = fake.fetchMock.getMockImplementation() as (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
    fake.fetchMock.mockImplementation((input: RequestInfo | URL, init?: RequestInit) => {
      if (String(input).endsWith("/v1/chat/completions")) Object.assign(seen, { header: (init?.headers as Record<string, string>)["X-Common-Client-Ref"], registered: Object.keys(ownStreams.get()) });
      return serve(input, init);
    });
    await user.type(await open(fake), "Hi{Enter}");
    await screen.findByText(/4 tokens/);

    expect(seen.header).toMatch(/^[0-9a-f-]{36}$/);
    expect(seen.registered).toEqual([seen.header]);
    const [question, answer] = replies();
    expect(question.client_ref).toBe(seen.header);
    expect(answer).toMatchObject({ client_ref: seen.header, job_id: expect.stringMatching(/^chatcmpl-/), device_actual: LOADED.device_actual, finish_reason: "stop" });
    const link = screen.getByRole("link", { name: `Job ${answer.job_id}` });
    expect(link.getAttribute("href")).toBe(`#/jobs/${answer.job_id}`);
    // The E3 row carries the same ref, so Jobs reads it as "This app".
    expect(fake.jobs !== "unsupported" && fake.jobs.recent.find((j) => j.id === answer.job_id)?.client_ref).toBe(seen.header);
  });

  it("does not send the header to a service before E2 (its CORS would refuse it), but Stop still works", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ e2: false });
    fake.loaded = LOADED;
    await user.type(await open(fake), "Hi{Enter}");
    await screen.findByText(/4 tokens/);
    expect((chatCall(fake)[1] as RequestInit).headers).not.toHaveProperty("X-Common-Client-Ref");
    expect(replies().some((m) => m.client_ref)).toBe(false);
  });

  it("Esc stops generation", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ chat: SLOW });
    fake.loaded = LOADED;
    await user.type(await open(fake), "Hello{Enter}");
    await screen.findByText(/w2 /);
    await user.keyboard("{Escape}");
    await screen.findByText(/Stopped\./);
    expect(replies()[1].stopped).toBe(true);
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
  });

  it("a failed reply is an Attention entry and an announcement until a reply succeeds", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ routes: { "POST /v1/chat/completions": () => apiError("backend_failed", "llama-server exited", 500) } });
    fake.loaded = LOADED;
    const box = await open(fake);
    await user.type(box, "Hi{Enter}");
    expect((await screen.findByRole("alert")).textContent).toContain("The inference engine stopped unexpectedly.");
    expect(attention.get().map((a) => a.text)).toEqual(["A reply from this app failed: The inference engine stopped unexpectedly."]);
    expect(replies().map((m) => m.role)).toEqual(["user"]);

    fake.routes = {};
    await user.type(box, "Again{Enter}");
    await screen.findByText(/4 tokens/);
    expect(attention.get()).toEqual([]);
  });
});

describe("Destination picker and models", () => {
  const laptop = PEERS_E1.peers[1];
  const unhealthy: Peer = { ...PEER, id: "dd".repeat(32), short_label: "dddd-dddd-dddd", name: "Busy box", health: "unhealthy" };
  const done = { ts_utc: "2026-10-22T10:31:00Z", kind: "remote_request", peer_id: PEER.id, fields: { role: "client", phase: "finished", request_id: "r1", peer: "0f1e-2d3c-4b5a", model_id: "qwen2.5-1.5b-instruct-q4_k_m", outcome: "done", completion_tokens: 5, duration_ms: 900 } };

  it("lists live, permitted devices with their Can run there word and the rest disabled with the reason", async () => {
    const fake = installFakeDaemon({
      network: NETWORK_ON,
      peers: [...PEERS_E1.peers, unhealthy],
      discovered: PEERS_E1.discovered,
      groups: group(PEER.id, laptop.id, unhealthy.id),
      audit: [...AUDIT, done],
      privacy: TRUSTED,
    });
    await open(fake);
    const sel = await screen.findByRole("combobox", { name: "Destination" });
    await waitFor(() =>
      expect(within(sel).getAllByRole("option").map((o) => [o.textContent, (o as HTMLOptionElement).disabled])).toEqual([
        ["This computer", false],
        ["Studio Mac · Can send · last request done", false],
        ["Laptop: not seen nearby", true],
        ["Busy box: not responding", true],
      ]),
    );
  });

  it("a connected device outside every prompt group is listed but not permitted", async () => {
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: TRUSTED });
    await open(fake);
    const opt = await screen.findByRole("option", { name: /^Studio Mac: not permitted here: add it to a group that allows prompts$/ });
    expect(opt).toHaveProperty("disabled", true);
  });

  it("offers a peer's reported models first, then the rest of the catalog as not reported", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: PEERS_E1.peers, discovered: PEERS_E1.discovered, groups: group(PEER.id), privacy: TRUSTED });
    await open(fake);
    await screen.findByRole("option", { name: /^Studio Mac · / });
    await user.selectOptions(screen.getByRole("combobox", { name: "Destination" }), `peer:${PEER.id}`);
    const model = screen.getByRole("combobox", { name: "Model" });
    const [reported, rest] = within(model).getAllByRole("group");
    expect(reported.getAttribute("label")).toBe("Reported by Studio Mac when the connection opened");
    // The unknown hash (id null) is not offered: it cannot be run from here.
    expect(within(reported).getAllByRole("option").map((o) => o.getAttribute("value"))).toEqual(["qwen2.5-1.5b-instruct-q4_k_m"]);
    expect(rest.getAttribute("label")).toBe("Not reported by Studio Mac");
    expect(within(rest).getAllByRole("option").map((o) => o.getAttribute("value"))).toEqual(["stories15M-q4_0", "llama-3.1-70b-q4_k_m"]);
    expect((model as HTMLSelectElement).value).toBe("qwen2.5-1.5b-instruct-q4_k_m");
    expect(screen.queryByText(/may not be on that device/)).toBeNull();
    await user.selectOptions(model, "llama-3.1-70b-q4_k_m");
    expect(screen.getByText("Llama 3.1 70B (Q4_K_M) may not be on that device.")).toBeTruthy();
    // The Run panel: E1 hardware with its timing tag, and RTT.
    const panel = screen.getByRole("complementary", { name: "Run details" });
    expect(panel.textContent).toContain("Apple M2 Pro · 16.0 GB");
    expect(panel.textContent).toContain("at its network start");
    expect(panel.textContent).toContain("4.2 ms");
  });

  it("falls back to this computer, with a visible notice, when the chosen device stops being connected", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: group(PEER.id), privacy: TRUSTED });
    await open(fake);
    await screen.findByRole("option", { name: /^Studio Mac · / });
    const sel = screen.getByRole("combobox", { name: "Destination" }) as HTMLSelectElement;
    await user.selectOptions(sel, `peer:${PEER.id}`);
    fake.peers[0].connected = false;
    await act(() => refresh("mesh"));
    await waitFor(() => expect(sel.value).toBe("local"));
    expect(runTarget.get()).toBe("local");
    expect(screen.getByText(/^Studio Mac: not seen nearby\. Replies now run on this computer\.$/)).toBeTruthy();
  });

  it("#/chat?model= picks a downloaded model and loads it with the Settings defaults", async () => {
    const user = userEvent.setup();
    settings.set({ device: "cpu", n_ctx: 2048, device_name: "" });
    const fake = installFakeDaemon();
    const box = await open(fake, "#/chat?model=stories15M-q4_0");
    expect(box.getAttribute("placeholder")).toBe("Load a model first");
    expect((screen.getByRole("combobox", { name: "Model" }) as HTMLSelectElement).value).toBe("stories15M-q4_0");
    await user.click(screen.getByRole("button", { name: "Load model" }));
    await waitFor(() => expect(box).toHaveProperty("disabled", false));
    expect(fake.loaded).toMatchObject({ model_id: "stories15M-q4_0", device_requested: "cpu", n_ctx: 2048 });
  });

  it("replacing the loaded model asks first and says paired devices can load another one while sharing is on", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK_ON, serve_peers: true } });
    fake.models.find((m) => m.id === LOADED.model_id)!.downloaded = true;
    fake.loaded = LOADED;
    await open(fake, "#/chat?model=stories15M-q4_0");
    await user.click(await screen.findByRole("button", { name: "Load TinyStories 15M (Q4_0) (replaces Qwen2.5 1.5B Instruct (Q4_K_M))" }));
    const confirm = screen.getByRole("group", { name: "Replace the loaded model?" });
    expect(confirm.textContent).toContain("Paired devices' requests can load a different model here.");
    expect(fake.loaded?.model_id).toBe(LOADED.model_id);
    await user.click(within(confirm).getByRole("button", { name: "Load TinyStories 15M (Q4_0)" }));
    await waitFor(() => expect(fake.loaded?.model_id).toBe("stories15M-q4_0"));
  });

  it("#/chat?reply= (Jobs › Open in Chat) scrolls to that reply, by job id or client ref", async () => {
    const scrolled: Element[] = [];
    Element.prototype.scrollIntoView = function (this: Element) {
      scrolled.push(this);
    };
    try {
      const fake = installFakeDaemon();
      fake.loaded = LOADED;
      chatHistory.set({
        messages: [
          { role: "user", content: "one", client_ref: "ref-1" },
          { role: "assistant", content: "first", client_ref: "ref-1", job_id: "chatcmpl-1" },
          { role: "user", content: "two", client_ref: "ref-2" },
        ],
      });
      await open(fake, "#/chat?reply=chatcmpl-1");
      await waitFor(() => expect(scrolled.at(-1)?.textContent).toContain("first"));
      act(() => void (window.location.hash = "#/chat?reply=ref-2"));
      await waitFor(() => expect(scrolled.at(-1)?.textContent).toBe("two"));
    } finally {
      delete (Element.prototype as Partial<Element>).scrollIntoView;
    }
  });

  it("Mod+I (the inspector toggle) hides and shows the Run panel, and the choice persists", async () => {
    const fake = installFakeDaemon();
    await open(fake);
    const panel = screen.getByRole("complementary", { name: "Run details" });
    expect(within(panel).getByText("Last reply")).toBeTruthy();
    act(() => void window.dispatchEvent(new Event(TOGGLE_EVENT)));
    expect(within(panel).queryByText("Last reply")).toBeNull();
    expect(within(panel).getByRole("button", { name: "Show run details" }).getAttribute("aria-expanded")).toBe("false");
    expect(ui.get().inspectors.chat?.collapsed).toBe(true);
    act(() => void window.dispatchEvent(new Event(TOGGLE_EVENT)));
    expect(within(panel).getByText("Last reply")).toBeTruthy();
  });

  it("New conversation confirms before deleting the conversation", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    fake.loaded = LOADED;
    chatHistory.set({ messages: [{ role: "user", content: "keep me" }] });
    await open(fake);
    await user.click(screen.getByRole("button", { name: "New conversation" }));
    const dialog = await screen.findByRole("dialog", { name: "Start a new conversation?" });
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(chatHistory.get().messages).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "New conversation" }));
    await user.click(within(await screen.findByRole("dialog", { name: "Start a new conversation?" })).getByRole("button", { name: "Delete conversation" }));
    expect(chatHistory.get().messages).toEqual([]);
  });
});
