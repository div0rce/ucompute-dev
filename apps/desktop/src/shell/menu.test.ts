// The macOS menu bar (plan §1.4) against a stand-in for Tauri's menu module: what is added where, what each item does,
// and that a key the menu owns fires once (the keydown handler leaves it to the menu).
import { act } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installMenu, resetMenu } from "./menu";
import { onShortcut, SHORTCUTS_EVENT } from "./shortcuts";

const fake = vi.hoisted(() => {
  type Item = { text?: string; accelerator?: string; action?: () => void; item?: string; kind?: string; title?: string; items?: Item[] };
  class Sub {
    kind = "Submenu";
    constructor(
      public title: string,
      public items: Item[] = [],
    ) {}
    static async new(o: { text: string; items?: Item[] }) {
      return new Sub(o.text, o.items ?? []);
    }
    async text() {
      return this.title;
    }
    async insert(it: Item | Item[], at: number) {
      this.items.splice(at, 0, ...[it].flat());
    }
    async append(it: Item | Item[]) {
      this.items.push(...[it].flat());
    }
  }
  const state = { menu: null as null | { subs: Sub[]; setAsAppMenu: ReturnType<typeof vi.fn> }, defaults: 0 };
  class Menu {
    subs = ["Common", "Edit", "View", "Window", "Help"].map((t) => new Sub(t, t === "Common" ? [{ text: "About Common" }, { item: "Separator" }, { text: "Quit Common" }] : []));
    setAsAppMenu = vi.fn(async () => null);
    static async default() {
      state.defaults++;
      const m = new Menu();
      state.menu = m;
      return m;
    }
    async items() {
      return this.subs;
    }
    async insert(it: Sub, at: number) {
      this.subs.splice(at, 0, it);
    }
    async append(it: Sub) {
      this.subs.push(it);
    }
  }
  return { Menu, Submenu: Sub, state };
});
vi.mock("@tauri-apps/api/menu", () => ({ Menu: fake.Menu, Submenu: fake.Submenu }));

const original = navigator.platform;
const setPlatform = (p: string) => Object.defineProperty(navigator, "platform", { value: p, configurable: true });
const w = window as unknown as Record<string, unknown>;

beforeEach(() => {
  resetMenu();
  fake.state.menu = null;
  fake.state.defaults = 0;
  w.__TAURI_INTERNALS__ = {};
  setPlatform("MacIntel");
});
afterEach(() => {
  delete w.__TAURI_INTERNALS__;
  setPlatform(original);
  resetMenu();
});

const items = (title: string) => fake.state.menu!.subs.find((s) => s.title === title)!.items;
const press = (init: KeyboardEventInit) => {
  const e = new KeyboardEvent("keydown", { cancelable: true, ...init });
  act(() => onShortcut(e));
  return e;
};

describe("macOS menu", () => {
  it("adds Settings… to the app menu, a Go menu before Window and Keyboard Shortcuts to Help, once", async () => {
    expect(await installMenu()).toBe(true);
    expect(await installMenu()).toBe(true);
    expect(fake.state.defaults).toBe(1);
    const m = fake.state.menu!;
    expect(m.subs.map((s) => s.title)).toEqual(["Common", "Edit", "View", "Go", "Window", "Help"]);
    expect(items("Common").map((i) => i.text ?? i.item)).toEqual(["About Common", "Separator", "Settings…", "Separator", "Quit Common"]);
    expect(items("Common")[2].accelerator).toBe("CmdOrCtrl+,");
    expect(items("Go").map((i) => [i.text ?? i.item, i.accelerator])).toEqual([
      ["Dashboard", "CmdOrCtrl+1"],
      ["Devices", "CmdOrCtrl+2"],
      ["Models", "CmdOrCtrl+3"],
      ["Jobs", "CmdOrCtrl+4"],
      ["Chat", "CmdOrCtrl+5"],
      ["Activity", "CmdOrCtrl+6"],
      ["Settings", "CmdOrCtrl+7"],
      ["Separator", undefined],
      ["Find", "CmdOrCtrl+F"],
      ["Toggle Inspector", "CmdOrCtrl+I"],
      ["Refresh", "CmdOrCtrl+R"],
    ]);
    expect(items("Help").map((i) => [i.text, i.accelerator])).toEqual([["Keyboard Shortcuts", "CmdOrCtrl+/"]]);
    expect(m.setAsAppMenu).toHaveBeenCalledTimes(1);
  });

  it("items do what their keys do", async () => {
    await installMenu();
    window.location.hash = "#/dashboard";
    act(() => items("Go")[1].action!());
    expect(window.location.hash).toBe("#/devices");
    act(() => items("Common")[2].action!());
    expect(window.location.hash).toBe("#/settings");
    const opened = vi.fn();
    window.addEventListener(SHORTCUTS_EVENT, opened);
    act(() => items("Help")[0].action!());
    expect(opened).toHaveBeenCalledTimes(1);
    window.removeEventListener(SHORTCUTS_EVENT, opened);
  });

  it("once installed, a ⌘ key the menu owns is left to it (one action per press); other keys still work here", async () => {
    window.location.hash = "#/dashboard";
    // Before the menu exists the keydown handler navigates.
    expect(press({ key: "2", metaKey: true }).defaultPrevented).toBe(true);
    expect(window.location.hash).toBe("#/devices");
    await installMenu();
    const e = press({ key: "3", metaKey: true });
    expect(e.defaultPrevented).toBe(false);
    expect(window.location.hash).toBe("#/devices");
    expect(press({ key: "F6" }).defaultPrevented).toBe(true);
  });

  it("is not built on Windows (no menu bar there) or outside the app", async () => {
    setPlatform("Win32");
    expect(await installMenu()).toBe(false);
    setPlatform("MacIntel");
    delete w.__TAURI_INTERNALS__;
    expect(await installMenu()).toBe(false);
    expect(fake.state.defaults).toBe(0);
  });
});
