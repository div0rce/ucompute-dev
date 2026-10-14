import { useEffect, useState } from "react";
import { api } from "../api/client";
import { num } from "../format";
import { href } from "../router";
import { daemon, runs } from "../state/stores";

export function Performance() {
  const list = runs.use();
  const d = daemon.use();
  const [metrics, setMetrics] = useState<Record<string, number> | null>(null);
  const [filter, setFilter] = useState("token");
  const last = list[0];

  useEffect(() => {
    if (!d.reachable) return;
    api.metrics().then(setMetrics, () => setMetrics(null));
  }, [d.reachable, list.length]);

  const rows = Object.entries(metrics ?? {}).filter(([k]) => !filter || k.includes(filter));

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Performance</h1>
        <p>Measured on this computer during this session. Nothing here is estimated.</p>
      </div>

      <div className="section">
        <h2>Last run</h2>
        {last ? (
          <dl className="kv">
            <dt>Model</dt>
            <dd>{last.model}</dd>
            <dt>Tokens generated</dt>
            <dd>{num(last.completion_tokens)}</dd>
            <dt>Time to first token</dt>
            <dd>{last.ttft_ms == null ? "—" : `${num(last.ttft_ms)} ms`}</dd>
            <dt>Generation speed</dt>
            <dd>{last.decode_tps == null ? "—" : `${num(last.decode_tps, 1)} tokens per second`}</dd>
          </dl>
        ) : (
          <div className="empty">
            No runs yet. Send a message in <a href={href("chat")}>Chat</a> and each reply will be recorded here with its token count, time to first token and speed.
          </div>
        )}
      </div>

      {list.length > 0 && (
        <div className="section">
          <h2>Recent runs</h2>
          <table>
            <thead>
              <tr>
                <th scope="col">Time</th>
                <th scope="col">Model</th>
                <th scope="col" className="num">Tokens</th>
                <th scope="col" className="num">First token</th>
                <th scope="col" className="num">Speed</th>
              </tr>
            </thead>
            <tbody>
              {list.slice(0, 20).map((r) => (
                <tr key={r.at}>
                  <td>{new Date(r.at).toLocaleTimeString()}</td>
                  <td>
                    {r.model}
                    {r.stopped ? " (stopped)" : ""}
                  </td>
                  <td className="num">{num(r.completion_tokens)}</td>
                  <td className="num">{r.ttft_ms == null ? "—" : `${num(r.ttft_ms)} ms`}</td>
                  <td className="num">{r.decode_tps == null ? "—" : `${num(r.decode_tps, 1)} tok/s`}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <div className="section">
        <h2>Service counters</h2>
        {metrics === null ? (
          <p className="muted">Counters appear when the background service is running.</p>
        ) : (
          <>
            <div className="toolbar">
              <label>
                <span className="small muted">Filter counters</span>
                <br />
                <input type="search" value={filter} onChange={(e) => setFilter(e.target.value)} />
              </label>
            </div>
            {rows.length === 0 ? (
              <p className="muted">No counters match.</p>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th scope="col">Counter</th>
                    <th scope="col" className="num">Value</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map(([k, v]) => (
                    <tr key={k}>
                      <td>
                        <code>{k}</code>
                      </td>
                      <td className="num">{num(v, 3)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </>
        )}
      </div>
    </section>
  );
}
