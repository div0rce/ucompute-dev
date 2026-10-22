import { useEffect, useRef, type ReactNode } from "react";

/**
 * A native modal `<dialog>` (`showModal()`: focus trap, inert background and Esc come from the platform). Children
 * render only while open. Initial focus goes to the element marked `data-autofocus` (put it on the primary safe
 * control: Confirm for a download, Cancel for a destructive action); focus returns to the opener on close.
 * `onClose` runs for Esc, `form method="dialog"` and when `open` turns false; it must be idempotent.
 */
export function Dialog({ open, label, onClose, children, className }: { open: boolean; label: string; onClose: () => void; children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDialogElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    const closed = () => {
      onCloseRef.current();
      opener.current?.focus?.();
      opener.current = null;
    };
    d.addEventListener("close", closed);
    return () => d.removeEventListener("close", closed);
  }, []);

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.hasAttribute("open")) {
      opener.current = document.activeElement as HTMLElement | null;
      d.showModal();
      d.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    } else if (!open && d.hasAttribute("open")) d.close();
  }, [open]);

  return (
    <dialog ref={ref} aria-label={label} className={className ? `dialog ${className}` : "dialog"}>
      {open && children}
    </dialog>
  );
}
