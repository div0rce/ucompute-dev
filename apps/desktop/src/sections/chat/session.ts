// The chat session controller (plan §2.5). Module level, so a reply keeps streaming while the user is on another
// section, and Jobs › Stop (through `ownStreams`) ends it like Chat's own Stop: the partial text is saved with
// `stopped: true`. The Chat view only subscribes.
import { api, shortLabel, type Target } from "../../api/client";
import { explain, type MappedError } from "../../api/errors";
import { go } from "../../router";
import { announce } from "../../shell/announcer";
import { dismissAttention, pushAttention } from "../../shell/attention";
import { attention, jobs, mesh, models, ownStreams } from "../../state/live";
import { createStore } from "../../state/store";
import { chatHistory, daemon, lastError, type StoredMessage } from "../../state/stores";

/** A stored reply (kept as a name for the footer and Run panel). */
export type Reply = StoredMessage;

export interface Streaming {
  ref: string;
  text: string;
  target: Target;
  model: string;
}
export const session = createStore<{ streaming: Streaming | null; error: MappedError | null }>({ streaming: null, error: null });

/** "this computer" or the paired device's name. */
export function targetName(target: Target | undefined): string {
  if (!target || target === "local") return "this computer";
  const id = target.slice("peer:".length);
  const p = mesh.get().data?.peers.find((x) => x.id === id);
  return p?.name || p?.short_label || shortLabel(id);
}
const modelName = (id: string) => models.get().data?.find((m) => m.id === id)?.name ?? id;

/** Where a local reply ran: the E3 row this app's ref names, else the loaded engine when it holds that model. */
function deviceFor(ref: string | undefined, model: string): string | null {
  const j = jobs.get().data;
  const row = ref && j && j !== "unsupported" ? [...j.active, ...j.recent].find((x) => x.client_ref === ref) : undefined;
  if (row?.device_actual) return row.device_actual;
  const loaded = daemon.get().status?.loaded;
  return loaded?.model_id === model ? loaded.device_actual : null;
}

/** Stops the reply being generated, if any (Chat's Stop and Esc). Jobs stops through `ownStreams` the same way. */
export function stop(): void {
  const s = session.get().streaming;
  if (s) ownStreams.get()[s.ref]?.stop();
}

export const clearError = () => session.set((s) => ({ ...s, error: null }));

/**
 * Sends `text` as the next user message and streams the reply. The client ref is registered in `ownStreams` before
 * the request is sent; it goes out as `X-Common-Client-Ref` only to a service that reports `status.engine` (E2 and
 * later allow the header in CORS; an older one would refuse the preflight).
 */
export async function send(text: string, model: string, target: Target): Promise<void> {
  if (session.get().streaming) return;
  const ref = crypto.randomUUID();
  const clientRef = daemon.get().status?.engine ? ref : undefined;
  const user: StoredMessage = { role: "user", content: text, ...(clientRef ? { client_ref: clientRef } : {}) };
  const history = [...chatHistory.get().messages, user];
  chatHistory.set({ messages: history });
  const ac = new AbortController();
  ownStreams.set((s) => ({ ...s, [ref]: { stop: () => ac.abort() } }));
  session.set({ streaming: { ref, text: "", target, model }, error: null });
  try {
    const res = await api.chat(
      { model, messages: history.map(({ role, content }) => ({ role, content })), target },
      (delta) => session.set((s) => (s.streaming?.ref === ref ? { ...s, streaming: { ...s.streaming, text: s.streaming.text + delta } } : s)),
      ac.signal,
      clientRef ? { clientRef } : {},
    );
    const reply: Reply = {
      role: "assistant",
      content: res.content,
      model,
      usage: res.usage,
      timings: res.timings,
      stopped: res.stopped,
      target: res.target,
      ...(res.target !== "local" ? { ran_on: targetName(res.target) } : { device_actual: deviceFor(clientRef, model) }),
      ...(clientRef ? { client_ref: clientRef } : {}),
      ...(res.id ? { job_id: res.id } : {}),
      finish_reason: res.finish_reason,
    };
    chatHistory.set((h) => ({ messages: [...h.messages, reply] }));
    // The cause of an earlier failure is gone once a reply arrives.
    for (const a of attention.get()) if (a.id.startsWith("reply:")) dismissAttention(a.id);
  } catch (e) {
    let m = explain(e);
    if (target !== "local" && (m.code === "model_not_downloaded" || m.code === "model_not_found")) {
      const where = targetName(target);
      m = { ...m, title: `${where} does not have ${modelName(model)}.`, next_step: `Download it on ${where}, or pick another model under Model.` };
    }
    session.set((s) => ({ ...s, error: m }));
    lastError.set(m);
    announce(`A reply from this app failed: ${m.title}`);
    pushAttention({ id: `reply:${ref}`, tone: "danger", text: `A reply from this app failed: ${m.title}`, action: { label: "Open Chat", run: () => go("chat") } });
  } finally {
    ownStreams.set(({ [ref]: _gone, ...rest }) => rest);
    session.set((s) => (s.streaming?.ref === ref ? { ...s, streaming: null } : s));
  }
}
