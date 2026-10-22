import type { CSSProperties } from "react";
import type { Estimate, Fit } from "../api/client";
import { humanBytes } from "../format";

/** Memory needed (weights, KV cache, compute buffers, overhead) drawn against what the machine has. */
export function FitBar({ fit, estimate, label }: { fit: Fit; estimate: Estimate; label: string }) {
  const capacity = fit.verdict === "fits" ? estimate.total_bytes + fit.headroom_bytes : Math.max(fit.needed_bytes, fit.available_bytes);
  // Only a custom property is set inline (plan §3.3): the width is measured geometry.
  const w = (b: number) => ({ "--w": `${Math.min(100, (b / Math.max(capacity, 1)) * 100).toFixed(2)}%` }) as CSSProperties;
  const over = fit.verdict === "too_large";
  const text = over
    ? `Needs ${humanBytes(estimate.total_bytes)} of memory; ${humanBytes(fit.available_bytes)} available`
    : `Needs ${humanBytes(estimate.total_bytes)} of ${humanBytes(capacity)} available memory`;
  return (
    <div className="fit">
      <div className={`fit-track${over ? " over" : ""}`} role="img" aria-label={`${label}: ${text}`}>
        <div className="fit-seg weights" style={w(estimate.weights_bytes)} />
        <div className="fit-seg kv" style={w(estimate.kv_cache_bytes)} />
        <div className="fit-seg compute" style={w(estimate.compute_buffer_bytes)} />
        <div className="fit-seg overhead" style={w(estimate.overhead_bytes)} />
      </div>
      <div className="fit-legend" aria-hidden="true">
        Weights {humanBytes(estimate.weights_bytes)}, KV cache {humanBytes(estimate.kv_cache_bytes)} at {estimate.n_ctx.toLocaleString()} context, buffers{" "}
        {humanBytes(estimate.compute_buffer_bytes + estimate.overhead_bytes)}
      </div>
    </div>
  );
}
