// F-10-10: leaving local-only needs the disclosure acknowledged; the destination picker only exists outside local-only.
// Owner D. Privacy is Settings › Privacy (`#/privacy` is its alias).
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { chatHistory, daemon, privacy, runTarget } from "../state/stores";
import { installFakeDaemon, LOADED, NETWORK_ON, PEER, PRIVACY } from "./mocks";

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  chatHistory.set({ messages: [] });
  privacy.set(null);
  runTarget.set("local");
});
afterEach(() => vi.unstubAllGlobals());

const puts = (fake: ReturnType<typeof installFakeDaemon>) =>
  fake.fetchMock.mock.calls.filter(([u, init]) => String(u).endsWith("/api/privacy") && (init as RequestInit)?.method === "PUT");

describe("Settings › Privacy", () => {
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

  // Deliberate change (plan §5.4): the disabled "Public compute" radio is gone; no backend exists (D-019), so there is no
  // control at all. Was: "public compute is present but disabled and honest about it".
  it("offers no public compute control", async () => {
    installFakeDaemon();
    await refreshStatus();
    window.location.hash = "#/privacy";
    render(<App poll={false} />);
    const group = await screen.findByRole("radiogroup", { name: "Privacy mode" });
    expect(within(group).getAllByRole("radio").map((r) => (r as HTMLInputElement).value)).toEqual(["local_only", "trusted_group"]);
    expect(screen.queryByRole("radio", { name: /Public/ })).toBeNull();
    expect(screen.getByRole("main").textContent).not.toMatch(/public compute|swarm|credit|contribut/i);
  });
});

// RunOn became the Chat Destination picker (plan §2.5): the select is named "Destination"; a paired device is enabled
// only while connected (`live`) and permitted from here, labelled with its Can run there word.
const HOME = { name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } };

describe("Destination picker", () => {
  it("is hidden in local-only mode and no target is sent", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], groups: [HOME] });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await screen.findByRole("link", { name: "Privacy mode: Local only" });
    expect(screen.queryByRole("combobox", { name: "Destination" })).toBeNull();
    expect(screen.getByText("Local only: replies run on this computer")).toBeTruthy();
    expect(screen.getByText(/Messages stay on this computer/)).toBeTruthy();

    await user.type(await screen.findByRole("textbox", { name: "Message" }), "Hi{Enter}");
    await screen.findByText(/4 tokens/);
    const body = JSON.parse(String((fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))![1] as RequestInit).body));
    expect("target" in body).toBe(false);
    expect(screen.queryByText(/Ran on/)).toBeNull();
  });

  // Deliberate change (plan §2.5): a paired device that is not connected is listed disabled with its State word.
  // Was: "No open connection is not a reason to refuse" (the Asleep option enabled).
  it("appears in trusted-devices mode, sends the peer target and the reply says where it ran", async () => {
    const user = userEvent.setup();
    const asleep = { ...PEER, id: "ee".repeat(32), short_label: "eeee-eeee-eeee", name: "Asleep", connected: false };
    const fake = installFakeDaemon({
      network: NETWORK_ON,
      peers: [PEER, asleep],
      groups: [{ ...HOME, members: [PEER.id, asleep.id] }],
      privacy: { ...PRIVACY, mode: "trusted_group" },
    });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const sel = await screen.findByRole("combobox", { name: "Destination" });
    await screen.findByRole("option", { name: "Studio Mac · Can send" });
    expect(screen.getByRole("option", { name: /^Asleep: / })).toHaveProperty("disabled", true);
    await user.selectOptions(sel, `peer:${PEER.id}`);

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Hi{Enter}");
    await screen.findByText(/Ran on Studio Mac/);
    const body = JSON.parse(String((fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))![1] as RequestInit).body));
    expect(body.target).toBe(`peer:${PEER.id}`);
    expect(chatHistory.get().messages[1].target).toBe(`peer:${PEER.id}`);
  });

  // Deliberate change (plan §2.5): was "can be chosen without an open connection". A just-paired device shows up when
  // the list is opened, disabled until it is connected and in a group that allows prompts; then it can be chosen.
  it("a device paired after the chat opened shows up on focus and can be chosen once connected and permitted", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, privacy: { ...PRIVACY, mode: "trusted_group" } });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const sel = await screen.findByRole("combobox", { name: "Destination" });
    expect(within(sel).getAllByRole("option").map((o) => o.textContent)).toEqual(["This computer"]);

    // Paired from Devices (or the other device) while the chat was open: the daemon reports connected: false.
    fake.completePairing();
    expect(fake.peers[0].connected).toBe(false);
    fireEvent.focus(sel);
    expect(await screen.findByRole("option", { name: /^Studio Mac: / })).toHaveProperty("disabled", true);

    // The daemon's redial connects it and the user allows prompts in a group.
    fake.peers[0].connected = true;
    fake.groups = [HOME];
    fireEvent.focus(sel);
    const opt = await screen.findByRole("option", { name: "Studio Mac · Can send" });
    expect(opt).toHaveProperty("disabled", false);
    await user.selectOptions(sel, `peer:${PEER.id}`);

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Hi{Enter}");
    await screen.findByText(/Ran on Studio Mac/);
    const body = JSON.parse(String((fake.fetchMock.mock.calls.find(([u]) => String(u).endsWith("/v1/chat/completions"))![1] as RequestInit).body));
    expect(body.target).toBe(`peer:${PEER.id}`);
  });
});
