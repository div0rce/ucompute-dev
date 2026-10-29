// F-10-11: the primary journey (Models -> Download -> Load -> Chat -> Stop) is completable with the keyboard only.
// Chat is driven through the frozen contract names only (textbox "Message", buttons "Send" and "Stop").
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { chatHistory, daemon } from "../state/stores";
import { installFakeDaemon } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
});
afterEach(() => vi.unstubAllGlobals());

const QWEN = "Qwen2.5 1.5B Instruct (Q4_K_M)";

describe("keyboard-only primary flow", () => {
  it("downloads, loads, chats and stops without a mouse", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ chat: { chunks: Array.from({ length: 200 }, (_, i) => `word${i} `), delayMs: 10 } });

    /**
     * Moves focus to `el` with keys only: Tab (Shift+Tab with `back`) until focus is on it or inside its toolbar
     * (one Tab stop), then arrow keys within the toolbar.
     */
    const keyTo = async (el: HTMLElement, back = false) => {
      const bar = el.closest('[role="toolbar"]');
      const there = () => (bar ? bar.contains(document.activeElement) : document.activeElement === el);
      for (let i = 0; i < 80 && !there(); i++) await user.tab({ shift: back });
      for (let i = 0; i < 20 && bar && document.activeElement !== el; i++) await user.keyboard("{ArrowRight}");
      expect(document.activeElement).toBe(el);
    };

    await refreshStatus();
    window.location.hash = "#/models";
    render(<App poll={false} />);

    // 1. Select the recommended model in the grid (one Tab stop; arrow keys move the selection).
    const grid = await screen.findByRole("grid", { name: "Models" });
    const row = await within(grid).findByRole("row", { name: new RegExp(QWEN.replace(/[().]/g, "\\$&")) });
    for (let i = 0; i < 80 && !grid.contains(document.activeElement); i++) await user.tab();
    for (let i = 0; i < 5 && row.getAttribute("aria-selected") !== "true"; i++) await user.keyboard("{ArrowDown}");
    expect(row.getAttribute("aria-selected")).toBe("true");

    // 2. Download it. The license is shown before anything is fetched, and the confirm control has focus.
    await keyTo(screen.getByRole("button", { name: "Download…" }), true);
    await user.keyboard("{Enter}");
    const dialog = await screen.findByRole("dialog", { name: /Confirm download of Qwen2.5/ });
    expect(dialog.textContent).toContain("License: Apache-2.0");
    expect(fake.fetchMock.mock.calls.some(([u]) => String(u).includes("/pull"))).toBe(false);
    expect(document.activeElement?.textContent).toMatch(/^Download 1\.0 GB/);
    await user.keyboard("{Enter}");
    expect(fake.fetchMock.mock.calls.some(([u]) => String(u).includes("/pull"))).toBe(true);
    await waitFor(() => expect(within(screen.getByRole("grid", { name: "Models" })).getByRole("row", { name: /Qwen2\.5/ }).textContent).toContain("Downloaded"));

    // 3. Load it; focus then moves to "Open in Chat".
    await keyTo(screen.getByRole("button", { name: "Load" }));
    await user.keyboard("{Enter}");
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole("button", { name: "Open in Chat" })));
    expect(fake.loaded?.model_id).toBe("qwen2.5-1.5b-instruct-q4_k_m");
    await user.keyboard("{Enter}");

    // 4. Chat.
    await screen.findByRole("heading", { level: 1, name: "Chat" });
    const box = await screen.findByRole("textbox", { name: "Message" });
    await waitFor(() => expect(box).not.toHaveProperty("disabled", true));
    await keyTo(box);
    await user.keyboard("Hello there{Enter}");

    // 5. Stop mid-stream, within 300 ms.
    await screen.findByText(/word2 /);
    await keyTo(screen.getByRole("button", { name: "Stop" }));
    const t0 = performance.now();
    await user.keyboard("{Enter}");
    const chatCall = fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))!;
    expect((chatCall[1] as RequestInit).signal?.aborted).toBe(true);
    expect(performance.now() - t0).toBeLessThan(300);
    await screen.findAllByText(/Stopped/);
    // Nothing is left to stop: the button is gone or disabled.
    const stop = screen.queryByRole("button", { name: "Stop" });
    expect(stop === null || stop.getAttribute("aria-disabled") === "true" || (stop as HTMLButtonElement).disabled).toBe(true);
    const saved = chatHistory.get().messages;
    expect(saved[1].stopped).toBe(true);
    expect(saved[1].content.length).toBeGreaterThan(0);
    expect(saved[1].content.length).toBeLessThan(200 * 6);
    // History is kept locally only.
    expect(localStorage.getItem("ucompute.chat")).toContain("Hello there");
  }, 30000);
});
