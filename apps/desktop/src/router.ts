import { useMemo, useSyncExternalStore } from "react";

/** The seven sections, in rail order (Mod+1 … Mod+7). */
export const SECTIONS = ["dashboard", "devices", "models", "jobs", "chat", "activity", "settings"] as const;
export type Section = (typeof SECTIONS)[number];

/** Routes of the old app, kept as aliases so deep links keep working. */
const ALIASES: Record<string, { section: Section; id?: string; params?: Record<string, string> }> = {
  home: { section: "dashboard" },
  peers: { section: "devices" },
  hardware: { section: "devices", id: "self", params: { tab: "hardware" } },
  performance: { section: "jobs" },
  diagnostics: { section: "activity" },
  privacy: { section: "settings", id: "privacy" },
  cluster: { section: "dashboard" },
  contributions: { section: "dashboard" },
};
export type LegacyRoute = keyof typeof ALIASES;

export interface Location {
  section: Section;
  /** Selected row key (device id or "self", model id, job id, activity key) or Settings page; null when none. */
  id: string | null;
  params: URLSearchParams;
}

/** `#/devices/<id>?tab=hardware` -> Location. Aliases resolve; unknown and empty routes are the Dashboard. */
export function parseHash(hash: string): Location {
  const raw = hash.replace(/^#\/?/, "");
  const q = raw.indexOf("?");
  const path = q < 0 ? raw : raw.slice(0, q);
  const params = new URLSearchParams(q < 0 ? "" : raw.slice(q + 1));
  const [head = "", ...rest] = path.split("/");
  const id = rest.length ? decodeURIComponent(rest.join("/")) || null : null;
  if ((SECTIONS as readonly string[]).includes(head)) return { section: head as Section, id, params };
  const alias = ALIASES[head];
  if (!alias) return { section: "dashboard", id: null, params };
  for (const [k, v] of Object.entries(alias.params ?? {})) if (!params.has(k)) params.set(k, v);
  return { section: alias.section, id: alias.id ?? id, params };
}

/** `#/section[/id][?params]`. Accepts old route names too (they resolve when parsed). */
export function href(section: Section | LegacyRoute, id?: string | null, params?: URLSearchParams | Record<string, string>): string {
  const p = new URLSearchParams(params instanceof URLSearchParams ? params : (params ?? {})).toString();
  return `#/${section}${id ? `/${encodeURIComponent(id)}` : ""}${p ? `?${p}` : ""}`;
}

const LOCATION_EVENT = "ucompute:location";

/** Goes to a section (pushes a history entry). */
export function go(section: Section | LegacyRoute, id?: string | null, params?: URLSearchParams | Record<string, string>): void {
  window.location.hash = href(section, id, params);
}

/** Changes the selection (and params) inside the current section without a history entry. */
export function select(id: string | null, params?: URLSearchParams | Record<string, string>): void {
  const { section } = parseHash(window.location.hash);
  window.history.replaceState(window.history.state, "", href(section, id, params));
  window.dispatchEvent(new Event(LOCATION_EVENT));
}

function subscribe(fn: () => void) {
  window.addEventListener("hashchange", fn);
  window.addEventListener(LOCATION_EVENT, fn);
  return () => {
    window.removeEventListener("hashchange", fn);
    window.removeEventListener(LOCATION_EVENT, fn);
  };
}
const snapshot = () => window.location.hash;

/** The current location. Re-renders on navigation and on `select`. */
export function useLocation(): Location {
  const hash = useSyncExternalStore(subscribe, snapshot, snapshot);
  return useMemo(() => parseHash(hash), [hash]);
}
