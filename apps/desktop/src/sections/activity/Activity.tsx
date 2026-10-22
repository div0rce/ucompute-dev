import "./activity.css";
import { useState } from "react";
import { api, ApiError, getBaseUrl, shortLabel } from "../../api/client";
import { ERROR_CODES, explain } from "../../api/errors";
import { ago, clock } from "../../format";
import { go, select, useLocation } from "../../router";
import { audit, jobs, mesh, models, network, useNeed } from "../../state/live";
import { daemon } from "../../state/stores";
import type { MenuItem } from "../../ui/contextMenu";
import type { Column } from "../../ui/DataTable";
import { DataTable } from "../../ui/DataTable";
import { Dialog } from "../../ui/Dialog";
import type { IconName } from "../../ui/icons";
import { Inspector, openInspector } from "../../ui/Inspector";
import { EmptyState, KV, ScopeBar, StateBadge, ToolButton, type Tone } from "../../ui/primitives";
import { frameState, SectionFrame } from "../../ui/SectionFrame";
import { activityRows, type ActivityRow, type Names, type Severity } from "./describe";

const SEVERITY: Record<Severity, { tone: Tone; icon: IconName; order: number }> = {
  Error: { tone: "danger", icon: "circle-x", order: 0 },
  Warning: { tone: "warn", icon: "circle-alert", order: 1 },
  Info: { tone: "neutral", icon: "activity", order: 2 },
};

/** Device and model names from the live stores; this computer's own id reads "This computer". */
export function useNames(): Names {
  const peers = mesh.use().data?.peers;
  const self = network.use().data?.peer_id;
  const catalog = models.use().data;
  return {
    device: (id) => {
      if (id === self) return "This computer";
      const p = peers?.find((x) => x.id === id);
      return p?.name || p?.short_label || shortLabel(id);
    },
    model: (id) => catalog?.find((m) => m.id === id)?.name ?? id,
  };
}

/** Activity rows over the live stores (also for the Dashboard's Recent activity and Devices › Activity). */
export function useActivityRows(): ActivityRow[] {
  const names = useNames();
  return activityRows(audit.use().data, jobs.use().data, names);
}

const eventJson = (r: ActivityRow) => JSON.stringify(r.job ?? (r.events.length === 1 ? r.events[0] : r.events));

/** The Activity table's columns (shared with the Dashboard and Devices › Activity). */
export function activityColumns(names: Names): Column<ActivityRow>[] {
  return [
    {
      id: "time",
      header: "Time",
      cell: (r) => ago(r.at),
      sort: (r) => Date.parse(r.at),
      detail: (r) => `${r.at} (UTC), ${new Date(r.at).toLocaleString()} here${r.count > 1 ? `; first ${clock(r.first)}` : ""}`,
      width: 96,
    },
    {
      id: "severity",
      header: "Severity",
      cell: (r) => <StateBadge tone={SEVERITY[r.severity].tone} icon={SEVERITY[r.severity].icon} label={r.severity} />,
      sort: (r) => SEVERITY[r.severity].order,
      text: (r) => r.severity,
      width: 96,
    },
    { id: "category", header: "Category", cell: (r) => r.category, sort: (r) => r.category, width: 96, detail: (r) => (r.job ? "Local jobs since the service started" : null) },
    { id: "event", header: "Event", cell: (r) => r.sentence, sort: (r) => r.sentence, width: 340 },
    { id: "device", header: "Device", cell: (r) => (r.peerId ? names.device(r.peerId) : "This computer"), sort: (r) => (r.peerId ? names.device(r.peerId) : "This computer"), width: 120 },
    { id: "code", header: "Code", cell: (r) => (r.code ? <code>{r.code}</code> : "—"), text: (r) => r.code ?? "", sort: (r) => r.code, width: 110 },
    { id: "count", header: "Count", cell: (r) => (r.count > 1 ? `×${r.count}` : "1"), sort: (r) => r.count, align: "end", width: 56 },
    { id: "kind", header: "Kind", cell: (r) => <code>{r.kind}</code>, text: (r) => r.kind, sort: (r) => r.kind, hidden: true, width: 130 },
    { id: "request", header: "Request ID", cell: (r) => (r.requestId ? <code>{r.requestId}</code> : "—"), text: (r) => r.requestId ?? "", sort: (r) => r.requestId, hidden: true, width: 150 },
    { id: "addr", header: "Address", cell: (r) => (r.addr ? <code>{r.addr}</code> : "—"), text: (r) => r.addr ?? "", sort: (r) => r.addr, hidden: true, width: 130 },
  ];
}

const SCOPES = [
  { id: "all", label: "All", keep: () => true },
  { id: "problems", label: "Warnings and errors", keep: (r: ActivityRow) => r.severity !== "Info" },
  { id: "connections", label: "Connections", keep: (r: ActivityRow) => r.category === "Connection" },
  { id: "pairing", label: "Pairing", keep: (r: ActivityRow) => r.category === "Pairing" },
  { id: "remote", label: "Remote runs", keep: (r: ActivityRow) => r.category === "Remote run" },
  { id: "execution", label: "Execution", keep: (r: ActivityRow) => r.category === "Execution" },
  { id: "settings", label: "Settings", keep: (r: ActivityRow) => r.category === "Settings" },
];

/** A value of an audit `fields` entry as text. */
const fieldText = (v: unknown) => (typeof v === "string" ? v : JSON.stringify(v));

function Details({ r, names, jobIds }: { r: ActivityRow; names: Names; jobIds: Set<string> }) {
  const newest = r.events[0];
  const fields = newest?.fields && typeof newest.fields === "object" ? Object.entries(newest.fields as Record<string, unknown>) : [];
  return (
    <div className="activity-details">
      <KV
        items={[
          { k: "Time", v: `${r.at} (UTC)`, note: new Date(r.at).toLocaleString() },
          ...(r.count > 1 ? [{ k: "First", v: `${r.first} (UTC)`, note: `${r.count} events merged` }] : []),
          { k: "Event", v: r.sentence },
          { k: "Severity", v: r.severity },
          { k: "Category", v: r.category, note: r.job ? "Local jobs since the service started; kept in memory (last 256)." : undefined },
          { k: "Kind", v: <code>{r.kind}</code> },
          { k: "Device", v: r.peerId ? names.device(r.peerId) : "This computer" },
          ...(r.peerId ? [{ k: "Device ID", v: <code>{r.peerId}</code> }] : []),
          ...(r.code ? [{ k: "Code", v: <code>{r.code}</code>, note: ERROR_CODES[r.code]?.next_step }] : []),
          ...(r.requestId ? [{ k: "Request ID", v: <code>{r.requestId}</code>, note: jobIds.has(r.requestId) ? undefined : "No matching job held by the service" }] : []),
          ...fields.map(([k, v]) => ({ k, v: <code>{fieldText(v)}</code> })),
        ]}
      />
      <details className="activity-raw">
        <summary>Raw JSON</summary>
        <pre className="mono">{JSON.stringify(r.job ?? (r.count > 1 ? r.events : newest), null, 2)}</pre>
      </details>
    </div>
  );
}

/** Activity (plan §2.6): the audit plus this service's finished local jobs, filtered here. Never prompt or reply text. */
export default function Activity() {
  const d = daemon.use();
  useNeed(["audit", "jobs", "mesh", "models", "network"]);
  const a = audit.use();
  const j = jobs.use().data;
  const loc = useLocation();
  const names = useNames();
  const rows = useActivityRows();
  const [scope, setScope] = useState("all");
  const [q, setQ] = useState("");
  const [device, setDevice] = useState("");
  const [copied, setCopied] = useState("");
  const [codesOpen, setCodesOpen] = useState(false);

  const jobList = j && j !== "unsupported" ? [...j.active, ...j.recent] : [];
  const jobIds = new Set(jobList.flatMap((x) => [x.id, ...(x.request_id ? [x.request_id] : [])]));
  const jobOf = (r: ActivityRow) => r.job?.id ?? (r.requestId ? jobList.find((x) => x.request_id === r.requestId || x.id === r.requestId)?.id : undefined);

  const devices = [...new Set(rows.map((r) => r.peerId ?? "self"))];
  const byDevice = device ? rows.filter((r) => (r.peerId ?? "self") === device) : rows;
  const shown = byDevice.filter(SCOPES.find((s) => s.id === scope)!.keep);
  const selected = rows.find((r) => r.key === loc.id) ?? null;
  const columns = activityColumns(names);

  async function copyDiagnostics() {
    const app = { app: "common-desktop 0.1.0", service_url: getBaseUrl(), service_spawned_by_app: d.spawned, reachable: d.reachable, generated_at: new Date().toISOString() };
    let report: Record<string, unknown>;
    let what: string;
    try {
      // The service's bundle is redacted on its side (keys, tokens, hex ids, prompts) before it reaches the app.
      report = { ...app, ...(await api.diagnostics()) };
      what = "the service's redacted bundle";
    } catch (e) {
      if (e instanceof ApiError && e.code !== "not_implemented" && e.status !== 404) {
        setCopied(`Could not build the bundle: ${explain(e).title}`);
        return;
      }
      // No bundle route (N5): status and hardware only.
      report = { ...app };
      try {
        report.status = await api.status();
      } catch (err) {
        report.status_error = String(err);
      }
      try {
        report.hardware = await api.hardware();
      } catch (err) {
        report.hardware_error = String(err);
      }
      what = "status and hardware details";
    }
    try {
      await navigator.clipboard.writeText(JSON.stringify(report, null, 2));
      setCopied(`Diagnostics copied. They contain ${what}, never your messages.`);
    } catch {
      setCopied("Could not access the clipboard.");
    }
  }

  const showDevice = (r: ActivityRow | null) => r?.peerId && go("devices", r.peerId);
  const showJob = (r: ActivityRow | null) => {
    const id = r && jobOf(r);
    if (id) go("jobs", id);
  };
  const menu = (r: ActivityRow): MenuItem[] => [
    { label: "Copy event", action: () => void navigator.clipboard?.writeText(eventJson(r)).catch(() => undefined) },
    { separator: true },
    { label: "Show device", enabled: !!r.peerId, action: () => showDevice(r) },
    { label: "Show job", enabled: !!jobOf(r), action: () => showJob(r) },
  ];

  const empty =
    rows.length === 0 ? (
      <EmptyState title="No activity yet." reason="Pairing, connections and requests to or from other devices are recorded here; local jobs appear until the service restarts." />
    ) : (
      <EmptyState
        title="No activity matches."
        reason={q ? `Nothing in this scope matches "${q}".` : "Nothing in this scope."}
        action={
          <button type="button" className="btn" onClick={() => (setQ(""), setScope("all"), setDevice(""))}>
            Show all activity
          </button>
        }
      />
    );

  return (
    <SectionFrame
      title="Activity"
      state={frameState(d, a)}
      actions={<ToolButton icon="copy" label="Copy diagnostics" onClick={() => void copyDiagnostics()} />}
      selectionActions={
        <>
          <ToolButton icon="monitor" label="Show device" disabledReason={!selected ? "Select an event first." : !selected.peerId ? "This event involves no other device." : null} onClick={() => showDevice(selected)} />
          <ToolButton icon="list-checks" label="Show job" disabledReason={!selected ? "Select an event first." : !jobOf(selected) ? "No job held by the service matches this event." : null} onClick={() => showJob(selected)} />
        </>
      }
      more={[{ label: "Error codes…", action: () => setCodesOpen(true) }]}
      scope={
        <>
          <ScopeBar label="Activity scope" items={SCOPES.map((s) => ({ id: s.id, label: s.label, count: byDevice.filter(s.keep).length }))} value={scope} onChange={setScope} />
          <label className="activity-device">
            <span className="visually-hidden">Device</span>
            <select value={device} onChange={(e) => setDevice(e.target.value)}>
              <option value="">All devices</option>
              {devices.map((id) => (
                <option key={id} value={id}>
                  {id === "self" ? "This computer" : names.device(id)}
                </option>
              ))}
            </select>
          </label>
        </>
      }
      filter={{ value: q, onChange: setQ, label: "Filter activity" }}
      banner={
        <>
          {scope === "execution" && (
            <p className="frame-banner">{j === "unsupported" ? "This service version does not report local jobs." : "Local jobs since the service started; kept in memory (last 256)."}</p>
          )}
          {copied && (
            <p className="frame-banner" role="status">
              {copied}
            </p>
          )}
        </>
      }
      inspector={
        <Inspector
          id="activity"
          label="Event details"
          hasSelection={!!selected}
          empty="Select an event to see its details."
          tabs={[{ id: "details", label: "Details", render: () => selected && <Details r={selected} names={names} jobIds={jobIds} /> }]}
        />
      }
    >
      <DataTable
        id="activity"
        label="Activity"
        columns={columns}
        rows={shown}
        rowKey={(r) => r.key}
        selected={selected?.key ?? null}
        onSelect={(k) => select(k)}
        onActivate={() => openInspector()}
        onCopy={eventJson}
        menu={menu}
        filter={q}
        defaultSort={{ id: "time", desc: true }}
        empty={empty}
      />
      <Dialog open={codesOpen} label="Error codes" onClose={() => setCodesOpen(false)} className="error-codes">
        <h2>Error codes</h2>
        <div className="error-codes-scroll">
          <table className="error-codes-table">
            <thead>
              <tr>
                <th scope="col">Code</th>
                <th scope="col">What happened</th>
                <th scope="col">What to do</th>
              </tr>
            </thead>
            <tbody>
              {Object.entries(ERROR_CODES).map(([code, e]) => (
                <tr key={code}>
                  <td>
                    <code>{code}</code>
                  </td>
                  <td>{e.title}</td>
                  <td>{e.next_step}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="dialog-actions">
          <button type="button" className="btn" data-autofocus onClick={() => setCodesOpen(false)}>
            Close
          </button>
        </div>
      </Dialog>
    </SectionFrame>
  );
}
