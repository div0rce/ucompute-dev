import { useEffect, useState } from "react";
import { api, ApiError, getBaseUrl, type AuditEvent } from "../api/client";
import { ERROR_CODES, explain, type MappedError } from "../api/errors";
import { humanDuration } from "../format";
import { daemon } from "../state/stores";

export function Diagnostics() {
  const d = daemon.use();
  const [copied, setCopied] = useState<string>("");
  const [audit, setAudit] = useState<AuditEvent[] | null>(null);
  const [auditError, setAuditError] = useState<MappedError | null>(null);

  useEffect(() => {
    if (!d.reachable) return;
    api.audit().then(
      (ev) => {
        setAudit(ev);
        setAuditError(null);
      },
      (e) => setAuditError(explain(e)),
    );
  }, [d.reachable]);

  async function copy() {
    const app = {
      app: "common-desktop 0.1.0",
      service_url: getBaseUrl(),
      service_spawned_by_app: d.spawned,
      reachable: d.reachable,
      generated_at: new Date().toISOString(),
    };
    let report: Record<string, unknown>;
    let what: string;
    try {
      // The daemon's bundle is redacted on its side (keys, tokens, hex ids, prompts) before it reaches us.
      report = { ...app, ...(await api.diagnostics()) };
      what = "the service's redacted bundle";
    } catch (e) {
      if (e instanceof ApiError && e.code !== "not_implemented" && e.status !== 404) {
        setCopied(`Could not build the bundle: ${explain(e).title}`);
        return;
      }
      // Older daemon without the bundle endpoint: status and hardware only, as before.
      report = { ...app };
      try {
        report.status = await api.status();
      } catch (err) {
        report.status_error = String(err);
      }
      try {
        report.hardware = await api.hardware();
      } catch (err) {
        report.hardware_error = String(err);
      }
      what = "status and hardware details";
    }
    try {
      await navigator.clipboard.writeText(JSON.stringify(report, null, 2));
      setCopied(`Diagnostics copied. They contain ${what}, never your messages.`);
    } catch {
      setCopied("Could not access the clipboard.");
    }
  }

  const s = d.status;
  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Diagnostics</h1>
      </div>

      <div className="section">
        <h2>Background service</h2>
        <dl className="kv">
          <dt>Status</dt>
          <dd className={d.reachable ? "ok" : d.reachable === false ? "danger" : ""}>
            {d.reachable ? "Running" : d.reachable === false ? "Not running" : "Checking"}
          </dd>
          <dt>Address</dt>
          <dd>{getBaseUrl()}</dd>
          <dt>Started by</dt>
          <dd>{d.spawned ? "This app" : "Something else (already running when the app opened)"}</dd>
          {s && (
            <>
              <dt>Version</dt>
              <dd>{s.version}</dd>
              <dt>Uptime</dt>
              <dd>{humanDuration(s.uptime_seconds)}</dd>
              <dt>Loaded model</dt>
              <dd>
                {s.loaded
                  ? `${s.loaded.model_id} via ${s.loaded.backend_name} ${s.loaded.backend_version} on ${s.loaded.device_actual}, ${s.loaded.offloaded_layers}/${s.loaded.total_layers} layers offloaded, loaded in ${s.loaded.load_ms} ms`
                  : "none"}
              </dd>
              <dt>Queue</dt>
              <dd>
                {s.queue.inflight} running, {s.queue.queued} waiting (limits {s.queue.max_concurrent} and {s.queue.max_queue})
              </dd>
            </>
          )}
        </dl>
        <div className="toolbar" style={{ marginTop: 12 }}>
          <button type="button" className="btn" onClick={copy}>
            Copy diagnostics
          </button>
          <span role="status" className="small muted">
            {copied}
          </span>
        </div>
      </div>

      <div className="section">
        <h2>Off-device activity</h2>
        <p className="muted small" style={{ maxWidth: "64ch" }}>
          Every pairing, connection, revocation and request involving another device, most recent first. Entries hold ids, sizes and
          reasons; never prompts or keys.
        </p>
        {auditError ? (
          <p className="muted small" style={{ marginTop: 10 }}>
            {auditError.code === "not_implemented" ? "This version of the service has no activity log." : `Could not read the activity log: ${auditError.title}`}
          </p>
        ) : audit === null ? (
          <p className="muted small" style={{ marginTop: 10 }}>
            {d.reachable === false ? "Available once the background service is running." : "Reading the log."}
          </p>
        ) : audit.length === 0 ? (
          <div className="empty" style={{ marginTop: 10 }}>No off-device activity has been recorded.</div>
        ) : (
          <table style={{ marginTop: 10 }}>
            <thead>
              <tr>
                <th scope="col">When</th>
                <th scope="col">Event</th>
                <th scope="col">Device</th>
                <th scope="col">Details</th>
              </tr>
            </thead>
            <tbody>
              {audit.map((e, i) => (
                <tr key={`${e.ts_utc}-${i}`}>
                  <td>{new Date(e.ts_utc).toLocaleString()}</td>
                  <td>
                    <code>{e.kind}</code>
                  </td>
                  <td>{e.peer_id ? `${e.peer_id.slice(0, 4)}-${e.peer_id.slice(4, 8)}-${e.peer_id.slice(8, 12)}` : "—"}</td>
                  <td className="small">{e.fields && Object.keys(e.fields).length ? JSON.stringify(e.fields) : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div className="section">
        <h2>What error messages mean</h2>
        <table>
          <thead>
            <tr>
              <th scope="col">Code</th>
              <th scope="col">What happened</th>
              <th scope="col">What to do</th>
            </tr>
          </thead>
          <tbody>
            {Object.entries(ERROR_CODES).map(([code, e]) => (
              <tr key={code}>
                <td>
                  <code>{code}</code>
                </td>
                <td>{e.title}</td>
                <td>{e.next_step}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
