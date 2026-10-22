// Models (plan §2.3): the catalog with local and paired-device availability, fit, downloads (app-tracked, surviving
// navigation), load/unload, delete and import. Owner C.
import "./models.css";
import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, shortLabel, type Device, type Model, type Peer, type Status } from "../../api/client";
import { explain, type MappedError } from "../../api/errors";
import { FitBar } from "../../components/FitBar";
import { openExternal, refreshStatus } from "../../daemon";
import { clock, fitText, humanBytes, humanDuration, num, plural } from "../../format";
import { go, select, useLocation } from "../../router";
import { useNow } from "../../shell/attention";
import { mesh, models as modelsLive, network, pulls, ready, useNeed, type Pull } from "../../state/live";
import { daemon, lastError, settings } from "../../state/stores";
import type { MenuItem } from "../../ui/contextMenu";
import { DataTable, type Column } from "../../ui/DataTable";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { Icon, type IconName } from "../../ui/icons";
import { Inspector, openInspector } from "../../ui/Inspector";
import { EmptyState, KV, ScopeBar, StateBadge, ToolButton, Unavailable, type Tone } from "../../ui/primitives";
import { frameState, SectionFrame } from "../../ui/SectionFrame";
import { DownloadDialog, ImportDialog } from "./ModelDialogs";
import { downloadingCount, MAX_PULLS, refreshModels, startPull } from "./pulls";

export interface ModelWord {
  word: string;
  tone: Tone;
  icon: IconName;
  note?: string;
  /** "CPU fallback: <reason>", shown with a warn icon next to the state. */
  warn?: string;
  /** Sort order of the State column. */
  rank: number;
}

/** Whether `m` is loaded: the status poll (2.5 s) is fresher than the catalog read when the service reported one. */
export const isLoaded = (m: Model, status: Status | null) => (status ? status.loaded?.model_id === m.id : m.loaded);

const peerLabel = (peers: Peer[], hex: string) => {
  const p = peers.find((x) => x.id === hex);
  return p ? p.name || p.short_label : shortLabel(hex);
};

/** The State column (plan §2.3 table, first match). `ownLoad`: this app's load call in flight (optimistic Loading). */
export function modelState(m: Model, x: { pull?: Pull; status: Status | null; ownLoad: string | null; peers: Peer[] }): ModelWord {
  const { pull, status } = x;
  if (pull?.state === "downloading") {
    const ev = pull.ev;
    const pct = ev && ev.total_bytes > 0 ? Math.floor((ev.downloaded_bytes / ev.total_bytes) * 100) : null;
    const note = ev ? `${humanBytes(ev.downloaded_bytes)} of ${humanBytes(ev.total_bytes)}${ev.resumed_from > 0 ? `, resumed from ${humanBytes(ev.resumed_from)}` : ""}` : undefined;
    return { word: pct == null ? "Downloading" : `Downloading ${pct}%`, tone: "active", icon: "download", note, rank: 0 };
  }
  const engine = status?.engine;
  if (engine?.state === "loading" && engine.loading_model_id === m.id) {
    const s = engine.loading_for_ms == null ? null : Math.round(engine.loading_for_ms / 1000);
    return { word: s == null ? "Loading" : `Loading · ${s} s`, tone: "active", icon: "loader", rank: 1 };
  }
  if (x.ownLoad === m.id && !isLoaded(m, status)) return { word: "Loading", tone: "active", icon: "loader", rank: 1 };
  if (isLoaded(m, status)) {
    const l = status?.loaded?.model_id === m.id ? status.loaded : null;
    const forPeer = l?.loaded_for?.startsWith("peer:") ? l.loaded_for.slice(5) : null;
    return {
      word: l ? `Loaded · ${l.device_actual}` : "Loaded",
      tone: "ok",
      icon: "circle-check",
      note: forPeer ? `for a request from ${peerLabel(x.peers, forPeer)}` : undefined,
      warn: l?.fallback_reason ? `CPU fallback: ${l.fallback_reason}` : undefined,
      rank: 2,
    };
  }
  if (pull?.state === "error") return { word: `Download failed: ${pull.error?.title ?? "unknown error"}`, tone: "danger", icon: "circle-x", rank: 3 };
  if (m.downloaded) return { word: "Downloaded", tone: "neutral", icon: "hard-drive", rank: 4 };
  return { word: "Not downloaded", tone: "neutral", icon: "box", rank: 5 };
}

/**
 * Where the model is: this computer when downloaded, plus connected devices that reported it when their connection
 * opened (E1). Devices that are not connected, or whose service does not report models, are counted as not reported.
 */
export function onDevices(m: Model, peers: Peer[]): { names: string[]; fromPeers: number; unreported: number } {
  const names = m.downloaded ? ["This computer"] : [];
  let fromPeers = 0;
  let unreported = 0;
  for (const p of peers) {
    if (!p.connected || !p.capabilities) unreported++;
    else if (p.capabilities.models.some((x) => x.id === m.id)) {
      names.push(p.name || p.short_label);
      fromPeers++;
    }
  }
  return { names, fromPeers, unreported };
}

const fitHeadroom = (m: Model) => (m.fit.verdict === "fits" ? m.fit.headroom_bytes : m.fit.available_bytes - m.fit.needed_bytes);
const basisText = (m: Model) =>
  `${m.estimate_basis === "gguf" ? "Estimated from the GGUF header" : "Estimated from the file size only (not downloaded yet)"} at ${num(m.estimate.n_ctx)} tokens of context (at most 4,096), against the memory this computer had free when the service started.`;

type Scope = "all" | "downloaded" | "loaded" | "downloading" | "not_downloaded" | "too_large";
const SCOPES: { id: Scope; label: string; empty: string }[] = [
  { id: "all", label: "All", empty: "No models." },
  { id: "downloaded", label: "Downloaded", empty: "No models are downloaded." },
  { id: "loaded", label: "Loaded", empty: "No model is loaded." },
  { id: "downloading", label: "Downloading", empty: "Nothing is downloading." },
  { id: "not_downloaded", label: "Not downloaded", empty: "Every model is downloaded." },
  { id: "too_large", label: "Too large", empty: "No model is too large for this computer." },
];

type LoadOpts = { device: Device; n_ctx: number | null };
/** An inline confirmation; a load may carry the Locations tab's one-off device and context. */
type Confirm = { kind: "load" | "delete"; id: string; opts?: LoadOpts } | null;

/** Focuses the toolbar button labelled `label` (after the render that enables it). */
const focusAction = (label: string) =>
  setTimeout(() => [...document.querySelectorAll<HTMLButtonElement>(".frame-toolbar .tool-btn")].find((b) => b.querySelector(".tool-label")?.textContent === label)?.focus(), 0);

export default function Models() {
  useNeed(["models"]);
  const live = modelsLive.use();
  const d = daemon.use();
  const pullMap = pulls.use();
  const peers = mesh.use().data?.peers ?? [];
  const net = network.use().data;
  const streak = ready.use().loadingStreak;
  const loc = useLocation();
  const [scope, setScope] = useState<Scope>("all");
  const [q, setQ] = useState("");
  const [dialog, setDialog] = useState<string | null>(null);
  const [importing, setImporting] = useState(false);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [pending, setPending] = useState<{ id: string; kind: "load" | "unload" | "delete" } | null>(null);
  const [actionError, setActionError] = useState<MappedError | null>(null);
  const opener = useRef<HTMLElement | null>(null);

  const list = live.data;
  const all = list ?? [];
  const status = d.status;
  const byId = (id: string | null) => (id ? (all.find((m) => m.id === id) ?? null) : null);
  const sel = byId(loc.id);
  const ownLoad = pending?.kind === "load" ? pending.id : null;
  const loadedModel = all.find((m) => isLoaded(m, status)) ?? null;
  // The status poll names the loaded model even when the catalog does not list it.
  const loadedId = status ? (status.loaded?.model_id ?? null) : (loadedModel?.id ?? null);
  const loadedName = loadedModel?.name ?? loadedId;
  // E2 reports a load in progress; before E2 two "a model is loading" answers from /ready in a row are the signal.
  const loadingAny = !!ownLoad || (status?.engine ? status.engine.state === "loading" : streak >= 2);
  const sharing = !!(net?.enabled && net.serve_peers);
  const state = (m: Model) => modelState(m, { pull: pullMap[m.id], status, ownLoad, peers });

  // `#/models?confirm=<id>` (Dashboard first-run row): select the model and open its download dialog once.
  const confirmId = loc.params.get("confirm");
  useEffect(() => {
    if (!confirmId || !list) return;
    const m = list.find((x) => x.id === confirmId);
    const params = new URLSearchParams(loc.params);
    params.delete("confirm");
    select(m ? m.id : loc.id, params);
    if (m && !m.downloaded && pulls.get()[m.id]?.state !== "downloading") setDialog(m.id);
  }, [confirmId, list === null]); // eslint-disable-line react-hooks/exhaustive-deps

  const fail = (e: unknown, title?: string) => {
    const m = explain(e);
    const mapped = title ? { ...m, title } : m;
    setActionError(mapped);
    lastError.set(mapped);
  };

  async function load(m: Model, opts?: LoadOpts) {
    setConfirm(null);
    setActionError(null);
    setPending({ id: m.id, kind: "load" });
    try {
      const s = opts ?? settings.get();
      await api.load(m.id, { device: s.device, ...(s.n_ctx ? { n_ctx: s.n_ctx } : {}) });
      await refreshStatus();
      void refreshModels();
      focusAction("Open in Chat");
    } catch (e) {
      fail(e);
    } finally {
      setPending(null);
    }
  }

  async function unload(m: Model) {
    setActionError(null);
    setPending({ id: m.id, kind: "unload" });
    try {
      await api.unload();
      await refreshStatus();
      void refreshModels();
    } catch (e) {
      const n = status?.loaded?.in_flight ?? status?.queue.inflight;
      fail(e, explain(e).code === "overloaded" && n ? `In use by ${plural(n, "request")}; try when they finish.` : undefined);
    } finally {
      setPending(null);
    }
  }

  async function remove(m: Model) {
    setConfirm(null);
    setActionError(null);
    setPending({ id: m.id, kind: "delete" });
    try {
      await api.remove(m.id);
      await Promise.all([refreshModels(), refreshStatus()]);
    } catch (e) {
      fail(e);
    } finally {
      setPending(null);
    }
  }

  const ask = (c: Confirm) => {
    opener.current = document.activeElement as HTMLElement | null;
    setConfirm(c);
  };
  const cancelConfirm = () => {
    setConfirm(null);
    setTimeout(() => opener.current?.focus?.(), 0);
  };

  // Why each action is not possible now (null = possible). Absent actions do not exist for this row's backend.
  const none = "Select a model first";
  const r = {
    download: !sel
      ? none
      : sel.downloaded
        ? "Already downloaded"
        : pullMap[sel.id]?.state === "downloading"
          ? "Downloading now"
          : downloadingCount() >= MAX_PULLS
            ? `${MAX_PULLS} downloads are already running; wait for one to finish`
            : null,
    load: !sel ? none : !sel.downloaded ? "Download it first" : isLoaded(sel, status) ? "Already loaded" : loadingAny ? "A model is loading" : null,
    unload: !sel ? none : !isLoaded(sel, status) ? "Not loaded" : null,
    chat: !sel ? none : !sel.downloaded ? "Download it first" : null,
    delete: !sel ? none : !sel.downloaded ? "Not downloaded" : null,
  };
  const replaces = sel && loadedId && loadedId !== sel.id ? loadedName : null;

  const doDownload = (m: Model) => {
    opener.current = document.activeElement as HTMLElement | null;
    setDialog(m.id);
  };
  const doLoad = (m: Model, opts?: LoadOpts) => (loadedId && loadedId !== m.id ? ask({ kind: "load", id: m.id, opts }) : void load(m, opts));
  const openChat = (m: Model) => go("chat", null, { model: m.id });

  const menu = (m: Model): MenuItem[] => {
    const loaded = isLoaded(m, status);
    return [
      { label: "Download…", action: () => doDownload(m), enabled: !m.downloaded && pullMap[m.id]?.state !== "downloading" && downloadingCount() < MAX_PULLS },
      { label: "Load", action: () => doLoad(m), enabled: m.downloaded && !loaded && !loadingAny },
      { label: "Unload", action: () => void unload(m), enabled: loaded },
      { label: "Open in Chat", action: () => openChat(m), enabled: m.downloaded },
      { separator: true },
      { label: "Copy ID", action: () => void navigator.clipboard?.writeText(m.id) },
      { separator: true },
      { label: "Delete…", action: () => ask({ kind: "delete", id: m.id }), enabled: m.downloaded },
    ];
  };

  const columns: Column<Model>[] = [
    {
      id: "name",
      header: "Name",
      width: 150,
      sort: (m) => m.name,
      text: (m) => m.name,
      cell: (m) => (
        <>
          {m.name}
          {m.tags.includes("recommended") && <span className="model-tag">Recommended</span>}
        </>
      ),
      detail: (m) => m.id,
    },
    {
      id: "state",
      header: "State",
      width: 126,
      live: true,
      sort: (m) => state(m).rank,
      text: (m) => state(m).word,
      cell: (m) => {
        const s = state(m);
        return (
          <>
            <StateBadge tone={s.tone} icon={s.icon} label={s.word} />
            {s.warn && (
              <span className="state tone-warn model-warn">
                <Icon name="circle-alert" />
                {s.warn}
              </span>
            )}
          </>
        );
      },
      note: (m) => state(m).note ?? null,
    },
    { id: "size", header: "Size", width: 64, align: "end", sort: (m) => m.bytes, cell: (m) => humanBytes(m.bytes) },
    { id: "params", header: "Params", width: 52, sort: (m) => m.parameters, cell: (m) => m.parameters },
    { id: "quant", header: "Quant", width: 64, sort: (m) => m.quantization, cell: (m) => m.quantization },
    {
      id: "fit",
      header: "Fit",
      width: 184,
      sort: fitHeadroom,
      text: (m) => fitText(m.fit),
      cell: (m) =>
        m.fit.verdict === "fits" ? (
          <StateBadge tone="ok" icon="circle-check" label={`Fits · ${humanBytes(m.fit.headroom_bytes)} headroom`} />
        ) : (
          <StateBadge tone="warn" icon="circle-alert" label={`Too large · needs ${humanBytes(m.fit.needed_bytes)} of ${humanBytes(m.fit.available_bytes)}`} />
        ),
      // ponytail: the full "est. at service start" caveat does not fit the column; "est." stays visible, the basis is in
      // the Tip and the Memory tab.
      note: () => "est.",
      detail: (m) => `${fitText(m.fit)}. ${basisText(m)}`,
    },
    { id: "memory", header: "Memory", width: 64, align: "end", sort: (m) => m.estimate.total_bytes, cell: (m) => humanBytes(m.estimate.total_bytes), detail: (m) => `At ${num(m.estimate.n_ctx)} tokens of context` },
    {
      id: "devices",
      header: "On devices",
      width: 130,
      sort: (m) => onDevices(m, peers).names.length,
      text: (m) => onDevices(m, peers).names.join(", "),
      cell: (m) => onDevices(m, peers).names.join(", ") || "None",
      note: (m) => {
        const o = onDevices(m, peers);
        return [o.fromPeers ? "reported at connect" : null, o.unreported ? `+${o.unreported} not reported` : null].filter(Boolean).join(" · ") || null;
      },
      detail: (m) => {
        const o = onDevices(m, peers);
        return `${o.names.join(", ") || "On no device that reports it"}.${o.fromPeers ? " Paired devices' models are as reported when their connection opened." : ""}${o.unreported ? ` ${plural(o.unreported, "paired device")} not reported (not connected, or its service does not report models).` : ""}`;
      },
    },
    { id: "license", header: "License", width: 80, sort: (m) => m.license, cell: (m) => m.license ?? "unknown" },
    { id: "family", header: "Family", hidden: true, sort: (m) => m.family, cell: (m) => m.family },
    { id: "source", header: "Source", hidden: true, sort: (m) => m.source, cell: (m) => m.source },
    { id: "redistribution", header: "Redistribution", hidden: true, sort: (m) => m.redistribution, cell: (m) => m.redistribution },
    { id: "id", header: "ID", hidden: true, sort: (m) => m.id, cell: (m) => <span className="mono">{m.id}</span>, text: (m) => m.id },
  ];

  const inScope = (m: Model, s: Scope) =>
    s === "all" ||
    (s === "downloaded" && m.downloaded) ||
    (s === "loaded" && isLoaded(m, status)) ||
    (s === "downloading" && pullMap[m.id]?.state === "downloading") ||
    (s === "not_downloaded" && !m.downloaded) ||
    (s === "too_large" && m.fit.verdict === "too_large");
  const rows = all.filter((m) => inScope(m, scope));

  const empty =
    all.length === 0 ? (
      <EmptyState
        title="No catalog entries; import a GGUF file"
        action={
          <button type="button" className="btn" onClick={() => setImporting(true)}>
            Import GGUF…
          </button>
        }
      />
    ) : q.trim() ? (
      <EmptyState
        title={`No models match "${q.trim()}"`}
        action={
          <button type="button" className="btn" onClick={() => setQ("")}>
            Clear filter
          </button>
        }
      />
    ) : (
      <EmptyState title={SCOPES.find((s) => s.id === scope)!.empty} />
    );

  const confirmModel = byId(confirm?.id ?? null);
  const banner = (
    <>
      {confirm && confirmModel && (
        <div
          className="notice model-confirm"
          role="group"
          aria-label={confirm.kind === "delete" ? `Confirm deletion of ${confirmModel.name}` : `Confirm loading ${confirmModel.name}`}
          onKeyDown={(e: KeyboardEvent) => {
            if (e.key === "Escape") {
              e.preventDefault();
              cancelConfirm();
            }
          }}
        >
          {confirm.kind === "delete" ? (
            <p>Delete the downloaded file for {confirmModel.name}? Removes the file; the catalog entry stays and it can be downloaded again.</p>
          ) : (
            <p>
              Loading {confirmModel.name} unloads {loadedName}.{sharing ? " Paired devices' requests can load a different model here." : ""}
            </p>
          )}
          <div className="notice-actions">
            {confirm.kind === "delete" ? (
              <button type="button" className="btn btn-danger btn-sm" onClick={() => void remove(confirmModel)}>
                Delete file
              </button>
            ) : (
              <button type="button" className="btn btn-primary btn-sm" onClick={() => void load(confirmModel, confirm.opts)}>
                Load {confirmModel.name}
              </button>
            )}{" "}
            <button type="button" className="btn btn-sm" autoFocus onClick={cancelConfirm}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {actionError && <ErrorNotice error={actionError} onDismiss={() => setActionError(null)} />}
    </>
  );

  return (
    <SectionFrame
      title="Models"
      state={frameState(d, live)}
      selectionActions={
        <>
          <ToolButton icon="download" label="Download…" primary={!r.download} disabledReason={r.download} onClick={() => sel && doDownload(sel)} />
          <ToolButton icon="play" label={replaces ? `Load (replaces ${replaces})` : "Load"} disabledReason={r.load} busy={!!sel && ownLoad === sel.id} onClick={() => sel && doLoad(sel)} />
          <ToolButton icon="stop" label="Unload" disabledReason={r.unload} busy={pending?.kind === "unload"} onClick={() => sel && void unload(sel)} />
          <ToolButton icon="message-square" label="Open in Chat" disabledReason={r.chat} onClick={() => sel && openChat(sel)} />
          <ToolButton icon="trash-2" label="Delete…" disabledReason={r.delete} busy={pending?.kind === "delete"} onClick={() => sel && ask({ kind: "delete", id: sel.id })} />
        </>
      }
      more={[{ label: "Import GGUF…", action: () => setImporting(true), enabled: d.reachable === true }]}
      scope={<ScopeBar label="Show models" items={SCOPES.map((s) => ({ id: s.id, label: s.label, count: all.filter((m) => inScope(m, s.id)).length }))} value={scope} onChange={(id) => setScope(id as Scope)} />}
      filter={{ value: q, onChange: setQ, label: "Filter models", placeholder: "Name, family, quantization or license" }}
      banner={banner}
      inspector={
        <Inspector
          id="models"
          label="Model details"
          hasSelection={!!sel}
          empty="Select a model to see its details"
          tabs={sel ? inspectorTabs(sel, { state: state(sel), status, peers, pull: pullMap[sel.id], loadReason: r.load, onLoad: (o) => doLoad(sel, o) }) : []}
        />
      }
    >
      <DataTable
        id="models"
        label="Models"
        columns={columns}
        rows={rows}
        rowKey={(m) => m.id}
        selected={sel ? sel.id : null}
        onSelect={(k) => select(k, loc.params)}
        onActivate={() => openInspector()}
        onCopy={(m) => m.id}
        menu={menu}
        onDelete={(m) => m.downloaded && ask({ kind: "delete", id: m.id })}
        filter={q}
        filterText={(m) => [m.name, m.id, m.family, m.quantization, m.license ?? "", ...m.tags].join(" ")}
        empty={empty}
      />
      <DownloadDialog
        model={byId(dialog)}
        onClose={() => setDialog(null)}
        onConfirm={(m) => {
          setDialog(null);
          void startPull(m.id);
        }}
      />
      <ImportDialog
        open={importing}
        onClose={() => setImporting(false)}
        onImported={(id) => {
          void refreshModels().then(() => select(id, loc.params));
        }}
      />
    </SectionFrame>
  );
}

function inspectorTabs(
  m: Model,
  x: { state: ModelWord; status: Status | null; peers: Peer[]; pull?: Pull; loadReason: string | null; onLoad: (o: LoadOpts) => void },
) {
  return [
    {
      id: "overview",
      label: "Overview",
      render: () => (
        <KV
          items={[
            { k: "State", v: <StateBadge tone={x.state.tone} icon={x.state.icon} label={x.state.word} />, note: x.state.note },
            { k: "ID", v: <span className="mono">{m.id}</span> },
            { k: "Name", v: m.name },
            { k: "Family", v: m.family },
            { k: "Parameters", v: m.parameters },
            { k: "Quantization", v: m.quantization },
            { k: "Size", v: `${humanBytes(m.bytes)} (${m.bytes.toLocaleString()} bytes)` },
            {
              k: "License",
              v: (
                <>
                  {m.license ?? "unknown"}{" "}
                  {m.license_url && (
                    <button type="button" className="btn btn-sm" onClick={() => void openExternal(m.license_url!)}>
                      View license
                    </button>
                  )}
                </>
              ),
            },
            { k: "Source", v: m.source },
            { k: "Redistribution", v: m.redistribution },
            { k: "Tags", v: m.tags.length ? m.tags.join(", ") : "None" },
            { k: "Chat", v: m.chat ? "Chat-capable" : "Text completion only" },
          ]}
        />
      ),
    },
    {
      id: "memory",
      label: "Memory",
      render: () => (
        <>
          <p className={m.fit.verdict === "fits" ? "tone-ok" : "tone-warn"}>{fitText(m.fit)}</p>
          <FitBar fit={m.fit} estimate={m.estimate} label={m.name} />
          <KV
            items={[
              { k: "Weights", v: humanBytes(m.estimate.weights_bytes) },
              { k: "KV cache", v: humanBytes(m.estimate.kv_cache_bytes) },
              { k: "Compute buffers", v: humanBytes(m.estimate.compute_buffer_bytes) },
              { k: "Overhead", v: humanBytes(m.estimate.overhead_bytes) },
              { k: "Total", v: humanBytes(m.estimate.total_bytes) },
              { k: "Context", v: `${num(m.estimate.n_ctx)} tokens` },
              { k: "Basis", v: basisText(m) },
            ]}
          />
          <p className="note model-caveat">Free memory was measured when the service started; other apps may use more or less now.</p>
        </>
      ),
    },
    { id: "locations", label: "Locations", render: () => <Locations m={m} status={x.status} peers={x.peers} loadReason={x.loadReason} onLoad={x.onLoad} /> },
    { id: "download", label: "Download", render: () => <DownloadTab m={m} pull={x.pull} /> },
  ];
}

function Locations({ m, status, peers, loadReason, onLoad }: { m: Model; status: Status | null; peers: Peer[]; loadReason: string | null; onLoad: (o: LoadOpts) => void }) {
  const l = status?.loaded?.model_id === m.id ? status.loaded : null;
  const s = settings.use();
  const [device, setDevice] = useState<Device>(s.device);
  const [ctx, setCtx] = useState(s.n_ctx ? String(s.n_ctx) : "");
  const loadedFor = (v: string | undefined) =>
    v === undefined ? <Unavailable reason="Not reported by this service version" /> : v === "local" ? "This computer" : `A request from ${peerLabel(peers, v.slice(5))}`;
  return (
    <div className="model-locations">
      <h3>This computer</h3>
      <KV
        items={[
          { k: "State", v: l ? "Loaded" : m.downloaded ? "Downloaded" : "Not downloaded" },
          ...(l
            ? [
                { k: "Device", v: l.device_actual, note: l.device_requested !== l.device_actual ? `requested ${l.device_requested}` : undefined },
                ...(l.fallback_reason ? [{ k: "CPU fallback", v: <span className="tone-warn">{l.fallback_reason}</span> }] : []),
                { k: "Offloaded layers", v: `${num(l.offloaded_layers)} of ${num(l.total_layers)}` },
                { k: "Context", v: `${num(l.n_ctx)} tokens` },
                { k: "Threads", v: num(l.threads) },
                { k: "Parallel slots", v: num(l.parallel_slots) },
                { k: "Load time", v: `${num(l.load_ms)} ms` },
                { k: "Requests using it", v: l.in_flight == null ? <Unavailable reason="Not reported by this service version" /> : num(l.in_flight) },
                { k: "Context note", v: l.context_note ?? "None" },
                { k: "Loaded for", v: loadedFor(l.loaded_for), note: l.loaded_at_utc ? `at ${clock(l.loaded_at_utc)}` : undefined },
              ]
            : []),
        ]}
      />
      {m.downloaded && !l && (
        <form
          className="model-load-form"
          onSubmit={(e) => {
            e.preventDefault();
            if (!loadReason) onLoad({ device, n_ctx: ctx ? Number(ctx) : null });
          }}
        >
          <label>
            <span>Device</span>
            <select value={device} onChange={(e) => setDevice(e.target.value as Device)}>
              <option value="auto">Auto</option>
              <option value="gpu">GPU</option>
              <option value="cpu">CPU</option>
            </select>
          </label>
          <label>
            <span>Context length</span>
            <input type="number" min={128} step={128} value={ctx} placeholder="Model default" onChange={(e) => setCtx(e.target.value)} />
          </label>
          <button type="submit" className="btn" aria-disabled={loadReason ? "true" : undefined}>
            Load once with these settings
          </button>
          {loadReason && <span className="note">{loadReason}</span>}
        </form>
      )}
      <h3>Paired devices</h3>
      {peers.length === 0 ? (
        <p className="muted">No paired devices.</p>
      ) : (
        <table className="sub-table">
          <caption className="note">Reported when each connection opened; models pulled or removed there since are not shown. Whether it is loaded there is not reported.</caption>
          <thead>
            <tr>
              <th scope="col">Device</th>
              <th scope="col">Has it</th>
            </tr>
          </thead>
          <tbody>
            {peers.map((p) => (
              <tr key={p.id}>
                <td>{p.name || p.short_label}</td>
                <td>
                  {p.capabilities === undefined ? (
                    <Unavailable reason="Not reported by this service version" />
                  ) : !p.connected || !p.capabilities ? (
                    <Unavailable reason="Not reported while not connected" />
                  ) : p.capabilities.models.some((x) => x.id === m.id) ? (
                    "Has it"
                  ) : (
                    "Doesn't report it"
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function DownloadTab({ m, pull }: { m: Model; pull?: Pull }) {
  const now = useNow(1000, pull?.state === "downloading");
  const ev = pull?.ev;
  return (
    <div className="model-download">
      {pull?.state === "downloading" ? (
        <>
          <progress aria-label={`Download progress of ${m.name}`} value={ev?.downloaded_bytes ?? 0} max={ev?.total_bytes || undefined} />
          <KV
            items={[
              { k: "Downloaded", v: ev ? `${humanBytes(ev.downloaded_bytes)} of ${humanBytes(ev.total_bytes)}` : "Starting…" },
              { k: "Over the network", v: ev ? humanBytes(ev.network_bytes) : "—" },
              { k: "Resumed from", v: ev && ev.resumed_from > 0 ? humanBytes(ev.resumed_from) : "Start" },
              { k: "Elapsed", v: humanDuration((now - pull.startedAt) / 1000) },
            ]}
          />
        </>
      ) : pull?.state === "error" && pull.error ? (
        <ErrorNotice error={pull.error} />
      ) : pull?.state === "done" ? (
        <p>Downloaded by this app; started at {clock(pull.startedAt)}.</p>
      ) : (
        <p>Not downloading.</p>
      )}
      <p className="note model-caveat">
        Only downloads started in this app are shown; a partial download's size is not reported. Downloads can't be cancelled; a stopped download resumes next time.
      </p>
    </div>
  );
}
