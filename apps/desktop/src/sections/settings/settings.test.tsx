// Settings (owner D, plan §2.7 and the D list in §5.4): the theme persists; the network and sharing switches send the
// right PUT bodies; the group pooled flag survives a PUT and is never shown; no public compute, credit or contributor
// control exists; the privacy "what is kept" text and the chat-history delete; security and storage read the service.
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../App";
import { refreshStatus } from "../../daemon";
import { refresh } from "../../state/live";
import { chatHistory, daemon, lastError, privacy, runTarget, settings } from "../../state/stores";
import { ui } from "../../state/ui";
import { installFakeDaemon, NETWORK, NETWORK_ON, PEER, PEERS_E1 } from "../../test/mocks";
import { KEPT, PAGES } from "./Settings";

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
  delete document.documentElement.dataset.theme;
});

const puts = (fake: ReturnType<typeof installFakeDaemon>, path: string) =>
  fake.fetchMock.mock.calls.filter(([u, init]) => String(u).endsWith(path) && (init as RequestInit)?.method === "PUT").map(([, init]) => JSON.parse(String((init as RequestInit).body)));

async function open(page: string, overrides: Parameters<typeof installFakeDaemon>[0] = {}) {
  const fake = installFakeDaemon(overrides);
  await refreshStatus();
  window.location.hash = `#/settings/${page}`;
  render(<App poll={false} />);
  await screen.findByRole("heading", { level: 2, name: PAGES.find((p) => p.id === page)!.label });
  return fake;
}

describe("Settings", () => {
  it("lists its pages in a nav and opens General for #/settings", async () => {
    installFakeDaemon();
    await refreshStatus();
    window.location.hash = "#/settings";
    render(<App poll={false} />);
    const nav = await screen.findByRole("navigation", { name: "Settings pages" });
    expect(within(nav).getAllByRole("link").map((a) => a.textContent)).toEqual(PAGES.map((p) => p.label));
    expect(within(nav).getByRole("link", { name: "General" }).getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("heading", { level: 2, name: "General" })).toBeTruthy();
    // About: the service as the app sees it (the old Diagnostics service lines).
    expect(within(screen.getByRole("main")).getByText("Running")).toBeTruthy();
    expect(screen.getByText("http://127.0.0.1:8192", { selector: "code" })).toBeTruthy();
    // The app's own version, read from package.json at build time.
    const { version } = (await import("../../../package.json")).default;
    expect(screen.getByText("App version", { selector: "dt" }).nextElementSibling!.textContent).toBe(version);
  });

  it("the theme applies at once and persists", async () => {
    const user = userEvent.setup();
    await open("general");
    await user.click(screen.getByRole("radio", { name: "Dark" }));
    expect(ui.get().theme).toBe("dark");
    expect(JSON.parse(localStorage.getItem("common.ui")!).theme).toBe("dark");
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("dark"));
    await user.click(screen.getByRole("radio", { name: "System" }));
    await waitFor(() => expect(document.documentElement.dataset.theme).toBeUndefined());
  });

  it("Execution: device and context length are kept on this computer; admission limits are read-only", async () => {
    const user = userEvent.setup();
    await open("execution");
    await user.click(screen.getByRole("radio", { name: "GPU" }));
    await user.type(screen.getByLabelText("Context length"), "4096");
    expect(settings.get()).toMatchObject({ device: "gpu", n_ctx: 4096 });
    expect(screen.getByText(/loads requested by other programs or paired devices use the service defaults/)).toBeTruthy();
    expect(screen.getAllByText("Configured in the service settings file; applies after restart.")).toHaveLength(2);
  });

  it("Network: turning networking off also turns sharing off", async () => {
    const user = userEvent.setup();
    const fake = await open("network", { network: { ...NETWORK_ON, serve_peers: true } });
    const sw = await screen.findByRole("switch", { name: "Networking" });
    await waitFor(() => expect(sw).toHaveProperty("checked", true));
    await user.click(sw);
    await screen.findByText("Networking is off.");
    expect(puts(fake, "/api/network")).toEqual([{ enabled: false, serve_peers: false }]);
    expect(fake.network.enabled).toBe(false);
  });

  it("Sharing: the switch sends serve_peers and is disabled with the reason while networking is off", async () => {
    const user = userEvent.setup();
    const fake = await open("sharing", { network: NETWORK });
    const sw = await screen.findByRole("switch", { name: "Run requests for paired devices" });
    expect(sw).toHaveProperty("disabled", true);
    expect(document.getElementById(sw.getAttribute("aria-describedby")!)!.textContent).toBe("Turn on networking first (Settings › Network).");

    fake.network = { ...NETWORK_ON };
    await act(() => refresh("network"));
    await waitFor(() => expect(screen.getByRole("switch", { name: "Run requests for paired devices" })).toHaveProperty("disabled", false));
    await user.click(screen.getByRole("switch", { name: "Run requests for paired devices" }));
    await screen.findByText("Sharing is on.");
    expect(puts(fake, "/api/network")).toEqual([{ enabled: true, serve_peers: true }]);
    expect(screen.getByText(/60 requests\/min · 20,000 tokens\/min · 3 at once/)).toBeTruthy();
  });

  it.each(["network", "sharing"])("%s: last known service values are not shown as current while the service is not running", async (page) => {
    await open(page, { network: { ...NETWORK_ON, serve_peers: true } });
    await waitFor(() => expect(screen.getAllByRole("switch").length).toBeGreaterThan(0));
    act(() => daemon.set((d) => ({ ...d, reachable: false, status: null })));
    expect(screen.queryAllByRole("switch")).toHaveLength(0);
    expect(screen.getAllByText("Not known while the background service is not running.").length).toBeGreaterThan(1);
    expect(screen.queryByText(NETWORK_ON.listen_addr!)).toBeNull();
    expect(screen.queryByText(/requests\/min/)).toBeNull();
  });

  it("Sharing: a group PUT keeps the pooled flag, which is never shown", async () => {
    const user = userEvent.setup();
    const fake = await open("sharing", {
      network: NETWORK_ON,
      peers: [PEER],
      groups: [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: false, allow_pooled_inference: true } }],
    });
    await user.click(await screen.findByRole("checkbox", { name: "Members may run prompts for each other" }));
    await waitFor(() => expect(puts(fake, "/api/groups")).toHaveLength(1));
    expect(puts(fake, "/api/groups")[0]).toEqual({ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: true } });
    expect(screen.getByRole("main").textContent).not.toMatch(/pool/i);
  });

  it("has no public compute, credit, contributor, Swarm, Cloud or billing control on any page", async () => {
    await open("general", { network: NETWORK_ON, peers: [PEER] });
    for (const p of PAGES) {
      act(() => void (window.location.hash = `#/settings/${p.id}`));
      await screen.findByRole("heading", { level: 2, name: p.label });
      expect(screen.getByRole("main").textContent).not.toMatch(/public compute|credit|contribut|swarm|cloud|billing/i);
    }
  });

  it("Privacy says what is kept and deletes the chat history after confirmation", async () => {
    const user = userEvent.setup();
    chatHistory.set({ messages: [{ role: "user", content: "Hi" }] });
    await open("privacy");
    expect(screen.getByText(KEPT)).toBeTruthy();
    expect(KEPT).toBe(
      "The service never stores prompts or replies. It keeps request details (model, device, tokens, timing) in memory until it restarts. It also keeps a log of pairing, connections and requests to and from other devices (audit.jsonl). Your conversation is stored only in this app on this computer.",
    );
    await user.click(screen.getByRole("button", { name: "Delete chat history on this computer" }));
    const confirm = screen.getByRole("group", { name: "Delete chat history?" });
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(chatHistory.get().messages).toHaveLength(1);
    await user.click(screen.getByRole("button", { name: "Delete chat history on this computer" }));
    await user.click(within(screen.getByRole("group", { name: "Delete chat history?" })).getByRole("button", { name: "Delete chat history" }));
    expect(chatHistory.get().messages).toEqual([]);
    expect(JSON.parse(localStorage.getItem("common.chat")!)).toEqual({ messages: [] });
    expect(screen.getByRole("button", { name: "Delete chat history on this computer" })).toHaveProperty("disabled", true);
  });

  it("Security groups this device's id 8×8 and lists revocations; Storage reads the service", async () => {
    await open("security", { network: NETWORK_ON, peers: PEERS_E1.peers, revocations: PEERS_E1.revocations });
    expect(await screen.findByText("a1b2c3d4 e5f60718 293a4b5c 6d7e8f90 a1b2c3d4 e5f60718 293a4b5c 6d7e8f90")).toBeTruthy();
    const row = (await screen.findByText("cdcd-cdcd-cdcd")).closest("tr")!;
    expect(within(row).getByText("This computer")).toBeTruthy();
    expect(within(row).getByText("1")).toBeTruthy();

    act(() => void (window.location.hash = "#/settings/storage"));
    await screen.findByRole("heading", { level: 2, name: "Storage" });
    await screen.findByText("16.0 GB");
    expect(screen.getByText("1 model, 18.2 MB")).toBeTruthy();
    expect(screen.getAllByText("not reported by the service").length).toBeGreaterThan(0);
  });
});
