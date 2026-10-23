import type { Device, Privacy, Status, Target, Timings, Usage } from "../api/client";
import type { MappedError } from "../api/errors";
import type { Live } from "./live";
import { createStore } from "./store";

export interface Settings {
  device: Device;
  /** Context length sent with load requests; null means the model's default. */
  n_ctx: number | null;
  /** Name other devices see when pairing with this one; empty means the daemon's (this computer's own name). */
  device_name: string;
}
export const settings = createStore<Settings>({ device: "auto", n_ctx: null, device_name: "" }, "common.settings");
// "This Mac" was the hard-coded default before the daemon reported a name, persisted by any settings change, and
// a Windows laptop paired under it. Nobody chose it, so it means "not set".
if (settings.get().device_name === "This Mac") settings.set({ ...settings.get(), device_name: "" });

export interface DaemonState {
  /** null = not checked yet; false only when `/api/status` failed and `GET /health` did not answer either. */
  reachable: boolean | null;
  version: string | null;
  /** Last good `/api/status`; kept while the service is busy, cleared when it is down. */
  status: Status | null;
  spawned: boolean;
  /** false while the service is reachable but a status poll timed out (busy, likely loading). Absent = not known false. */
  responding?: boolean;
  /** When `status` was fetched (epoch ms). */
  at?: number | null;
  /** The last `/api/status` answered with an HTTP error: the service runs, but `status` is the last good one, not live. */
  statusError?: MappedError | null;
}
export const daemon = createStore<DaemonState>({ reachable: null, version: null, status: null, spawned: false, responding: true, at: null });
/** The kept `status` is the last good one, not live: a status poll timed out (busy) or answered with an HTTP error. */
export const statusNotLive = (d: DaemonState) => d.responding === false || !!d.statusError;

export interface StoredMessage {
  role: "user" | "assistant";
  content: string;
  model?: string;
  usage?: Usage | null;
  timings?: Timings | null;
  stopped?: boolean;
  /** Where a user message was sent or a reply ran; absent from older messages (see chat `contextFor`). */
  target?: Target;
  /** Display name of the device for a peer target, captured at send time. */
  ran_on?: string;
  /** The `X-Common-Client-Ref` this app sent with the request (matches `Job.client_ref`). */
  client_ref?: string;
  /** The response id (`chatcmpl-…`), which is the E3 job id. */
  job_id?: string;
  /** Where a local reply ran (`device_actual`): the E3 row by client ref, else the loaded model at completion. */
  device_actual?: string | null;
  finish_reason?: string | null;
}
export const chatHistory = createStore<{ messages: StoredMessage[] }>({ messages: [] }, "common.chat");

export const lastError = createStore<MappedError | null>(null);

/** Current privacy mode from the daemon, polled like the live stores: a failed read keeps the last mode and sets `error`. */
export const privacy = createStore<Live<Privacy>>({ data: null, at: null, error: null });

/** Where the next request runs. Session-only: forgetting it on restart is the safe default. */
export const runTarget = createStore<Target>("local");
