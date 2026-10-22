import { useContext, useId, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from "react";
import { announce } from "../shell/announcer";
import { setTable, ui } from "../state/ui";
import { isMac, popupMenu, type MenuItem } from "./contextMenu";
import { Icon } from "./icons";
import { FrameStale } from "./SectionFrame";

export interface Column<R> {
  id: string;
  header: string;
  cell: (r: R) => ReactNode;
  /** Sort key; null = unknown, always last whatever the direction. Strings sort naturally ("Device 2" < "Device 10"). */
  sort?: (r: R) => string | number | null;
  /** Plain text the filter box matches; default: the cell when it is a string or number, else a string sort key. */
  text?: (r: R) => string;
  align?: "start" | "end";
  /** Width budget in px. */
  width?: number;
  /** Hidden by default (the column chooser in the header context menu shows it). */
  hidden?: boolean;
  /** VISIBLE muted suffix; any caveat that changes the meaning goes here, never only in `detail`. */
  note?: (r: R) => string | null;
  /** Extra text in the Tip of the focused or hovered cell (aria-describedby). */
  detail?: (r: R) => string | null;
  /** Shows "Unknown (not refreshed)" while the frame is stale. */
  live?: boolean;
}

export interface DataTableProps<R> {
  /** Persistence key for column visibility and sort (`ui.tables[id]`). */
  id: string;
  /** The grid's accessible name (contract: "Devices", "Models", "Jobs", "Activity", "Computers", "Running now", "Recent activity"). */
  label: string;
  columns: Column<R>[];
  rows: R[];
  rowKey: (r: R) => string;
  selected: string | null;
  onSelect: (key: string | null) => void;
  /** Enter or double-click. Never destructive. */
  onActivate?: (r: R) => void;
  /** Text the copy shortcut puts on the clipboard while a cell of this row has focus and no text is selected. */
  onCopy?: (r: R) => string;
  /** Native context menu items (Tauri only; nothing opens in a browser or tests). */
  menu?: (r: R) => MenuItem[];
  /** Delete (Windows) / ⌘⌫ (macOS). The handler must confirm first. */
  onDelete?: (r: R) => void;
  /** Rows kept first whatever the sort (this computer). */
  pinned?: (r: R) => boolean;
  /** Dims rows and blanks `live` columns; defaults to the SectionFrame's busy/stale state. */
  stale?: boolean;
  /** Show at most this many rows (Dashboard blocks). */
  limit?: number;
  /** The filter box text: rows whose text contains it (case-insensitive) stay. */
  filter?: string;
  /** Text the filter matches for a row; default: every visible column's text and notes. */
  filterText?: (r: R) => string;
  /** The sort before the user picks one; null keeps the rows' own order (default: the first sortable column). */
  defaultSort?: { id: string; desc?: boolean } | null;
  /** Shown instead of the grid when no row is left. */
  empty: ReactNode;
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
type Key = string | number | null;
function compare(a: Key, b: Key, desc: boolean): number {
  if (a == null || b == null) return a == null ? (b == null ? 0 : 1) : -1;
  const c = typeof a === "number" && typeof b === "number" ? a - b : collator.compare(String(a), String(b));
  return desc ? -c : c;
}

function cellText<R>(c: Column<R>, r: R): string {
  if (c.text) return c.text(r);
  const v = c.cell(r);
  const s = typeof v === "string" || typeof v === "number" ? String(v) : typeof c.sort?.(r) === "string" ? (c.sort(r) as string) : "";
  return `${s} ${c.note?.(r) ?? ""}`;
}

const NOT_REFRESHED = "Unknown (not refreshed)";
const PAGE = 10;

/**
 * A sortable grid with single selection (APG grid, cell focus): one Tab stop; ↑↓ Home End PgUp PgDn move the selection
 * keeping the column; ←→ move between cells of the selected row; ↑ from the first row reaches the header, where
 * Enter or Space sorts. A new sort key demotes the old one to secondary. Rows keep their DOM node by key, so focus
 * stays on the same row across polls; when that row disappears focus moves to the table.
 */
export function DataTable<R>(p: DataTableProps<R>) {
  const frameStale = useContext(FrameStale);
  const stale = p.stale ?? frameStale;
  const layout = ui.use().tables[p.id] ?? {};
  const tipBase = useId();
  const tableRef = useRef<HTMLTableElement>(null);
  const wantFocus = useRef(false);
  const hadFocus = useRef(false);
  const [cursor, setCursor] = useState<{ row: string | "header" | null; col: number }>({ row: null, col: 0 });
  const [tip, setTip] = useState<string | null>(null);

  const visible = p.columns.filter((c) => layout.columns?.[c.id] ?? !c.hidden);
  const firstSortable = p.columns.find((c) => c.sort);
  const sortKeys = layout.sort?.length ? layout.sort : p.defaultSort ? [{ id: p.defaultSort.id, desc: !!p.defaultSort.desc }] : p.defaultSort !== null && firstSortable ? [{ id: firstSortable.id, desc: false }] : [];
  // The secondary key defaults to the first sortable column (Name).
  const keys = sortKeys.length === 1 && firstSortable && sortKeys[0].id !== firstSortable.id ? [...sortKeys, { id: firstSortable.id, desc: false }] : sortKeys;

  // ponytail: sorted on every render; fine to ~1000 rows (the audit cap), memoize if a table grows past that.
  const shown = (() => {
    const q = p.filter?.trim().toLowerCase();
    const filtered = q ? p.rows.filter((r) => (p.filterText ? p.filterText(r) : visible.map((c) => cellText(c, r)).join(" ")).toLowerCase().includes(q)) : p.rows;
    const cols = keys.map((k) => ({ col: p.columns.find((c) => c.id === k.id), desc: k.desc })).filter((k) => k.col?.sort);
    const sorted = [...filtered].sort((a, b) => {
      const pa = p.pinned?.(a) ? 0 : 1;
      const pb = p.pinned?.(b) ? 0 : 1;
      if (pa !== pb) return pa - pb;
      for (const { col, desc } of cols) {
        const c = compare(col!.sort!(a), col!.sort!(b), desc);
        if (c) return c;
      }
      return 0;
    });
    return p.limit != null ? sorted.slice(0, p.limit) : sorted;
  })();

  const rowKeys = shown.map(p.rowKey);
  const activeRow = cursor.row === "header" ? "header" : cursor.row && rowKeys.includes(cursor.row) ? cursor.row : p.selected && rowKeys.includes(p.selected) ? p.selected : (rowKeys[0] ?? null);
  const col = Math.min(cursor.col, Math.max(0, visible.length - 1));

  useLayoutEffect(() => {
    const t = tableRef.current;
    if (!t) return;
    const lost = hadFocus.current && (document.activeElement === document.body || !t.contains(document.activeElement));
    if (!wantFocus.current && !lost) return;
    wantFocus.current = false;
    // The row that had focus is gone: focus the table itself (arrow keys continue from the selection or first row).
    const gone = lost && cursor.row !== "header" && cursor.row != null && !rowKeys.includes(cursor.row);
    const target = gone ? null : t.querySelector<HTMLElement>(`[data-active="true"]`);
    (target ?? t).focus();
  });

  function sortBy(id: string) {
    const c = p.columns.find((x) => x.id === id);
    if (!c?.sort) return;
    const [primary] = keys;
    const next = primary?.id === id ? [{ id, desc: !primary.desc }, ...keys.slice(1, 2)] : [{ id, desc: false }, ...(primary ? [primary] : [])];
    setTable(p.id, { sort: next });
    announce(`Sorted by ${c.header}, ${next[0].desc ? "descending" : "ascending"}`);
  }

  function move(row: string | "header", nextCol = col) {
    wantFocus.current = true;
    setCursor({ row, col: nextCol });
    if (row !== "header" && row !== p.selected) p.onSelect(row);
  }

  async function openMenu(r: R, at?: { x: number; y: number }) {
    if (p.menu) await popupMenu(p.menu(r), at);
  }

  /** The column chooser: header context menu, or Shift+F10 / the Menu key with a header focused (WCAG 2.1.1). */
  function chooseColumns(at?: { x: number; y: number }) {
    const items: MenuItem[] = p.columns.map((c) => {
      const on = visible.includes(c);
      return {
        label: c.header,
        checked: on,
        // The last visible column cannot be hidden.
        enabled: !(on && visible.length === 1),
        action: () => setTable(p.id, { columns: { ...layout.columns, [c.id]: !on } }),
      };
    });
    void popupMenu(items, at);
  }
  /** Under the focused element, for menus opened from the keyboard. */
  const underFocus = () => {
    const b = document.activeElement?.getBoundingClientRect();
    return b ? { x: b.left, y: b.bottom } : undefined;
  };

  function onKeyDown(e: KeyboardEvent) {
    const i = activeRow === "header" ? -1 : activeRow ? rowKeys.indexOf(activeRow) : -1;
    const last = rowKeys.length - 1;
    const to = (n: number) => (n < 0 ? "header" : rowKeys[Math.min(Math.max(n, 0), last)]);
    const r = activeRow && activeRow !== "header" ? shown[i] : undefined;
    switch (e.key) {
      case "ArrowDown":
        return e.preventDefault(), move(to(i + 1));
      case "ArrowUp":
        return e.preventDefault(), move(to(i - 1));
      case "PageDown":
        return e.preventDefault(), move(to(i + PAGE));
      case "PageUp":
        return e.preventDefault(), move(to(Math.max(0, i - PAGE)));
      case "Home":
        return e.preventDefault(), move(to(0));
      case "End":
        return e.preventDefault(), move(to(last));
      case "ArrowRight":
        return e.preventDefault(), move(activeRow ?? to(0), Math.min(col + 1, visible.length - 1));
      case "ArrowLeft":
        return e.preventDefault(), move(activeRow ?? to(0), Math.max(col - 1, 0));
      case "Enter":
      case " ":
        if (activeRow === "header") return e.preventDefault(), sortBy(visible[col].id);
        if (e.key === "Enter" && r && p.onActivate) return e.preventDefault(), p.onActivate(r);
        return;
      case "ContextMenu":
      case "F10":
        if (e.key === "F10" && !e.shiftKey) return;
        if (activeRow === "header") return e.preventDefault(), chooseColumns(underFocus());
        if (r) return e.preventDefault(), void openMenu(r);
        return;
      case "Delete":
      case "Backspace":
        if (r && p.onDelete && (isMac() ? e.key === "Backspace" && e.metaKey : e.key === "Delete")) return e.preventDefault(), p.onDelete(r);
    }
  }

  if (shown.length === 0) return <>{p.empty}</>;
  const primary = keys[0];

  return (
    <div className="grid-scroll">
      <table
        ref={tableRef}
        role="grid"
        aria-label={p.label}
        aria-rowcount={shown.length + 1}
        aria-colcount={visible.length}
        className="grid"
        tabIndex={-1}
        onKeyDown={onKeyDown}
        onFocus={() => (hadFocus.current = true)}
        onBlur={(e) => {
          // A focused row removed by a poll blurs with no target and leaves the DOM: keep tracking so focus moves to the table.
          const to = e.relatedTarget as Node | null;
          if (to ? !e.currentTarget.contains(to) : (e.target as Node).isConnected) hadFocus.current = false;
        }}
        onContextMenu={(e) => e.preventDefault()}
      >
        <colgroup>
          {visible.map((c) => (
            <col key={c.id} style={c.width ? ({ "--w": `${c.width}px` } as CSSProperties) : undefined} />
          ))}
        </colgroup>
        <thead
          onContextMenu={(e) => {
            e.preventDefault();
            chooseColumns({ x: e.clientX, y: e.clientY });
          }}
        >
          <tr role="row" aria-rowindex={1}>
            {visible.map((c, ci) => {
              const sorted = primary?.id === c.id ? (primary.desc ? "descending" : "ascending") : undefined;
              const active = activeRow === "header" && ci === col;
              return (
                <th key={c.id} role="columnheader" scope="col" aria-sort={sorted} className={c.align === "end" ? "num" : undefined}>
                  {c.sort ? (
                    <button type="button" tabIndex={active ? 0 : -1} data-active={active || undefined} title={c.header} onClick={() => sortBy(c.id)} onFocus={() => setCursor({ row: "header", col: ci })}>
                      <span className="th-label">{c.header}</span>
                      {sorted && <Icon name={sorted === "ascending" ? "arrow-up" : "arrow-down"} />}
                    </button>
                  ) : (
                    <span tabIndex={active ? 0 : -1} data-active={active || undefined} title={c.header}>
                      <span className="th-label">{c.header}</span>
                    </span>
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {shown.map((r, ri) => {
            const k = rowKeys[ri];
            const isSel = k === p.selected;
            return (
              <tr
                key={k}
                role="row"
                aria-rowindex={ri + 2}
                aria-selected={isSel}
                className={stale ? "stale" : undefined}
                data-copy={p.onCopy?.(r)}
                onClick={() => p.onSelect(k)}
                onDoubleClick={() => p.onActivate?.(r)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  p.onSelect(k);
                  void openMenu(r, { x: e.clientX, y: e.clientY });
                }}
              >
                {visible.map((c, ci) => {
                  const active = activeRow === k && ci === col;
                  const blank = stale && c.live;
                  const note = blank ? null : c.note?.(r);
                  const detail = blank ? null : c.detail?.(r);
                  const tipId = `${tipBase}-${ri}-${ci}`;
                  const cellId = `${k}:${c.id}`;
                  return (
                    <td
                      key={c.id}
                      role="gridcell"
                      className={c.align === "end" ? "num" : undefined}
                      tabIndex={active ? 0 : -1}
                      data-active={active || undefined}
                      aria-describedby={detail ? tipId : undefined}
                      onFocus={() => {
                        setCursor({ row: k, col: ci });
                        if (detail) setTip(cellId);
                      }}
                      onBlur={() => setTip((t) => (t === cellId ? null : t))}
                      onMouseEnter={() => detail && setTip(cellId)}
                      onMouseLeave={() => setTip((t) => (t === cellId ? null : t))}
                    >
                      {blank ? <span className="note">{NOT_REFRESHED}</span> : c.cell(r)}
                      {note && <span className="note">{note}</span>}
                      {detail && (
                        <span role="tooltip" id={tipId} className="tip" hidden={tip !== cellId}>
                          {detail}
                        </span>
                      )}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
