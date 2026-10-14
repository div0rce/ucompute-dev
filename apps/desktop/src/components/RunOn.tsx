import { useEffect, useState } from "react";
import { api, type Peer, type Target } from "../api/client";
import { privacy, runTarget } from "../state/stores";

/** Returns the display name for a target: "this Mac" or the paired device's name. */
export function targetName(target: Target | undefined, peers: Peer[]): string {
  if (!target || target === "local") return "this Mac";
  const id = target.slice("peer:".length);
  const p = peers.find((x) => x.id === id);
  return p?.name || p?.short_label || `${id.slice(0, 4)}-${id.slice(4, 8)}-${id.slice(8, 12)}`;
}

/**
 * "Run on" selector. Rendered only when the privacy mode allows leaving this device; in local-only mode it
 * renders nothing and the target stays "local", so the selector cannot be the thing that leaks.
 */
export function RunOn({ onPeers }: { onPeers?: (peers: Peer[]) => void } = {}) {
  const mode = privacy.use()?.mode ?? "local_only";
  const target = runTarget.use();
  const [peers, setPeers] = useState<Peer[]>([]);

  useEffect(() => {
    if (mode === "local_only") {
      runTarget.set("local");
      return;
    }
    api.peers().then(
      (r) => {
        setPeers(r.peers);
        onPeers?.(r.peers);
        // A device that was removed or revoked since the selection is no longer a valid target.
        const t = runTarget.get();
        if (t !== "local" && !r.peers.some((p) => `peer:${p.id}` === t)) runTarget.set("local");
      },
      () => undefined,
    );
  }, [mode, onPeers]);

  if (mode === "local_only") return null;
  return (
    <label className="run-on">
      <span className="small muted">Run on</span>
      <select value={target} onChange={(e) => runTarget.set(e.target.value as Target)}>
        <option value="local">This Mac</option>
        {peers.map((p) => (
          <option key={p.id} value={`peer:${p.id}`} disabled={!p.connected}>
            {p.name || p.short_label}
            {p.connected ? "" : " (offline)"}
          </option>
        ))}
      </select>
    </label>
  );
}
