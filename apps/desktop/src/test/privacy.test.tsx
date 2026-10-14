// F-10-10: leaving local-only needs the disclosure acknowledged; the peer selector only exists outside local-only.
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { chatHistory, daemon, privacy, runTarget } from "../state/stores";
import { installFakeDaemon, LOADED, NETWORK, PEER, PRIVACY } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
  privacy.set(null);
  runTarget.set("local");
});
afterEach(() => vi.unstubAllGlobals());

const puts = (fake: ReturnType<typeof installFakeDaemon>) =>
  fake.fetchMock.mock.calls.filter(([u, init]) => String(u).endsWith("/api/privacy") && (init as RequestInit)?.method === "PUT");

describe("Privacy view", () => {
  it("switching away from local-only shows the disclosure and requires acknowledgement before PUT", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon();
    await refreshStatus();
    window.location.hash = "#/privacy";
    render(<App poll={false} />);

    await screen.findByText(/Current mode: Local only\./);
    expect(screen.getByRole("link", { name: "Privacy mode: Local only" })).toBeTruthy();
    expect(screen.queryByText(PRIVACY.disclosure)).toBeNull();

    await user.click(screen.getByRole("radio", { name: /Trusted devices/ }));
    const region = await screen.findByRole("region", { name: "Before you switch" });
    expect(within(region).getByText(PRIVACY.disclosure)).toBeTruthy();
    const apply = screen.getByRole("button", { name: "Switch to trusted devices" });
    expect(apply).toHaveProperty("disabled", true);
    await user.click(apply);
    expect(puts(fake)).toHaveLength(0);

    await user.click(within(region).getByRole("checkbox", { name: /I understand/ }));
    expect(apply).toHaveProperty("disabled", false);
    await user.click(apply);
    await waitFor(() => expect(fake.privacy.mode).toBe("trusted_group"));
    expect(JSON.parse(String((puts(fake)[0][1] as RequestInit).body))).toEqual({ mode: "trusted_group", acknowledged: true });
    await screen.findByText("Switched to trusted devices.");
    expect(screen.getByRole("link", { name: "Privacy mode: Trusted devices" })).toBeTruthy();

    // Going back needs no acknowledgement.
    await user.click(screen.getByRole("radio", { name: /Local only/ }));
    expect(screen.queryByRole("region", { name: "Before you switch" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Switch to local only" }));
    await waitFor(() => expect(fake.privacy.mode).toBe("local_only"));
    expect(screen.getByRole("link", { name: "Privacy mode: Local only" })).toBeTruthy();
  });

  it("public compute is present but disabled and honest about it", async () => {
    installFakeDaemon();
    await refreshStatus();
    window.location.hash = "#/privacy";
    render(<App poll={false} />);
    const pub = await screen.findByRole("radio", { name: /Public compute/ });
    expect(pub).toHaveProperty("disabled", true);
    expect(screen.getByText(/Coming later\. Not available in this version/)).toBeTruthy();
  });
});

describe("Run on selector", () => {
  it("is hidden in local-only mode and no target is sent", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK, enabled: true }, peers: [PEER] });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await screen.findByRole("link", { name: "Privacy mode: Local only" });
    expect(screen.queryByRole("combobox", { name: "Run on" })).toBeNull();
    expect(screen.getByText(/Messages stay on this computer/)).toBeTruthy();

    await user.type(await screen.findByRole("textbox", { name: "Message" }), "Hi{Enter}");
    await screen.findByText(/4 tokens/);
    const body = JSON.parse(String((fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))![1] as RequestInit).body));
    expect("target" in body).toBe(false);
    expect(screen.queryByText(/Ran on/)).toBeNull();
  });

  it("appears in trusted-devices mode, sends the peer target and the reply says where it ran", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: { ...NETWORK, enabled: true }, peers: [PEER, { ...PEER, id: "ee".repeat(32), short_label: "eeee-eeee-eeee", name: "Asleep", connected: false, health: "disconnected" }], privacy: { ...PRIVACY, mode: "trusted_group" } });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const sel = await screen.findByRole("combobox", { name: "Run on" });
    await screen.findByRole("option", { name: "Studio Mac" });
    expect(screen.getByRole("option", { name: "Asleep (offline)" })).toHaveProperty("disabled", true);
    await user.selectOptions(sel, `peer:${PEER.id}`);

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Hi{Enter}");
    await screen.findByText(/Ran on Studio Mac\./);
    const body = JSON.parse(String((fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))![1] as RequestInit).body));
    expect(body.target).toBe(`peer:${PEER.id}`);
    expect(chatHistory.get().messages[1].target).toBe(`peer:${PEER.id}`);
  });

  it("is shown on Home only outside local-only", async () => {
    installFakeDaemon({ privacy: { ...PRIVACY, mode: "trusted_group" } });
    await refreshStatus();
    window.location.hash = "#/home";
    render(<App poll={false} />);
    await screen.findByRole("combobox", { name: "Run on" });
  });
});
