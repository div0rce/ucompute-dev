// What needs the user's attention: derived conditions (service down or busy, CPU fallback, an open pairing window)
// plus dismissable event entries (download failed, a reply from this app failed, lost connection to a device). The
// same events go to the announcer, so sighted and screen-reader users get the same information.
import { useEffect, useState } from "react";
import { refreshStatus } from "../daemon";
import { go } from "../router";
import { attention, mesh, models, network, pairing, pulls, type AttentionEntry, type Pull } from "../state/live";
import { daemon } from "../state/stores";
import { announce } from "./announcer";

export interface AttentionItem extends AttentionEntry {
  /** Event entries can be dismissed; derived conditions clear when their cause does. */
  dismissable: boolean;
}

/** Adds (or replaces, by id) a dismissable entry. D adds "A reply from this app failed: <title>" this way. */
export function pushAttention(e: AttentionEntry): void {
  attention.set((a) => [...a.filter((x) => x.id !== e.id), e]);
}
export function dismissAttention(id: string): void {
  attention.set((a) => a.filter((x) => x.id !== id));
}

/** Re-renders every `ms` while `on`. */
export function useNow(ms: number, on = true): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms, on]);
  return now;
}

const mmss = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

/** Everything that needs attention now, most severe first. */
export function useAttention(): AttentionItem[] {
  const d = daemon.use();
  const p = pairing.use();
  const entries = attention.use();
  const now = useNow(1000, !!p);
  const items: AttentionItem[] = [];
  if (d.reachable === false)
    items.push({ id: "service-down", tone: "danger", text: "Service not running. Retrying automatically.", action: { label: "Retry now", run: () => void refreshStatus() }, dismissable: false });
  else if (d.reachable && d.responding === false)
    items.push({ id: "service-busy", tone: "warn", text: "Service busy (not answering)", action: { label: "Open Jobs", run: () => go("jobs") }, dismissable: false });
  const fallback = d.reachable ? d.status?.loaded?.fallback_reason : null;
  if (fallback) items.push({ id: "cpu-fallback", tone: "warn", text: `Running on CPU: ${fallback}`, dismissable: false });
  if (p && p.expiresAt > now)
    items.push({ id: "pairing", tone: "warn", text: `Pairing open · ${mmss(p.expiresAt - now)}`, action: { label: "Resume pairing", run: () => go("devices", null, { pair: "1" }) }, dismissable: false });
  return [...items, ...entries.map((e) => ({ ...e, dismissable: true }))];
}

const modelName = (id: string) => models.get().data?.find((m) => m.id === id)?.name ?? id;

/** Watches the stores for transitions; returns the stop function. */
export function startWatchers(): () => void {
  let reachable = daemon.get().reachable;
  const offDaemon = daemon.subscribe(() => {
    const now = daemon.get().reachable;
    if (reachable === true && now === false) announce("Service stopped");
    if (reachable === false && now === true) announce("Service started");
    reachable = now;
  });

  // Trusted devices with an open connection, Not responding included (that connection is still open, so a poll that
  // lands in the 1-2 s before the close neither hides the loss nor fakes a reconnect). null until the first mesh data
  // (no announcements for the starting state).
  let connected: Map<string, string> | null = null;
  const offMesh = mesh.subscribe(() => {
    const m = mesh.get().data;
    if (!m) return;
    const next = new Map(m.peers.filter((p) => p.connected).map((p) => [p.id, p.name || p.short_label]));
    if (connected) {
      for (const [id, name] of next)
        if (!connected.has(id)) {
          announce(`Connected to ${name}`);
          dismissAttention(`lost:${id}`);
        }
      for (const [id, name] of connected) {
        const p = m.peers.find((x) => x.id === id);
        // Not asked for: still paired, networking still on, not disconnected here, and the connection actually closed
        // (a "Not responding" blip that recovers is not a loss).
        if (p && !p.connected && !p.held && network.get().data?.enabled !== false) {
          announce(`Lost connection to ${name}`);
          pushAttention({ id: `lost:${id}`, tone: "warn", text: `Lost connection to ${name}`, action: { label: "Show device", run: () => go("devices", id) } });
        }
      }
    }
    connected = next;
  });

  let before: Record<string, Pull> = pulls.get();
  const offPulls = pulls.subscribe(() => {
    const now = pulls.get();
    for (const [id, p] of Object.entries(now)) {
      const was = before[id]?.state;
      if (was === p.state) continue;
      if (p.state === "downloading") dismissAttention(`pull:${id}`);
      if (p.state === "done" && was === "downloading") announce(`Download finished: ${modelName(id)}`);
      if (p.state === "error") {
        announce(`Download failed: ${modelName(id)}`);
        pushAttention({ id: `pull:${id}`, tone: "danger", text: `Download failed: ${modelName(id)}`, action: { label: "Show model", run: () => go("models", id) } });
      }
    }
    before = now;
  });

  return () => {
    offDaemon();
    offMesh();
    offPulls();
  };
}
