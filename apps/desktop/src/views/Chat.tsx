import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Model, type Peer } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { RunOn, targetName } from "../components/RunOn";
import { refreshStatus } from "../daemon";
import { num } from "../format";
import { href } from "../router";
import { chatHistory, daemon, lastError, privacy, runs, runTarget, settings, type StoredMessage } from "../state/stores";

export function Chat() {
  const d = daemon.use();
  const { messages } = chatHistory.use();
  const loaded = d.status?.loaded ?? null;
  const [models, setModels] = useState<Model[]>([]);
  const [pick, setPick] = useState("");
  const [loading, setLoading] = useState(false);
  const [input, setInput] = useState("");
  const [streaming, setStreaming] = useState<string | null>(null);
  const [error, setError] = useState<MappedError | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const logRef = useRef<HTMLDivElement>(null);
  const peersRef = useRef<Peer[]>([]);
  const onPeers = useCallback((p: Peer[]) => (peersRef.current = p), []);
  const localOnly = (privacy.use()?.mode ?? "local_only") === "local_only";

  useEffect(() => {
    if (loaded) return;
    api.models().then((ms) => {
      const downloaded = ms.filter((m) => m.downloaded);
      setModels(downloaded);
      setPick((p) => p || downloaded[0]?.id || "");
    }, () => undefined);
  }, [loaded]);

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
    if (!text || !loaded || streaming !== null) return;
    setInput("");
    setError(null);
    const user: StoredMessage = { role: "user", content: text };
    const history = [...messages, user];
    chatHistory.set({ messages: history });
    setStreaming("");
    const ac = new AbortController();
    abortRef.current = ac;
    // Local-only is enforced here too, not only by hiding the selector.
    const target = localOnly ? "local" : runTarget.get();
    try {
      const res = await api.chat(
        { model: loaded.model_id, messages: history.map(({ role, content }) => ({ role, content })), target },
        (delta) => setStreaming((s) => (s ?? "") + delta),
        ac.signal,
      );
      const assistant: StoredMessage = {
        role: "assistant",
        content: res.content,
        model: loaded.model_id,
        usage: res.usage,
        timings: res.timings,
        stopped: res.stopped,
        target: res.target,
        ran_on: targetName(res.target, peersRef.current),
      };
      chatHistory.set((h) => ({ messages: [...h.messages, assistant] }));
      runs.set((r) => [
        {
          at: Date.now(),
          model: loaded.model_id,
          completion_tokens: res.usage?.completion_tokens ?? null,
          ttft_ms: res.timings?.time_to_first_token_ms ?? null,
          decode_tps: res.timings?.decode_tokens_per_second ?? null,
          stopped: res.stopped,
        },
        ...r,
      ]);
    } catch (e) {
      const m = explain(e);
      setError(m);
      lastError.set(m);
    } finally {
      setStreaming(null);
      abortRef.current = null;
    }
  }

  function stop() {
    abortRef.current?.abort();
  }

  return (
    <section className="view chat" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Chat</h1>
        {loaded ? (
          <p>
            Talking to {loaded.model_id} on {loaded.device_actual}
            {loaded.fallback_reason ? ` (requested ${loaded.device_requested}: ${loaded.fallback_reason})` : ""}, {loaded.n_ctx.toLocaleString()} token context.
          </p>
        ) : d.reachable === false ? (
          <p>Common's background service is not running, so there is nothing to chat with yet.</p>
        ) : (
          <p>No model is loaded. Pick a downloaded model to start, or get one from the <a href={href("models")}>model library</a>.</p>
        )}
        {loaded && (
          <div className="toolbar" style={{ marginTop: 10 }}>
            <RunOn onPeers={onPeers} />
          </div>
        )}
        {!loaded && d.reachable && (
          <div className="toolbar" style={{ marginTop: 10 }}>
            <label>
              <span className="small muted">Model</span>
              <br />
              <select value={pick} onChange={(e) => setPick(e.target.value)} disabled={models.length === 0}>
                {models.length === 0 && <option value="">No downloaded models</option>}
                {models.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.name}
                  </option>
                ))}
              </select>
            </label>
            <button type="button" className="btn btn-primary" onClick={loadPicked} disabled={!pick || loading} style={{ alignSelf: "end" }}>
              {loading ? "Loading" : "Load model"}
            </button>
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
                {m.usage ? `${num(m.usage.completion_tokens)} tokens` : "tokens unknown"}
                {m.timings ? `, first token in ${num(m.timings.time_to_first_token_ms)} ms, ${num(m.timings.decode_tokens_per_second, 1)} tok/s` : ""}
                {m.target && m.target !== "local" ? `. Ran on ${m.ran_on ?? targetName(m.target, peersRef.current)}.` : ""}
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
            placeholder={loaded ? "Write a message. Enter sends, Shift+Enter adds a line." : "Load a model first"}
            disabled={!loaded}
            rows={2}
          />
        </label>
        <div className="buttons">
          {streaming !== null ? (
            <button type="button" className="btn btn-danger" onClick={stop}>
              Stop
            </button>
          ) : (
            <button type="submit" className="btn btn-primary" disabled={!loaded || !input.trim()}>
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
