import type { Target } from "../../api/client";
import { live, promptReadiness, type DeviceRow } from "../../state/devices";
import { refresh } from "../../state/live";
import type { Group } from "../../api/client";

const lowerFirst = (s: string) => s.charAt(0).toLowerCase() + s.slice(1);

export interface Destination {
  value: Target;
  label: string;
  /** Why it cannot be chosen now; null when it can. */
  disabled: string | null;
}

/**
 * The destinations Chat offers (plan §2.5): this computer, then every paired device. A device is enabled only while
 * `live` and permitted from here (`promptReadiness`); its label carries its Can run there word, because only the other
 * device knows whether it will serve. Others are listed disabled with the reason. `notLive`: the rows are the last good
 * mesh data, not live (service down or busy, or a read failed): no device is offered as connected from them.
 */
export function destinations(rows: DeviceRow[], groups: Group[] | null, notLive: string | null = null): Destination[] {
  const out: Destination[] = [{ value: "local", label: "This computer", disabled: null }];
  for (const r of rows) {
    if (r.trust !== "trusted" || !r.peer) continue;
    const p = r.peer;
    // Groups not read yet: whether it is permitted is not known (never "not permitted").
    const ready = groups ? promptReadiness(p, groups, false) : null;
    const why = notLive
      ? `state not known (${notLive})`
      : !live(p)
        ? lowerFirst(r.state.word)
        : !ready
          ? "groups not read yet"
          : !ready.ok
            ? `not permitted here: ${ready.text.replace(/^Cannot run prompts from this computer yet: /, "").replace(/\.$/, "")}`
            : null;
    out.push({ value: `peer:${p.id}`, label: why ? `${r.name}: ${why}` : `${r.name} · ${r.runThere?.word ?? "Can send"}`, disabled: why });
  }
  return out;
}

/** The Destination select; in local-only privacy it is replaced by a sentence (the target is forced local in Chat). */
export function DestinationPicker({ options, value, localOnly, onChange }: { options: Destination[]; value: Target; localOnly: boolean; onChange: (t: Target) => void }) {
  if (localOnly) return <span className="chat-local-only">Local only: replies run on this computer</span>;
  return (
    <label className="chat-select">
      <span className="chat-select-label">Destination</span>
      {/* ponytail: refreshed on focus too, so a device paired since the last poll shows up when the list opens. */}
      <select value={value} onFocus={() => void Promise.all([refresh("mesh"), refresh("groups")])} onChange={(e) => onChange(e.target.value as Target)}>
        {options.map((o) => (
          <option key={o.value} value={o.value} disabled={!!o.disabled}>
            {o.label}
          </option>
        ))}
      </select>
    </label>
  );
}
