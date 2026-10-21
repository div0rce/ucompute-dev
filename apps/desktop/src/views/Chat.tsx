import { useEffect, useRef, useState } from "react";
import { api, type Model, type Peer } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { RunOn, targetName } from "../components/RunOn";
import { refreshStatus } from "../daemon";
import { num, plural } from "../format";
import { href } from "../router";
import { chatHistory, daemon, lastError, privacy, runs, runTarget, settings, type StoredMessage } from "../state/stores";

export function Chat() {
  const d = daemon.use();
  const { messages } = chatHistory.use();
  const loaded = d.status?.loaded ?? null;
  /** Every model this daemon knows (store + catalog): a remote run can name any of them. */
  const [models, setModels] = useState<Model[]>([]);
  /** The downloaded model to load here. */
  const [pick, setPick] = useState("");
  /** The model to run on a paired device; empty follows the model loaded here. */
  const [remotePick, setRemotePick] = useState("");
  const [loading, setLoading] = useState(false);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState<string | null>(null);
  const [error, setError] = useState<MappedError | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const localOnly = (privacy.use()?.mode ?? "local_only") === "local_only";
  // Local-only is enforced here too, not only by hiding the selector.
  const chosen = runTarget.use();
  const target = localOnly ? "local" : chosen;
  const remote = target !== "local";
  const downloaded = models.filter((m) => m.downloaded);
  // ponytail: no route lists a peer's models, so every id this daemon can name is offered; the peer loads it on
  // demand and answers model_not_downloaded when it lacks it (a plain message in `send`). Add a peer-models route
  // (from its Capabilities) if picking a model the device does not have becomes common.
  const remoteModel = remotePick || loaded?.model_id || pick || models[0]?.id || "";
  const model = remote ? remoteModel : loaded?.model_id;

  useEffect(() => {
    if (!d.reachable) return;
    api.models().then((ms) => {
      setModels(ms);
      setPick((p) => p || ms.find((m) => m.downloaded)?.id || "");
    }, () => undefined);
  }, [d.reachable, loaded?.model_id]);

  useEffect(() => {
    if (logRef.current) logRef.current.scrollTop = logRef.current.scrollHeight;
  }, [messages, streaming]);

  async function loadPicked() {
    if (!pick) return;
    setLoading(true);
    setError(null);
    try {
      const s = settings.get();
      await api.load(pick, { device: s.device, ...(s.n_ctx ? { n_ctx: s.n_ctx } : {}) });
      await refreshStatus();
    } catch (e) {
      const m = explain(e);
      setError(m);
      lastError.set(m);
    } finally {
      setLoading(false);
    }
  }

  async function send() {
    const text = input.trim();
    if (!text || !model || streaming !== null) return;
    setInput("");
    setError(null);
    const user: StoredMessage = { role: "user", content: text };
    const history = [...messages, user];
    chatHistory.set({ messages: history });
    setStreaming("");
    const ac = new AbortController();
    abortRef.current = ac;
    try {
      const res = await api.chat(
        { model, messages: history.map(({ role, content }) => ({ role, content })), target },
        (delta) => setStreaming((s) => (s ?? "") + delta),
        ac.signal,
      );
      const assistant: StoredMessage = {
        role: "assistant",
        content: res.content,
        model,
        usage: res.usage,
        timings: res.timings,
        stopped: res.stopped,
        target: res.target,
        ran_on: targetName(res.target, peers),
      };
      chatHistory.set((h) => ({ messages: [...h.messages, assistant] }));
      runs.set((r) => [
        {
          at: Date.now(),
          model,
          completion_tokens: res.usage?.completion_tokens ?? null,
          ttft_ms: res.timings?.time_to_first_token_ms ?? null,
          decode_tps: res.timings?.decode_tokens_per_second ?? null,
          stopped: res.stopped,
        },
        ...r,
      ]);
    } catch (e) {
      let m = explain(e);
      if (remote && (m.code === "model_not_downloaded" || m.code === "model_not_found")) {
        const where = targetName(target, peers);
        m = { ...m, title: `${where} does not have ${nameOf(model)}.`, next_step: `Download it on ${where}, or pick another model under Model.` };
      }
      setError(m);
      lastError.set(m);
    } finally {
      setStreaming(null);
      abortRef.current = null;
    }
  }

  const nameOf = (id: string) => models.find((m) => m.id === id)?.name ?? id;

  function stop() {
    abortRef.current?.abort();
  }

  return (
    <section className="view chat" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Chat</h1>
        {remote ? (
          <p>
            Prompts run on {targetName(target, peers)}, which loads the model you pick if it has it. Nothing needs to be loaded on this computer.
          </p>
        ) : loaded ? (
          <p>
            Talking to {loaded.model_id} on {loaded.device_actual}
            {loaded.fallback_reason ? ` (requested ${loaded.device_requested}: ${loaded.fallback_reason})` : ""}, {loaded.n_ctx.toLocaleString()} token context.
          </p>
        ) : d.reachable === false ? (
          <p>Common's background service is not running, so there is nothing to chat with yet.</p>
        ) : (
          <p>No model is loaded. Pick a downloaded model to start, or get one from the <a href={href("models")}>model library</a>.</p>
        )}
        {d.reachable && !(localOnly && loaded) && (
          <div className="toolbar" style={{ marginTop: 10 }}>
            <RunOn onPeers={setPeers} />
            {remote ? (
              <label>
                <span className="small muted">Model</span>
                <br />
                <select value={remoteModel} onChange={(e) => setRemotePick(e.target.value)} disabled={models.length === 0}>
                  {models.length === 0 && <option value="">No models</option>}
                  {models.map((m) => (
                    <option key={m.id} value={m.id}>
                      {m.name}
                    </option>
                  ))}
                </select>
              </label>
            ) : (
              !loaded && (
                <>
                  <label>
                    <span className="small muted">Model</span>
                    <br />
                    <select value={pick} onChange={(e) => setPick(e.target.value)} disabled={downloaded.length === 0}>
                      {downloaded.length === 0 && <option value="">No downloaded models</option>}
                      {downloaded.map((m) => (
                        <option key={m.id} value={m.id}>
                          {m.name}
                        </option>
                      ))}
                    </select>
                  </label>
                  <button type="button" className="btn btn-primary" onClick={loadPicked} disabled={!pick || loading} style={{ alignSelf: "end" }}>
                    {loading ? "Loading" : "Load model"}
                  </button>
                </>
              )
            )}
          </div>
        )}
        {error && (
          <div style={{ marginTop: 10 }}>
            <ErrorNotice error={error} onDismiss={() => setError(null)} />
          </div>
        )}
      </div>

      <div className="chat-log" ref={logRef} aria-label="Conversation" role="log">
        {messages.length === 0 && streaming === null && (
          <div className="empty">
            {localOnly
              ? "Messages stay on this computer. Nothing you type is sent anywhere else."
              : "Messages are saved on this computer. A prompt is sent to another device only when you pick one under Run on."}
          </div>
        )}
        {messages.map((m, i) => (
          <article key={i} className={`msg ${m.role}`} aria-label={m.role === "user" ? "You" : "Model"}>
            <div className="body">{m.content}</div>
            {m.role === "assistant" && (
              <div className="foot">
                {m.stopped ? "Stopped. " : ""}
                {m.usage ? plural(m.usage.completion_tokens, "token") : "tokens unknown"}
                {m.timings ? `, first token in ${num(m.timings.time_to_first_token_ms)} ms, ${num(m.timings.decode_tokens_per_second, 1)} tok/s` : ""}
                {m.target && m.target !== "local" ? `. Ran on ${m.ran_on ?? targetName(m.target, peers)}.` : ""}
              </div>
            )}
          </article>
        ))}
        {streaming !== null && (
          <article className="msg assistant" aria-label="Model, generating">
            <div className="body">
              {streaming}
              <span className="cursor" aria-hidden="true" />
            </div>
          </article>
        )}
      </div>
      <div className="visually-hidden" role="status">
        {streaming !== null ? "Generating a reply" : messages.length > 0 ? "Reply complete" : ""}
      </div>

      <form
        className="composer"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        <label style={{ display: "contents" }}>
          <span className="visually-hidden">Message</span>
          <textarea
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                send();
              }
            }}
            placeholder={model ? "Write a message. Enter sends, Shift+Enter adds a line." : remote ? "Pick a model first" : "Load a model first"}
            disabled={!model}
            rows={2}
          />
        </label>
        <div className="buttons">
          {streaming !== null ? (
            <button type="button" className="btn btn-danger" onClick={stop}>
              Stop
            </button>
          ) : (
            <button type="submit" className="btn btn-primary" disabled={!model || !input.trim()}>
              Send
            </button>
          )}
          <button type="button" className="btn" onClick={() => chatHistory.set({ messages: [] })} disabled={messages.length === 0 || streaming !== null}>
            Clear
          </button>
        </div>
      </form>
    </section>
  );
}
