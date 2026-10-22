import { cloneElement, useId, useState, type FocusEvent, type KeyboardEvent, type MouseEvent, type ReactElement } from "react";

type Handler<E> = ((e: E) => void) | undefined;
const chain =
  <E,>(a: Handler<E>, b: (e: E) => void) =>
  (e: E) => {
    a?.(e);
    b(e);
  };

/**
 * A tooltip shown on hover AND keyboard focus, dismissable with Esc, wired with aria-describedby (never a `title`).
 * The text stays in the DOM (hidden) so the description is always available. Anything that changes a value's meaning
 * must also be visible elsewhere (plan §0.2): a Tip only adds detail.
 */
export function Tip({ text, children }: { text: string | null; children: ReactElement<Record<string, unknown>> }) {
  const id = useId();
  const [open, setOpen] = useState(false);
  const p = children.props;
  const show = () => setOpen(true);
  const hide = () => setOpen(false);
  // No text: the same wrapper, so a control that gains or loses its tip is not remounted (and keeps focus).
  if (text == null) return <span className="tip-host">{children}</span>;
  return (
    <span className="tip-host">
      {cloneElement(children, {
        "aria-describedby": [p["aria-describedby"], id].filter(Boolean).join(" "),
        onMouseEnter: chain(p.onMouseEnter as Handler<MouseEvent>, show),
        onMouseLeave: chain(p.onMouseLeave as Handler<MouseEvent>, hide),
        onFocus: chain(p.onFocus as Handler<FocusEvent>, show),
        onBlur: chain(p.onBlur as Handler<FocusEvent>, hide),
        onKeyDown: chain(p.onKeyDown as Handler<KeyboardEvent>, (e) => e.key === "Escape" && hide()),
      })}
      <span role="tooltip" id={id} className="tip" hidden={!open}>
        {text}
      </span>
    </span>
  );
}
