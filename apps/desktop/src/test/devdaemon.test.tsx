// Dev-only `?daemon=` selection for SINGLE-HOST UI testing: two tabs on the Vite dev server, two daemons.
import { render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { configure, getBaseUrl } from "../api/client";
import { devDaemonUrl } from "../daemon";
import { daemon, privacy } from "../state/stores";
import { installFakeDaemon } from "./mocks";

const DEFAULT = "http://127.0.0.1:8192";

beforeEach(() => {
  daemon.set({ reachable: null, version: null, status: null, spawned: false });
  privacy.set({ data: null, at: null, error: null });
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  window.history.replaceState(null, "", "/");
  configure({ baseUrl: DEFAULT });
});

describe("devDaemonUrl", () => {
  it("accepts loopback http with a numeric port in development", () => {
    expect(import.meta.env.DEV).toBe(true);
    expect(devDaemonUrl("?daemon=http://127.0.0.1:18301")).toBe("http://127.0.0.1:18301");
    expect(devDaemonUrl("?daemon=http://localhost:18302/")).toBe("http://localhost:18302");
    expect(devDaemonUrl("?x=1&daemon=http%3A%2F%2F127.0.0.1%3A8192")).toBe(DEFAULT);
  });

  it("ignores anything else", () => {
    for (const v of [
      "",
      "?daemon=",
      "?daemon=http://127.0.0.1",
      "?daemon=http://127.0.0.1:",
      "?daemon=http://127.0.0.1:abc",
      "?daemon=http://127.0.0.1:0",
      "?daemon=http://127.0.0.1:70000",
      "?daemon=https://127.0.0.1:18301",
      "?daemon=http://127.0.0.2:18301",
      "?daemon=http://192.168.1.10:18301",
      "?daemon=http://example.com:80",
      "?daemon=http://localhost.example.com:80",
      "?daemon=http://user@127.0.0.1:18301",
      "?daemon=http://127.0.0.1:18301/api",
      "?daemon=http://127.0.0.1:18301?x=1",
      "?daemon=javascript:alert(1)",
      "?daemon=%20http://127.0.0.1:18301",
    ]) {
      expect(devDaemonUrl(v), v).toBeNull();
    }
  });

  it("is ignored in a production build", () => {
    vi.stubEnv("DEV", false);
    expect(devDaemonUrl("?daemon=http://127.0.0.1:18301")).toBeNull();
  });
});

describe("app with ?daemon=", () => {
  it("talks to the chosen daemon and shows its URL in Settings › General (development)", async () => {
    const fake = installFakeDaemon();
    window.history.replaceState(null, "", "/?daemon=http://127.0.0.1:18301#/settings/general");
    render(<App />);
    // The status bar says "Running" too; this is the About block's own service line (plan §2.7).
    await within(screen.getByRole("main")).findByText("Running");
    expect(getBaseUrl()).toBe("http://127.0.0.1:18301");
    const urls = fake.fetchMock.mock.calls.map(([u]) => String(u));
    expect(urls.length).toBeGreaterThan(0);
    expect(urls.every((u) => u.startsWith("http://127.0.0.1:18301/"))).toBe(true);
    expect(screen.getByText("http://127.0.0.1:18301", { selector: "code" })).toBeTruthy();
    expect(screen.getByText(/chosen with \?daemon=, development only/)).toBeTruthy();
  });

  it("keeps the default daemon in a production build", async () => {
    vi.stubEnv("DEV", false);
    const fake = installFakeDaemon();
    window.history.replaceState(null, "", "/?daemon=http://127.0.0.1:18301#/settings/general");
    render(<App />);
    await within(screen.getByRole("main")).findByText("Running");
    await waitFor(() => expect(getBaseUrl()).toBe(DEFAULT));
    expect(fake.fetchMock.mock.calls.every(([u]) => String(u).startsWith(`${DEFAULT}/`))).toBe(true);
    expect(screen.getByText(DEFAULT, { selector: "code" })).toBeTruthy();
    expect(screen.queryByText(/development only/)).toBeNull();
  });
});
