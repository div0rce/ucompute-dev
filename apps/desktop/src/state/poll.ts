// Poll cadence (plan §5.3). One timer ticks and starts each refresh whose interval has passed; refreshes are
// single-flight (live.ts), so a slow service never piles requests up. Nothing but status polls while the service is
// not reachable.
import { refreshStatus } from "../daemon";
import { isNeeded, jobs, mesh, network, pulls, refresh, refreshMany, type Key } from "./live";
import { daemon } from "./stores";

/** Refreshed whenever the service becomes reachable and polled at their base cadence regardless of the view. */
export const ALWAYS: readonly Key[] = ["network", "mesh", "privacy", "groups", "hardware", "models", "jobs", "audit"];

type PollKey = Key | "status";
const KEYS: readonly PollKey[] = ["status", "ready", "network", "mesh", "privacy", "groups", "hardware", "models", "jobs", "metrics", "audit"];

/** Milliseconds between refreshes of `key` now; null = not polled (refreshed on need or after actions only). */
export function intervalFor(key: PollKey): number | null {
  const d = daemon.get();
  switch (key) {
    case "status":
      return 2500;
    case "ready":
      // Until E2 reports `engine`, two consecutive "a model is loading" answers are the only load signal.
      return d.status && !d.status.engine ? 2500 : null;
    case "network":
      return 5000;
    case "mesh":
      // 2 s while connected, so "Not responding" (about 2 s long) is drawn.
      if (mesh.get().data?.peers.some((p) => p.connected)) return 2000;
      return network.get().data?.enabled ? 5000 : 30000;
    case "privacy":
    case "groups":
      return 30000;
    case "hardware":
      return 120000;
    case "models":
      return Object.values(pulls.get()).some((p) => p.state === "downloading") ? 5000 : null;
    case "jobs":
      // A service without /api/jobs (before E3) is asked again only once a minute, in case it was upgraded.
      if (jobs.get().data === "unsupported") return 60000;
      return isNeeded("jobs") ? 2000 : 5000;
    case "metrics":
      return isNeeded("metrics") ? 5000 : null;
    case "audit":
      return isNeeded("audit") ? 5000 : 30000;
  }
}

let statusRun: Promise<void> | null = null;
const status = (timeoutMs?: number) => (statusRun ??= refreshStatus(timeoutMs).finally(() => (statusRun = null)));

/** Status, the always-on stores and whatever mounted views need, at once (Mod+R / F5). */
export async function refreshAll(needed: readonly Key[] = []): Promise<void> {
  await status();
  if (daemon.get().reachable) await refreshMany([...new Set([...ALWAYS, ...needed])]);
}

/** Starts polling; returns the stop function. `timeoutMs` is the per-request timeout (default 5 s, tests inject less). */
export function startPoller({ tickMs = 500, timeoutMs }: { tickMs?: number; timeoutMs?: number } = {}): () => void {
  const last = new Map<PollKey, number>();
  const started = Date.now();
  const tick = () => {
    const now = Date.now();
    const reachable = daemon.get().reachable === true;
    for (const key of KEYS) {
      const every = intervalFor(key);
      if (every == null || (key !== "status" && !reachable)) continue;
      if (now - (last.get(key) ?? started) < every) continue;
      last.set(key, now);
      void (key === "status" ? status(timeoutMs) : refresh(key, timeoutMs));
    }
  };
  const t = setInterval(tick, tickMs);
  return () => clearInterval(t);
}
