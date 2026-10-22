import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { select, useLocation } from "../router";
import { setInspector, ui } from "../state/ui";
import { Icon } from "./icons";

export const MIN_HEIGHT = 140;
const STEP = 16;
/** One grid row (base.css `--row-h`). */
const ROW = 28;
/** Window events the shell dispatches: Mod+I toggles the current section's inspector; Enter/double-click opens it. */
export const TOGGLE_EVENT = "ucompute:toggle-inspector";
export const OPEN_EVENT = "ucompute:open-inspector";
/** Opens the inspector and focuses its tab list (a table's default action). */
export const openInspector = () => window.dispatchEvent(new Event(OPEN_EVENT));

export interface InspectorTab {
  id: string;
  label: string;
  render: () => ReactNode;
}

/** The tab an inspector shows: the URL's (`?tab=`), else the last one used, else the first. */
export const activeTab = <T extends { id: string }>(tabs: T[], fromUrl: string | null, last: string | undefined): T | undefined =>
  tabs.find((t) => t.id === fromUrl) ?? tabs.find((t) => t.id === last) ?? tabs[0];

/**
 * The bottom-docked inspector for the selected row. Tabs follow the APG pattern (automatic activation; clicking the
 * active tab does nothing); the selected tab lives in the URL (`?tab=`) and the last one, the height and the collapsed
 * state persist per `id`. With no selection it shows `empty` and renders no tab (so tab-specific data is not needed).
 * A single tab renders as a plain panel.
 */
export function Inspector({ id, label, hasSelection, empty, defaultHeight = "38%", tabs }: { id: string; label: string; hasSelection: boolean; empty: string; defaultHeight?: "fit" | "38%"; tabs: InspectorTab[] }) {
  const prefs = ui.use().inspectors[id] ?? {};
  const loc = useLocation();
  const base = useId();
  const rootRef = useRef<HTMLElement>(null);
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const [measured, setMeasured] = useState<number | null>(null);
  const [room, setRoom] = useState(0);
  const collapsed = !!prefs.collapsed;
  const fromUrl = loc.params.get("tab");
  // ponytail: undefined only with no tabs, which renders no tab (hasSelection is false then).
  const tab = activeTab(tabs, fromUrl, prefs.tab)!;

  // Default heights: 38% of the main area, or (Devices) the table's content plus one row capped at 50%. The table's
  // own height, not `.frame-main`'s: that one stretches to fill the body, so its scrollHeight is never the content.
  // "fit" re-measures on every render (a poll adding a row); the user's dragged height wins over it; setState bails out
  // when unchanged. ponytail: one forced layout per render of the section, fine at a 2 s poll.
  useLayoutEffect(() => {
    if (defaultHeight !== "fit" && measured != null) return;
    const body = rootRef.current?.parentElement;
    const main = body?.querySelector<HTMLElement>(".frame-main");
    if (!body) return;
    const total = body.clientHeight;
    setRoom(total);
    const content = main ? (main.querySelector<HTMLElement>(".grid")?.offsetHeight ?? (main.firstElementChild as HTMLElement | null)?.offsetHeight ?? 0) : 0;
    const want = defaultHeight === "fit" ? total - Math.min(content + ROW, total * 0.5) : total * 0.38;
    setMeasured(Math.max(MIN_HEIGHT, Math.round(want)));
  });

  const height = prefs.height ?? measured ?? MIN_HEIGHT;
  const setHeight = (h: number) => setInspector(id, { height: Math.max(MIN_HEIGHT, Math.round(h)) });
  const toggle = () => setInspector(id, { collapsed: !collapsed });

  useEffect(() => {
    const onToggle = () => setInspector(id, { collapsed: !(ui.get().inspectors[id]?.collapsed ?? false) });
    const onOpen = () => {
      setInspector(id, { collapsed: false });
      setTimeout(() => rootRef.current?.querySelector<HTMLElement>('[role="tab"][aria-selected="true"], .inspector-panel')?.focus(), 0);
    };
    window.addEventListener(TOGGLE_EVENT, onToggle);
    window.addEventListener(OPEN_EVENT, onOpen);
    return () => {
      window.removeEventListener(TOGGLE_EVENT, onToggle);
      window.removeEventListener(OPEN_EVENT, onOpen);
    };
  }, [id]);

  function choose(next: string, focus = false) {
    if (next === tab.id) return;
    setInspector(id, { tab: next });
    const params = new URLSearchParams(loc.params);
    params.set("tab", next);
    select(loc.id, params);
    if (focus) tabRefs.current[tabs.findIndex((t) => t.id === next)]?.focus();
  }

  function onTabKey(e: KeyboardEvent) {
    const i = tabs.findIndex((t) => t.id === tab.id);
    const j = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? tabs.length - 1 : null;
    if (j == null) return;
    e.preventDefault();
    choose(tabs[(j + tabs.length) % tabs.length].id, true);
  }

  // Splitter: drag, or arrow keys (Up grows the inspector).
  const drag = useRef<{ y: number; h: number } | null>(null);
  const onPointerDown = (e: PointerEvent) => {
    drag.current = { y: e.clientY, h: height };
    (e.currentTarget as Element).setPointerCapture?.(e.pointerId);
  };
  const onPointerMove = (e: PointerEvent) => drag.current && setHeight(drag.current.h + (drag.current.y - e.clientY));
  const onPointerUp = () => (drag.current = null);
  const onSplitKey = (e: KeyboardEvent) => {
    const d = e.key === "ArrowUp" ? STEP : e.key === "ArrowDown" ? -STEP : 0;
    if (!d) return;
    e.preventDefault();
    setHeight(height + d);
  };

  const tabbed = hasSelection && tabs.length > 1;
  const panel = (t: InspectorTab) => (
    <div className="inspector-panel" role={tabs.length > 1 ? "tabpanel" : undefined} id={`${base}-panel`} aria-labelledby={tabs.length > 1 ? `${base}-tab-${t.id}` : undefined} tabIndex={0}>
      {t.render()}
    </div>
  );

  return (
    <section ref={rootRef} className={`inspector${collapsed ? " collapsed" : ""}`} aria-label={label} data-region="inspector" style={{ "--inspector-h": `${height}px` } as CSSProperties}>
      {!collapsed && (
        <div
          role="separator"
          aria-orientation="horizontal"
          aria-label={`Resize ${label}`}
          aria-valuenow={height}
          aria-valuemin={MIN_HEIGHT}
          aria-valuemax={Math.max(height, room)}
          aria-valuetext={`${height} pixels`}
          tabIndex={0}
          className="splitter"
          onKeyDown={onSplitKey}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
        />
      )}
      <div className="inspector-bar">
        {/* The inspector's h2: the tab panels' h3 groups sit under it, not straight under the section's h1. */}
        <h2 className={tabbed ? "visually-hidden" : "inspector-title"}>{label}</h2>
        {tabbed && (
          <div role="tablist" aria-label={label} className="tablist" onKeyDown={onTabKey}>
            {tabs.map((t, i) => (
              <button
                key={t.id}
                ref={(el) => {
                  tabRefs.current[i] = el;
                }}
                type="button"
                role="tab"
                id={`${base}-tab-${t.id}`}
                aria-selected={t.id === tab.id}
                aria-controls={`${base}-panel`}
                tabIndex={t.id === tab.id ? 0 : -1}
                onClick={() => choose(t.id)}
              >
                {t.label}
              </button>
            ))}
          </div>
        )}
        <button type="button" className="icon-btn" aria-expanded={!collapsed} aria-label={collapsed ? `Show ${label}` : `Hide ${label}`} onClick={toggle}>
          <Icon name="chevron-down" className={collapsed ? "flip" : undefined} />
        </button>
      </div>
      {!collapsed && (hasSelection ? panel(tab) : <p className="inspector-empty">{empty}</p>)}
    </section>
  );
}
