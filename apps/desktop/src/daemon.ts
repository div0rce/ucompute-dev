import { api, ApiError, configure } from "./api/client";
import { explain } from "./api/errors";
import { startPoller } from "./state/poll";
import { daemon } from "./state/stores";

export interface DaemonInfo {
  base_url: string;
  spawned: boolean;
  /** Times the app restarted its own service after it stopped unexpectedly. Absent from an older app shell. */
  restarts?: number;
  /** It stopped unexpectedly and the app restarts it after a short wait. */
  restarting?: boolean;
  /** It stopped again after three restarts within a minute; the app no longer restarts it. */
  gave_up?: boolean;
  /** Where the app writes the service's output; null when it could not. */
  log_path?: string | null;
}

const fromShell = (i: DaemonInfo) => ({ spawned: i.spawned, restarts: i.restarts ?? 0, restarting: !!i.restarting, gaveUp: !!i.gave_up, logPath: i.log_path ?? null });
/** Inside the app and not a `?daemon=` tab: the shell's own service is the one shown, so its restarts are too. */
let shellOwnsService = false;

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

/** Windows Firewall state for the bundled daemon (src-tauri `firewall.rs`). */
export interface FirewallStatus {
  supported: boolean;
  /** An enabled inbound allow rule for UDP on the Private profile names the bundled daemon. */
  allowed: boolean;
  rule_program: string | null;
  /** Connected networks Windows treats as Public, where that rule does not apply. Absent from an older app shell. */
  public_networks?: string[];
  /** Connected domain networks, where that rule does not apply either; their administrator decides. Absent from an older app shell. */
  domain_networks?: string[];
  /** The serving daemon was not started by this app: its rule is neither checked nor added. Absent from an older app shell. */
  adopted?: boolean;
}

/** Unsupported outside the desktop app and on systems other than Windows. Rejects when the rules could not be read. */
export async function firewallStatus(): Promise<FirewallStatus> {
  if (!inTauri()) return { supported: false, allowed: false, rule_program: null, public_networks: [] };
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<FirewallStatus>("firewall_status");
}

/** Adds the rule through the Windows elevation prompt; rejects with "cancelled" when the user declines it there. */
export async function allowThroughFirewall(): Promise<void> {
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("allow_through_firewall");
}

/**
 * One poll of `/api/status`. A timeout or a network failure asks `GET /health` (1 s): if it answers, the service is
 * running but busy (`responding: false`, last status kept); only when it does not is the service down
 * (`reachable: false`, status cleared). An HTTP error means the service answered, but the kept status is not live:
 * `statusError` marks it (the status bar says "not updated"). Inside the app it first reads the shell's restarts of
 * its own service (`daemon_info`), so a service seen running again is already known as restarted.
 */
export async function refreshStatus(timeoutMs = 5000): Promise<void> {
  if (shellOwnsService && inTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke<DaemonInfo>("daemon_info").then(
      (i) => daemon.set((d) => ({ ...d, ...fromShell(i) })),
      () => undefined,
    );
  }
  try {
    const status = await api.status({ timeoutMs });
    daemon.set((d) => ({ ...d, reachable: true, responding: true, version: status.version, status, at: Date.now(), statusError: null }));
  } catch (err) {
    if (err instanceof ApiError && err.status > 0) {
      daemon.set((d) => ({ ...d, reachable: true, responding: true, statusError: explain(err) }));
      return;
    }
    const alive = await api.health({ timeoutMs: 1000 }).then(
      () => true,
      () => false,
    );
    daemon.set((d) => (alive ? { ...d, reachable: true, responding: false } : { ...d, reachable: false, responding: true, status: null, statusError: null }));
  }
}

/** Finds the daemon, takes one status reading, then starts the poller (status every 2.5 s plus the live stores). */
export async function startDaemonPolling(): Promise<() => void> {
  const info = await discoverDaemon();
  shellOwnsService = inTauri() && !devDaemonUrl(window.location.search);
  configure({ baseUrl: info.base_url });
  daemon.set((d) => ({ ...d, ...fromShell(info) }));
  await refreshStatus();
  return startPoller();
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
