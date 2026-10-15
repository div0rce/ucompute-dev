import { api, configure } from "./api/client";
import { daemon } from "./state/stores";

export interface DaemonInfo {
  base_url: string;
  spawned: boolean;
}

const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/**
 * Development only, for SINGLE-HOST UI testing: `?daemon=http://127.0.0.1:<port>` (or `http://localhost:<port>`) points
 * this tab at another local daemon, so two tabs on the Vite dev server can drive two daemons. Anything else is
 * ignored. Production builds never honour it: `import.meta.env.DEV` is the constant false there and the rest of
 * this function is compiled out.
 */
export function devDaemonUrl(search: string): string | null {
  if (!import.meta.env.DEV) return null;
  const m = /^http:\/\/(127\.0\.0\.1|localhost):(\d{1,5})\/?$/.exec(new URLSearchParams(search).get("daemon") ?? "");
  const port = Number(m?.[2]);
  return m && port >= 1 && port <= 65535 ? `http://${m[1]}:${port}` : null;
}

/** Asks the Tauri shell where the daemon is; in a plain browser (pnpm dev) assumes the default port. */
export async function discoverDaemon(): Promise<DaemonInfo> {
  const dev = devDaemonUrl(window.location.search);
  if (dev) return { base_url: dev, spawned: false };
  if (inTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<DaemonInfo>("daemon_info");
  }
  return { base_url: "http://127.0.0.1:8192", spawned: false };
}

/** One poll: /api/status (implies health). Updates the daemon store. */
export async function refreshStatus(): Promise<void> {
  try {
    const status = await api.status();
    daemon.set((d) => ({ ...d, reachable: true, version: status.version, status }));
  } catch {
    daemon.set((d) => ({ ...d, reachable: false, status: null }));
  }
}

export async function startDaemonPolling(intervalMs = 2500): Promise<() => void> {
  const info = await discoverDaemon();
  configure({ baseUrl: info.base_url });
  daemon.set((d) => ({ ...d, spawned: info.spawned }));
  await refreshStatus();
  const t = setInterval(refreshStatus, intervalMs);
  return () => clearInterval(t);
}

/** Opens a link in the system browser (Tauri) or a new tab (browser dev mode). */
export async function openExternal(url: string): Promise<void> {
  if (inTauri()) {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url);
  } else {
    window.open(url, "_blank", "noopener,noreferrer");
  }
}
