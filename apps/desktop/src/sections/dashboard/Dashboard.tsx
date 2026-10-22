// Dashboard (plan §2.1): one column of dense tables built from the section components, each under a caption that links
// to its section. No inspector, no cards, no sparklines; the status bar already shows this computer on every screen.
import "./dashboard.css";
import { useEffect, useId, useState, type ReactNode } from "react";
import { api, type Model } from "../../api/client";
import { explain } from "../../api/errors";
import { refreshStatus } from "../../daemon";
import { humanBytes } from "../../format";
import { go, href } from "../../router";
import { dismissAttention, useAttention } from "../../shell/attention";
import { useDeviceRows, type DeviceRow } from "../../state/devices";
import { audit, dialing, jobs, mesh, models, network, pulls, refreshFresh, useNeed } from "../../state/live";
import { daemon, lastError, settings } from "../../state/stores";
import { DataTable, type Column } from "../../ui/DataTable";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { FirewallNotice, refreshFirewall, useFirewallMissing } from "../../ui/FirewallNotice";
import { Icon } from "../../ui/icons";
import { EmptyState, StateBadge, ToolButton } from "../../ui/primitives";
import { frameState, SectionFrame } from "../../ui/SectionFrame";
import { Tip } from "../../ui/Tip";
import { activityColumns, useActivityRows, useNames } from "../activity/Activity";
import { connectBlocked, connectTo, deviceMenu, useDeviceColumns } from "../devices/table";
import { isActive, useJobs } from "../jobs/Jobs";

/** Columns shown in their section but hidden here by default (the column chooser shows them), so blocks fit the
 * default 1100 px window. */
const hide = <R,>(columns: Column<R>[], ids: string[]): Column<R>[] => columns.map((c) => (ids.includes(c.id) ? { ...c, hidden: true } : c));

function Block({ title, to, actions, children }: { title: string; to: string; actions?: ReactNode; children: ReactNode }) {
  const id = useId();
  return (
    <section className="dash-block" aria-labelledby={id}>
      <div className="dash-caption">
        <h2 id={id}>
          <a href={to}>{title}</a>
        </h2>
        {actions}
      </div>
      {children}
    </section>
  );
}

/** A small fix button: a supported action that is not possible now stays focusable with its reason. */
function Fix({ label, onClick, reason }: { label: string; onClick: () => void; reason?: string | null }) {
  const b = (
    <button type="button" className="btn btn-sm" aria-disabled={reason ? "true" : undefined} onClick={() => !reason && onClick()}>
      {label}
    </button>
  );
  return reason ? <Tip text={reason}>{b}</Tip> : b;
}

// ---- 1. Attention ------------------------------------------------------------------------------------------------

function Attention({ rows }: { rows: DeviceRow[] }) {
  const items = useAttention();
  const err = lastError.use();
  const fw = useFirewallMissing();
  // connectBlocked reads these: re-render when they change.
  dialing.use();
  network.use();
  const unreachable = rows.filter((r) => r.peer && r.state.word === "Unreachable");
  if (!items.length && !unreachable.length && !fw && !err) return null;
  return (
    <section className="dash-block" aria-labelledby="dash-attention">
      <div className="dash-caption">
        <h2 id="dash-attention">Needs attention</h2>
      </div>
      <ul className="dash-attention">
        {items.map((a) => (
          <li key={a.id} className={`tone-${a.tone}`}>
            <Icon name={a.tone === "danger" ? "circle-x" : "circle-alert"} />
            <span className="dash-text">{a.text}</span>
            {a.action && <Fix label={a.action.label} onClick={a.action.run} />}
            {a.dismissable && <Fix label="Dismiss" onClick={() => dismissAttention(a.id)} />}
          </li>
        ))}
        {unreachable.map((r) => (
          <li key={`unreachable:${r.id}`} className="tone-warn">
            <Icon name="circle-alert" />
            <span className="dash-text">
              {r.name} is unreachable{r.state.detail ? `: ${r.state.detail}` : ""}
            </span>
            <Fix label={`Connect to ${r.name}`} onClick={() => void connectTo(r.peer!)} reason={connectBlocked(r.peer!, r.name)} />
          </li>
        ))}
        {fw && (
          <li className="dash-notice">
            <FirewallNotice compact />
          </li>
        )}
        {err && (
          <li className="dash-notice">
            <p className="dash-label">Last problem</p>
            <ErrorNotice error={err} onDismiss={() => lastError.set(null)} />
          </li>
        )}
      </ul>
    </section>
  );
}

// ---- 2. First run ------------------------------------------------------------------------------------------------

function FirstRun({ catalog }: { catalog: Model[] }) {
  const p = pulls.use();
  const loadedId = daemon.use().status?.loaded?.model_id ?? null;
  const [running, setRunning] = useState(false);
  const rec = catalog.find((m) => m.tags.includes("recommended") && m.fit.verdict === "fits");
  const anyDownloaded = catalog.some((m) => m.downloaded);
  // Shown until something is downloaded; a download of the recommended model started here keeps it, for Run.
  if (anyDownloaded && !(rec && p[rec.id])) return null;

  async function run(m: Model) {
    setRunning(true);
    try {
      const s = settings.get();
      await api.load(m.id, { device: s.device, ...(s.n_ctx ? { n_ctx: s.n_ctx } : {}) });
      await refreshStatus();
      void refreshFresh("models");
      go("chat");
    } catch (e) {
      lastError.set(explain(e));
    } finally {
      setRunning(false);
    }
  }

  let body: ReactNode;
  if (!rec)
    body = (
      <span className="dash-text">
        No recommended model fits this computer. <a href={href("models")}>Browse all models</a>
      </span>
    );
  else {
    const pull = p[rec.id];
    const pct = pull?.ev && pull.ev.total_bytes ? Math.floor((pull.ev.downloaded_bytes / pull.ev.total_bytes) * 100) : null;
    const fit = rec.fit.verdict === "fits" ? `Fits · ${humanBytes(rec.fit.headroom_bytes)} headroom` : "";
    body = (
      <>
        <span className="dash-text">
          <strong>{rec.name}</strong>
          <span className="note">
            {humanBytes(rec.bytes)} · {rec.license || "licence not stated"} · {fit}
          </span>
        </span>
        {pull?.state === "downloading" ? (
          <StateBadge tone="active" icon="loader" label={pct == null ? "Downloading" : `Downloading ${pct}%`} />
        ) : loadedId === rec.id ? (
          <a className="btn btn-sm btn-primary" href={href("chat")}>
            Open chat
          </a>
        ) : rec.downloaded ? (
          <button type="button" className="btn btn-sm btn-primary" aria-disabled={running ? "true" : undefined} onClick={() => !running && void run(rec)}>
            {running ? "Loading…" : "Run"}
          </button>
        ) : (
          <a className="btn btn-sm btn-primary" href={href("models", null, { confirm: rec.id })}>
            Download…
          </a>
        )}
      </>
    );
  }
  return (
    <Block title="Get started" to={href("models")}>
      <div className="dash-firstrun">{body}</div>
    </Block>
  );
}

// ---- 3. Computers ------------------------------------------------------------------------------------------------

/** Narrower than in Devices, so the default columns fit the default 1100 px window beside the rail and the padding. */
const DASH_WIDTH: Record<string, number> = { name: 150, state: 130, use: 100, hardware: 130 };

/** The Devices table and row menu (B's `table.tsx`), limited to this computer and paired devices. */
function Computers({ rows }: { rows: DeviceRow[] }) {
  const columns = useDeviceColumns().map((c) => (DASH_WIDTH[c.id] ? { ...c, width: DASH_WIDTH[c.id] } : c));
  const [sel, setSel] = useState<string | null>(null);
  const shown = rows.filter((r) => r.trust === "self" || r.trust === "trusted");
  return (
    <Block title="Computers" to={href("devices")}>
      <DataTable
        id="dashboard.computers"
        label="Computers"
        columns={columns}
        rows={shown}
        rowKey={(r) => r.key}
        selected={sel}
        onSelect={setSel}
        onActivate={(r) => go("devices", r.key)}
        onCopy={(r) => r.id}
        menu={deviceMenu}
        pinned={(r) => r.trust === "self"}
        empty={<EmptyState title="No computers to show" />}
      />
      {shown.length === 1 && (
        <p className="dash-note">
          No paired devices. <a href={href("devices", null, { pair: "1" })}>Pair a device…</a>
        </p>
      )}
    </Block>
  );
}

// ---- 4. Running now ----------------------------------------------------------------------------------------------

const RECENT = 5;

/** The Jobs table, rows, menu and Stop (C's `useJobs`): active jobs, then the last 5 finished. */
function RunningNow() {
  const j = useJobs();
  const q = daemon.use().status?.queue;
  const [sel, setSel] = useState<string | null>(null);
  if (j.data === "unsupported")
    return (
      <Block title="Running now" to={href("jobs")}>
        <p className="dash-note">Job list not reported by this service version.{q ? ` In use ${q.inflight}/${q.max_concurrent} · Waiting ${q.queued}.` : ""}</p>
      </Block>
    );
  const active = j.rows.filter(isActive);
  const rows = [...active, ...j.rows.filter((x) => !isActive(x)).slice(0, RECENT)];
  const picked = rows.find((x) => x.id === sel) ?? null;
  return (
    <Block title="Running now" to={href("jobs")} actions={<ToolButton icon="stop" label="Stop" onClick={() => picked && j.stop(picked)} disabledReason={j.stopReason(picked)} />}>
      {/* Only finished rows below: the title alone would claim they are running. */}
      {rows.length > 0 && !active.length && <p className="dash-note">Nothing is running. Recently finished:</p>}
      <DataTable
        id="dashboard.jobs"
        label="Running now"
        columns={hide(j.columns, ["wait", "ttft"])}
        rows={rows}
        defaultSort={null}
        rowKey={(x) => x.id}
        selected={sel}
        onSelect={setSel}
        onActivate={(x) => go("jobs", x.id)}
        onCopy={(x) => x.id}
        menu={j.menu}
        empty={<p className="dash-note">Nothing is running. Requests from Chat, other programs on this computer and paired devices appear here.</p>}
      />
    </Block>
  );
}

// ---- 5. Recent activity ------------------------------------------------------------------------------------------

/** The Activity table (D's rows and columns: §2.6 mapping, duplicates collapsed, local jobs), newest 8. */
function RecentActivity() {
  const names = useNames();
  const rows = useActivityRows();
  const [sel, setSel] = useState<string | null>(null);
  return (
    <Block title="Recent activity" to={href("activity")}>
      <DataTable
        id="dashboard.activity"
        label="Recent activity"
        columns={hide(activityColumns(names), ["code"])}
        rows={rows}
        rowKey={(r) => r.key}
        selected={sel}
        onSelect={setSel}
        onActivate={(r) => go("activity", r.key)}
        onCopy={(r) => JSON.stringify(r.job ?? (r.events.length === 1 ? r.events[0] : r.events))}
        defaultSort={{ id: "time", desc: true }}
        limit={8}
        empty={<p className="dash-note">No activity yet. Pairing, connections and requests to or from other devices are recorded here; local jobs appear until the service restarts.</p>}
      />
    </Block>
  );
}

export default function Dashboard() {
  const d = daemon.use();
  const state = frameState(d, network.use(), mesh.use(), models.use(), jobs.use(), audit.use());
  const rows = useDeviceRows();
  const catalog = models.use().data;
  useNeed(["jobs", "models"]);
  useEffect(() => void refreshFirewall(), []);
  return (
    <SectionFrame title="Dashboard" state={state}>
      <div className="dash">
        <Attention rows={rows} />
        {catalog && <FirstRun catalog={catalog} />}
        <Computers rows={rows} />
        <RunningNow />
        <RecentActivity />
      </div>
    </SectionFrame>
  );
}
