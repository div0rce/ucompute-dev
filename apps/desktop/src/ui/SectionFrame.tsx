import { createContext, useId, useLayoutEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import type { MappedError } from "../api/errors";
import { mapError } from "../api/errors";
import { UNREACHABLE } from "../api/client";
import { refreshStatus } from "../daemon";
import { clock } from "../format";
import { neededKeys, type Live } from "../state/live";
import { refreshAll } from "../state/poll";
import type { DaemonState } from "../state/stores";
import { popupMenu, type MenuItem } from "./contextMenu";
import { ActionsDisabled, EmptyState, ToolButton } from "./primitives";

/**
 * What a section can show (plan §2 shared states). `busy`: the service answers `/health` but a poll timed out, so rows
 * stay, dimmed. `stale`: a store's refresh failed, last data kept. `failed`: a store's first read failed, so there is
 * no data at all: the body names the failure instead of claiming the list is empty. Only `down` (connection refused
 * and no `/health`) empties tables. `loading`: a store has not answered yet, busy or not (a busy service that was never
 * read has no rows to keep, and its empty state would be a guess).
 */
export type FrameState =
  | "ok"
  | "loading"
  | "connecting"
  | "down"
  | { busy: { since: number | null } }
  | { stale: { since: number | null; error: MappedError } }
  | { failed: MappedError };

/** The frame state from the daemon and the stores a section shows. */
export function frameState(daemon: DaemonState, ...stores: Live<unknown>[]): FrameState {
  if (daemon.reachable === null) return "connecting";
  if (daemon.reachable === false) return "down";
  const ats = stores.map((s) => s.at).filter((a): a is number => a != null);
  const since = ats.length ? Math.min(...ats) : (daemon.at ?? null);
  if (daemon.responding === false) return stores.some((s) => s.data === null) ? "loading" : { busy: { since } };
  const never = stores.find((s) => s.error && s.data === null);
  if (never?.error) return { failed: never.error };
  const failed = stores.find((s) => s.error);
  if (failed?.error) return { stale: { since: failed.at, error: failed.error } };
  if (stores.some((s) => s.data === null)) return "loading";
  return "ok";
}

/**
 * True while rows are shown but not live (busy or stale): DataTable dims rows and blanks `live` columns, and the
 * inspector shows its live values as not refreshed.
 */
export const FrameStale = createContext(false);

export interface FilterProps {
  value: string;
  onChange: (v: string) => void;
  /** Accessible name; default "Filter". */
  label?: string;
  placeholder?: string;
}

/** The filter box: its own Tab stop; Mod+F focuses it (`data-filter`), Esc clears it. */
export function FilterBox({ value, onChange, label = "Filter", placeholder = "Filter" }: FilterProps) {
  return (
    <input
      type="search"
      className="filter-box"
      data-filter=""
      aria-label={label}
      placeholder={placeholder}
      value={value}
      onChange={(e) => onChange(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Escape" && value) {
          e.preventDefault();
          onChange("");
        }
      }}
    />
  );
}

/** `role=toolbar` around the action buttons only: one Tab stop, arrow keys move between buttons (roving tabindex). */
function Toolbar({ label, children }: { label: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const current = useRef(0);
  const buttons = () => [...(ref.current?.querySelectorAll<HTMLButtonElement>("button") ?? [])];
  useLayoutEffect(() => {
    const bs = buttons();
    if (current.current >= bs.length) current.current = 0;
    bs.forEach((b, i) => (b.tabIndex = i === current.current ? 0 : -1));
  });
  const onKeyDown = (e: KeyboardEvent) => {
    const bs = buttons();
    const i = bs.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    const next = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? bs.length - 1 : null;
    if (next == null) return;
    e.preventDefault();
    const j = (next + bs.length) % bs.length;
    bs.forEach((b, k) => (b.tabIndex = k === j ? 0 : -1));
    current.current = j;
    bs[j].focus();
  };
  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label={label}
      className="toolbar-actions"
      onKeyDown={onKeyDown}
      onFocus={(e) => {
        const i = buttons().indexOf(e.target as unknown as HTMLButtonElement);
        if (i >= 0) current.current = i;
      }}
    >
      {children}
    </div>
  );
}

export interface SectionFrameProps {
  title: string;
  /** Switches and selects (not buttons): each its own Tab stop, before the toolbar. */
  controls?: ReactNode;
  /** Section ToolButtons (inside `role=toolbar`). */
  actions?: ReactNode;
  /** ToolButtons acting on the selected row, after a separator; disabled with the reason while the data is stale. */
  selectionActions?: ReactNode;
  /** Lowest-priority actions in a native "More" menu. */
  more?: MenuItem[];
  /** Usually a ScopeBar; left of the scope row. */
  scope?: ReactNode;
  filter?: FilterProps;
  /** Section banners (for example the Firewall notice), under the scope row. */
  banner?: ReactNode;
  /** The table (or the section body). Replaced by an EmptyState while `connecting`, `loading`, `failed` or `down`. */
  children: ReactNode;
  inspector?: ReactNode;
  state: FrameState;
}

/** The section layout: toolbar, scope row, state banner, table, inspector (plan §1.1). Renders the page's only h1. */
export function SectionFrame({ title, controls, actions, selectionActions, more, scope, filter, banner, children, inspector, state }: SectionFrameProps) {
  const h1 = useId();
  const busy = typeof state === "object" && "busy" in state ? state.busy : null;
  const stale = typeof state === "object" && "stale" in state ? state.stale : null;
  const failed = typeof state === "object" && "failed" in state ? state.failed : null;
  const notLive = busy ? "Service busy; not updated since " + clock(busy.since) : stale ? `Not updated since ${clock(stale.since)}` : null;
  const down = mapError(UNREACHABLE, "");

  let body: ReactNode = children;
  if (state === "connecting") body = <EmptyState title="Connecting to the background service…" />;
  else if (state === "loading") body = <EmptyState title="Loading…" />;
  else if (failed)
    body = (
      <EmptyState
        title={`Not read from the service: ${failed.title}`}
        reason={failed.next_step}
        action={
          <button type="button" className="btn" onClick={() => void refreshAll(neededKeys())}>
            Try again
          </button>
        }
      />
    );
  else if (state === "down")
    body = (
      <EmptyState
        title="The background service is not running"
        reason={down.next_step}
        action={
          <button type="button" className="btn" onClick={() => void refreshStatus()}>
            Try again
          </button>
        }
      />
    );

  return (
    <section className="frame" aria-labelledby={h1}>
      <header className="frame-toolbar" data-region="toolbar">
        <h1 id={h1}>{title}</h1>
        {controls && <div className="frame-controls">{controls}</div>}
        {(actions || selectionActions || more) && (
          <Toolbar label={`${title} actions`}>
            {actions}
            {actions && selectionActions && <span role="separator" aria-orientation="vertical" className="tb-sep" />}
            <ActionsDisabled.Provider value={notLive}>{selectionActions}</ActionsDisabled.Provider>
            {more && more.length > 0 && <ToolButton icon="ellipsis" label="More" onClick={() => void popupMenu(more)} />}
          </Toolbar>
        )}
      </header>
      {(scope || filter) && (
        <div className="frame-scope" data-region="scope">
          {scope}
          {filter && <FilterBox {...filter} />}
        </div>
      )}
      {busy && <p className="frame-banner tone-warn">Service running, busy (likely loading a model). Not updated since {clock(busy.since)}.</p>}
      {stale && (
        <div className="frame-banner tone-warn">
          <span>
            Not updated since {clock(stale.since)}: {stale.error.title}
          </span>
          <details>
            <summary>Details</summary>
            <p>{stale.error.next_step}</p>
            <p className="mono small">
              {stale.error.code}: {stale.error.detail}
              {stale.error.hint ? ` (${stale.error.hint})` : ""}
            </p>
          </details>
        </div>
      )}
      {banner}
      <div className="frame-body">
        <FrameStale.Provider value={!!(busy || stale)}>
          <div className="frame-main" data-region="table">
            {body}
          </div>
          {state !== "down" && !failed && inspector}
        </FrameStale.Provider>
      </div>
    </section>
  );
}
