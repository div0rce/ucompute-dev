// Native menus (Tauri `Menu.popup`, covered by `core:menu:default`): row context menus, the column chooser and More ▾.
// Outside the app (browser dev, tests) there is no native menu: `popupMenu` returns false and nothing opens; tests
// mock this module to read the items. ponytail: no in-page fallback menu; add one if the browser dev build needs menus.

export type MenuItem =
  | { label: string; action: () => void; enabled?: boolean; /** a check item when set */ checked?: boolean }
  | { separator: true };

export const inTauri = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
/** macOS uses ⌘ where Windows uses Ctrl (and ⌘⌫ where Windows uses Delete). */
export const isMac = () => /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);

/** Pops `items` up at the pointer (or `at`, window coordinates). Resolves true when a native menu was shown. */
export async function popupMenu(items: MenuItem[], at?: { x: number; y: number }): Promise<boolean> {
  if (!inTauri() || items.length === 0) return false;
  const [{ Menu }, { LogicalPosition }] = await Promise.all([import("@tauri-apps/api/menu"), import("@tauri-apps/api/dpi")]);
  const menu = await Menu.new({
    items: items.map((it, i) =>
      "separator" in it
        ? { item: "Separator" as const }
        : it.checked !== undefined
          ? { id: `i${i}`, text: it.label, checked: it.checked, enabled: it.enabled ?? true, action: () => it.action() }
          : { id: `i${i}`, text: it.label, enabled: it.enabled ?? true, action: () => it.action() },
    ),
  });
  await menu.popup(at ? new LogicalPosition(at.x, at.y) : undefined);
  return true;
}
