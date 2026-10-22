// Global keyboard map (plan §1.3). Mod is ⌘ on macOS and Ctrl on Windows; the other modifier never triggers.
import { go, SECTIONS } from "../router";
import { neededKeys } from "../state/live";
import { refreshAll } from "../state/poll";
import { isMac } from "../ui/contextMenu";
import { TOGGLE_EVENT } from "../ui/Inspector";

export const SHORTCUTS_EVENT = "ucompute:shortcuts";

/** The platform's modifier is down and the other one is not. */
export const mod = (e: KeyboardEvent) => (isMac() ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey);
/** "⌘" or "Ctrl+" for labels. */
export const modLabel = () => (isMac() ? "⌘" : "Ctrl+");

/** What the keys and the macOS menu items do (one implementation for both). */
export const commands = {
  /** Focuses the section's filter box; false when the section has none. */
  focusFilter(): boolean {
    const box = document.querySelector<HTMLInputElement>("[data-filter]");
    if (!box) return false;
    box.focus();
    box.select();
    return true;
  },
  toggleInspector: () => void window.dispatchEvent(new Event(TOGGLE_EVENT)),
  refresh: () => void refreshAll(neededKeys()),
  showShortcuts: () => void window.dispatchEvent(new Event(SHORTCUTS_EVENT)),
};

/**
 * ⌘ combinations the macOS menu bar owns once menu.ts installed it (`e.key`, lower case). The menu item fires for
 * them, so this handler leaves them alone (no preventDefault either): one key press is one action, whichever of the
 * webview and the menu sees the key first.
 */
let menuKeys: ReadonlySet<string> = new Set();
export function setMenuKeys(keys: Iterable<string>): void {
  menuKeys = new Set(keys);
}

/** F6 / Shift+F6: rail → toolbar → scope row → table → inspector → status bar (`[data-region]`, DOM order). */
function cycleRegions(back: boolean) {
  const regions = [...document.querySelectorAll<HTMLElement>("[data-region]")].filter((r) => r.getClientRects().length > 0 || !r.hidden);
  if (!regions.length) return;
  const i = regions.findIndex((r) => r.contains(document.activeElement));
  const next = regions[(i + (back ? -1 : 1) + regions.length) % regions.length];
  const target =
    next.querySelector<HTMLElement>('[tabindex="0"], [aria-current="page"], a[href], button:not([tabindex="-1"]), input, select, textarea') ?? next;
  if (target === next && !next.hasAttribute("tabindex")) next.tabIndex = -1;
  target.focus();
}

export function onShortcut(e: KeyboardEvent): void {
  if (e.defaultPrevented || e.isComposing) return;
  if (e.key === "F6") {
    e.preventDefault();
    return cycleRegions(e.shiftKey);
  }
  if (e.metaKey && !e.ctrlKey && !e.altKey && menuKeys.has(e.key.toLowerCase())) return;
  const refresh = e.key === "F5" || (mod(e) && e.key.toLowerCase() === "r");
  if (refresh) {
    // A reload would drop app-tracked downloads, the pairing session and streams: production builds refresh data only.
    if (!import.meta.env.DEV) e.preventDefault();
    return commands.refresh();
  }
  if (!mod(e) || e.altKey) return;
  const n = Number(e.key);
  if (Number.isInteger(n) && n >= 1 && n <= SECTIONS.length) {
    e.preventDefault();
    return go(SECTIONS[n - 1]);
  }
  switch (e.key) {
    case ",":
      e.preventDefault();
      return go("settings");
    case "f":
    case "F":
      if (commands.focusFilter()) e.preventDefault();
      return;
    case "i":
    case "I":
      e.preventDefault();
      return commands.toggleInspector();
    case "/":
      e.preventDefault();
      return commands.showShortcuts();
  }
}

/**
 * The webview's own context menu (Reload, Inspect…) is suppressed on tables and chrome; it stays on text fields and
 * on selected text, where Copy and Paste are what the user wants. Tables open their native row menu themselves.
 */
export function onContextMenu(e: MouseEvent): void {
  if ((e.target as Element | null)?.closest?.('input, textarea, [contenteditable="true"], [contenteditable=""]')) return;
  const sel = document.getSelection();
  // Range.collapsed, not Selection.isCollapsed: the same in browsers, and jsdom only implements the former.
  if (sel && sel.rangeCount > 0 && !sel.getRangeAt(0).collapsed) return;
  e.preventDefault();
}

/** Installs the key and context-menu handlers on `window`; returns the uninstall function. */
export function installShortcuts(): () => void {
  window.addEventListener("keydown", onShortcut);
  window.addEventListener("contextmenu", onContextMenu);
  return () => {
    window.removeEventListener("keydown", onShortcut);
    window.removeEventListener("contextmenu", onContextMenu);
  };
}
