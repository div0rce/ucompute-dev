import { useEffect, useState } from "react";
import { api, type Hardware as Hw } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { humanBytes, plural } from "../format";

/** Raw value first (what `common hardware --json` prints), human form second. */
const bytes = (n: number) => `${n.toLocaleString()} bytes (${humanBytes(n)})`;

export function Hardware() {
  const [hw, setHw] = useState<Hw | null>(null);
  const [error, setError] = useState<MappedError | null>(null);

  useEffect(() => {
    api.hardware().then(setHw, (e) => setError(explain(e)));
  }, []);

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Hardware</h1>
        <p>What Common found on this computer. These are the same values the command line prints with <code>common hardware --json</code>.</p>
      </div>
      {error && <ErrorNotice error={error} />}
      {hw && (
        <>
          <div className="section">
            <h2>This computer</h2>
            <dl className="kv">
              <dt>Chip</dt>
              <dd>{hw.host.chip}</dd>
              <dt>System</dt>
              <dd>
                {hw.host.os} {hw.host.os_version}, {hw.host.arch}
              </dd>
              <dt>Processor</dt>
              <dd>
                {hw.cpu.model}, {plural(hw.cpu.physical_cores, "core")} ({plural(hw.cpu.logical_cores, "thread")}), {hw.cpu.arch}
              </dd>
              <dt>Memory</dt>
              <dd>
                {bytes(hw.memory.total_bytes)} total, {bytes(hw.memory.available_bytes)} available{hw.memory.unified ? ", shared between CPU and GPU" : ""}
              </dd>
              <dt>Free disk</dt>
              <dd>{bytes(hw.disk_free_bytes)}</dd>
              <dt>Probe time</dt>
              <dd>{hw.probe_ms} ms</dd>
            </dl>
          </div>

          <div className="section">
            <h2>Graphics</h2>
            {hw.gpus.length === 0 ? (
              <p className="muted">No GPU was detected.</p>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th scope="col">Name</th>
                    <th scope="col">Vendor</th>
                    <th scope="col">Backend</th>
                    <th scope="col" className="num">Cores</th>
                    <th scope="col">Memory</th>
                  </tr>
                </thead>
                <tbody>
                  {hw.gpus.map((g) => (
                    <tr key={g.name + g.backend}>
                      <td>{g.name}</td>
                      <td>{g.vendor}</td>
                      <td>{g.backend}</td>
                      <td className="num">{g.cores}</td>
                      <td>{bytes(g.memory_bytes)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          <div className="section">
            <h2>Inference backends</h2>
            <table>
              <thead>
                <tr>
                  <th scope="col">Backend</th>
                  <th scope="col">Available</th>
                  <th scope="col">Why</th>
                </tr>
              </thead>
              <tbody>
                {hw.backends.map((b) => (
                  <tr key={b.name}>
                    <td>{b.name}</td>
                    <td className={b.available ? "ok" : "warn"}>{b.available ? "Yes" : "No"}</td>
                    <td>{b.available ? (b.reason ?? "Ready to use.") : (b.reason ?? "Not available on this computer.")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="section">
            <h2>Processor features</h2>
            <p className="small muted">{hw.cpu.features.length ? hw.cpu.features.join(", ") : "none reported"}</p>
          </div>

          <details className="section">
            <summary>Raw JSON</summary>
            <pre className="small">{JSON.stringify(hw, null, 2)}</pre>
          </details>
        </>
      )}
    </section>
  );
}
