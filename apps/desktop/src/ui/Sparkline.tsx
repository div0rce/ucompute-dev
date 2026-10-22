import { num } from "../format";

const W = 160;
const H = 32;

/**
 * A small line chart (`role=img`); the accessible name gives last/min/max per series. `null` is a gap: no line is
 * drawn across it and it is not counted as 0. Series colours follow their order (`.spark-0` … `.spark-2`).
 */
export function Sparkline({ series, label }: { series: { label: string; values: (number | null)[] }[]; label: string }) {
  const known = series.flatMap((s) => s.values).filter((v): v is number => v != null);
  const max = Math.max(1, ...known);
  const n = Math.max(2, ...series.map((s) => s.values.length));
  const x = (i: number) => (i / (n - 1)) * W;
  const y = (v: number) => H - 1 - (v / max) * (H - 2);
  const summary = series
    .map((s) => {
      const v = s.values.filter((x): x is number => x != null);
      const last = s.values.length ? s.values[s.values.length - 1] : null;
      return v.length ? `${s.label}: last ${last == null ? "unknown" : num(last, 1)}, min ${num(Math.min(...v), 1)}, max ${num(Math.max(...v), 1)}` : `${s.label}: no data`;
    })
    .join("; ");
  return (
    <svg className="sparkline" role="img" aria-label={`${label}. ${summary}`} viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none">
      {series.map((s, si) => {
        const runs: string[][] = [[]];
        s.values.forEach((v, i) => (v == null ? runs.push([]) : runs[runs.length - 1].push(`${x(i).toFixed(1)},${y(v).toFixed(1)}`)));
        return runs.filter((r) => r.length > 1).map((r, ri) => <polyline key={`${si}-${ri}`} className={`spark-${si}`} points={r.join(" ")} />);
      })}
    </svg>
  );
}
