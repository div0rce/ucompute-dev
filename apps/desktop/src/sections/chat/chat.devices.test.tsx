// Chat cases moved from src/test/devices.test.tsx in Phase 0 (owner D), assertions unchanged: a paired device runs
// prompts without a model loaded here (two-machine defect E2), this computer named neutrally, and plurals (E3).
// Unit level against the fake daemon; the physical two-machine checks are the integrator's. Phase 1 (D): the "Run on"
// select is the Destination picker, and a paired device must be connected and in a group that allows prompts to be
// chosen (plan §2.5), so these scenarios put Studio Mac in one.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { fitText, plural } from "../../format";
import { chatHistory, daemon, lastError, privacy, runTarget, settings } from "../../state/stores";
import { refresh } from "../../state/live";
import { apiError, installFakeDaemon, LOADED, NETWORK_ON, PEER, PRIVACY } from "../../test/mocks";

beforeEach(() => {
  daemon.set({ reachable: true, version: "0.1.0", status: null, spawned: false });
  chatHistory.set({ messages: [] });
  lastError.set(null);
  privacy.set(null);
  runTarget.set("local");
  settings.set({ device: "auto", n_ctx: null, device_name: "" });
});
afterEach(() => vi.unstubAllGlobals());

const TRUSTED = { ...PRIVACY, mode: "trusted_group" as const };
const HOME = [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }];
const posts = (fake: ReturnType<typeof installFakeDaemon>, path: string) =>
  fake.fetchMock.mock.calls
    .filter(([u, init]) => String(u).endsWith(path) && (init as RequestInit)?.method === "POST")
    .map(([, init]) => JSON.parse(String((init as RequestInit).body ?? "{}")));
const chatBody = (fake: ReturnType<typeof installFakeDaemon>) => posts(fake, "/v1/chat/completions").at(-1);

describe("E1: no macOS wording on other systems", () => {
  it("names this computer neutrally where it is the target or the fit", async () => {
    expect(fitText({ verdict: "fits", headroom_bytes: 1024 ** 3 })).toBe("Fits on this computer (1.0 GB to spare)");
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: TRUSTED });
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const sel = await screen.findByRole("combobox", { name: "Destination" });
    expect(within(sel).getAllByRole("option")[0].textContent).toBe("This computer");
  });
});

describe("E2: a paired device runs prompts without a model loaded here", () => {
  it("offers every model this daemon knows, sends it with the peer target, and says where it ran", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: HOME, privacy: TRUSTED });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const box = await screen.findByRole("textbox", { name: "Message" });
    expect(box).toHaveProperty("disabled", true);

    await screen.findByRole("option", { name: /^Studio Mac · / });
    await user.selectOptions(await screen.findByRole("combobox", { name: "Destination" }), `peer:${PEER.id}`);
    expect(screen.queryByRole("button", { name: "Load model" })).toBeNull();
    expect(screen.getByText(/Prompts run on Studio Mac, which loads the model you pick/)).toBeTruthy();
    const model = screen.getByRole("combobox", { name: "Model" });
    // Not only what is downloaded here: the other device may have a model this one does not.
    expect(within(model).getAllByRole("option").map((o) => o.getAttribute("value"))).toEqual(["qwen2.5-1.5b-instruct-q4_k_m", "stories15M-q4_0", "llama-3.1-70b-q4_k_m"]);
    await user.selectOptions(model, "llama-3.1-70b-q4_k_m");
    await waitFor(() => expect(box).toHaveProperty("disabled", false));

    await user.type(box, "Hi{Enter}");
    await screen.findByText(/Ran on Studio Mac/);
    expect(chatBody(fake)).toMatchObject({ model: "llama-3.1-70b-q4_k_m", target: `peer:${PEER.id}` });
    expect(fake.loaded).toBeNull();
  });

  it("tells the user plainly when the device does not have the model", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: HOME, privacy: TRUSTED, peerModels: { [PEER.id]: ["stories15M-q4_0"] } });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await screen.findByRole("option", { name: /^Studio Mac · / });
    await user.selectOptions(screen.getByRole("combobox", { name: "Destination" }), `peer:${PEER.id}`);
    await user.selectOptions(screen.getByRole("combobox", { name: "Model" }), "qwen2.5-1.5b-instruct-q4_k_m");
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Hi{Enter}");
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Studio Mac does not have Qwen2.5 1.5B Instruct (Q4_K_M).");
    expect(alert.textContent).toContain("Download it on Studio Mac, or pick another model under Model.");
    expect(alert.textContent).toContain("model_not_downloaded");
    // Not saved as an empty reply.
    expect(chatHistory.get().messages.map((m) => m.role)).toEqual(["user"]);

    await user.selectOptions(screen.getByRole("combobox", { name: "Model" }), "stories15M-q4_0");
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Again{Enter}");
    await screen.findByText(/Ran on Studio Mac/);
  });

  it("the Run panel never shows the last known connection as live once the service stops", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: HOME, privacy: TRUSTED });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await screen.findByRole("option", { name: /^Studio Mac · / });
    await user.selectOptions(screen.getByRole("combobox", { name: "Destination" }), `peer:${PEER.id}`);
    const panel = screen.getByRole("complementary", { name: "Run details" });
    const value = (k: string) => within(panel).getByText(k).nextElementSibling!.textContent;
    await waitFor(() => expect(value("State")).toBe("Connected"));
    vi.stubGlobal("fetch", vi.fn(async () => Promise.reject(new TypeError("Failed to fetch"))));
    await act(() => refreshStatus());
    expect(value("State")).toBe("— service not running");
    expect(value("Can run there")).toBe("— service not running");
    // Nor in Destination: the device is not offered as connected from the kept rows, and the user's choice is kept.
    const option = screen.getByRole("option", { name: "Studio Mac: state not known (service not running)" });
    expect(option).toHaveProperty("disabled", true);
    expect(runTarget.get()).toBe(`peer:${PEER.id}`);
  });

  it("a failed mesh read leaves the destination's state unknown and nothing is sent there, without dropping the choice", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: HOME, privacy: TRUSTED });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await screen.findByRole("option", { name: /^Studio Mac · / });
    await user.selectOptions(screen.getByRole("combobox", { name: "Destination" }), `peer:${PEER.id}`);
    fake.routes["GET /api/peers"] = () => apiError("internal", "store unreadable", 500);
    await act(() => refresh("mesh"));
    expect(await screen.findByRole("option", { name: "Studio Mac: state not known (not refreshed)" })).toHaveProperty("disabled", true);
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveProperty("disabled", true);
    expect(runTarget.get()).toBe(`peer:${PEER.id}`);
  });

  it("a local run still needs a model loaded here", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: HOME, privacy: TRUSTED });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const box = await screen.findByRole("textbox", { name: "Message" });
    expect(box).toHaveProperty("disabled", true);
    expect(box.getAttribute("placeholder")).toBe("Load a model first");
    expect(await screen.findByRole("button", { name: "Load model" })).toBeTruthy();
  });
});

describe("E3: counts are pluralised", () => {
  it("one device, two devices, one token", async () => {
    expect([plural(1, "device"), plural(2, "device"), plural(0, "token"), plural(1024, "token")]).toEqual(["1 device", "2 devices", "0 tokens", "1,024 tokens"]);
    const other = { ...PEER, id: "cd".repeat(32), short_label: "cdcd-cdcd-cdcd", name: "Old laptop" };
    installFakeDaemon({
      network: NETWORK_ON,
      peers: [PEER, other],
      groups: [
        { name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } },
        { name: "work", members: [PEER.id, other.id], policy: { allow_remote_inference: false, allow_pooled_inference: false } },
      ],
    });
    // The groups editor now lives in Settings › Sharing & permissions (was the Peers view).
    window.location.hash = "#/settings/sharing";
    render(<App poll={false} />);
    expect(await screen.findByText("Group home with 1 device.")).toBeTruthy();
    expect(screen.getByText("Group work with 2 devices.")).toBeTruthy();
  });

  it("a one-token reply says 1 token", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ chat: { chunks: ["Hi"], delayMs: 0 } });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await user.type(await screen.findByRole("textbox", { name: "Message" }), "Hi{Enter}");
    // The footer lists its facts separately (plan §2.5): "1 token", then "TTFT 120 ms".
    const one = await screen.findByText("1 token");
    expect(one.closest(".reply-foot")!.textContent).toMatch(/1 token · TTFT 120 ms · 42\.5 tok\/s/);
  });
});

describe("destinations before groups are read", () => {
  it("a connected device is disabled as not known yet, never as not permitted", async () => {
    const { destinations } = await import("./DestinationPicker");
    const { deviceRows } = await import("../../state/devices");
    const rows = deviceRows({ daemon: { reachable: true, responding: true, version: "0.1.0", status: null, spawned: false }, network: NETWORK_ON, mesh: { peers: [PEER], discovered: [], revocations: [] }, groups: null, models: null, hardware: null, deviceName: "", localOnly: false, jobs: null, audit: null, dialing: null, now: Date.now() });
    expect(destinations(rows, null)[1]).toMatchObject({ disabled: "groups not read yet", label: "Studio Mac: groups not read yet" });
    expect(destinations(rows, HOME)[1].disabled).toBeNull();
  });
});
