import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach } from "vitest";
import { resetAnnouncer } from "../shell/announcer";
import { resetLive } from "../state/live";
import { ui } from "../state/ui";

// jsdom's HTMLDialogElement has only `open`. Emulated here: showModal/show set it (and focus [autofocus] or the first
// focusable child), close removes it and fires `close`, Esc inside an open dialog fires `cancel` then closes. The focus
// trap, inert background and focus return are NOT emulated; the real-browser pass checks those (plan §5.5 step 3).
const proto = window.HTMLDialogElement?.prototype as (HTMLDialogElement & { __polyfilled?: boolean }) | undefined;
if (proto && typeof proto.showModal !== "function") {
  const FOCUSABLE = "[autofocus], button, [href], input, select, textarea, [tabindex]:not([tabindex='-1'])";
  const open = function (this: HTMLDialogElement) {
    this.setAttribute("open", "");
    (this.querySelector<HTMLElement>("[autofocus]") ?? this.querySelector<HTMLElement>(FOCUSABLE))?.focus();
  };
  proto.showModal = open;
  proto.show = open;
  proto.close = function (this: HTMLDialogElement, rv?: string) {
    if (!this.hasAttribute("open")) return;
    this.removeAttribute("open");
    if (rv !== undefined) this.returnValue = rv;
    this.dispatchEvent(new Event("close"));
  };
  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    const d = (e.target as Element | null)?.closest?.("dialog[open]");
    if (d instanceof HTMLDialogElement && d.dispatchEvent(new Event("cancel", { cancelable: true }))) d.close();
  });
}

beforeEach(() => {
  localStorage.clear();
  window.location.hash = "";
  resetLive();
  resetAnnouncer();
  ui.set({ theme: "system", tables: {}, inspectors: {} });
});
afterEach(() => {
  cleanup();
});
