import { createStore } from "./store";

export type Theme = "system" | "light" | "dark";

/** A table's layout: column visibility overrides (true = shown) and up to two sort keys, primary first. */
export interface TableLayout {
  columns?: Record<string, boolean>;
  sort?: { id: string; desc: boolean }[];
}

/** An inspector's height in px (once the user dragged the splitter), collapsed state and last tab. */
export interface InspectorLayout {
  height?: number;
  collapsed?: boolean;
  tab?: string;
}

export interface UiPrefs {
  theme: Theme;
  /** Keyed by `DataTable` `id`. */
  tables: Record<string, TableLayout>;
  /** Keyed by `Inspector` `id`. */
  inspectors: Record<string, InspectorLayout>;
}

/** Per-viewer conveniences in localStorage `common.ui` (reads and writes in try/catch, in `createStore`). */
export const ui = createStore<UiPrefs>({ theme: "system", tables: {}, inspectors: {} }, "common.ui");

export function setTable(id: string, patch: Partial<TableLayout>): void {
  ui.set((u) => ({ ...u, tables: { ...u.tables, [id]: { ...u.tables[id], ...patch } } }));
}
export function setInspector(id: string, patch: Partial<InspectorLayout>): void {
  ui.set((u) => ({ ...u, inspectors: { ...u.inspectors, [id]: { ...u.inspectors[id], ...patch } } }));
}

/** Applies the theme to the page (`data-theme`) and, in the app, to the native title bar. */
export async function applyTheme(theme: Theme): Promise<void> {
  if (theme === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = theme;
  if (!("__TAURI_INTERNALS__" in window)) return;
  try {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().setTheme(theme === "system" ? null : theme);
  } catch {
    /* an older shell without the set-theme permission keeps the system title bar */
  }
}
