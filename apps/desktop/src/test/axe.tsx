// Shared by the a11y tests (shell and every section): the axe scan and the realistic scenario the views are scanned in.
import { render } from "@testing-library/react";
import axe from "axe-core";
import { expect } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { chatHistory, daemon, lastError, privacy } from "../state/stores";
import { installFakeDaemon, LOADED, NETWORK_ON, PEER, PRIVACY, STRANGER } from "./mocks";

/**
 * Fails on any violation, moderate ones (heading-order) too: the plan's gate is 0. Contrast and label-in-name need a
 * real layout engine (checked in the real-browser pass).
 */
export async function scan(container: HTMLElement) {
  const result = await axe.run(container, {
    rules: { "color-contrast": { enabled: false } },
  });
  expect(result.violations.map((v) => `${v.id}: ${v.help} (${v.nodes.map((n) => n.target.join(" ")).join(", ")})`)).toEqual([]);
  expect(result.passes.length, "axe actually ran").toBeGreaterThan(5);
  return result;
}

export function resetStores() {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
  lastError.set(null);
  privacy.set({ data: null, at: null, error: null });
}

/**
 * Networking on, a paired device in a group that allows prompts, a stranger nearby, trusted-devices privacy, a model
 * loaded, one chat exchange and a last error; then renders the app at `#/<route>`.
 */
export async function scenario(route: string) {
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
  lastError.set({ code: "insufficient_memory", title: "This model needs more memory than your computer has available.", next_step: "Choose a smaller model.", detail: "need 9 GiB", hint: null });
  window.location.hash = `#/${route}`;
  const { container } = render(<App poll={false} />);
  return { fake, container };
}
