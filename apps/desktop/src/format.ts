export function humanBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "unknown";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[i]}`;
}

export function humanDuration(seconds: number): string {
  if (seconds < 60) return `${Math.round(seconds)} s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} min`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return `${h} h ${m} min`;
}

/** Fit sentence for a model row: "Fits on this computer" or "Needs X GB, you have Y GB". */
export function fitText(fit: { verdict: "fits"; headroom_bytes: number } | { verdict: "too_large"; needed_bytes: number; available_bytes: number; reason: string }): string {
  return fit.verdict === "fits"
    ? `Fits on this computer (${humanBytes(fit.headroom_bytes)} to spare)`
    : `Needs ${humanBytes(fit.needed_bytes)}, you have ${humanBytes(fit.available_bytes)}`;
}

export const num = (n: number | null | undefined, digits = 0) =>
  n == null || !Number.isFinite(n) ? "—" : n.toLocaleString(undefined, { maximumFractionDigits: digits });

/** "1 device", "2 devices", "1,024 tokens". `many` for irregular plurals. */
export const plural = (n: number, one: string, many = `${one}s`) => `${num(n)} ${n === 1 ? one : many}`;

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "always", style: "short" });
/** "12 sec. ago", "3 min. ago" (locale-formatted) from an ISO timestamp or epoch ms; "—" when unknown. */
export function ago(t: string | number | null | undefined, now = Date.now()): string {
  const ms = typeof t === "string" ? Date.parse(t) : t;
  if (ms == null || !Number.isFinite(ms)) return "—";
  const s = Math.max(0, Math.round((now - ms) / 1000));
  if (s < 60) return rtf.format(-s, "second");
  if (s < 3600) return rtf.format(-Math.floor(s / 60), "minute");
  if (s < 86400) return rtf.format(-Math.floor(s / 3600), "hour");
  return rtf.format(-Math.floor(s / 86400), "day");
}

/** Local wall-clock time "HH:MM:SS" (24 h); "—" when unknown. */
export function clock(t: string | number | null | undefined): string {
  const ms = typeof t === "string" ? Date.parse(t) : t;
  if (ms == null || !Number.isFinite(ms)) return "—";
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
}

/** A byte rate: "12 KB/s", "< 1 KB/s"; "—" when unknown. */
export function byteRate(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond == null || !Number.isFinite(bytesPerSecond)) return "—";
  return bytesPerSecond < 1024 ? "< 1 KB/s" : `${humanBytes(bytesPerSecond)}/s`;
}
