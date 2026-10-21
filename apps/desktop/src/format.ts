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
