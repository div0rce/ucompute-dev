// Devices inspector tabs (plan §2.2 "Inspector tabs and fields"). Every value says where it comes from and when it was
// measured; what this computer does not know reads "Not reported", never a guess.
import { useEffect, useRef, useState, type FormEvent } from "react";
import { shortLabel, type AuditEvent, type Hardware, type Job, type Peer } from "../../api/client";
import { mapError } from "../../api/errors";
import { ago, humanBytes, humanDuration, num, plural } from "../../format";
import { href, select, useLocation } from "../../router";
import { MODE_LABEL } from "../../shell/StatusBar";
import { dialErrorText, lastOutcome, live, promptReadiness, type DeviceRow } from "../../state/devices";
import { audit, dialing, groups as groupsStore, hardware, jobs, mesh, models, network, refreshFresh, samples } from "../../state/live";
import { daemon, privacy } from "../../state/stores";
import { DataTable } from "../../ui/DataTable";
import { activityColumns, useActivityRows, useNames } from "../activity/Activity";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { firewall, FirewallNotice } from "../../ui/FirewallNotice";
import { GroupMembership } from "../../ui/Groups";
import type { InspectorTab } from "../../ui/Inspector";
import { KV, Unavailable } from "../../ui/primitives";
import { frameState } from "../../ui/SectionFrame";
import {
  canPairWith,
  capsGap,
  connectBlocked,
  connectOutcome,
  connectTo,
  connRate,
  copyId,
  dialAddrError,
  disconnect,
  focusDevices,
  frozenWhy,
  grouped,
  hardwareGap,
  openDevice,
  revokeDevice,
  RunCell,
  StateCell,
  when,
} from "./table";

/** Why the inspector's own actions wait (busy or stale rows), as the toolbar's (`frozenWhy`); null while live. */
const useFrozen = (): string | null => frozenWhy(frameState(daemon.use(), network.use(), mesh.use()));

/** Raw value first (what `common hardware --json` prints), human form second. */
const bytes = (n: number | null | undefined) => (n == null ? "Not reported" : `${n.toLocaleString()} bytes (${humanBytes(n)})`);
const yes = (b: boolean) => (b ? "On" : "Off");

/** The tabs a row has: this computer and paired devices get all seven; nearby and revoked rows what is known. */
export function deviceTabs(r: DeviceRow): InspectorTab[] {
  const all: (InspectorTab & { for: DeviceRow["trust"][] })[] = [
    { id: "overview", label: "Overview", for: ["self", "trusted", "untrusted", "revoked"], render: () => <Overview r={r} /> },
    { id: "hardware", label: "Hardware", for: ["self", "trusted"], render: () => (r.trust === "self" ? <SelfHardware /> : <PeerHardware r={r} />) },
    { id: "models", label: "Models", for: ["self", "trusted"], render: () => <ModelsTab r={r} /> },
    { id: "performance", label: "Performance", for: ["self", "trusted"], render: () => <Performance r={r} /> },
    { id: "connection", label: "Connection", for: ["self", "trusted", "untrusted"], render: () => <Connection r={r} /> },
    { id: "permissions", label: "Permissions", for: ["self", "trusted"], render: () => <Permissions r={r} /> },
    { id: "activity", label: "Activity", for: ["self", "trusted", "untrusted", "revoked"], render: () => <ActivityTab r={r} /> },
  ];
  return all.filter((t) => t.for.includes(r.trust));
}

function IdValue({ id }: { id: string }) {
  if (!id) return <>—</>;
  return (
    <span className="device-id-row">
      <code className="device-id">{grouped(id)}</code>
      <button type="button" className="btn btn-sm" aria-label="Copy device ID" onClick={() => copyId(id)}>
        Copy
      </button>
    </span>
  );
}

/**
 * An inline confirmation (`role=group`): focus starts on Cancel (plan §3.7: destructive actions), so the Delete key
 * followed by Enter never unpairs or revokes; Esc or Cancel returns focus to the opener.
 */
export function Confirm({ label, text, action, onConfirm, onCancel }: { label: string; text: string; action: string; onConfirm: () => void; onCancel: () => void }) {
  const opener = useRef(document.activeElement as HTMLElement | null);
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => btn.current?.focus(), []);
  const cancel = () => {
    onCancel();
    setTimeout(() => opener.current?.isConnected && opener.current.focus(), 0);
  };
  return (
    <div
      className="confirm"
      role="group"
      aria-label={label}
      onKeyDown={(e) => {
        if (e.key !== "Escape") return;
        e.stopPropagation();
        cancel();
      }}
    >
      <p>{text}</p>
      <div className="confirm-actions">
        <button type="button" className="btn btn-danger" onClick={onConfirm}>
          {action}
        </button>
        <button ref={btn} type="button" className="btn" onClick={cancel}>
          Cancel
        </button>
      </div>
    </div>
  );
}

// Overview ----------------------------------------------------------------------------------------------------------

function Overview({ r }: { r: DeviceRow }) {
  if (r.trust === "self") return <SelfOverview r={r} />;
  if (r.trust === "trusted") return <PeerOverview r={r} p={r.peer!} />;
  if (r.trust === "untrusted") return <NearbyOverview r={r} />;
  return <RevokedOverview r={r} />;
}

function SelfOverview({ r }: { r: DeviceRow }) {
  const d = daemon.use();
  return (
    <>
      <KV
        items={[
          { k: "Name", v: <>{r.name} <a href={href("settings", "network")}>Change in Settings › Network</a></> },
          { k: "Device ID", v: <IdValue id={r.id} /> },
          { k: "Short label", v: r.shortLabel || "—" },
          { k: "State", v: <StateCell r={r} />, note: r.state.detail },
          { k: "Can run here", v: <RunCell w={r.runThere} />, note: [r.runThere?.note, r.runThere?.detail].filter(Boolean).join(" · ") || undefined },
          { k: "Service version", v: d.version ?? "—" },
          { k: "Running for", v: d.status ? humanDuration(d.status.uptime_seconds) : "—" },
          { k: "Started by this app", v: d.spawned ? "Yes" : "No (started outside this app)" },
        ]}
      />
      <p className="small muted prose tab-foot">Compare the full id on both screens when pairing. The short label only tells devices apart; it is not proof of identity.</p>
    </>
  );
}

function lastAnswer(id: string, events: AuditEvent[] | null): string {
  // Not read yet is not "none tried".
  if (!events) return "Last request from here: not read yet.";
  const last = lastOutcome(events, id);
  if (!last) return "No request tried yet.";
  const word = last.outcome === "done" ? "done" : mapError(last.outcome, "").title;
  return `Last request from here: ${word} (${when(last.at)}).`;
}

function PeerOverview({ r, p }: { r: DeviceRow; p: Peer }) {
  const g = groupsStore.use().data;
  const mode = privacy.use()?.mode;
  const events = audit.use().data;
  const loc = useLocation();
  const caps = r.caps;
  // Groups or Privacy not read yet: what is missing is not known, so nothing is claimed missing.
  const ready = g && mode ? promptReadiness(p, g, mode === "local_only") : { ok: false, text: "Not known until this computer's groups and privacy mode are read." };
  const frozen = useFrozen();
  const confirming = loc.params.get("confirm") === "revoke" && !frozen;
  const back = () => select(r.key, { tab: "overview" });
  return (
    <>
      <KV
        items={[
          { k: "Name", v: r.name },
          { k: "Trust", v: "Trusted (paired)" },
          {
            k: "State",
            v: (
              <>
                <StateCell r={r} />
                {r.state.detail && <span className="detail">{r.state.detail}</span>}
                {!p.connected && p.last_error && r.state.word !== "Unreachable" && <span className="detail">{dialErrorText(p.last_error)}</span>}
                {/* The address tried, unless the service's message already names it. */}
                {!p.connected && p.last_error && !p.last_error.message.includes(p.last_error.addr) && (
                  <span className="detail">
                    Address tried: <code>{p.last_error.addr}</code>
                  </span>
                )}
              </>
            ),
          },
          {
            k: "Can run there",
            v: (
              <>
                <RunCell w={r.runThere} />
                <span className="detail">{ready.text}</span>
                {r.runThere?.note && <span className="detail">{r.runThere.note}</span>}
                <span className="detail">{lastAnswer(p.id, events)}</span>
              </>
            ),
          },
          { k: "Can use this computer", v: r.useHere?.word ?? "—", note: r.useHere?.note },
          { k: "Device ID", v: <IdValue id={p.id} /> },
          { k: "Short label", v: p.short_label },
          { k: "Paired", v: new Date(p.paired_at_utc).toLocaleString() },
          { k: "Last seen", v: live(p) ? "Now" : p.last_seen_utc ? when(p.last_seen_utc) : "Never" },
          { k: "Groups", v: p.groups.join(", ") || "None" },
          {
            k: "Software",
            v: caps ? `Common ${caps.common_version} · protocol minor ${caps.protocol_minor} · ${caps.engines.join(", ") || "no engine reported"}` : <Unavailable reason={capsGap(caps) ?? ""} />,
          },
        ]}
      />
      <section className="danger-zone" aria-label="Danger">
        <h3>Danger</h3>
        {confirming ? (
          <Confirm
            label={`Confirm revoking ${r.name}`}
            text={`Revoke ${r.name}? This computer signs a revocation and sends it to every device in its groups. Its connections close now, and it cannot reconnect until it is paired again.`}
            action="Revoke"
            onConfirm={() => {
              select(null, { tab: "overview" });
              focusDevices();
              void revokeDevice(p);
            }}
            onCancel={back}
          />
        ) : (
          <>
            <button type="button" className="btn btn-danger" disabled={!!frozen} onClick={() => select(r.key, { tab: "overview", confirm: "revoke" })}>
              Revoke {r.name}
            </button>
            {frozen && <span className="note">{frozen}</span>}
          </>
        )}
      </section>
    </>
  );
}

function NearbyOverview({ r }: { r: DeviceRow }) {
  const d = r.discovered!;
  return (
    <KV
      items={[
        { k: "Device ID", v: <IdValue id={d.id} /> },
        { k: "Short label", v: r.shortLabel },
        { k: "Addresses", v: <span className="mono">{(d.addrs?.length ? d.addrs : [d.addr]).join(", ")}</span> },
        {
          k: "Pairing window",
          v: d.pairing ? `Announced a pairing window ${ago(d.seen_utc)}` : "Not in pairing mode",
          note: d.pairing ? "The announcement can outlive the window." : undefined,
        },
        { k: "Protocol", v: d.proto },
        { k: "Seen", v: when(d.seen_utc) },
      ]}
    />
  );
}

function RevokedOverview({ r }: { r: DeviceRow }) {
  const rev = r.revocation!;
  const net = network.use().data;
  const peers = mesh.use().data?.peers ?? [];
  const by = rev.revoked_by === net?.peer_id ? "This computer" : (peers.find((p) => p.id === rev.revoked_by)?.name ?? shortLabel(rev.revoked_by));
  return (
    <KV
      items={[
        { k: "Device ID", v: <IdValue id={rev.peer_id} /> },
        { k: "Revoked", v: new Date(rev.at_utc).toLocaleString() },
        { k: "Revoked by", v: by },
        { k: "Sequence", v: num(rev.seq) },
        { k: "Nearby", v: r.discovered ? `Announced nearby ${ago(r.discovered.seen_utc)}` : "Not announced nearby" },
      ]}
    />
  );
}

// Hardware ----------------------------------------------------------------------------------------------------------

function HardwareKV({ hw, tag }: { hw: Hardware; tag?: string }) {
  return (
    <KV
      items={[
        { k: "Chip", v: hw.host.chip },
        { k: "System", v: `${hw.host.os} ${hw.host.os_version}, ${hw.host.arch}` },
        { k: "Processor", v: `${hw.cpu.model}, ${plural(hw.cpu.physical_cores, "core")} (${plural(hw.cpu.logical_cores, "thread")}), ${hw.cpu.arch}` },
        { k: "Processor features", v: hw.cpu.features.length ? hw.cpu.features.join(", ") : "None reported" },
        { k: "Memory", v: `${bytes(hw.memory.total_bytes)} total${hw.memory.unified ? ", shared between CPU and GPU" : ""}` },
        { k: "Available memory", v: bytes(hw.memory.available_bytes), note: tag },
        { k: "Free disk", v: bytes(hw.disk_free_bytes), note: tag },
      ]}
    />
  );
}

function HardwareTables({ hw }: { hw: Hardware }) {
  return (
    <>
      <h3>Graphics</h3>
      {hw.gpus.length === 0 ? (
        <p className="muted">No GPU was detected.</p>
      ) : (
        <table className="mini">
          <thead>
            <tr>
              <th scope="col">Name</th>
              <th scope="col">Vendor</th>
              <th scope="col">Backend</th>
              <th scope="col" className="num">
                Cores
              </th>
              <th scope="col">Memory</th>
            </tr>
          </thead>
          <tbody>
            {hw.gpus.map((g) => (
              <tr key={g.name + g.backend}>
                <td>{g.name}</td>
                <td>{g.vendor}</td>
                <td>{g.backend}</td>
                <td className="num">{num(g.cores)}</td>
                <td>{g.memory_bytes == null ? "—" : humanBytes(g.memory_bytes)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <h3>Inference backends</h3>
      <table className="mini">
        <thead>
          <tr>
            <th scope="col">Backend</th>
            <th scope="col">Available</th>
            <th scope="col">Why</th>
          </tr>
        </thead>
        <tbody>
          {hw.backends.map((b) => (
            <tr key={b.name}>
              <td>{b.name}</td>
              <td className={b.available ? "tone-ok" : "tone-warn"}>{b.available ? "Yes" : "No"}</td>
              <td>{b.available ? (b.reason ?? "Ready to use.") : (b.reason ?? "Not available on this computer.")}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </>
  );
}

/** `/api/hardware` in full: the same values `common hardware --json` prints (F-10-04), plus the raw JSON. */
function SelfHardware() {
  const h = hardware.use();
  const [probing, setProbing] = useState(false);
  if (!h.data) return h.error ? <ErrorNotice error={h.error} /> : <p className="muted">Reading the hardware report…</p>;
  return (
    <>
      {/* A failed re-probe or poll keeps the last report, which says when it was probed. */}
      {h.error && <ErrorNotice error={h.error} />}
      <div className="tab-head">
        <p className="small muted">
          What Common found on this computer, probed {h.at ? when(h.at) : "—"} in {num(h.data.probe_ms, 1)} ms. The same values <code>common hardware --json</code> prints.
        </p>
        <button
          type="button"
          className="btn btn-sm"
          disabled={probing}
          onClick={async () => {
            setProbing(true);
            await refreshFresh("hardware");
            setProbing(false);
          }}
        >
          Re-probe
        </button>
      </div>
      <HardwareKV hw={h.data} />
      <HardwareTables hw={h.data} />
      <details>
        <summary>Raw JSON</summary>
        <pre className="small">{JSON.stringify(h.data, null, 2)}</pre>
      </details>
    </>
  );
}

/** What the device sent when this connection opened, measured when its networking started; never presented as live. */
function PeerHardware({ r }: { r: DeviceRow }) {
  const gap = hardwareGap(r.caps);
  if (gap || !r.caps?.hardware) return <p className="muted">{gap}.</p>;
  const caps = r.caps;
  const hw = caps.hardware!;
  const tag = "at its network start";
  return (
    <>
      <p className="small muted prose tab-head">
        Hardware and free memory measured when {r.name}'s networking started (time not reported). Sent and signed at {when(caps.issued_at_utc)} when this connection opened. Not independently
        verified.
      </p>
      <HardwareKV hw={hw} tag={tag} />
      <KV items={[{ k: "Free memory", v: bytes(caps.free_memory_bytes), note: tag }]} />
      <HardwareTables hw={hw} />
    </>
  );
}

// Models ------------------------------------------------------------------------------------------------------------

function ModelsTab({ r }: { r: DeviceRow }) {
  const catalog = models.use().data;
  if (r.trust === "self") {
    const have = (catalog ?? []).filter((m) => m.downloaded);
    if (!catalog) return <p className="muted">Reading the model list…</p>;
    if (!have.length) return <p className="muted">No model downloaded on this computer.</p>;
    return (
      <table className="mini">
        <thead>
          <tr>
            <th scope="col">Model</th>
            <th scope="col" className="num">
              Size
            </th>
            <th scope="col">State</th>
          </tr>
        </thead>
        <tbody>
          {have.map((m) => (
            <tr key={m.id}>
              <td>
                <a href={href("models", m.id)}>{m.name}</a> <span className="note mono">{m.id}</span>
              </td>
              <td className="num">{humanBytes(m.bytes)}</td>
              <td>{m.loaded ? "Loaded" : "Downloaded"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    );
  }
  const gap = capsGap(r.caps);
  if (gap || !r.caps) return <p className="muted">{gap}.</p>;
  const names = new Map((catalog ?? []).map((m) => [m.id, m.name]));
  return (
    <>
      <p className="small muted tab-head">Reported when this connection opened; models pulled or removed there since then are not shown.</p>
      {r.caps.models.length === 0 ? (
        <p className="muted">No model files reported.</p>
      ) : (
        <ul className="plain-list">
          {r.caps.models.map((m) => (
            <li key={m.sha256}>
              {m.id ? (
                <>
                  {names.get(m.id) ?? m.id} <span className="note mono">{m.id}</span>
                </>
              ) : (
                <>
                  Unknown model <code>{m.sha256.slice(0, 12)}…</code> <span className="note">(not in this computer's catalog; cannot be run from here)</span>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

// Performance -------------------------------------------------------------------------------------------------------

const median = (xs: number[]) => {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const nums = (xs: (number | null | undefined)[]) => xs.filter((x): x is number => typeof x === "number");

/**
 * Finished E3 jobs grouped by `key`: runs, median TTFT and median decode tok/s. Measured values only. `ttft: false`
 * for another device's engine table: a job's TTFT is timed on this computer from arrival (routing, network and that
 * device's queue and load included), so it is never presented as that engine's figure (plan §2.2).
 */
function JobStats({ rows, keyOf, caption, ttft: showTtft = true }: { rows: Job[]; keyOf: (j: Job) => string; caption: string; ttft?: boolean }) {
  const groups = new Map<string, Job[]>();
  for (const j of rows) groups.set(keyOf(j), [...(groups.get(keyOf(j)) ?? []), j]);
  if (!groups.size) return <p className="muted">No finished runs yet. {caption}</p>;
  return (
    <table className="mini">
      <caption>{caption}</caption>
      <thead>
        <tr>
          <th scope="col">Model</th>
          <th scope="col" className="num">
            Runs
          </th>
          {showTtft && (
            <th scope="col" className="num">
              Median TTFT
            </th>
          )}
          <th scope="col" className="num">
            Median decode
          </th>
        </tr>
      </thead>
      <tbody>
        {[...groups].map(([k, js]) => {
          const ttftMs = median(nums(js.map((j) => j.ttft_ms)));
          const tps = median(nums(js.map((j) => j.decode_tokens_per_second)));
          return (
            <tr key={k}>
              <td>{k}</td>
              <td className="num">{num(js.length)}</td>
              {showTtft && <td className="num">{ttftMs == null ? "—" : `${num(ttftMs)} ms`}</td>}
              <td className="num">{tps == null ? "—" : `${num(tps, 1)} tok/s`}</td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function Performance({ r }: { r: DeviceRow }) {
  const d = daemon.use();
  const j = jobs.use().data;
  const events = audit.use().data;
  const peers = mesh.use().data?.peers ?? [];
  const recent = j && j !== "unsupported" ? j.recent.filter((x) => x.state === "done") : null;
  const noJobs = j === "unsupported" ? <p className="muted">Job timings are not reported by this service version.</p> : !recent ? <p className="muted">Reading jobs…</p> : null;
  if (r.trust === "self") {
    const l = d.status?.loaded;
    const forWhom = l?.loaded_for && l.loaded_for !== "local" ? `a request from ${peers.find((p) => `peer:${p.id}` === l.loaded_for)?.name ?? shortLabel(l.loaded_for.slice(5))}` : l?.loaded_for ? "this computer" : null;
    const local = recent?.filter((x) => x.target === "local") ?? [];
    return (
      <>
        <h3>Loaded model, measured here</h3>
        {l ? (
          <KV
            items={[
              { k: "Model", v: l.model_id },
              { k: "Runs on", v: l.device_actual, note: l.fallback_reason ? `CPU fallback: ${l.fallback_reason}` : undefined },
              { k: "Offloaded layers", v: `${l.offloaded_layers} of ${l.total_layers}` },
              { k: "Context", v: `${num(l.n_ctx)} tokens`, note: l.context_note ?? undefined },
              { k: "Threads", v: num(l.threads) },
              { k: "Parallel slots", v: num(l.parallel_slots) },
              { k: "Load time", v: `${num(l.load_ms)} ms` },
              { k: "Loaded for", v: forWhom ?? "Not reported by this service version" },
            ]}
          />
        ) : (
          <p className="muted">No model loaded.</p>
        )}
        <h3>Finished runs on this computer</h3>
        {noJobs ?? <JobStats rows={local} keyOf={(x) => `${x.model_id} · ${x.device_actual ?? "device not reported"}`} caption={`Last ${plural(local.length, "finished run")} since the service started. TTFT counts from arrival here, including waiting and loading.`} />}
      </>
    );
  }
  // Two sources, two tables: the other engine's own timings, and the end-to-end view from here. Never mixed.
  const sent = recent?.filter((x) => x.target === `peer:${r.id}`) ?? [];
  const finished = (events ?? []).filter((e) => {
    const f = (e.fields ?? {}) as Record<string, unknown>;
    return e.kind === "remote_request" && e.peer_id === r.id && f.role === "client" && f.phase === "finished";
  });
  const byModel = new Map<string, Record<string, unknown>[]>();
  for (const e of finished) {
    const f = e.fields as Record<string, unknown>;
    const m = String(f.model_id ?? "unknown");
    byModel.set(m, [...(byModel.get(m) ?? []), f]);
  }
  return (
    <>
      <h3>Measured by {r.name}'s engine</h3>
      {noJobs ?? <JobStats rows={sent} keyOf={(x) => x.model_id} ttft={false} caption={`Measured by ${r.name}'s engine; GPU/CPU not reported.`} />}
      <p className="small muted">No median TTFT here: it is timed on this computer from arrival, so it includes the network and {r.name}'s queue and load, not only its engine.</p>
      <h3>End to end from this computer</h3>
      {byModel.size === 0 ? (
        <p className="muted">No request to {r.name} recorded in Activity yet.</p>
      ) : (
        <table className="mini">
          <caption>End to end from this computer, including queue, load, prompt and network.</caption>
          <thead>
            <tr>
              <th scope="col">Model</th>
              <th scope="col" className="num">
                Requests
              </th>
              <th scope="col">Outcomes</th>
              <th scope="col" className="num">
                Tokens/s
              </th>
            </tr>
          </thead>
          <tbody>
            {[...byModel].map(([m, fs]) => {
              const outcomes = new Map<string, number>();
              for (const f of fs) outcomes.set(String(f.outcome), (outcomes.get(String(f.outcome)) ?? 0) + 1);
              const rates = nums(fs.map((f) => (typeof f.completion_tokens === "number" && typeof f.duration_ms === "number" && f.duration_ms > 0 ? (f.completion_tokens * 1000) / f.duration_ms : null)));
              const tps = median(rates);
              return (
                <tr key={m}>
                  <td>{m}</td>
                  <td className="num">{num(fs.length)}</td>
                  <td>{[...outcomes].map(([o, n]) => `${o} ×${n}`).join(", ")}</td>
                  <td className="num">{tps == null ? "—" : num(tps, 1)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </>
  );
}

// Connection --------------------------------------------------------------------------------------------------------

function Connection({ r }: { r: DeviceRow }) {
  if (r.trust === "self") return <SelfConnection />;
  if (r.trust === "trusted") return <PeerConnection r={r} p={r.peer!} />;
  return <NearbyConnection r={r} />;
}

function NearbyConnection({ r }: { r: DeviceRow }) {
  const frozen = useFrozen();
  const d = r.discovered!;
  return (
    <>
      <KV items={[{ k: "Addresses", v: <span className="mono">{(d.addrs?.length ? d.addrs : [d.addr]).join(", ")}</span>, note: `seen nearby ${ago(d.seen_utc)}` }]} />
      {canPairWith(r) && (
        <p className="tab-foot">
          {/* Enabled without the announcement too: it can lag a pairing window just opened there (plan §2.2). */}
          <button type="button" className="btn" disabled={!!frozen} onClick={() => openDevice(r.key, { pair: "join" })}>
            Pair with {r.name}…
          </button>
          {(frozen || !d.pairing) && <span className="note">{frozen ?? "Not announced in pairing mode yet: start pairing on that device first."}</span>}
        </p>
      )}
    </>
  );
}

export const NETWORK_HELP = (listen: string | null | undefined, mdns: boolean) =>
  `Off by default. When on, Common listens${listen ? ` on ${listen}` : ""} for connections from paired devices only, encrypted and checked against the key saved at pairing.${mdns ? " It also announces its id on this network so paired devices can find it." : ""}`;

function SelfConnection() {
  const net = network.use().data;
  const fw = firewall.use();
  const peers = mesh.use().data?.peers ?? [];
  if (!net) return <p className="muted">Reading the network state…</p>;
  const connected = peers.filter(live).length;
  return (
    <>
      <KV
        items={[
          { k: "Networking", v: yes(net.enabled) },
          { k: "Listening on", v: net.listen_addr ? <code>{net.listen_addr}</code> : "Not listening" },
          { k: "Address for other devices", v: net.dial_addr ? <code>{net.dial_addr}</code> : "—", note: net.dial_addr ? "A paired device that cannot find this one on the network can connect to it at this address." : undefined },
          { k: "Discovery (mDNS)", v: yes(net.mdns) },
          { k: "Devices connected", v: `${plural(connected, "device")} · ${plural(net.connections, "connection")}` },
          { k: "Runs requests for paired devices", v: yes(net.serve_peers) },
          { k: "Windows Firewall", v: !fw ? "Not read" : !fw.supported ? "Not applicable on this system" : fw.allowed ? "Rule for the service found" : "No rule for the service found" },
        ]}
      />
      <p className="small muted prose tab-foot">{NETWORK_HELP(net.listen_addr, net.mdns)}</p>
      <FirewallNotice />
    </>
  );
}

function PeerConnection({ r, p }: { r: DeviceRow; p: Peer }) {
  const frozen = useFrozen();
  const conn = samples.use().conn;
  const net = network.use().data;
  const link = r.link;
  const d = r.discovered;
  const since = link ? (Date.now() - Date.parse(link.opened_at_utc)) / 1000 : 0;
  const avg = (b: number) => (since > 0 ? ` (${humanBytes(b / since)}/s since open)` : "");
  const rate = connRate(conn, link?.id);
  return (
    <>
      <KV
        items={[
          { k: "State", v: <StateCell r={r} />, note: r.state.detail },
          ...(link
            ? [
                { k: "Address", v: <code>{link.remote_addr}</code>, note: "connected" },
                { k: "Direction", v: link.direction === "outbound" ? "Outbound (this computer dialled)" : `Inbound (${r.name} dialled)` },
                { k: "Open since", v: when(link.opened_at_utc) },
                { k: "Round trip", v: `${num(link.rtt_ms, 1)} ms` },
                { k: "Sent", v: `${humanBytes(link.tx_bytes)}${avg(link.tx_bytes)}`, note: rate ? `now ${humanBytes(rate.tx)}/s` : undefined },
                { k: "Received", v: `${humanBytes(link.rx_bytes)}${avg(link.rx_bytes)}`, note: rate ? `now ${humanBytes(rate.rx)}/s` : undefined },
                { k: "Lost packets", v: num(link.lost_packets) },
              ]
            : [{ k: "Connection", v: link === undefined ? "Details not reported by this service version" : "Not connected" }]),
          {
            k: "Known addresses",
            v: (
              <>
                {d && <span className="detail mono">{(d.addrs?.length ? d.addrs : [d.addr]).join(", ")} <span className="note">seen nearby {ago(d.seen_utc)}</span></span>}
                {p.last_addr && <span className="detail mono">{p.last_addr} <span className="note">last known</span></span>}
                {!d && !p.last_addr && "Not known"}
              </>
            ),
          },
          ...(p.last_error ? [{ k: "Last error", v: mapError(p.last_error.code, p.last_error.message).title, note: `${dialErrorText(p.last_error)} (${p.last_error.addr})` }] : []),
        ]}
      />
      <ConnectForm r={r} p={p} networkOn={!!net?.enabled} />
      {p.held !== undefined && p.connected && (
        <p className="tab-foot">
          <button type="button" className="btn" disabled={!!frozen} onClick={() => void disconnect(p)}>
            Disconnect {r.name}
          </button>
          <span className="note">{frozen ?? "Until you connect again, turn networking off and on, or the service restarts."}</span>
        </p>
      )}
    </>
  );
}

/** Connect with an optional `ip:port`: only a hint, the key saved at pairing decides (the service validates it). */
function ConnectForm({ r, p, networkOn }: { r: DeviceRow; p: Peer; networkOn: boolean }) {
  const dial = dialing.use();
  const out = connectOutcome.use()[p.id];
  const [addr, setAddr] = useState("");
  const [bad, setBad] = useState<ReturnType<typeof dialAddrError>>(null);
  const status = useRef<HTMLParagraphElement>(null);
  const busy = dial === p.id;
  const frozen = useFrozen();
  const blocked = frozen ?? connectBlocked(p, r.name);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (dialing.get() || frozen) return;
    // Checked here first (plan §2.2: ip:port, validated); nothing is sent for an address the service would refuse.
    const invalid = addr.trim() ? dialAddrError(addr) : null;
    setBad(invalid);
    if (invalid) return;
    // The form goes away once connected, so the outcome takes focus.
    if (await connectTo(p, addr.trim() || undefined)) setTimeout(() => status.current?.focus(), 0);
  }

  return (
    <>
      {out && "ok" in out && (
        <p ref={status} tabIndex={-1} role="status" className="tone-ok">
          {out.ok}
        </p>
      )}
      {!p.connected &&
        (networkOn ? (
          <form className="connect-form" aria-label={`Connect to ${r.name}`} onSubmit={submit}>
            <div className="field">
              <label htmlFor={`connect-addr-${p.id}`}>Address (optional)</label>
              <input
                id={`connect-addr-${p.id}`}
                type="text"
                value={addr}
                placeholder="192.168.1.20:8193"
                autoComplete="off"
                aria-describedby={`connect-help-${p.id}`}
                aria-invalid={bad ? true : undefined}
                onChange={(e) => setAddr(e.target.value)}
              />
              <span id={`connect-help-${p.id}`} className="help small muted">
                Leave empty to try the addresses this computer knows, or enter the address shown on {r.name} under Devices › This computer › Connection. Whatever answers there must still prove it is {r.name} with the
                key saved at pairing.
              </span>
            </div>
            {bad ? <ErrorNotice error={bad} /> : out && "error" in out && <ErrorNotice error={out.error} />}
            <button type="submit" className="btn btn-primary" disabled={!!dial || !!frozen}>
              {busy ? "Connecting" : "Connect"}
            </button>
            {((dial && !busy) || frozen) && <span className="note">{blocked}</span>}
          </form>
        ) : (
          <p className="muted tab-foot">Networking is off: turn it on to connect.</p>
        ))}
    </>
  );
}

// Permissions -------------------------------------------------------------------------------------------------------

function Permissions({ r }: { r: DeviceRow }) {
  const net = network.use().data;
  const mode = privacy.use()?.mode;
  const events = audit.use().data;
  if (r.trust === "self")
    return (
      <>
        <h3>What this computer grants all paired devices</h3>
        <KV
          items={[
            { k: "Networking", v: net ? yes(net.enabled) : "—" },
            { k: "Runs requests for paired devices", v: net ? yes(net.serve_peers) : "—" },
            {
              k: "Device quota",
              v: net ? `${num(net.peer_quota.requests_per_minute)} requests/min, ${num(net.peer_quota.tokens_per_minute)} tokens/min, ${num(net.peer_quota.max_concurrent)} at once` : "—",
              note: "Set in the service configuration; applies after restart.",
            },
            { k: "Engine slots for devices", v: net ? num(net.peer_slots) : "—" },
            { k: "Privacy mode", v: mode ? MODE_LABEL[mode] : "—" },
          ]}
        />
        <p className="tab-foot">
          <a href={href("settings", "sharing")}>Change in Settings › Sharing &amp; permissions</a>
        </p>
      </>
    );
  return (
    <>
      <h3>What this computer grants {r.name}</h3>
      <KV items={[{ k: "Can use this computer", v: r.useHere?.word ?? "—", note: r.useHere?.note }]} />
      <GroupMembership peerId={r.id} />
      <h3>What {r.name} grants this computer</h3>
      <p>
        Not reported. <span className="muted">{lastAnswer(r.id, events)}</span>
      </p>
    </>
  );
}

// Activity ----------------------------------------------------------------------------------------------------------

/** The Activity section's rows and columns (plan §2.6), narrowed to this device; this computer's are those without one. */
function ActivityTab({ r }: { r: DeviceRow }) {
  const names = useNames();
  const all = useActivityRows();
  const [sel, setSel] = useState<string | null>(null);
  const rows = all.filter((a) => (r.trust === "self" ? a.peerId === null : a.peerId === r.id));
  return (
    <>
      <DataTable
        id="device-activity"
        label={`Activity for ${r.name}`}
        columns={activityColumns(names)}
        rows={rows}
        rowKey={(a) => a.key}
        selected={sel}
        onSelect={setSel}
        empty={<p className="muted">No activity recorded for {r.trust === "self" ? "this computer" : r.name} yet.</p>}
      />
      <p className="small muted tab-foot">
        Pairing, connections and requests are kept in the audit log; local jobs appear until the service restarts. <a href={href("activity")}>Open Activity</a>
      </p>
    </>
  );
}
