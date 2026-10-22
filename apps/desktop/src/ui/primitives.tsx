import { createContext, useContext, type ReactNode } from "react";
import { num } from "../format";
import { Icon, type IconName } from "./icons";
import { Tip } from "./Tip";

export type Tone = "ok" | "active" | "warn" | "danger" | "neutral";

/** State = icon + word + colour, always together (plan §0.3). `note` is visible; `detail` is a Tip. */
export function StateBadge({ tone, icon, label, note, detail }: { tone: Tone; icon: IconName; label: string; note?: string; detail?: string }) {
  const body = (
    <span className={`state tone-${tone}`}>
      <Icon name={icon} className={icon === "loader" ? "spin" : undefined} />
      <span className="state-word">{label}</span>
      {note && <span className="note">{note}</span>}
    </span>
  );
  return detail ? <Tip text={detail}>{body}</Tip> : body;
}

/** Key/value list (`<dl>`). `note` is a visible muted suffix. */
export function KV({ items }: { items: { k: string; v: ReactNode; note?: string }[] }) {
  return (
    <dl className="kv">
      {items.map((i) => (
        <div className="kv-row" key={i.k}>
          <dt>{i.k}</dt>
          <dd>
            {i.v}
            {i.note && <span className="note">{i.note}</span>}
          </dd>
        </div>
      ))}
    </dl>
  );
}

/** "—" with its reason, always visible (plan §0.2): unknown is never blank and never a guess. */
export function Unavailable({ reason }: { reason: string }) {
  return (
    <span className="unavailable">
      <span aria-hidden="true">—</span> <span className="note">{reason}</span>
    </span>
  );
}

export function EmptyState({ title, reason, action }: { title: string; reason?: string; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <p className="empty-title">{title}</p>
      {reason && <p className="empty-reason">{reason}</p>}
      {action && <div className="empty-action">{action}</div>}
    </div>
  );
}

/** Set by SectionFrame around selection actions while its data is stale: every ToolButton inside is disabled with it. */
export const ActionsDisabled = createContext<string | null>(null);

/**
 * An action button. A supported action that is not possible now stays visible and focusable (`aria-disabled`) with
 * the reason as its Tip and description. Below 1200 px wide only the icon shows; the label stays its accessible name
 * and appears as a bubble on hover or focus.
 */
export function ToolButton({
  icon,
  label,
  onClick,
  disabledReason,
  pressed,
  shortcut,
  primary,
  busy,
  detail,
}: {
  icon: IconName;
  label: string;
  /** Visible after the label but not part of the name, for text that changes while the button may have focus (a countdown). */
  detail?: string;
  onClick: () => void;
  disabledReason?: string | null;
  pressed?: boolean;
  /** Shown in the Tip, for example "⌘R". */
  shortcut?: string;
  primary?: boolean;
  /** A request is pending: the icon becomes a spinning loader (still while reduced motion is on). */
  busy?: boolean;
}) {
  const frameReason = useContext(ActionsDisabled);
  const reason = disabledReason ?? frameReason;
  const btn = (
    <button
      type="button"
      className={`tool-btn${primary ? " primary" : ""}`}
      aria-disabled={reason ? "true" : undefined}
      aria-pressed={pressed}
      onClick={(e) => {
        if (reason) {
          e.preventDefault();
          return;
        }
        onClick();
      }}
    >
      <Icon name={busy ? "loader" : icon} className={busy ? "spin" : undefined} />
      <span className="tool-label">
        {label}
        {detail && <span aria-hidden="true"> · {detail}</span>}
      </span>
    </button>
  );
  return <Tip text={reason ?? (shortcut ? `Shortcut: ${shortcut}` : null)}>{btn}</Tip>;
}

/** A segmented filter (`role=group`, `aria-pressed` buttons) with live counts, next to the table it filters. */
export function ScopeBar({ label, items, value, onChange }: { label: string; items: { id: string; label: string; count: number }[]; value: string; onChange: (id: string) => void }) {
  return (
    <div role="group" aria-label={label} className="scope-bar">
      {items.map((it) => (
        <button key={it.id} type="button" className="scope-btn" aria-pressed={value === it.id} onClick={() => onChange(it.id)}>
          {it.label} <span className="scope-count">{num(it.count)}</span>
        </button>
      ))}
    </div>
  );
}
