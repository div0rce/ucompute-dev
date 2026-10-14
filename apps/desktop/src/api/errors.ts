// Plain-language explanations for every stable error code (F-10-06).
// Texts mirror `ErrorCode::user_message` in crates/common-core/src/error.rs; errors.test.ts parses that file
// and fails if a code or a message drifts.

import { ApiError, UNREACHABLE } from "./client";

export interface Explanation {
  title: string;
  next_step: string;
}

export const ERROR_CODES: Record<string, Explanation> = {
  invalid_request: { title: "The request was not understood.", next_step: "Check the prompt and settings, then try again." },
  model_not_found: { title: "That model is not in Common's catalog.", next_step: "Pick a model from the library or import a GGUF file." },
  model_not_downloaded: { title: "This model has not been downloaded yet.", next_step: "Download it from the model library, then run it." },
  model_file_invalid: { title: "This model file is damaged or is not a GGUF file.", next_step: "Delete it and download the model again." },
  integrity_mismatch: { title: "The downloaded file does not match its published checksum.", next_step: "Common deleted the file. Download it again; if this repeats, the source may be compromised." },
  insufficient_memory: { title: "This model needs more memory than your computer has available.", next_step: "Choose a smaller model or quantization, lower the context length, or close other apps." },
  context_too_long: { title: "The prompt is longer than this model can read at once.", next_step: "Shorten the prompt or choose a model with a larger context window." },
  backend_unavailable: { title: "The inference engine is not installed or failed verification.", next_step: "Run Common's setup to download the pinned engine." },
  backend_failed: { title: "The inference engine stopped unexpectedly.", next_step: "Try again. If it keeps failing, open Diagnostics and share the log." },
  gpu_unavailable: { title: "Your GPU could not be used for this model.", next_step: "Common can run it on the CPU instead, which will be slower." },
  timeout: { title: "The model took too long to respond.", next_step: "Try a shorter prompt or a smaller model." },
  cancelled: { title: "The request was stopped.", next_step: "Nothing further is needed." },
  overloaded: { title: "Common is busy with other requests.", next_step: "Wait a moment and try again." },
  download_failed: { title: "The download did not complete.", next_step: "Check your internet connection and try again; downloads resume where they stopped." },
  not_implemented: { title: "This feature is not available yet.", next_step: "Check the roadmap in the documentation." },
  policy_denied: { title: "Your settings do not allow this action.", next_step: "Review Privacy and Network settings." },
  unauthorized: { title: "This computer is not authorized to use the API.", next_step: "Provide the API key configured on the server." },
  io: { title: "Common could not read or write a file.", next_step: "Check disk space and permissions." },
  internal: { title: "Something went wrong inside Common.", next_step: "Try again. If it keeps failing, open Diagnostics and share the log." },
  peer_unauthorized: { title: "This computer is not authorized to run that on the selected device.", next_step: "Pair the device and add it to a trusted group in Peers." },
  peer_revoked: { title: "That device's access was revoked.", next_step: "Pair it again if you trust it." },
  pairing_locked: { title: "Too many failed pairing attempts.", next_step: "Wait a minute, then start pairing again with a fresh code." },
  pairing_failed: { title: "Pairing did not complete.", next_step: "Check the code and that both devices are on the same network, then try again." },
  peer_lost: { title: "The connection to the other device was lost.", next_step: "Check that it is awake and on the network, then retry." },
  protocol_incompatible: { title: "The other device runs an incompatible version of Common.", next_step: "Update Common on both devices." },
  frame_too_large: { title: "A message from the other device was too large and was rejected.", next_step: "Update Common on both devices; if this persists, revoke the device." },
};

/** Desktop-only condition, not a daemon code: fetch itself failed. */
const UNREACHABLE_EXPLANATION: Explanation = {
  title: "Common's background service is not running.",
  next_step: "Reopen the app so it can start the service, or run common serve from a terminal.",
};

export interface MappedError extends Explanation {
  code: string;
  /** Developer detail from the daemon, shown small. */
  detail: string;
  hint: string | null;
}

export function mapError(code: string, message: string, hint: string | null = null): MappedError {
  const base =
    code === UNREACHABLE ? UNREACHABLE_EXPLANATION : (ERROR_CODES[code] ?? ERROR_CODES.internal);
  return { code, ...base, detail: message, hint };
}

/** Maps anything thrown by the API client (or elsewhere) to a plain-language error. */
export function explain(err: unknown): MappedError {
  if (err instanceof ApiError) return mapError(err.code, err.message, err.hint);
  const msg = err instanceof Error ? err.message : String(err);
  return mapError("internal", msg);
}
