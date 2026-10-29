// The macOS menu bar (plan §1.4), built in JS at startup, inside the app only (Windows has no menu bar, which matches
// modern Windows apps; every command is reachable from the rail, toolbars, context menus and keys there). Starts from
// Tauri's default menu (app, Edit, View, Window, Help: Edit carries ⌘C/⌘V for text fields), then adds Settings… to the
// app menu, a Go menu and Keyboard Shortcuts to Help. `core:menu:default` (in `core:default`) covers every call.
// The app menu is named after `productName` ("UCompute"), the same name the window and rail show (D-018, D-028).
import type { Submenu } from "@tauri-apps/api/menu";
import { go, SECTIONS } from "../router";
import { inTauri, isMac } from "../ui/contextMenu";
import { SECTION_META } from "./sections";
import { commands, setMenuKeys } from "./shortcuts";

let installing: Promise<boolean> | null = null;

/** Installs the menu once (StrictMode mounts twice). Resolves false outside the macOS app or when the shell refuses. */
export function installMenu(): Promise<boolean> {
  if (!inTauri() || !isMac()) return Promise.resolve(false);
  return (installing ??= build().catch(() => false));
}

async function build(): Promise<boolean> {
  const { Menu, Submenu } = await import("@tauri-apps/api/menu");
  const menu = await Menu.default();
  const subs = (await menu.items()).filter((i): i is Submenu => i.kind === "Submenu");
  const titles = await Promise.all(subs.map((s) => s.text()));

  // App menu: About, separator, Settings…, separator, Services … Quit (the macOS order).
  await subs[0].insert([{ id: "settings", text: "Settings…", accelerator: "CmdOrCtrl+,", action: () => go("settings") }, { item: "Separator" }], 2);

  const goMenu = await Submenu.new({
    text: "Go",
    items: [
      ...SECTIONS.map((s, i) => ({ id: `go-${s}`, text: SECTION_META[s].label, accelerator: `CmdOrCtrl+${i + 1}`, action: () => go(s) })),
      { item: "Separator" },
      { id: "find", text: "Find", accelerator: "CmdOrCtrl+F", action: () => void commands.focusFilter() },
      { id: "inspector", text: "Toggle Inspector", accelerator: "CmdOrCtrl+I", action: commands.toggleInspector },
      { id: "refresh", text: "Refresh", accelerator: "CmdOrCtrl+R", action: commands.refresh },
    ],
  });
  // After View, before Window (Help stays last).
  const win = titles.indexOf("Window");
  await (win >= 0 ? menu.insert(goMenu, win) : menu.append(goMenu));

  const shortcuts = { id: "shortcuts", text: "Keyboard Shortcuts", accelerator: "CmdOrCtrl+/", action: commands.showShortcuts };
  const help = subs[titles.indexOf("Help")];
  await (help ? help.append(shortcuts) : menu.append(await Submenu.new({ text: "Help", items: [shortcuts] })));

  await menu.setAsAppMenu();
  // From now on the menu owns these ⌘ keys; the keydown handler leaves them to it (no double fire).
  setMenuKeys([..."1234567", ",", "f", "i", "r", "/"]);
  return true;
}

/** Test helper. */
export function resetMenu(): void {
  installing = null;
  setMenuKeys([]);
}
