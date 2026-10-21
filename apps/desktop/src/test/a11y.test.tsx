// F-10-11: every main view renders with realistic data and produces no serious/critical axe violations.
import { render, screen } from "@testing-library/react";
import axe from "axe-core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { chatHistory, daemon, lastError, privacy, runs } from "../state/stores";
import { installFakeDaemon, LOADED, NETWORK_ON, PEER, PRIVACY, STRANGER } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
  runs.set([]);
  lastError.set(null);
  privacy.set(null);
});
afterEach(() => vi.unstubAllGlobals());

async function scan(container: HTMLElement) {
  const result = await axe.run(container, {
    // Contrast needs a real layout engine; everything else runs in jsdom.
    rules: { "color-contrast": { enabled: false } },
  });
  const serious = result.violations.filter((v) => v.impact === "serious" || v.impact === "critical");
  expect(serious.map((v) => `${v.id}: ${v.help} (${v.nodes.map((n) => n.target.join(" ")).join(", ")})`)).toEqual([]);
  expect(result.passes.length, "axe actually ran").toBeGreaterThan(5);
  return result;
}

const VIEWS: [string, string, () => Promise<unknown>][] = [
  ["home", "Home", () => screen.findByText(/Qwen2.5 1.5B Instruct/)],
  ["models", "Models", () => screen.findByText(/TinyStories 15M/)],
  ["chat", "Chat", () => screen.findByRole("textbox", { name: "Message" })],
  ["hardware", "Hardware", () => screen.findByText("Apple M2", { selector: "dd" })],
  ["performance", "Performance", () => screen.findByText(/common_tokens_generated_total/)],
  ["settings", "Settings", () => screen.findByLabelText("Context length")],
  ["diagnostics", "Diagnostics", () => screen.findByText("insufficient_memory")],
  // Peers with networking on, a paired device, a group, a stranger nearby and a pairing code showing.
  ["peers", "Peers", () => screen.findByRole("group", { name: "Pairing code" })],
  // Privacy in trusted-devices mode with the disclosure open (the acknowledge checkbox visible).
  ["privacy", "Privacy", () => screen.findByRole("checkbox", { name: /I understand/ })],
];

describe("axe accessibility scan", () => {
  it.each(VIEWS)("#/%s has no serious or critical violations", async (route, title, ready) => {
    const fake = installFakeDaemon({
      network: NETWORK_ON,
      peers: [PEER],
      discovered: [STRANGER],
      groups: [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }],
      privacy: { ...PRIVACY, mode: "trusted_group" },
    });
    fake.loaded = LOADED;
    await refreshStatus();
    chatHistory.set({
      messages: [
        { role: "user", content: "Hi" },
        { role: "assistant", content: "Hello.", model: LOADED.model_id, usage: { prompt_tokens: 1, completion_tokens: 2, total_tokens: 3 }, timings: { time_to_first_token_ms: 100, prompt_tokens_per_second: 10, decode_tokens_per_second: 20 }, stopped: false },
      ],
    });
    runs.set([{ at: Date.now(), model: LOADED.model_id, completion_tokens: 2, ttft_ms: 100, decode_tps: 20, stopped: false }]);
    lastError.set({ code: "insufficient_memory", title: "This model needs more memory than your computer has available.", next_step: "Choose a smaller model.", detail: "need 9 GiB", hint: null });
    window.location.hash = `#/${route}`;
    const { container } = render(<App poll={false} />);
    expect(await screen.findByRole("heading", { level: 1, name: title })).toBeTruthy();
    if (route === "peers") {
      await screen.findByText("Studio Mac", { selector: ".model-name" });
      (await screen.findByRole("button", { name: "Pair this device" })).click();
    }
    if (route === "privacy") {
      // The disclosure only appears when leaving local-only, so start there and pick trusted devices.
      fake.privacy = { ...PRIVACY };
      privacy.set({ ...PRIVACY });
      (await screen.findByRole("radio", { name: /Trusted devices/ })).click();
    }
    await ready();
    await scan(container);
  }, 20000);

  it("peers view with a device seen nearby, one that failed to answer and an open connect panel showing an error", async () => {
    const away = { ...PEER, id: "ee".repeat(32), short_label: "eeee-eeee-eeee", name: "Laptop", connected: false, health: "disconnected" as const, last_addr: "172.20.10.6:8193", last_error: { addr: "172.20.10.6:8193", code: "peer_lost", message: "no answer from 172.20.10.6:8193 within 4 s", hint: "the device may be asleep or on another network, or a firewall may block UDP port 8193", at_utc: "2026-10-19T03:40:00Z" } };
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER, away], discovered: [{ ...STRANGER, id: PEER.id, trusted: true, addrs: [STRANGER.addr] }, STRANGER] });
    window.location.hash = "#/peers";
    const { container } = render(<App poll={false} />);
    (await screen.findByRole("button", { name: "Connect to Laptop" })).click();
    (await screen.findByRole("button", { name: "Connect" })).click();
    await screen.findByText("The connection to the other device was lost.");
    await scan(container);
  }, 20000);

  it("models view with an open download confirmation and a row error", async () => {
    installFakeDaemon();
    window.location.hash = "#/models?confirm=qwen2.5-1.5b-instruct-q4_k_m";
    const { container } = render(<App poll={false} />);
    await screen.findByRole("group", { name: /Confirm download of Qwen2.5/ });
    await scan(container);
  }, 20000);
});
