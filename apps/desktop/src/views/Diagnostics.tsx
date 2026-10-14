import { useState } from "react";
import { api, getBaseUrl } from "../api/client";
import { ERROR_CODES } from "../api/errors";
import { humanDuration } from "../format";
import { daemon } from "../state/stores";

export function Diagnostics() {
  const d = daemon.use();
  const [copied, setCopied] = useState<string>("");

  async function copy() {
    const report: Record<string, unknown> = {
      app: "common-desktop 0.1.0",
      service_url: getBaseUrl(),
      service_spawned_by_app: d.spawned,
      reachable: d.reachable,
      generated_at: new Date().toISOString(),
    };
    try {
      report.status = await api.status();
    } catch (e) {
      report.status_error = String(e);
    }
    try {
      report.hardware = await api.hardware();
    } catch (e) {
      report.hardware_error = String(e);
    }
    try {
      await navigator.clipboard.writeText(JSON.stringify(report, null, 2));
      setCopied("Diagnostics copied. They contain status and hardware details, never your messages.");
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
