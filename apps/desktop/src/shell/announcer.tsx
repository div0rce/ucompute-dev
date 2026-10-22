// The shell's single polite live region. It announces transitions only (service stopped/started, a trusted device
// connected or lost, a download finished or failed, a reply from this app failed, a table re-sorted), at most one
// every 2 s; the rest wait in order. Tables and the status bar never announce.
import { createStore } from "../state/store";

export const THROTTLE_MS = 2000;
const said = createStore<{ text: string; n: number }>({ text: "", n: 0 });
let queue: string[] = [];
let lastAt = -Infinity;
let timer: ReturnType<typeof setTimeout> | null = null;

function flush() {
  timer = null;
  const text = queue.shift();
  if (text === undefined) return;
  lastAt = Date.now();
  said.set((s) => ({ text, n: s.n + 1 }));
  if (queue.length) timer = setTimeout(flush, THROTTLE_MS);
}

/** Queues `text` for the live region. A repeat of the text already waiting last is dropped. */
export function announce(text: string): void {
  if (queue[queue.length - 1] === text) return;
  queue.push(text);
  if (timer) return;
  const wait = lastAt + THROTTLE_MS - Date.now();
  if (wait <= 0) flush();
  else timer = setTimeout(flush, wait);
}

/** Test helper. */
export function resetAnnouncer(): void {
  if (timer) clearTimeout(timer);
  timer = null;
  queue = [];
  lastAt = -Infinity;
  said.set({ text: "", n: 0 });
}

export function Announcer() {
  const s = said.use();
  return (
    <div className="visually-hidden" aria-live="polite" aria-atomic="true" data-testid="announcer">
      <span key={s.n}>{s.text}</span>
    </div>
  );
}
