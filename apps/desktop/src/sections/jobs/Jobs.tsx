// Jobs (plan §2.4): every request the service runs, sends to a paired device or serves for one, from E3
// `GET /api/jobs`. Real states only: no queue position, no reorder, no priority, and Stop only on this app's own
// streams (there is no cancel-by-id route, N1). Owner C.
import "./jobs.css";
import { useContext, useRef, useState, type CSSProperties } from "react";
import { shortLabel, type Group, type Job, type Model, type Peer } from "../../api/client";
import { mapError } from "../../api/errors";
import { ago, clock, humanBytes, num } from "../../format";
import { go, select, useLocation } from "../../router";
import { useNow } from "../../shell/attention";
import { groups as groupsLive, hardware, jobs as jobsLive, metrics, mesh, models, network, ownStreams, samples, tokenRates, useNeed } from "../../state/live";
import { chatHistory, daemon, type StoredMessage } from "../../state/stores";
import type { MenuItem } from "../../ui/contextMenu";
import { DataTable, type Column } from "../../ui/DataTable";
import { ErrorNotice } from "../../ui/ErrorNotice";
import type { IconName } from "../../ui/icons";
import { Inspector, openInspector } from "../../ui/Inspector";
import { EmptyState, KV, ScopeBar, StateBadge, ToolButton, Unavailable, type Tone } from "../../ui/primitives";
import { FrameStale, frameState, SectionFrame } from "../../ui/SectionFrame";
import { Sparkline } from "../../ui/Sparkline";

/** The states that mean a job has ended. Anything else, including a state added by a newer service,
 * is not known to have ended, so it stays with the active jobs (never shown as an outcome). */
const FINISHED = new Set<string>(["done", "error", "cancelled"]);
export const isActive = (j: Job) => !FINISHED.has(j.state);
const knownState = (j: Job) => FINISHED.has(j.state) || ["queued", "connecting", "loading", "running"].includes(j.state);

const CONNECTING = { word: "Connecting to the device", tone: "active", icon: "loader", rank: 2 } as const;

/** The State word (plan §2.4). `rank` is the State sort: running, loading, connecting and queued, error, cancelled, done. */
export function jobState(j: Job): { word: string; tone: Tone; icon: IconName; rank: number; detail?: string } {
  switch (j.state) {
    case "running":
      return { word: "Running", tone: "active", icon: "play", rank: 0 };
    case "loading":
      return { word: "Loading model", tone: "active", icon: "loader", rank: 1 };
    case "connecting":
      return CONNECTING;
    case "queued":
      // A service from before `connecting` reports a request to another device (which takes no slot here) as queued while it reaches that device.
      return j.target.startsWith("peer:") ? CONNECTING : { word: "Waiting for a slot", tone: "neutral", icon: "clock", rank: 2 };
    case "error": {
      const e = mapError(j.error_code ?? "internal", "");
      return { word: `Failed: ${e.title}`, tone: "danger", icon: "circle-x", rank: 3, detail: e.next_step };
    }
    case "cancelled":
      return j.error_code === "client_gone"
        ? { word: "Requester disconnected", tone: "neutral", icon: "square", rank: 4 }
        : { word: "Stopped", tone: "neutral", icon: "square", rank: 4 };
    case "done":
      return { word: "Done", tone: "ok", icon: "circle-check", rank: 5, detail: j.finish_reason ? `Finish reason: ${j.finish_reason}` : undefined };
    default:
      // A state added by a newer service: shown as reported, never guessed (and never a crash).
      return { word: `State not known to this app: ${String((j as Job).state)}`, tone: "neutral", icon: "circle-alert", rank: 2 };
  }
}

/** A request this app sent: its client ref is one of this app's streams or chat replies (or the reply's job id matches). */
export function isOwn(j: Job, streams: Record<string, unknown>, messages: StoredMessage[]): boolean {
  if (j.origin !== "local") return false;
  if (j.client_ref && (j.client_ref in streams || messages.some((m) => m.client_ref === j.client_ref))) return true;
  return messages.some((m) => m.job_id === j.id);
}

const peerOf = (t: string) => (t.startsWith("peer:") ? t.slice(5) : null);
const peerName = (peers: Peer[], hex: string) => {
  const p = peers.find((x) => x.id === hex);
  return p ? p.name || p.short_label : shortLabel(hex);
};
/** Milliseconds as "210 ms" or "4.0 s"; "—" when not measured. */
export const ms = (v: number | null | undefined) => (v == null ? "—" : v < 1000 ? `${num(v)} ms` : `${num(v / 1000, 1)} s`);
const time = (j: Job) => Date.parse(j.started_at_utc ?? j.created_at_utc);
/** What a job's TTFT measures (the registry times it here, from the request's arrival). Not an engine figure. */
const TTFT_BASIS = "From arrival here: includes waiting, loading and, for another device, the network.";
const KIND: Record<Job["kind"], string> = { chat: "Chat", completion: "Completion", embeddings: "Embeddings", served_for_peer: "Served for device" };

/**
 * Job rows, columns and actions, shared with the Dashboard's "Running now" block. Rows: active by state order
 * (running, loading, connecting and queued; registry order within), then finished, newest first whatever their outcome (plan §2.4):
 * the tables keep this order until the user sorts. Live tok/s on an active row is Δtokens/Δt between this hook's last
 * two job reads.
 */
export function useJobs() {
  const live = jobsLive.use();
  const peers = mesh.use().data?.peers ?? [];
  const catalog = models.use().data ?? [];
  const streams = ownStreams.use();
  const { messages } = chatHistory.use();
  const data = live.data;
  const supported = data != null && data !== "unsupported";
  // Every second while something runs (elapsed, "so far"); otherwise only relative start times move.
  const now = useNow(supported && data.active.length > 0 ? 1000 : 15000, supported);

  // ponytail: per-job samples recorded during render, guarded by the read time; a store if a third reader appears.
  const hist = useRef(new Map<string, { at: number; n: number }[]>());
  const seenAt = useRef<number | null>(null);
  if (supported && live.at != null && live.at !== seenAt.current) {
    seenAt.current = live.at;
    const next = new Map<string, { at: number; n: number }[]>();
    for (const j of data.active) if (j.completion_tokens != null) next.set(j.id, [...(hist.current.get(j.id) ?? []), { at: live.at, n: j.completion_tokens }].slice(-2));
    hist.current = next;
  }
  const rate = (j: Job): number | null => {
    if (!isActive(j)) return j.decode_tokens_per_second ?? null;
    const [a, b] = hist.current.get(j.id) ?? [];
    return a && b && b.at > a.at ? ((b.n - a.n) * 1000) / (b.at - a.at) : null;
  };

  const rows: Job[] = supported ? [...[...data.active].sort((a, b) => jobState(a).rank - jobState(b).rank), ...[...data.recent].sort((a, b) => Date.parse(b.finished_at_utc ?? b.created_at_utc) - Date.parse(a.finished_at_utc ?? a.created_at_utc))] : [];
  const own = (j: Job) => isOwn(j, streams, messages);
  const ownStream = (j: Job) => (isActive(j) && j.client_ref ? (streams[j.client_ref] ?? null) : null);
  const modelName = (id: string) => catalog.find((m: Model) => m.id === id)?.name ?? id;
  const runsOn = (j: Job) => {
    const p = peerOf(j.target);
    return p ? peerName(peers, p) : j.device_actual ? `This computer · ${j.device_actual}` : "This computer";
  };
  const requestedBy = (j: Job) => {
    const p = peerOf(j.origin);
    return p ? peerName(peers, p) : own(j) ? "This app" : "This computer (another API client or not matched)";
  };
  const soFar = (j: Job) => (isActive(j) ? "so far" : null);

  const columns: Column<Job>[] = [
    {
      id: "state",
      header: "State",
      width: 150,
      live: true,
      sort: (j) => jobState(j).rank,
      text: (j) => jobState(j).word,
      cell: (j) => {
        const s = jobState(j);
        return <StateBadge tone={s.tone} icon={s.icon} label={s.word} />;
      },
      detail: (j) => jobState(j).detail ?? null,
    },
    { id: "model", header: "Model", width: 140, sort: (j) => modelName(j.model_id), cell: (j) => modelName(j.model_id), detail: (j) => j.model_id },
    { id: "runs", header: "Runs on", width: 130, sort: runsOn, cell: runsOn, note: (j) => (peerOf(j.target) ? "GPU/CPU not reported" : null), detail: runsOn },
    { id: "by", header: "Requested by", width: 120, sort: requestedBy, cell: requestedBy, detail: (j) => (peerOf(j.origin) ? "Served for that device" : requestedBy(j)) },
    { id: "started", header: "Started", width: 76, sort: time, cell: (j) => ago(j.started_at_utc ?? j.created_at_utc, now), detail: (j) => `${clock(j.started_at_utc ?? j.created_at_utc)} (${j.started_at_utc ?? j.created_at_utc})` },
    { id: "wait", header: "Wait", width: 60, align: "end", sort: (j) => j.queue_ms ?? null, cell: (j) => ms(j.queue_ms) },
    { id: "ttft", header: "TTFT", width: 64, align: "end", sort: (j) => j.ttft_ms ?? null, cell: (j) => ms(j.ttft_ms), detail: (j) => (j.ttft_ms == null ? null : TTFT_BASIS) },
    {
      id: "duration",
      header: "Duration",
      width: 90,
      align: "end",
      live: true,
      sort: (j) => (isActive(j) ? (j.started_at_utc ? now - Date.parse(j.started_at_utc) : null) : (j.duration_ms ?? null)),
      cell: (j) => (isActive(j) ? (j.started_at_utc ? ms(Math.max(0, now - Date.parse(j.started_at_utc))) : "—") : ms(j.duration_ms)),
      note: (j) => (isActive(j) ? (j.started_at_utc ? "so far" : "not started") : null),
    },
    { id: "tokens", header: "Tokens", width: 84, align: "end", live: true, sort: (j) => j.completion_tokens ?? null, cell: (j) => num(j.completion_tokens), note: soFar, detail: (j) => (j.prompt_tokens != null ? `Prompt: ${num(j.prompt_tokens)} tokens` : null) },
    { id: "tps", header: "tok/s", width: 76, align: "end", live: true, sort: rate, cell: (j) => num(rate(j), 1), note: soFar },
    { id: "result", header: "Result", hidden: true, sort: (j) => j.finish_reason ?? j.error_code ?? null, cell: (j) => j.finish_reason ?? j.error_code ?? "—" },
    { id: "kind", header: "Kind", hidden: true, sort: (j) => KIND[j.kind], cell: (j) => KIND[j.kind] },
    { id: "id", header: "Job ID", hidden: true, sort: (j) => j.id, cell: (j) => <span className="mono">{j.id}</span>, text: (j) => j.id },
    { id: "request", header: "Request ID", hidden: true, sort: (j) => j.request_id ?? null, cell: (j) => <span className="mono">{j.request_id ?? "—"}</span>, text: (j) => j.request_id ?? "" },
    { id: "ref", header: "Client ref", hidden: true, sort: (j) => j.client_ref ?? null, cell: (j) => <span className="mono">{j.client_ref ?? "—"}</span>, text: (j) => j.client_ref ?? "" },
  ];

  /** Why Stop is not possible on `j` (null = it is). */
  const stopReason = (j: Job | null) => (!j ? "Select a job first" : !isActive(j) ? "It has finished" : !knownState(j) ? "Its state is not known to this app" : !ownStream(j) ? "Only requests started in this app can be stopped" : null);
  const stop = (j: Job) => ownStream(j)?.stop();
  // Chat's `?reply=` names a reply by its job id, or by its client ref when no chunk (and so no id) arrived.
  const openChat = (j: Job) => go("chat", null, { reply: messages.some((m) => m.job_id === j.id) ? j.id : (j.client_ref ?? j.id) });
  const deviceOf = (j: Job) => peerOf(j.target) ?? peerOf(j.origin);
  const menu = (j: Job): MenuItem[] => [
    { label: "Stop", action: () => stop(j), enabled: !stopReason(j) },
    { label: "Open in Chat", action: () => openChat(j), enabled: own(j) },
    { label: "Show device", action: () => go("devices", deviceOf(j)), enabled: !!deviceOf(j) },
    { separator: true },
    { label: "Copy ID", action: () => void navigator.clipboard?.writeText(j.id) },
  ];
  // Filter: model, device and id (plan §2.4).
  const filterText = (j: Job) => [j.model_id, modelName(j.model_id), runsOn(j), requestedBy(j), j.id, j.request_id ?? "", j.client_ref ?? ""].join(" ");

  return { live, data, supported, rows, columns, own, stop, stopReason, openChat, deviceOf, menu, filterText, runsOn, requestedBy, modelName, rate, peers };
}

type Scope = "all" | "active" | "recent" | "failed" | "served";
const SCOPES: { id: Scope; label: string; test: (j: Job) => boolean; empty: string }[] = [
  { id: "all", label: "All", test: () => true, empty: "Nothing is running. Requests from Chat, other programs on this computer and paired devices appear here." },
  { id: "active", label: "Active", test: isActive, empty: "Nothing is running. Requests from Chat, other programs on this computer and paired devices appear here." },
  { id: "recent", label: "Recent", test: (j) => !isActive(j), empty: "No finished jobs since the service started." },
  { id: "failed", label: "Failed", test: (j) => j.state === "error", empty: "No failed jobs since the service started." },
  { id: "served", label: "Served for devices", test: (j) => j.kind === "served_for_peer", empty: "Nothing served for paired devices since the service started." },
];

/** `Slots 1 of 4 in use · devices may use up to 3 · waiting 0 of 32 · rejected since service start: 2 (…)`. */
function SlotStrip() {
  const q = daemon.use().status?.queue;
  const slots = network.use().data?.peer_slots;
  const m = metrics.use().data;
  const full = m?.['ucompute_requests_rejected_total{reason="queue_full"}'];
  const timedOut = m?.['ucompute_requests_rejected_total{reason="queue_timeout"}'];
  const rejected = full == null || timedOut == null ? "rejected since service start: not reported" : `rejected since service start: ${num(full + timedOut)} (queue full ${num(full)}, timed out ${num(timedOut)})`;
  return (
    <p className="jobs-strip">
      {q ? `Slots ${num(q.inflight)} of ${num(q.max_concurrent)} in use` : "Slots: not reported"}
      {slots != null && ` · devices may use up to ${num(slots)}`}
      {q && ` · waiting ${num(q.queued)} of ${num(q.max_queue)}`} · {rejected}
    </p>
  );
}

/** Tokens/s by where they ran, from E3 live counts sampled on every jobs read while the app runs (poll.ts: jobs is always polled). */
function TokenRate() {
  const s = samples.use().tokens;
  const r = tokenRates(s);
  return (
    <figure className="jobs-rate">
      <Sparkline
        label="Tokens per second"
        series={[
          { label: "On this computer", values: r.local },
          { label: "Sent to devices", values: r.sent },
          { label: "Served for devices", values: r.served },
        ]}
      />
      <figcaption className="note">
        <span className="spark-key spark-key-0">This computer</span> <span className="spark-key spark-key-1">Sent</span> <span className="spark-key spark-key-2">Served</span> · Tokens/s, sampled about
        every 5 s while this app was open and the service answering, last ≤ 1 h.
      </figcaption>
    </figure>
  );
}

export default function Jobs() {
  useNeed(["jobs", "metrics"]);
  const d = daemon.use();
  const loc = useLocation();
  const j = useJobs();
  const [q, setQ] = useState("");
  const scope = (SCOPES.find((s) => s.id === loc.params.get("scope")) ?? SCOPES[0]).id;
  const setScope = (id: string) => {
    const params = new URLSearchParams(loc.params);
    if (id === "all") params.delete("scope");
    else params.set("scope", id);
    select(loc.id, params);
  };

  if (j.data === "unsupported")
    return (
      <SectionFrame title="Jobs" state={frameState(d, j.live)}>
        <SlotStrip />
        <EmptyState title="This service version does not report jobs" reason="Update the service to see each request; the slot counts above are all it reports." />
      </SectionFrame>
    );

  const sel = j.rows.find((x) => x.id === loc.id) ?? null;
  const gone = loc.id != null && j.supported && !sel;
  const s = SCOPES.find((x) => x.id === scope)!;
  const rows = j.rows.filter(s.test);
  const stopReason = j.stopReason(sel);
  const device = sel ? j.deviceOf(sel) : null;

  return (
    <SectionFrame
      title="Jobs"
      state={frameState(d, j.live)}
      selectionActions={
        <>
          <ToolButton icon="stop" label="Stop" disabledReason={stopReason} onClick={() => sel && j.stop(sel)} />
          <ToolButton icon="message-square" label="Open in Chat" disabledReason={!sel ? "Select a job first" : !j.own(sel) ? "Only requests from this app's Chat have a reply there" : null} onClick={() => sel && j.openChat(sel)} />
          <ToolButton icon="monitor" label="Show device" disabledReason={!sel ? "Select a job first" : !device ? "It ran on this computer for this computer" : null} onClick={() => device && go("devices", device)} />
        </>
      }
      scope={<ScopeBar label="Show jobs" items={SCOPES.map((x) => ({ id: x.id, label: x.label, count: j.rows.filter(x.test).length }))} value={scope} onChange={setScope} />}
      filter={{ value: q, onChange: setQ, label: "Filter jobs", placeholder: "Model, device or id" }}
      banner={
        <div className="jobs-head">
          <SlotStrip />
          <TokenRate />
        </div>
      }
      inspector={
        <Inspector
          id="jobs"
          label="Job details"
          hasSelection={!!sel}
          empty={gone ? "No longer held by the service (cleared at restart or past 256)" : "Select a job to see its details"}
          tabs={
            sel
              ? [
                  { id: "overview", label: "Overview", render: () => <Overview job={sel} x={j} /> },
                  { id: "timing", label: "Timing & route", render: () => <TimingRoute job={sel} x={j} /> },
                ]
              : []
          }
        />
      }
    >
      <DataTable
        id="jobs"
        label="Jobs"
        columns={j.columns}
        rows={rows}
        rowKey={(x) => x.id}
        selected={sel ? sel.id : null}
        onSelect={(k) => select(k, loc.params)}
        onActivate={() => openInspector()}
        onCopy={(x) => x.id}
        menu={j.menu}
        filter={q}
        filterText={j.filterText}
        defaultSort={null}
        empty={
          q.trim() && rows.length > 0 ? (
            <EmptyState
              title={`No jobs match "${q.trim()}"`}
              action={
                <button type="button" className="btn" onClick={() => setQ("")}>
                  Clear filter
                </button>
              }
            />
          ) : (
            <EmptyState title={s.empty} />
          )
        }
      />
    </SectionFrame>
  );
}

type JobsCtx = ReturnType<typeof useJobs>;

function Overview({ job, x }: { job: Job; x: JobsCtx }) {
  const s = jobState(job);
  // Busy or stale: an active job may have finished since, so its state and counts are not shown as live (the table
  // blanks the same columns). A finished job's values no longer change.
  const nr = useContext(FrameStale) && isActive(job) ? <Unavailable reason="not refreshed" /> : null;
  return (
    <>
      {job.state === "error" && <ErrorNotice error={mapError(job.error_code ?? "internal", job.error_code ?? "")} />}
      <KV
        items={[
          { k: "State", v: nr ?? <StateBadge tone={s.tone} icon={s.icon} label={s.word} /> },
          { k: "Model", v: x.modelName(job.model_id), note: job.model_id },
          { k: "Runs on", v: x.runsOn(job), note: peerOf(job.target) ? "GPU/CPU not reported" : undefined },
          { k: "Requested by", v: x.requestedBy(job) },
          { k: "Kind", v: KIND[job.kind] },
          { k: "Created", v: clock(job.created_at_utc), note: job.created_at_utc },
          { k: "Started", v: clock(job.started_at_utc), note: job.started_at_utc ?? undefined },
          { k: "First token", v: clock(job.first_token_at_utc) },
          { k: "Finished", v: clock(job.finished_at_utc) },
          { k: "Wait", v: ms(job.queue_ms) },
          { k: "Load", v: ms(job.load_ms) },
          { k: "Time to first token", v: ms(job.ttft_ms), note: job.ttft_ms == null ? undefined : TTFT_BASIS },
          { k: "Prompt", v: ms(job.prompt_ms) },
          { k: "Decode", v: ms(job.decode_ms) },
          { k: "Duration", v: ms(job.duration_ms) },
          { k: "Prompt tokens", v: num(job.prompt_tokens) },
          { k: "Completion tokens", v: nr ?? num(job.completion_tokens), note: isActive(job) && !nr ? "so far" : undefined },
          { k: "Tokens per second", v: nr ?? num(x.rate(job), 1), note: isActive(job) && !nr ? "so far" : undefined },
          { k: "Finish reason", v: job.finish_reason ?? "—" },
          { k: "Error code", v: job.error_code ?? "—" },
          { k: "Job ID", v: <span className="mono">{job.id}</span> },
          { k: "Request ID", v: <span className="mono">{job.request_id ?? "—"}</span> },
          { k: "Client ref", v: <span className="mono">{job.client_ref ?? "—"}</span> },
        ]}
      />
    </>
  );
}

/** Measured phases only: an unmeasured (null) phase is omitted, never estimated. */
export function PhaseBar({ job }: { job: Job }) {
  const all: [string, number | null | undefined][] = [
    ["Wait", job.queue_ms],
    ["Load", job.load_ms],
    ["Prompt", job.prompt_ms],
    ["Decode", job.decode_ms],
  ];
  const phases = all.filter((p): p is [string, number] => p[1] != null);
  if (phases.length === 0) return <p className="muted">No phase has been measured yet.</p>;
  const sum = phases.reduce((a, [, v]) => a + v, 0);
  const scale = Math.max(job.duration_ms ?? 0, sum, 1);
  const text = `${phases.map(([k, v]) => `${k} ${ms(v)}`).join(", ")}${job.duration_ms != null ? `; total ${ms(job.duration_ms)}` : ""}`;
  return (
    <div className="phase">
      <div className="phase-track" role="img" aria-label={`Phases: ${text}`}>
        {phases.map(([k, v]) => (
          <div key={k} className={`phase-seg phase-${k.toLowerCase()}`} style={{ "--w": `${((v / scale) * 100).toFixed(2)}%` } as CSSProperties} />
        ))}
      </div>
      <p className="phase-legend" aria-hidden="true">
        {text}
      </p>
    </div>
  );
}

function TimingRoute({ job, x }: { job: Job; x: JobsCtx }) {
  const groups: Group[] = groupsLive.use().data ?? [];
  const hw = hardware.use().data;
  const from = peerOf(job.origin);
  const to = peerOf(job.target);
  const allowedBy = from ? groups.filter((g) => g.members.includes(from) && g.policy.allow_remote_inference).map((g) => g.name) : null;
  const caps = to ? x.peers.find((p) => p.id === to)?.capabilities : undefined;
  const executing = !to ? (
    hw ? (
      `This computer · ${hw.host.chip} · ${humanBytes(hw.memory.total_bytes)}`
    ) : (
      <Unavailable reason="Not read yet" />
    )
  ) : caps === undefined ? (
    <Unavailable reason="Not reported by this service version" />
  ) : caps === null ? (
    <Unavailable reason="Not reported while not connected" />
  ) : caps.hardware === null ? (
    <Unavailable reason="Sent in a format this version cannot read" />
  ) : (
    <>
      {caps.hardware.host.chip} · {humanBytes(caps.hardware.memory.total_bytes)} <span className="note">at its network start</span>
    </>
  );
  return (
    <>
      <PhaseBar job={job} />
      <KV
        items={[
          { k: "Target", v: <span className="mono">{job.target}</span>, note: to ? peerName(x.peers, to) : "This computer" },
          { k: "Origin", v: <span className="mono">{job.origin}</span>, note: from ? peerName(x.peers, from) : "This computer" },
          { k: "Request ID", v: <span className="mono">{job.request_id ?? "—"}</span> },
          ...(allowedBy ? [{ k: "Allowed by", v: allowedBy.length ? allowedBy.join(", ") : "No group here allows prompts from it now" }] : []),
          { k: "Executing device", v: executing },
        ]}
      />
      <p className="note jobs-caveat">Jobs are kept in memory (last 256) and cleared when the service restarts. Requests to and from other devices also remain in Activity.</p>
    </>
  );
}
