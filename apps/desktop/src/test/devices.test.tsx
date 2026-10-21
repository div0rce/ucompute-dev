// Defects seen with the installed apps on a Mac and a Windows laptop (docs/evidence/desktop-two-machine-2026-10-19.md)
// and the Peers view's discovery and connection state. Unit level against the fake daemon: no test here claims two
// devices; the physical two-machine checks are the integrator's.
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { refreshStatus } from "../daemon";
import { fitText, plural } from "../format";
import { chatHistory, daemon, lastError, privacy, runTarget, settings } from "../state/stores";
import { Peers } from "../views/Peers";
import { installFakeDaemon, LOADED, NETWORK, NETWORK_ON, PEER, PRIVACY, STRANGER } from "./mocks";

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
const posts = (fake: ReturnType<typeof installFakeDaemon>, path: string) =>
  fake.fetchMock.mock.calls
    .filter(([u, init]) => String(u).endsWith(path) && (init as RequestInit)?.method === "POST")
    .map(([, init]) => JSON.parse(String((init as RequestInit).body ?? "{}")));
const chatBody = (fake: ReturnType<typeof installFakeDaemon>) => posts(fake, "/v1/chat/completions").at(-1);

describe("E1: no macOS wording on other systems", () => {
  it.each(["home", "models", "chat", "hardware", "settings", "peers", "privacy"])("#/%s never says this Mac", async (route) => {
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: route === "privacy" ? PRIVACY : TRUSTED });
    fake.loaded = LOADED;
    await refreshStatus();
    window.location.hash = `#/${route}`;
    const { container } = render(<App poll={false} />);
    await screen.findByRole("heading", { level: 1 });
    await new Promise((r) => setTimeout(r, 100)); // every fetch of the view answered
    const placeholders = [...container.querySelectorAll("[placeholder]")].map((e) => e.getAttribute("placeholder")).join(" ");
    expect(`${container.textContent} ${placeholders}`).not.toMatch(/this Mac/i);
  });

  it("names this computer neutrally where it is the target or the fit", async () => {
    expect(fitText({ verdict: "fits", headroom_bytes: 1024 ** 3 })).toBe("Fits on this computer (1.0 GB to spare)");
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: TRUSTED });
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const sel = await screen.findByRole("combobox", { name: "Run on" });
    expect(within(sel).getAllByRole("option")[0].textContent).toBe("This computer");
  });

  it("pairs under the daemon's device name unless the user set one", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON });
    render(<Peers pollMs={20} />);
    const name = await screen.findByRole("textbox", { name: "Name other devices see" });
    expect(name).toHaveProperty("value", "");
    expect(name.getAttribute("placeholder")).toBe(NETWORK.device_name);
    expect(screen.getByText(`Leave empty to use this computer's name, ${NETWORK.device_name}.`)).toBeTruthy();

    await user.click(screen.getByRole("button", { name: "Pair this device" }));
    await screen.findByRole("group", { name: "Pairing code" });
    expect(posts(fake, "/api/pair/start")).toEqual([{}]);

    fake.completePairing();
    await screen.findByText("Paired with Studio Mac.");
    await user.type(name, "Kitchen PC");
    await user.click(screen.getByRole("button", { name: "Pair this device" }));
    await waitFor(() => expect(posts(fake, "/api/pair/start")).toEqual([{}, { name: "Kitchen PC" }]));
  });

  it("forgets the old hard-coded default but keeps a name the user chose", async () => {
    for (const [stored, expected] of [
      ["This Mac", ""],
      ["Kitchen PC", "Kitchen PC"],
    ]) {
      vi.resetModules();
      localStorage.setItem("common.settings", JSON.stringify({ device: "gpu", n_ctx: 4096, device_name: stored }));
      const fresh = await import("../state/stores");
      expect(fresh.settings.get()).toEqual({ device: "gpu", n_ctx: 4096, device_name: expected });
    }
  });
});

describe("E2: a paired device runs prompts without a model loaded here", () => {
  it("offers every model this daemon knows, sends it with the peer target, and says where it ran", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: TRUSTED });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    const box = await screen.findByRole("textbox", { name: "Message" });
    expect(box).toHaveProperty("disabled", true);

    await user.selectOptions(await screen.findByRole("combobox", { name: "Run on" }), `peer:${PEER.id}`);
    expect(screen.queryByRole("button", { name: "Load model" })).toBeNull();
    expect(screen.getByText(/Prompts run on Studio Mac, which loads the model you pick/)).toBeTruthy();
    const model = screen.getByRole("combobox", { name: "Model" });
    // Not only what is downloaded here: the other device may have a model this one does not.
    expect(within(model).getAllByRole("option").map((o) => o.getAttribute("value"))).toEqual(["qwen2.5-1.5b-instruct-q4_k_m", "stories15M-q4_0", "llama-3.1-70b-q4_k_m"]);
    await user.selectOptions(model, "llama-3.1-70b-q4_k_m");
    await waitFor(() => expect(box).toHaveProperty("disabled", false));

    await user.type(box, "Hi{Enter}");
    await screen.findByText(/Ran on Studio Mac\./);
    expect(chatBody(fake)).toMatchObject({ model: "llama-3.1-70b-q4_k_m", target: `peer:${PEER.id}` });
    expect(fake.loaded).toBeNull();
  });

  it("tells the user plainly when the device does not have the model", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: TRUSTED, peerModels: { [PEER.id]: ["stories15M-q4_0"] } });
    await refreshStatus();
    window.location.hash = "#/chat";
    render(<App poll={false} />);
    await screen.findByRole("option", { name: "Studio Mac" });
    await user.selectOptions(screen.getByRole("combobox", { name: "Run on" }), `peer:${PEER.id}`);
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
    await screen.findByText(/Ran on Studio Mac\./);
  });

  it("a local run still needs a model loaded here", async () => {
    installFakeDaemon({ network: NETWORK_ON, peers: [PEER], privacy: TRUSTED });
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
    render(<Peers pollMs={20} />);
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
    expect((await screen.findByText(/^1 token,/)).textContent).toMatch(/^1 token, first token in/);
  });
});

describe("Peers: discovery and connection state", () => {
  const OLD = { ...PEER, id: "cd".repeat(32), short_label: "cdcd-cdcd-cdcd", name: "Old laptop", connected: false, health: "disconnected" as const, last_addr: null };
  const AWAY = {
    ...PEER,
    id: "ee".repeat(32),
    short_label: "eeee-eeee-eeee",
    name: "Laptop",
    connected: false,
    health: "disconnected" as const,
    last_addr: "172.20.10.6:8193",
    last_error: { addr: "172.20.10.6:8193", code: "peer_lost", message: "no answer from 172.20.10.6:8193 within 4 s", hint: "the device may be asleep or on another network, or a firewall may block UDP port 8193", at_utc: "2026-10-19T03:40:00Z" },
  };
  const cell = async (name: string, col: number) => within((await screen.findByText(name, { selector: ".model-name" })).closest("tr")!).getAllByRole("cell")[col];

  it("shows where each paired device is: seen nearby, last known, or not known; and why it is not connected", async () => {
    installFakeDaemon({
      network: NETWORK_ON,
      peers: [{ ...PEER, health: "connected" }, AWAY, OLD],
      discovered: [{ ...STRANGER, id: PEER.id, trusted: true, pairing: false, addr: "192.168.1.77:8193", addrs: ["192.168.1.77:8193", "10.0.0.9:8193"] }],
    });
    render(<Peers pollMs={20} />);
    expect((await cell("Studio Mac", 2)).textContent).toBe("192.168.1.77:8193, 10.0.0.9:8193Seen on this network");
    expect((await cell("Laptop", 2)).textContent).toBe("172.20.10.6:8193Last known, not seen on this network");
    expect((await cell("Old laptop", 2)).textContent).toBe("Not known");
    // The reason and what to check, only while not connected.
    const health = (await cell("Laptop", 1)).textContent!;
    expect(health).toMatch(/^UnreachableLast try at .+: no answer from 172\.20\.10\.6:8193 within 4 s\. The device may be asleep or on another network, or a firewall may block UDP port 8193\.$/);
    expect((await cell("Studio Mac", 1)).textContent).toBe("Connected");
    // Paired devices are not offered for pairing among nearby strangers.
    expect(screen.queryByRole("button", { name: /^Pair with/ })).toBeNull();
    // This computer's own address, for the other device to type.
    expect(screen.getByText("192.168.1.10:8193", { selector: "code" })).toBeTruthy();
  });

  it("reconnects by address after the device moved networks; the typed address is only a hint, the key decides", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [AWAY], answersAt: { [AWAY.id]: "10.12.0.44:8193" }, impostors: ["10.12.0.45:8193"] });
    render(<Peers pollMs={1000} />);
    // Connected devices and strangers get no Connect button; this one does.
    await user.click(await screen.findByRole("button", { name: "Connect to Laptop" }));
    const panel = screen.getByRole("form", { name: "Connect to Laptop" });
    const addr = within(panel).getByLabelText("Address (optional)");
    await waitFor(() => expect(document.activeElement).toBe(addr));

    // 1. Retry with what the daemon knows: the old address does not answer.
    await user.keyboard("{Enter}");
    expect((await within(panel).findByRole("alert")).textContent).toContain("The connection to the other device was lost.");
    expect(within(panel).getByRole("alert").textContent).toContain("firewall may block UDP port 8193");

    // 2. Another Common device answers at the typed address: refused, nothing changes.
    await user.type(addr, "10.12.0.45:8193{Enter}");
    // Never "not authorized, pair it": the address proved nothing, and the device itself may be fine.
    await waitFor(() =>
      expect(within(panel).getByRole("alert").textContent).toContain(
        "The device that answered did not prove it is Laptop, or Laptop no longer trusts this computer.Nothing was trusted or saved. Check the address shown on Laptop under Peers > This device.peer_unauthorized: the device at 10.12.0.45:8193 did not prove it is",
      ),
    );
    expect(fake.peers[0]).toMatchObject({ connected: false, last_addr: "172.20.10.6:8193" });

    // 3. The device's new address: connected, and that address becomes the pinned hint.
    await user.clear(addr);
    await user.type(addr, "10.12.0.44:8193{Enter}");
    await screen.findByText("Connected to Laptop.");
    expect(screen.queryByRole("form", { name: "Connect to Laptop" })).toBeNull();
    // The panel and its Connect button are gone; focus lands on the outcome, not the top of the page.
    await waitFor(() => expect(document.activeElement?.textContent).toBe("Connected to Laptop."));
    expect((await cell("Laptop", 1)).textContent).toBe("Connected");
    expect((await cell("Laptop", 2)).textContent).toContain("10.12.0.44:8193");
    expect(posts(fake, `/api/peers/${AWAY.id}/connect`)).toEqual([{}, { addr: "10.12.0.45:8193" }, { addr: "10.12.0.44:8193" }]);
    expect(screen.queryByRole("button", { name: "Connect to Laptop" })).toBeNull();
  });

  it("names each state the daemon can back; a cached nearby record never makes a device connected or ready", async () => {
    privacy.set(TRUSTED);
    const dev = (n: number, extra: Partial<typeof PEER>) => ({ ...PEER, id: String(n).repeat(64), short_label: `${n}${n}${n}${n}-x`, name: `Device ${n}`, last_addr: null, ...extra });
    const near = (n: number) => ({ ...STRANGER, id: String(n).repeat(64), trusted: true, pairing: false, addrs: [STRANGER.addr] });
    installFakeDaemon({
      network: NETWORK_ON,
      peers: [
        dev(1, { connected: true, health: "connected" }),
        dev(2, { connected: true, health: "unhealthy" }),
        dev(3, { connected: false, health: "reconnecting" }),
        dev(4, { connected: false, health: "disconnected" }),
        dev(5, { connected: false }),
        dev(6, { connected: false, health: "disconnected", last_error: AWAY.last_error }),
        // Lost, and the daemon's redial since then failed: health keeps saying reconnecting until one succeeds.
        dev(7, { connected: false, health: "reconnecting", last_error: AWAY.last_error }),
      ],
      // 4 and 6 announce themselves nearby (signed records the daemon may still hold after they left); so does a stranger.
      discovered: [near(4), near(6), STRANGER],
      groups: [
        { name: "home", members: ["1", "4", "6"].map((n) => n.repeat(64)), policy: { allow_remote_inference: true, allow_pooled_inference: false } },
        { name: "work", members: ["2".repeat(64)], policy: { allow_remote_inference: false, allow_pooled_inference: false } },
      ],
    });
    render(<Peers pollMs={1000} />);
    const state = async (n: number) => (await cell(`Device ${n}`, 1)).textContent;
    const prompts = async (n: number) => (await cell(`Device ${n}`, 3)).querySelector(".model-meta")!.textContent;
    expect(await state(1)).toBe("Connected");
    expect(await prompts(1)).toBe("Can run prompts from this computer.");
    expect(await state(2)).toBe("Not responding");
    expect(await prompts(2)).toBe('Cannot run prompts from this computer yet: turn on "Members may run prompts for each other" in its group.');
    expect(await state(3)).toBe("Reconnecting");
    expect(await prompts(3)).toBe("Cannot run prompts from this computer yet: connect it, add it to a group that allows prompts.");
    // Seen nearby and in a group that allows prompts, but no open connection: neither connected nor ready.
    expect(await state(4)).toBe("Not connected");
    expect((await cell("Device 4", 2)).textContent).toContain("Seen on this network");
    expect(await prompts(4)).toBe("Cannot run prompts from this computer yet: connect it.");
    expect(await state(5)).toBe("Offline");
    expect((await cell("Device 5", 2)).textContent).toBe("Not known");
    // Seen nearby, yet the dial failed (a firewall, say): the failure wins, with its reason.
    expect(await state(6)).toMatch(/^UnreachableLast try at .+: no answer from 172\.20\.10\.6:8193 within 4 s\./);
    expect(await prompts(6)).toBe("Cannot run prompts from this computer yet: connect it.");
    expect(await state(7)).toMatch(/^UnreachableLast try at /);
    // Discovered, not paired: listed apart, untrusted, never in the paired table.
    const nearby = (await screen.findByRole("heading", { name: "Nearby, not paired" })).closest(".section") as HTMLElement;
    expect(within(nearby).getByText(STRANGER.id.slice(0, 4), { exact: false }).closest("tr")!.textContent).toContain("Not trusted");
    expect(within(nearby).queryByText("Device 4")).toBeNull();
  });

  it("calls a device offline only where this computer looks for devices, and says when Privacy blocks prompts", async () => {
    installFakeDaemon({
      network: { ...NETWORK_ON, mdns: false },
      peers: [{ ...PEER, connected: false, last_addr: "192.168.1.77:8193" }],
      groups: [{ name: "home", members: [PEER.id], policy: { allow_remote_inference: true, allow_pooled_inference: false } }],
    });
    render(<Peers pollMs={1000} />);
    expect((await cell("Studio Mac", 1)).textContent).toBe("Not connected");
    expect((await cell("Studio Mac", 3)).textContent).toBe("homeCannot run prompts from this computer yet: connect it, allow trusted devices under Privacy.");
  });

  it("a malformed address is the daemon's invalid_request, shown in the panel", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: [AWAY] });
    render(<Peers pollMs={1000} />);
    await user.click(await screen.findByRole("button", { name: "Connect to Laptop" }));
    await user.type(screen.getByLabelText("Address (optional)"), "laptop.local{Enter}");
    expect((await screen.findByRole("alert")).textContent).toContain("invalid_request");
  });

  it("offers Connect only while networking is on", async () => {
    installFakeDaemon({ network: NETWORK, peers: [AWAY] });
    render(<Peers pollMs={20} />);
    await screen.findByText("Laptop", { selector: ".model-name" });
    expect(screen.queryByRole("button", { name: "Connect to Laptop" })).toBeNull();
  });

  it("says why no device shows up: discovery off here, or a network that blocks it", async () => {
    const fake = installFakeDaemon({ network: { ...NETWORK_ON, mdns: false } });
    const { unmount } = render(<Peers pollMs={20} />);
    expect(await screen.findByText(/This computer is not looking for devices on the network\. Pair by address instead/)).toBeTruthy();
    unmount();
    fake.network = NETWORK_ON;
    render(<Peers pollMs={20} />);
    expect(await screen.findByText(/campus and office Wi-Fi networks, stop devices from finding each other; pair by address instead/)).toBeTruthy();
  });

  it("the keyboard reaches Connect, and Cancel closes the panel", async () => {
    const user = userEvent.setup();
    installFakeDaemon({ network: NETWORK_ON, peers: [AWAY] });
    render(<Peers pollMs={1000} />);
    const btn = await screen.findByRole("button", { name: "Connect to Laptop" });
    btn.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Address (optional)")));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("form", { name: "Connect to Laptop" })).toBeNull();
    // Back on the button that opened the panel, not the top of the page.
    await waitFor(() => expect(document.activeElement).toBe(btn));
  });

  it("while one device is connecting, the others' Connect waits, so neither outcome is lost", async () => {
    const user = userEvent.setup();
    const fake = installFakeDaemon({ network: NETWORK_ON, peers: [AWAY, OLD] });
    // Hold the dial, as a silent address does for up to 4 s per try.
    const real = fake.fetchMock.getMockImplementation() as typeof fetch;
    let release = () => {};
    const held = new Promise<void>((r) => (release = r));
    fake.fetchMock.mockImplementation(async (u: RequestInfo | URL, init?: RequestInit) => {
      if (String(u).endsWith("/connect")) await held;
      return real(u, init);
    });
    render(<Peers pollMs={1000} />);
    await user.click(await screen.findByRole("button", { name: "Connect to Laptop" }));
    await user.keyboard("{Enter}");
    await screen.findByRole("button", { name: "Connecting" });
    expect((await cell("Laptop", 1)).textContent).toMatch(/^Connecting/);
    expect((screen.getByRole("button", { name: "Connect to Old laptop" }) as HTMLButtonElement).disabled).toBe(true);
    release();
    // Laptop's failure is shown in its own panel; then the other device can be tried.
    const panel = screen.getByRole("form", { name: "Connect to Laptop" });
    expect((await within(panel).findByRole("alert")).textContent).toContain("The connection to the other device was lost.");
    expect((screen.getByRole("button", { name: "Connect to Old laptop" }) as HTMLButtonElement).disabled).toBe(false);
  });
});
