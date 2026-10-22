// Downloads this app started. Progress lives in the app-level `pulls` store, so it survives navigation and a second
// pull of the same model is never started (plan §2.3). There is no cancel route: a stopped download resumes next time.
import { api } from "../../api/client";
import { explain } from "../../api/errors";
import { pulls, refreshFresh, type Pull } from "../../state/live";
import { lastError } from "../../state/stores";

/** The daemon runs at most this many downloads at once (admin.rs `MAX_PULLS`; 429 beyond). */
export const MAX_PULLS = 2;

export const downloadingCount = () => Object.values(pulls.get()).filter((p) => p.state === "downloading").length;

/**
 * Starts downloading `id` unless this app is already downloading it or `MAX_PULLS` run. Resolves when the download
 * ends (never rejects: a failure lands in `pulls[id].error`).
 */
export async function startPull(id: string): Promise<void> {
  if (pulls.get()[id]?.state === "downloading" || downloadingCount() >= MAX_PULLS) return;
  const set = (patch: Partial<Pull>) =>
    pulls.set((all) => ({ ...all, [id]: { ...all[id], ...patch } }));
  pulls.set((all) => ({ ...all, [id]: { state: "downloading", ev: null, startedAt: Date.now() } }));
  try {
    // api.pull rejects on an error event inside the stream and on a stream that ends without `done`.
    await api.pull(id, (ev) => set({ ev }));
    set({ state: "done" });
  } catch (e) {
    const error = explain(e);
    set({ state: "error", error });
    lastError.set(error);
  }
  await refreshModels();
}

/** Re-reads the catalog after an action, never answered by a read that started before it (`refreshFresh`). */
export async function refreshModels(): Promise<void> {
  await refreshFresh("models");
}
