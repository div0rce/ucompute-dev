// Audit event (and finished E3 job) -> Activity row (plan §2.6). One table-driven mapping, unit-tested per kind.
// Rows hold ids, counts, codes and durations only: the audit never carries prompt or reply text, and neither does E3.
import { shortLabel, type AuditEvent, type Job, type JobsData } from "../../api/client";
import { ERROR_CODES } from "../../api/errors";
import { humanDuration, num, plural } from "../../format";

export type Severity = "Info" | "Warning" | "Error";
export type Category = "Pairing" | "Connection" | "Trust" | "Settings" | "Remote run" | "Execution" | "Maintenance" | "Other";

export interface Described {
  category: Category;
  severity: Severity;
  sentence: string;
}

export interface Names {
  /** Device name for a 64-hex id (this computer's own id included). */
  device: (id: string) => string;
  /** Model name for a model id. */
  model: (id: string) => string;
}
const plain: Names = { device: shortLabel, model: (id) => id };

type Fields = Record<string, unknown>;
const s = (v: unknown) => (typeof v === "string" ? v : null);
const n = (v: unknown) => (typeof v === "number" ? v : null);
/** A stable code's title without its full stop; an unknown code verbatim. */
export const codeTitle = (code: string) => ERROR_CODES[code]?.title.replace(/\.$/, "") ?? code;
const secs = (ms: number | null) => (ms == null ? "an unknown time" : `${num(ms / 1000, 1)} s`);
const tokensIn = (tokens: number | null, ms: number | null) => `${tokens == null ? "tokens unknown" : plural(tokens, "token")} in ${secs(ms)}`;

const OUTCOME: Record<string, string> = { done: "done", ended: "done", cancelled: "cancelled", client_gone: "requester disconnected", incomplete: "incomplete" };
const outcomeSeverity = (o: string): Severity => (o === "done" || o === "ended" ? "Info" : o === "cancelled" || o === "client_gone" || o === "incomplete" ? "Warning" : "Error");
const LOST: Record<string, string> = { timed_out: "timed out", reset: "connection reset", closed_by_peer: "closed by the other device" };
const DISCONNECTED: Record<string, string> = { revoked: "revoked", unpaired: "unpaired", by_user: "by you" };
const onOff = (v: unknown) => (v === true ? "on" : v === false ? "off" : "unknown");
const MODE: Record<string, string> = { local_only: "Local only", trusted_group: "Trusted devices" };

/** Category, severity and sentence for one audit event. */
export function describe(e: AuditEvent, names: Names = plain): Described {
  const f = (e.fields && typeof e.fields === "object" ? e.fields : {}) as Fields;
  const dev = e.peer_id ? names.device(e.peer_id) : "a device";
  const reason = s(f.reason);
  const addr = s(f.addr);
  switch (e.kind) {
    case "pairing_started":
      return { category: "Pairing", severity: "Info", sentence: `Pairing window opened for ${n(f.window_secs) ?? "?"} s` };
    case "pairing_succeeded":
      return { category: "Pairing", severity: "Info", sentence: `Paired with ${dev}${s(f.role) ? ` (${s(f.role)})` : ""}` };
    case "pairing_failed":
      return reason === "cancelled"
        ? { category: "Pairing", severity: "Info", sentence: "Pairing window closed without pairing" }
        : { category: "Pairing", severity: "Warning", sentence: `Pairing failed: ${reason ? codeTitle(reason) : "reason not recorded"}` };
    case "pairing_locked":
      return { category: "Pairing", severity: "Warning", sentence: `Pairing locked after ${plural(n(f.failures) ?? 0, "wrong code")}` };
    case "peer_connected":
      return { category: "Connection", severity: "Info", sentence: `Connected to ${dev}${s(f.direction) ? ` (${s(f.direction)})` : ""}${addr ? ` at ${addr}` : ""}` };
    case "peer_disconnected":
      return { category: "Connection", severity: "Info", sentence: `Disconnected ${dev}: ${reason ? (DISCONNECTED[reason] ?? reason) : "reason not recorded"}` };
    case "peer_lost": {
      const for_ = n(f.connected_s);
      return { category: "Connection", severity: "Warning", sentence: `Lost connection to ${dev}: ${reason ? (LOST[reason] ?? reason) : "reason not recorded"}${for_ == null ? "" : ` after ${humanDuration(for_)}`}` };
    }
    case "peer_rejected": {
      const why = reason === "held" ? "disconnected by you" : reason ? codeTitle(reason) : "reason not recorded";
      const at = addr ? ` (${addr})` : "";
      return f.direction === "outbound"
        ? { category: "Connection", severity: "Warning", sentence: `Could not connect to ${dev}: ${why}${at}` }
        : { category: "Connection", severity: reason === "held" ? "Info" : "Warning", sentence: `Refused ${e.peer_id ? dev : "a connection"}: ${why}${at}` };
    }
    case "peer_revoked":
      return { category: "Trust", severity: "Info", sentence: `Revoked ${dev}${f.source === "peer" ? " (announced by another device)" : ""}` };
    case "peer_removed":
      return { category: "Trust", severity: "Info", sentence: `Unpaired ${dev}` };
    case "network_changed":
      return { category: "Settings", severity: "Info", sentence: `Networking ${onOff(f.enabled)}, sharing ${onOff(f.serve_peers)}` };
    case "privacy_changed":
      return { category: "Settings", severity: "Info", sentence: `Privacy mode: ${MODE[s(f.from) ?? ""] ?? s(f.from) ?? "?"} → ${MODE[s(f.to) ?? ""] ?? s(f.to) ?? "?"}` };
    case "group_updated":
      return {
        category: "Settings",
        severity: "Info",
        sentence: `Group ${s(f.name) ?? "?"}: ${plural(n(f.members) ?? 0, "device")}, prompts ${f.allow_remote_inference === true ? "allowed" : "not allowed"}`,
      };
    case "remote_request": {
      const model = names.model(s(f.model_id) ?? "a model");
      if (f.phase === "sent") return { category: "Remote run", severity: "Info", sentence: `Sent ${model} to ${dev}` };
      const o = s(f.outcome) ?? "unknown";
      // The client side ran it there; the server side (no `role`) ran it here for that device.
      const where = f.role === "client" ? `on ${dev}` : `for ${dev}`;
      return { category: "Remote run", severity: outcomeSeverity(o), sentence: `${model} ${where}: ${OUTCOME[o] ?? codeTitle(o)}, ${tokensIn(n(f.completion_tokens), n(f.duration_ms))}` };
    }
    case "audit_rotated":
      return { category: "Maintenance", severity: "Info", sentence: "Log rotated" };
    default:
      return { category: "Other", severity: "Info", sentence: e.kind };
  }
}

const JOB_WORD: Record<string, string> = { done: "done", cancelled: "stopped", error: "failed" };
/** An Execution row's text for a finished E3 job. */
export function describeJob(j: Job, names: Names = plain): Described {
  const where = j.target === "local" ? "this computer" : names.device(j.target.slice("peer:".length));
  const word = j.state === "error" ? `failed: ${codeTitle(j.error_code ?? "internal")}` : j.state === "cancelled" && j.error_code === "client_gone" ? "requester disconnected" : (JOB_WORD[j.state] ?? j.state);
  return {
    category: "Execution",
    severity: j.state === "error" ? "Error" : j.state === "cancelled" ? "Warning" : "Info",
    sentence: `${names.model(j.model_id)} on ${where}: ${word}, ${tokensIn(j.completion_tokens ?? null, j.duration_ms ?? null)}`,
  };
}

export interface ActivityRow extends Described {
  /** Stable across polls: `ts_utc|kind|peer_id` of the oldest event in the run (plus `#n` when that repeats), or `job|<id>`. */
  key: string;
  /** Newest time in the run (ISO). */
  at: string;
  /** Oldest time in the run (ISO). */
  first: string;
  count: number;
  kind: string;
  /** `reason` or `outcome`; for a job its error code or finish reason. */
  code: string | null;
  /** null: this computer. */
  peerId: string | null;
  requestId: string | null;
  addr: string | null;
  /** The run's events, newest first (empty for a job row). */
  events: AuditEvent[];
  job?: Job;
}

/** Kinds whose adjacent repeats merge into one row (the redial flood, evidence defect 4). Never jobs or requests. */
export const COLLAPSIBLE = new Set(["peer_rejected", "pairing_failed", "peer_connected", "peer_lost", "audit_rotated"]);
const fieldsOf = (e: AuditEvent) => (e.fields && typeof e.fields === "object" ? (e.fields as Fields) : {});
const sameRun = (a: AuditEvent, b: AuditEvent) => {
  const fa = fieldsOf(a);
  const fb = fieldsOf(b);
  return a.kind === b.kind && (a.peer_id ?? null) === (b.peer_id ?? null) && fa.reason === fb.reason && fa.outcome === fb.outcome && fa.addr === fb.addr;
};

/**
 * Activity rows from the audit (oldest first, as the service returns it) and the E3 recent jobs. A job whose
 * `request_id` matches a `remote_request` event is left out (the audit row covers it), so Execution rows are
 * effectively local jobs. Returned newest first.
 */
export function activityRows(events: AuditEvent[] | null, jobs: JobsData | "unsupported" | null, names: Names = plain): ActivityRow[] {
  const evs = events ?? [];
  // Runs, built oldest first so a newer event joins the run before it; keys come from the run's oldest event.
  const runs: AuditEvent[][] = [];
  for (const e of evs) {
    const last = runs[runs.length - 1];
    if (last && COLLAPSIBLE.has(e.kind) && sameRun(last[0], e)) last.unshift(e);
    else runs.push([e]);
  }
  const seen = new Map<string, number>();
  const rows: ActivityRow[] = runs.map((run) => {
    const newest = run[0];
    const oldest = run[run.length - 1];
    const base = `${oldest.ts_utc}|${oldest.kind}|${oldest.peer_id ?? ""}`;
    const k = (seen.get(base) ?? 0) + 1;
    seen.set(base, k);
    const f = fieldsOf(newest);
    return {
      ...describe(newest, names),
      key: k === 1 ? base : `${base}#${k}`,
      at: newest.ts_utc,
      first: oldest.ts_utc,
      count: run.length,
      kind: newest.kind,
      code: s(f.reason) ?? s(f.outcome),
      peerId: newest.peer_id ?? null,
      requestId: s(f.request_id),
      addr: s(f.addr),
      events: run,
    };
  });
  if (jobs && jobs !== "unsupported") {
    const audited = new Set(evs.filter((e) => e.kind === "remote_request").map((e) => s(fieldsOf(e).request_id)).filter((x): x is string => !!x));
    for (const j of jobs.recent) {
      if (j.request_id && audited.has(j.request_id)) continue;
      const at = j.finished_at_utc ?? j.created_at_utc;
      rows.push({
        ...describeJob(j, names),
        key: `job|${j.id}`,
        at,
        first: at,
        count: 1,
        kind: "job",
        code: j.error_code ?? j.finish_reason ?? null,
        peerId: j.target === "local" ? (j.origin === "local" ? null : j.origin.slice("peer:".length)) : j.target.slice("peer:".length),
        requestId: j.request_id ?? null,
        addr: null,
        events: [],
        job: j,
      });
    }
  }
  return rows.sort((a, b) => Date.parse(b.at) - Date.parse(a.at));
}
