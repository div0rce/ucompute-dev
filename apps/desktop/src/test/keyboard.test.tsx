// F-10-11: the primary journey (Models -> Download -> Run -> Chat -> Stop) is completable with the keyboard only.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { chatHistory, daemon, runs } from "../state/stores";
import { installFakeDaemon } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
  runs.set([]);
});
afterEach(() => vi.unstubAllGlobals());

describe("keyboard-only primary flow", () => {
  it("downloads, runs, chats and stops without a mouse", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ chat: { chunks: Array.from({ length: 200 }, (_, i) => `word${i} `), delayMs: 10 } });

    /** Presses Tab until `el` has focus. */
    const tabTo = async (el: Element) => {
      for (let i = 0; i < 80 && document.activeElement !== el; i++) await user.tab();
      expect(document.activeElement).toBe(el);
    };

    window.location.hash = "#/models";
    render(<App poll={false} />);

    // 1. Download the recommended model. The license is shown before anything is fetched.
    const row = (await screen.findByText("Qwen2.5 1.5B Instruct (Q4_K_M)")).closest("tr")!;
    const downloadBtn = row.querySelector<HTMLButtonElement>("button.btn-primary")!;
    expect(downloadBtn.textContent).toBe("Download");
    await tabTo(downloadBtn);
    await user.keyboard("{Enter}");
    const confirm = await screen.findByRole("group", { name: /Confirm download of Qwen2.5/ });
    expect(confirm.textContent).toContain("License: Apache-2.0");
    expect(fake.fetchMock.mock.calls.some(([u]) => String(u).includes("/pull"))).toBe(false);
    // Focus moved into the confirmation; Enter starts the download.
    expect(document.activeElement?.textContent).toMatch(/^Download 1\.0 GB/);
    await user.keyboard("{Enter}");
    expect(fake.fetchMock.mock.calls.some(([u]) => String(u).includes("/pull"))).toBe(true);

    // 2. Run it. After the download the Run button receives focus.
    const runBtn = await within(row).findByRole("button", { name: "Run" });
    await waitFor(() => expect(document.activeElement).toBe(runBtn));
    await user.keyboard("{Enter}");

    // 3. Chat. Loading navigates to the chat view with the model loaded.
    await screen.findByRole("heading", { level: 1, name: "Chat" });
    expect(fake.loaded?.model_id).toBe("qwen2.5-1.5b-instruct-q4_k_m");
    const box = await screen.findByRole("textbox", { name: "Message" });
    await waitFor(() => expect(box).not.toHaveProperty("disabled", true));
    await tabTo(box);
    await user.keyboard("Hello there{Enter}");

    // 4. Stop mid-stream.
    const stopBtn = await screen.findByRole("button", { name: "Stop" });
    await screen.findByText(/word2 /);
    await tabTo(stopBtn);
    const t0 = performance.now();
    await user.keyboard("{Enter}");
    const chatCall = fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))!;
    expect((chatCall[1] as RequestInit).signal?.aborted).toBe(true);
    expect(performance.now() - t0).toBeLessThan(300);
    await screen.findByText(/Stopped\./);
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    const saved = chatHistory.get().messages;
    expect(saved[1].stopped).toBe(true);
    expect(saved[1].content.length).toBeGreaterThan(0);
    expect(saved[1].content.length).toBeLessThan(200 * 6);
    // History is kept locally only.
    expect(localStorage.getItem("common.chat")).toContain("Hello there");
  }, 30000);
});
