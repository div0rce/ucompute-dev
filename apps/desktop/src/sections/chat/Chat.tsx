import "./chat.css";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { api, type Model, type Target } from "../../api/client";
import { explain, type MappedError } from "../../api/errors";
import { refreshStatus } from "../../daemon";
import { humanBytes, num, plural } from "../../format";
import { href, useLocation } from "../../router";
import { announce } from "../../shell/announcer";
import { useDeviceRows, type DeviceRow, type Word } from "../../state/devices";
import { groups, hardware, mesh, models, network, refreshFresh, useNeed } from "../../state/live";
import { chatHistory, daemon, lastError, privacy, runTarget, settings, statusNotLive } from "../../state/stores";
import { setInspector, ui } from "../../state/ui";
import { Dialog } from "../../ui/Dialog";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { Icon, type IconName } from "../../ui/icons";
import { TOGGLE_EVENT } from "../../ui/Inspector";
import { KV, StateBadge, ToolButton, Unavailable } from "../../ui/primitives";
import { frameState, SectionFrame } from "../../ui/SectionFrame";
import { DestinationPicker, destinations } from "./DestinationPicker";
import { clearError, contextFor, send, session, stop, targetName, type Reply } from "./session";

const ICON: Record<Word["tone"], IconName> = { ok: "circle-check", active: "loader", warn: "circle-alert", danger: "circle-x", neutral: "clock" };
const badge = (w: Word | null | undefined) => (w ? <StateBadge tone={w.tone} icon={ICON[w.tone]} label={w.word} note={w.note} detail={w.detail} /> : <Unavailable reason="not reported" />);
const ms = (v: number | null | undefined) => (v == null ? "—" : `${num(v)} ms`);
const tps = (v: number | null | undefined) => (v == null ? "—" : `${num(v, 1)} tok/s`);

/** Visible pieces joined with a hidden-from-speech middle dot. */
function Dotted({ items }: { items: ReactNode[] }) {
  return (
    <>
      {items.map((it, i) => (
        <span key={i}>
          {i > 0 && <span aria-hidden="true"> · </span>}
          {it}
        </span>
      ))}
    </>
  );
}

/** Per-reply footer, from the chat result only (never estimated). */
function Footer({ m, nameOf }: { m: Reply; nameOf: (id: string) => string }) {
  const peer = m.target && m.target !== "local";
  const where = peer ? `Ran on ${m.ran_on ?? targetName(m.target)}` : `This computer · ${m.device_actual ?? "—"}`;
  const t = m.timings;
  return (
    <div className="reply-foot">
      <span>{m.stopped ? "Stopped." : "Done."}</span>{" "}
      <Dotted
        items={[
          where,
          m.model ? nameOf(m.model) : "model unknown",
          m.usage ? plural(m.usage.completion_tokens, "token") : "tokens unknown",
          `TTFT ${ms(t?.time_to_first_token_ms)}`,
          tps(t?.decode_tokens_per_second),
          ...(m.job_id ? [<a key="job" href={href("jobs", m.job_id)}>Job {m.job_id}</a>] : []),
        ]}
      />
      {peer && <span className="note">measured by {m.ran_on ?? targetName(m.target)}; GPU/CPU not reported</span>}
    </div>
  );
}

function hardwareOf(row: DeviceRow | undefined, selfHw: ReturnType<typeof hardware.get>["data"]): ReactNode {
  if (!row) return <Unavailable reason="not known" />;
  if (row.key === "self") return selfHw ? `${selfHw.host.chip} · ${humanBytes(selfHw.memory.total_bytes)}` : <Unavailable reason="not read yet" />;
  if (row.caps === undefined) return <Unavailable reason="Not reported by this service version." />;
  if (row.caps === null) return <Unavailable reason="Not reported while not connected." />;
  if (!row.caps.hardware) return <Unavailable reason="Sent in a format this version cannot read." />;
  return (
    <>
      {row.caps.hardware.host.chip} · {humanBytes(row.caps.hardware.memory.total_bytes)}
      <span className="note">at its network start</span>
    </>
  );
}

/** Chat (plan §2.5): transcript, composer, and a right-docked Run panel. One conversation, stored on this computer. */
export default function Chat() {
  const d = daemon.use();
  useNeed(["models", "mesh", "groups", "network", "jobs", "hardware", "privacy"]);
  const loc = useLocation();
  const { messages } = chatHistory.use();
  const { streaming, error } = session.use();
  const catalog = models.use().data;
  const groupsData = groups.use().data;
  const meshLive = mesh.use();
  const netLive = network.use();
  const meshData = meshLive.data;
  const net = netLive.data;
  const selfHw = hardware.use().data;
  const rows = useDeviceRows();
  const panelHidden = !!ui.use().inspectors.chat?.collapsed;
  const localOnly = (privacy.use().data?.mode ?? "local_only") === "local_only";
  const chosen = runTarget.use();
  // Local-only is enforced here too, not only by hiding the picker.
  const target: Target = localOnly ? "local" : chosen;
  const remote = target !== "local";
  // The destination's State, Can run there and RTT come from the device rows, which keep the last good mesh data:
  // unknown while the service is down or busy or a read failed (the Devices table blanks the same columns), and no
  // device is offered as connected from them.
  const rowsFs = frameState(d, meshLive, netLive);
  const notLive = rowsFs === "ok" ? null : rowsFs === "down" ? "service not running" : rowsFs === "connecting" || rowsFs === "loading" ? "not read yet" : "not refreshed";
  const options = destinations(rows, groupsData, notLive);
  const loaded = d.status?.loaded ?? null;
  const paramModel = loc.params.get("model");
  const [pick, setPick] = useState(paramModel ?? "");
  const [remotePick, setRemotePick] = useState("");
  const [input, setInput] = useState("");
  const [loading, setLoading] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<MappedError | null>(null);
  const [confirmReplace, setConfirmReplace] = useState(false);
  const [confirmNew, setConfirmNew] = useState(false);
  const [notice, setNotice] = useState("");
  const logRef = useRef<HTMLDivElement>(null);
  const contextHint = useId();

  const nameOf = (id: string) => catalog?.find((m) => m.id === id)?.name ?? id;
  const peerRow = remote ? rows.find((r) => r.key === target.slice("peer:".length)) : undefined;
  const destName = remote ? (peerRow?.name ?? targetName(target)) : "this computer";

  // Local: downloaded models (plus the loaded one, before the catalog arrives).
  const downloaded: Pick<Model, "id" | "name">[] = (catalog ?? []).filter((m) => m.downloaded);
  if (loaded && !downloaded.some((m) => m.id === loaded.model_id)) downloaded.unshift({ id: loaded.model_id, name: nameOf(loaded.model_id) });
  const localModel = downloaded.some((m) => m.id === pick) ? pick : (loaded?.model_id ?? downloaded[0]?.id ?? "");
  // No status read yet (starting, or busy since the app opened): whether a model is loaded is not known.
  const statusUnread = !d.status;
  const needsLoad = !remote && !statusUnread && !!localModel && loaded?.model_id !== localModel;

  // Peer: what it reported when the connection opened first (E1); without E1 every model this service knows.
  const all = catalog ?? [];
  const reported = peerRow?.caps ? new Set(peerRow.caps.models.map((m) => m.id).filter((x): x is string => !!x)) : null;
  const firstGroup = reported ? all.filter((m) => reported.has(m.id)) : all;
  const rest = reported ? all.filter((m) => !reported.has(m.id)) : [];
  const offered = [...firstGroup, ...rest].map((m) => m.id);
  const remoteModel = offered.includes(remotePick) ? remotePick : loaded && firstGroup.some((m) => m.id === loaded.model_id) ? loaded.model_id : (firstGroup[0]?.id ?? (reported ? "" : (offered[0] ?? "")));
  const unreported = !!reported && rest.some((m) => m.id === remoteModel);

  const destOk = !remote || options.some((o) => o.value === target && !o.disabled);
  const model = remote ? remoteModel : needsLoad ? "" : (loaded?.model_id ?? "");
  const canSend = d.reachable !== false && !!model && destOk;
  const placeholder =
    d.reachable === false
      ? "The background service is not running"
      : remote && notLive
        ? `${destName}'s state is not known (${notLive})`
        : remote
          ? remoteModel
            ? "Write a message. Enter sends, Shift+Enter adds a line."
            : "Pick a model first"
          : statusUnread
            ? "Waiting for the background service's status"
            : !loaded
              ? "Load a model first"
              : needsLoad
                ? "Load the selected model first"
                : "Write a message. Enter sends, Shift+Enter adds a line.";

  useEffect(() => {
    if (paramModel) setPick(paramModel);
  }, [paramModel]);

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [messages, streaming?.text]);

  // `#/chat?reply=<job id or client ref>` (Jobs › Open in Chat) scrolls to that message.
  const replyParam = loc.params.get("reply");
  useEffect(() => {
    const q = replyParam && CSS.escape(replyParam);
    // The last match is the reply (its question carries the same client ref).
    const hits = q ? logRef.current?.querySelectorAll(`[data-job="${q}"], [data-ref="${q}"]`) : undefined;
    hits?.[hits.length - 1]?.scrollIntoView?.({ block: "center" });
  }, [replyParam]);

  // A selected device that stops being usable falls back to this computer, said out loud and on screen. Not while the
  // rows are not live: its option is disabled (nothing is sent), but a busy or briefly unread service is no reason to
  // drop the user's choice.
  const targetOpt = options.find((o) => o.value === target);
  const targetWhy = remote && meshData && groupsData && !notLive ? (targetOpt ? targetOpt.disabled : "no longer paired") : null;
  useEffect(() => {
    if (!targetWhy) return;
    const msg = `${destName}: ${targetWhy}. Replies now run on this computer.`;
    runTarget.set("local");
    setNotice(msg);
    announce(msg);
  }, [targetWhy, destName]);

  // Mod+I toggles the Run panel; Esc stops a reply being generated.
  useEffect(() => {
    const toggle = () => setInspector("chat", { collapsed: !(ui.get().inspectors.chat?.collapsed ?? false) });
    window.addEventListener(TOGGLE_EVENT, toggle);
    return () => window.removeEventListener(TOGGLE_EVENT, toggle);
  }, []);
  useEffect(() => {
    if (!streaming) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || (e.target as Element | null)?.closest?.("dialog")) return;
      e.preventDefault();
      stop();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [streaming]);

  async function load(id: string) {
    setLoading(id);
    setLoadError(null);
    setConfirmReplace(false);
    try {
      const s = settings.get();
      await api.load(id, { device: s.device, ...(s.n_ctx ? { n_ctx: s.n_ctx } : {}) });
      await refreshStatus();
      void refreshFresh("models");
    } catch (e) {
      const m = explain(e);
      setLoadError(m);
      lastError.set(m);
    } finally {
      setLoading(null);
    }
  }

  function submit() {
    const text = input.trim();
    if (!text || !canSend || streaming) return;
    setInput("");
    setNotice("");
    void send(text, model, target);
  }

  const lastReply = [...messages].reverse().find((m) => m.role === "assistant");
  const selfRow = rows.find((r) => r.key === "self");
  const destRow = remote ? peerRow : selfRow;
  const lastPeer = lastReply?.target && lastReply.target !== "local";
  const t = lastReply?.timings;
  const emptyText = localOnly
    ? "Messages stay on this computer. Nothing you type is sent anywhere else."
    : "Messages are saved on this computer. A prompt is sent to another device only when you pick one under Destination, and that device gets only the messages exchanged with it since you last used another destination.";
  // What `contextFor` leaves out: everything before the trailing run with this device (it may have had some of it).
  const partialContext = remote && contextFor(messages, target).length < messages.length;
  const nothingDownloaded = !remote && catalog !== null && !loaded && downloaded.length === 0;

  const controls = (
    <>
      <DestinationPicker options={options} value={target} localOnly={localOnly} onChange={(v) => (runTarget.set(v), setNotice(""))} />
      <label className="chat-select">
        <span className="chat-select-label">Model</span>
        {remote ? (
          <select value={remoteModel} onChange={(e) => setRemotePick(e.target.value)} disabled={offered.length === 0}>
            {offered.length === 0 && <option value="">No models</option>}
            {reported ? (
              <>
                <optgroup label={`Reported by ${destName} when the connection opened`}>
                  {firstGroup.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
                </optgroup>
                <optgroup label={`Not reported by ${destName}`}>
                  {rest.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
                </optgroup>
              </>
            ) : (
              all.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.name}
                </option>
              ))
            )}
          </select>
        ) : (
          <select value={localModel} onChange={(e) => (setPick(e.target.value), setConfirmReplace(false))} disabled={downloaded.length === 0}>
            {downloaded.length === 0 && <option value="">No downloaded models</option>}
            {downloaded.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        )}
      </label>
    </>
  );

  const actions = (
    <>
      {needsLoad && d.reachable && (
        <ToolButton
          icon="play"
          primary
          busy={loading !== null}
          label={loaded ? `Load ${nameOf(localModel)} (replaces ${nameOf(loaded.model_id)})` : "Load model"}
          disabledReason={loading ? "Loading…" : null}
          onClick={() => (loaded ? setConfirmReplace(true) : void load(localModel))}
        />
      )}
      <ToolButton
        icon="plus"
        label="New conversation"
        disabledReason={streaming ? "A reply is being generated; stop it first." : messages.length === 0 ? "The conversation is already empty." : null}
        onClick={() => setConfirmNew(true)}
      />
    </>
  );

  const banner = (
    <>
      {remote && (
        <p className="chat-hint">
          Prompts run on {destName}, which loads the model you pick if it has it. Nothing needs to be loaded on this computer.
          {unreported && <span className="note">{nameOf(remoteModel)} may not be on that device.</span>}
        </p>
      )}
      {/* Also the composer's description: it appears as the Destination changes, away from the focus. */}
      {partialContext && (
        <p className="chat-hint" id={contextHint}>
          {destName} gets only the messages exchanged with it since you last used another destination; earlier messages are not sent with this prompt.
        </p>
      )}
      {notice && (
        <p className="chat-hint tone-warn" role="status">
          {notice}
        </p>
      )}
      {confirmReplace && loaded && (
        <div role="group" aria-label="Replace the loaded model?" className="notice tone-warn">
          <p>
            Loading {nameOf(localModel)} unloads {nameOf(loaded.model_id)}.{net?.serve_peers ? " Paired devices' requests can load a different model here." : ""}
          </p>
          <div className="notice-actions">
            <button type="button" className="btn btn-primary" onClick={() => void load(localModel)}>
              Load {nameOf(localModel)}
            </button>{" "}
            <button type="button" className="btn" onClick={() => setConfirmReplace(false)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {loadError && <ErrorNotice error={loadError} onDismiss={() => setLoadError(null)} />}
      {error && <ErrorNotice error={error} onDismiss={clearError} />}
    </>
  );

  const fs = frameState(d);
  const unknown = notLive && <Unavailable reason={notLive} />;
  return (
    <SectionFrame title="Chat" controls={controls} actions={actions} banner={banner} state={typeof fs === "object" ? fs : "ok"}>
      <div className={`chat-body${panelHidden ? " panel-hidden" : ""}`}>
        <div className="chat-main">
          <div className="chat-log" ref={logRef} aria-label="Conversation" role="log">
            {messages.length === 0 && !streaming && (
              <div className="empty">
                {nothingDownloaded ? (
                  <>
                    Download a model first. <a href={href("models")}>Open Models</a>
                  </>
                ) : (
                  emptyText
                )}
              </div>
            )}
            {messages.map((m, i) => (
              <article key={i} className={`msg ${m.role}`} aria-label={m.role === "user" ? "You" : "Model"} data-job={m.job_id} data-ref={m.client_ref}>
                <div className="body">{m.content}</div>
                {m.role === "assistant" && <Footer m={m} nameOf={nameOf} />}
              </article>
            ))}
            {streaming && (
              <article className="msg assistant" aria-label="Model, generating">
                <div className="body">
                  {streaming.text}
                  <span className="cursor" aria-hidden="true" />
                </div>
                <div className="reply-foot">
                  <Dotted items={["Generating", streaming.target === "local" ? "This computer" : `On ${targetName(streaming.target)}`, nameOf(streaming.model), "tokens counted when finished"]} />
                </div>
              </article>
            )}
          </div>
          <div className="visually-hidden" role="status">
            {streaming ? "Generating a reply" : messages.length > 0 ? "Reply complete" : ""}
          </div>
          <form
            className="composer"
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
          >
            <label className="composer-field">
              <span className="visually-hidden">Message</span>
              <textarea
                value={input}
                onChange={(e) => setInput(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !e.shiftKey) {
                    e.preventDefault();
                    submit();
                  }
                }}
                placeholder={placeholder}
                aria-describedby={partialContext ? contextHint : undefined}
                disabled={!canSend}
                rows={2}
              />
            </label>
            {streaming ? (
              <button type="button" className="btn btn-danger" onClick={stop}>
                Stop
              </button>
            ) : (
              <button type="submit" className="btn btn-primary" disabled={!canSend || !input.trim()}>
                Send
              </button>
            )}
          </form>
        </div>

        <aside className="run-panel" aria-label="Run details" data-region="inspector">
          <div className="run-panel-bar">
            <h2>Run</h2>
            <button type="button" className="icon-btn" aria-expanded={!panelHidden} aria-label={panelHidden ? "Show run details" : "Hide run details"} onClick={() => setInspector("chat", { collapsed: !panelHidden })}>
              <Icon name="panel-right" />
            </button>
          </div>
          {!panelHidden && (
            <div className="run-panel-body">
              <h3>Destination</h3>
              <KV
                items={[
                  { k: "Device", v: remote ? destName : "This computer" },
                  { k: "State", v: unknown || badge(destRow?.state) },
                  // This computer's own word reads the service status (Not running, Busy), not the mesh.
                  { k: "Can run there", v: (remote && unknown) || badge(destRow?.runThere) },
                  { k: "Hardware", v: hardwareOf(destRow, selfHw) },
                  ...(remote
                    ? [{ k: "RTT", v: unknown || (peerRow?.link ? `${num(peerRow.link.rtt_ms, 1)} ms` : <Unavailable reason={peerRow?.link === undefined ? "Not reported by this service version." : "Not connected."} />) }]
                    : [
                        {
                          k: "Loaded model",
                          // A kept status (busy, or a read that failed) is not current: busy is most often another model loading.
                          v: statusUnread ? <Unavailable reason="status not read yet" /> : statusNotLive(d) ? <Unavailable reason="not refreshed" /> : loaded ? `${nameOf(loaded.model_id)} · ${loaded.device_actual}` : "No model loaded",
                        },
                      ]),
                ]}
              />
              <h3>Last reply</h3>
              {lastReply ? (
                <>
                  <KV
                    items={[
                      { k: "Prompt tokens", v: num(lastReply.usage?.prompt_tokens) },
                      { k: "Completion tokens", v: num(lastReply.usage?.completion_tokens) },
                      { k: "TTFT", v: ms(t?.time_to_first_token_ms) },
                      { k: "Prompt", v: ms(t?.prompt_ms) },
                      { k: "Prompt speed", v: tps(t?.prompt_tokens_per_second) },
                      { k: "Decode", v: ms(t?.decode_ms) },
                      { k: "Decode speed", v: tps(t?.decode_tokens_per_second) },
                      { k: "Finish reason", v: lastReply.stopped ? "stopped" : (lastReply.finish_reason ?? "—") },
                    ]}
                  />
                  <p className="note run-caption">{lastPeer ? `Measured by ${lastReply.ran_on ?? targetName(lastReply.target)}; GPU/CPU not reported.` : "Measured by this computer's engine."}</p>
                </>
              ) : (
                <p className="muted small">No reply yet.</p>
              )}
            </div>
          )}
        </aside>
      </div>

      <Dialog open={confirmNew} label="Start a new conversation?" onClose={() => setConfirmNew(false)}>
        <h2>Start a new conversation?</h2>
        <p className="prose">The current conversation is deleted from this computer. Nothing else keeps a copy.</p>
        <div className="dialog-actions">
          <button type="button" className="btn" data-autofocus onClick={() => setConfirmNew(false)}>
            Cancel
          </button>
          <button
            type="button"
            className="btn btn-danger"
            onClick={() => {
              chatHistory.set({ messages: [] });
              setConfirmNew(false);
            }}
          >
            Delete conversation
          </button>
        </div>
      </Dialog>
    </SectionFrame>
  );
}
