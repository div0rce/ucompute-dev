// describe.ts (owner D): every audit kind's category, severity and sentence; Execution rows from E3 with the
// `request_id` de-duplication; duplicate collapse only for the listed kinds; row keys stable across polls.
import { describe as group, expect, it } from "vitest";
import type { AuditEvent, JobsData } from "../../api/client";
import { AUDIT_E4, JOBS, PEER_ID } from "../../test/mocks";
import { activityRows, describe, describeJob, type Names } from "./describe";

const names: Names = { device: (id) => (id === PEER_ID ? "Studio Mac" : id.slice(0, 4)), model: (id) => (id === "qwen" || id === "qwen2.5-1.5b-instruct-q4_k_m" ? "Qwen" : id) };
const ev = (kind: string, fields: unknown = {}, peer_id: string | null = PEER_ID, ts_utc = "2026-10-22T10:00:00Z"): AuditEvent => ({ ts_utc, kind, ...(peer_id ? { peer_id } : {}), fields });

group("describe: one row per audit kind", () => {
  it.each([
    [ev("pairing_started", { port: 1, window_secs: 120 }, null), "Pairing", "Info", "Pairing window opened for 120 s"],
    [ev("pairing_succeeded", { role: "joiner", addr: "1.2.3.4:5" }), "Pairing", "Info", "Paired with Studio Mac (joiner)"],
    [ev("pairing_failed", { reason: "pairing_failed" }, null), "Pairing", "Warning", "Pairing failed: Pairing did not complete"],
    [ev("pairing_failed", { reason: "cancelled" }, null), "Pairing", "Info", "Pairing window closed without pairing"],
    [ev("pairing_failed", { reason: "qr_peer_mismatch" }, null), "Pairing", "Warning", "Pairing failed: qr_peer_mismatch"],
    [ev("pairing_locked", { failures: 5 }, null), "Pairing", "Warning", "Pairing locked after 5 wrong codes"],
    [ev("peer_connected", { addr: "192.168.1.77:8193", direction: "outbound", minor: 1 }), "Connection", "Info", "Connected to Studio Mac (outbound) at 192.168.1.77:8193"],
    [ev("peer_disconnected", { reason: "by_user", connections: 1 }), "Connection", "Info", "Disconnected Studio Mac: by you"],
    [ev("peer_disconnected", { reason: "revoked" }), "Connection", "Info", "Disconnected Studio Mac: revoked"],
    [ev("peer_disconnected", { reason: "unpaired" }), "Connection", "Info", "Disconnected Studio Mac: unpaired"],
    [ev("peer_lost", { reason: "timed_out", addr: "x", connected_s: 72 }), "Connection", "Warning", "Lost connection to Studio Mac: timed out after 1 min"],
    [ev("peer_lost", { reason: "closed_by_peer", addr: "x", connected_s: 5 }), "Connection", "Warning", "Lost connection to Studio Mac: closed by the other device after 5 s"],
    [ev("peer_rejected", { addr: "192.168.1.77:8193", reason: "held", detail: "disconnected by you", suppressed: 0 }), "Connection", "Info", "Refused Studio Mac: disconnected by you (192.168.1.77:8193)"],
    [ev("peer_rejected", { addr: "10.0.0.2:8193", reason: "peer_unauthorized" }, null), "Connection", "Warning", "Refused a connection: This computer is not authorized to run that on the selected device (10.0.0.2:8193)"],
    [ev("peer_rejected", { addr: "10.0.0.2:8193", direction: "outbound", reason: "peer_lost" }), "Connection", "Warning", "Could not connect to Studio Mac: The connection to the other device was lost (10.0.0.2:8193)"],
    [ev("peer_revoked", { seq: 1, by: "x", source: "local" }), "Trust", "Info", "Revoked Studio Mac"],
    [ev("peer_removed"), "Trust", "Info", "Unpaired Studio Mac"],
    [ev("network_changed", { enabled: true, serve_peers: false }, null), "Settings", "Info", "Networking on, sharing off"],
    [ev("privacy_changed", { from: "local_only", to: "trusted_group", acknowledged: true }, null), "Settings", "Info", "Privacy mode: Local only → Trusted devices"],
    [ev("group_updated", { name: "home", members: 2, allow_remote_inference: true, allow_pooled_inference: false }, null), "Settings", "Info", "Group home: 2 devices, prompts allowed"],
    [ev("remote_request", { role: "client", phase: "sent", peer: "0f1e", model_id: "qwen" }), "Remote run", "Info", "Sent Qwen to Studio Mac"],
    [ev("remote_request", { role: "client", phase: "finished", request_id: "r", model_id: "qwen", outcome: "done", completion_tokens: 5, duration_ms: 1500 }), "Remote run", "Info", "Qwen on Studio Mac: done, 5 tokens in 1.5 s"],
    [ev("remote_request", { role: "client", phase: "finished", model_id: "qwen", outcome: "cancelled", completion_tokens: 0, duration_ms: 200 }), "Remote run", "Warning", "Qwen on Studio Mac: cancelled, 0 tokens in 0.2 s"],
    [ev("remote_request", { role: "client", phase: "finished", model_id: "qwen", outcome: "policy_denied", completion_tokens: 0, duration_ms: 10 }), "Remote run", "Error", "Qwen on Studio Mac: Your settings do not allow this action, 0 tokens in 0 s"],
    [ev("remote_request", { request_id: "r", peer: "0f1e", model_id: "qwen", outcome: "ended", prompt_tokens: 3, completion_tokens: 1, duration_ms: 900 }), "Remote run", "Info", "Qwen for Studio Mac: done, 1 token in 0.9 s"],
    [ev("remote_request", { request_id: "r", model_id: "qwen", outcome: "client_gone", completion_tokens: 2, duration_ms: 900 }), "Remote run", "Warning", "Qwen for Studio Mac: requester disconnected, 2 tokens in 0.9 s"],
    [ev("audit_rotated", { file: "audit.jsonl", rotated_bytes: 1, dropped_bytes: 0 }, null), "Maintenance", "Info", "Log rotated"],
    [ev("something_new", { x: 1 }), "Other", "Info", "something_new"],
  ])("%#: %o", (e, category, severity, sentence) => {
    expect(describe(e, names)).toEqual({ category, severity, sentence });
  });

  it("finished E3 jobs read as Execution rows", () => {
    const [done, sent, failed] = JOBS.recent;
    expect(describeJob(done, names)).toEqual({ category: "Execution", severity: "Info", sentence: "Qwen on this computer: done, 160 tokens in 4 s" });
    expect(describeJob(sent, names).sentence).toBe("Qwen on Studio Mac: done, 120 tokens in 4.6 s");
    expect(describeJob(failed, names)).toEqual({ category: "Execution", severity: "Error", sentence: "Qwen on this computer: failed: UCompute is busy with other requests, tokens unknown in 0 s" });
    expect(describeJob({ ...done, state: "cancelled" }, names).severity).toBe("Warning");
    // The registry puts client_gone in error_code (JobHandle::fail); finish_reason stays null.
    expect(describeJob({ ...done, state: "cancelled", finish_reason: null, error_code: "client_gone" }, names).sentence).toMatch(/: requester disconnected, /);
    expect(describeJob({ ...done, state: "cancelled", finish_reason: null, error_code: "cancelled" }, names).sentence).toMatch(/: stopped, /);
  });
});

group("activityRows", () => {
  it("merges E3 recent jobs as Execution rows, leaving out a job the audit already covers by request_id", () => {
    const sentId = JOBS.recent[1].request_id!;
    const audit = [ev("remote_request", { role: "client", phase: "finished", request_id: sentId, model_id: "qwen", outcome: "done", completion_tokens: 120, duration_ms: 4600 }, PEER_ID, "2026-10-22T10:30:15Z")];
    const rows = activityRows(audit, JOBS, names);
    expect(rows.filter((r) => r.category === "Execution").map((r) => r.key)).toEqual(["job|chatcmpl-1a2b3c4d", "job|job-0000000000000001"]);
    expect(rows.find((r) => r.key === "job|job-0000000000000001")).toMatchObject({ severity: "Error", code: "overloaded", peerId: null });
    // Active jobs are the Jobs section's; only finished ones are history.
    expect(rows.some((r) => r.job?.state === "running")).toBe(false);
    // Without E3 (or before it answered) there are no Execution rows, and nothing is invented.
    expect(activityRows(audit, "unsupported", names).map((r) => r.category)).toEqual(["Remote run"]);
    expect(activityRows(audit, null, names)).toHaveLength(1);
  });

  it("collapses adjacent repeats only for the listed kinds; two finished requests stay two rows", () => {
    const t = (s: number) => `2026-10-22T10:00:${String(s).padStart(2, "0")}Z`;
    const rejected = (s: number, addr = "10.0.0.2:8193") => ev("peer_rejected", { addr, reason: "peer_unauthorized" }, PEER_ID, t(s));
    const finished = (s: number) => ev("remote_request", { role: "client", phase: "finished", request_id: `r${s}`, model_id: "qwen", outcome: "done", completion_tokens: 1, duration_ms: 1 }, PEER_ID, t(s));
    const events = [rejected(1), rejected(2), rejected(3), rejected(4, "10.0.0.9:8193"), finished(5), finished(6), rejected(7), ev("network_changed", { enabled: true, serve_peers: false }, null, t(8)), ev("network_changed", { enabled: true, serve_peers: false }, null, t(9))];
    const rows = activityRows(events, null, names);
    expect(rows.map((r) => [r.kind, r.count])).toEqual([
      ["network_changed", 1],
      ["network_changed", 1],
      ["peer_rejected", 1],
      ["remote_request", 1],
      ["remote_request", 1],
      ["peer_rejected", 1],
      ["peer_rejected", 3],
    ]);
    const run = rows[rows.length - 1];
    expect([run.at, run.first, run.events.length]).toEqual([t(3), t(1), 3]);
  });

  it("keeps every row key across a poll that adds events, including one that joins the newest run", () => {
    const base = [...AUDIT_E4, ev("peer_lost", { reason: "timed_out", addr: "a", connected_s: 1 }, PEER_ID, "2026-10-22T10:40:00Z")];
    const before = activityRows(base, JOBS, names).map((r) => r.key);
    const later = [
      ...base,
      ev("peer_lost", { reason: "timed_out", addr: "a", connected_s: 3 }, PEER_ID, "2026-10-22T10:41:00Z"),
      ev("peer_connected", { addr: "a", direction: "outbound" }, PEER_ID, "2026-10-22T10:42:00Z"),
    ];
    const after = activityRows(later, JOBS, names);
    expect(after.map((r) => r.key)).toEqual(expect.arrayContaining(before));
    expect(after.find((r) => r.kind === "peer_lost" && r.peerId === PEER_ID)).toMatchObject({ key: `2026-10-22T10:40:00Z|peer_lost|${PEER_ID}`, count: 2 });
    expect(new Set(after.map((r) => r.key)).size).toBe(after.length);
  });

  it("gives identical events distinct keys, oldest first, so later ones never renumber earlier ones", () => {
    const same = ev("remote_request", { role: "client", phase: "sent", model_id: "qwen" });
    const k1 = activityRows([same, same], null, names).map((r) => r.key);
    const k2 = activityRows([same, same, same], null, names).map((r) => r.key);
    expect(k1.sort()).toEqual([`${same.ts_utc}|remote_request|${PEER_ID}`, `${same.ts_utc}|remote_request|${PEER_ID}#2`]);
    expect(k2).toEqual(expect.arrayContaining(k1));
  });

  it("holds ids, counts and codes only: no field of a row carries message text", () => {
    const rows = activityRows(AUDIT_E4, JOBS as JobsData, names);
    const keys = new Set(rows.flatMap((r) => Object.keys(r.job ?? {})));
    for (const k of ["prompt", "messages", "content", "completion", "text"]) expect(keys.has(k)).toBe(false);
  });
});
