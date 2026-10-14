import { useSyncExternalStore } from "react";

/** Tiny observable value with an optional localStorage mirror. */
export function createStore<T>(initial: T, storageKey?: string) {
  let state = initial;
  if (storageKey) {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw) state = { ...initial, ...(JSON.parse(raw) as T) };
    } catch {
      /* storage unavailable or corrupt: keep defaults */
    }
  }
  const subs = new Set<() => void>();
  const get = () => state;
  const set = (next: T | ((prev: T) => T)) => {
    state = typeof next === "function" ? (next as (p: T) => T)(state) : next;
    if (storageKey) {
      try {
        localStorage.setItem(storageKey, JSON.stringify(state));
      } catch {
        /* ignore */
      }
    }
    subs.forEach((fn) => fn());
  };
  const subscribe = (fn: () => void) => {
    subs.add(fn);
    return () => subs.delete(fn);
  };
  const use = () => useSyncExternalStore(subscribe, get, get);
  return { get, set, subscribe, use };
}
