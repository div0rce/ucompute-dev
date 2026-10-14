import type { Device, Status, Timings, Usage } from "../api/client";
import type { MappedError } from "../api/errors";
import { createStore } from "./store";

export interface Settings {
  device: Device;
  /** Context length sent with load requests; null means the model's default. */
  n_ctx: number | null;
}
export const settings = createStore<Settings>({ device: "auto", n_ctx: null }, "common.settings");

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
