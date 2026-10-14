import type { Device, Privacy, Status, Target, Timings, Usage } from "../api/client";
import type { MappedError } from "../api/errors";
import { createStore } from "./store";

export interface Settings {
  device: Device;
  /** Context length sent with load requests; null means the model's default. */
  n_ctx: number | null;
  /** Name other devices see when pairing with this one. */
  device_name: string;
}
export const settings = createStore<Settings>({ device: "auto", n_ctx: null, device_name: "This Mac" }, "common.settings");

export interface DaemonState {
  reachable: boolean | null; // null = not checked yet
  version: string | null;
  status: Status | null;
  spawned: boolean;
}
export const daemon = createStore<DaemonState>({ reachable: null, version: null, status: null, spawned: false });

export interface StoredMessage {
  role: "user" | "assistant";
  content: string;
  model?: string;
  usage?: Usage | null;
  timings?: Timings | null;
  stopped?: boolean;
  /** Where the reply ran; absent for replies from before peers existed. */
  target?: Target;
  /** Display name of the device for a peer target, captured at send time. */
  ran_on?: string;
}
export const chatHistory = createStore<{ messages: StoredMessage[] }>({ messages: [] }, "common.chat");

export interface Run {
  at: number;
  model: string;
  completion_tokens: number | null;
  ttft_ms: number | null;
  decode_tps: number | null;
  stopped: boolean;
}
/** Session-only; cleared when the app closes. */
export const runs = createStore<Run[]>([]);

export const lastError = createStore<MappedError | null>(null);

/** Current privacy mode from the daemon; null until fetched. Shown in the app header and gates the peer selector. */
export const privacy = createStore<Privacy | null>(null);

/** Where the next request runs. Session-only: forgetting it on restart is the safe default. */
export const runTarget = createStore<Target>("local");
