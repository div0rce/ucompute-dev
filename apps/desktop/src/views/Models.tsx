import { useCallback, useEffect, useRef, useState } from "react";
import { api, type Model, type PullEvent } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { FitBar } from "../components/FitBar";
import { openExternal, refreshStatus } from "../daemon";
import { fitText, humanBytes } from "../format";
import { navigate } from "../router";
import { lastError, settings } from "../state/stores";

type Confirm = { id: string; action: "download" | "delete" } | null;

/** `#/models?confirm=<id>` opens the download confirmation for that model (used by Home). */
function confirmFromHash(): Confirm {
  const q = window.location.hash.split("?")[1];
  const id = q ? new URLSearchParams(q).get("confirm") : null;
  return id ? { id, action: "download" } : null;
}

export function Models() {
  const [models, setModels] = useState<Model[] | null>(null);
  const [error, setError] = useState<MappedError | null>(null);
  const [query, setQuery] = useState("");
  const [confirm, setConfirm] = useState<Confirm>(confirmFromHash);
  const [rowError, setRowError] = useState<Record<string, MappedError>>({});
  const [progress, setProgress] = useState<Record<string, PullEvent>>({});
  const [busy, setBusy] = useState<Record<string, string>>({});
  const confirmRef = useRef<HTMLButtonElement>(null);

  const reload = useCallback(async () => {
    try {
      setModels(await api.models());
      setError(null);
    } catch (e) {
      setError(explain(e));
    }
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  useEffect(() => {
    if (confirm) confirmRef.current?.focus();
  }, [confirm]);

  const fail = (id: string, e: unknown) => {
    const m = explain(e);
    setRowError((r) => ({ ...r, [id]: m }));
    lastError.set(m);
  };

  async function download(m: Model) {
    setConfirm(null);
    setBusy((b) => ({ ...b, [m.id]: "Downloading" }));
    setRowError(({ [m.id]: _, ...rest }) => rest);
    try {
      await api.pull(m.id, (ev) => setProgress((p) => ({ ...p, [m.id]: ev })));
      await reload();
      setTimeout(() => document.getElementById(`run-${m.id}`)?.focus(), 0);
    } catch (e) {
      fail(m.id, e);
    } finally {
      setBusy(({ [m.id]: _, ...rest }) => rest);
      setProgress(({ [m.id]: _, ...rest }) => rest);
    }
  }

  async function run(m: Model) {
    setBusy((b) => ({ ...b, [m.id]: "Loading" }));
    try {
      const s = settings.get();
      await api.load(m.id, { device: s.device, ...(s.n_ctx ? { n_ctx: s.n_ctx } : {}) });
      await refreshStatus();
      navigate("chat");
    } catch (e) {
      fail(m.id, e);
      setBusy(({ [m.id]: _, ...rest }) => rest);
    }
  }

  async function remove(m: Model) {
    setConfirm(null);
    setBusy((b) => ({ ...b, [m.id]: "Deleting" }));
    try {
      await api.remove(m.id);
      await reload();
      await refreshStatus();
    } catch (e) {
      fail(m.id, e);
    } finally {
      setBusy(({ [m.id]: _, ...rest }) => rest);
    }
  }

  const q = query.trim().toLowerCase();
  const shown = (models ?? []).filter(
    (m) => !q || [m.name, m.id, m.family, m.quantization, m.license ?? "", ...m.tags].some((s) => s.toLowerCase().includes(q)),
  );

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Models</h1>
        <p>Every model shows its size, license and whether it fits in this computer's memory before you download it.</p>
      </div>

      <div className="toolbar">
        <label className="grow">
          <span className="small muted">Search models</span>
          <input type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Name, family, quantization or license" style={{ width: "100%" }} />
        </label>
        <button type="button" className="btn" onClick={reload}>
          Refresh
        </button>
      </div>

      {error && <ErrorNotice error={error} />}
      {models && shown.length === 0 && <div className="empty">{models.length === 0 ? "The catalog is empty." : `No models match "${query}".`}</div>}

      {shown.length > 0 && (
        <table>
          <thead>
            <tr>
              <th scope="col">Model</th>
              <th scope="col">Size</th>
              <th scope="col">License</th>
              <th scope="col">Memory</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {shown.map((m) => {
              const p = progress[m.id];
              const isBusy = m.id in busy;
              const c = confirm?.id === m.id ? confirm : null;
              return (
                <tr key={m.id} className="model-row">
                  <td>
                    <div className="model-name">
                      {m.name}
                      {m.tags.includes("recommended") && <span className="tag recommended">Recommended</span>}
                      {m.loaded && <span className="tag">Loaded</span>}
                      {m.downloaded && !m.loaded && <span className="tag">Downloaded</span>}
                    </div>
                    <div className="model-meta">
                      {m.parameters} parameters, {m.quantization}, {m.chat ? "chat" : "text completion only"}
                    </div>
                    {p && (
                      <div style={{ marginTop: 8 }}>
                        <progress aria-label={`Downloading ${m.name}`} value={p.downloaded_bytes} max={p.total_bytes || undefined} />
                        <div className="progress-text">
                          {humanBytes(p.downloaded_bytes)} of {humanBytes(p.total_bytes)}
                          {p.resumed_from > 0 ? `, resumed from ${humanBytes(p.resumed_from)}` : ""}
                        </div>
                      </div>
                    )}
                    {c?.action === "download" && (
                      <div className="confirm" role="group" aria-label={`Confirm download of ${m.name}`}>
                        <div>
                          <strong>License: {m.license || "unknown"}.</strong> {humanBytes(m.bytes)} will be downloaded from {m.source}.
                          {m.redistribution && m.redistribution !== "unknown" ? ` Redistribution: ${m.redistribution}.` : ""}
                        </div>
                        <div className="actions">
                          <button ref={confirmRef} type="button" className="btn btn-primary btn-sm" onClick={() => download(m)}>
                            Download {humanBytes(m.bytes)}
                          </button>
                          {m.license_url && (
                            <button type="button" className="btn btn-sm" onClick={() => openExternal(m.license_url!)}>
                              View license
                            </button>
                          )}
                          <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
                            Cancel
                          </button>
                        </div>
                      </div>
                    )}
                    {c?.action === "delete" && (
                      <div className="confirm" role="group" aria-label={`Confirm deletion of ${m.name}`}>
                        <div>Delete the downloaded file for {m.name}? The catalog entry stays and it can be downloaded again.</div>
                        <div className="actions">
                          <button ref={confirmRef} type="button" className="btn btn-danger btn-sm" onClick={() => remove(m)}>
                            Delete file
                          </button>
                          <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
                            Cancel
                          </button>
                        </div>
                      </div>
                    )}
                    {rowError[m.id] && (
                      <div style={{ marginTop: 8 }}>
                        <ErrorNotice error={rowError[m.id]} onDismiss={() => setRowError(({ [m.id]: _, ...rest }) => rest)} />
                      </div>
                    )}
                  </td>
                  <td>{humanBytes(m.bytes)}</td>
                  <td>{m.license || "unknown"}</td>
                  <td>
                    <div className={m.fit.verdict === "fits" ? "ok" : "warn"}>{fitText(m.fit)}</div>
                    <FitBar fit={m.fit} estimate={m.estimate} label={m.name} />
                  </td>
                  <td>
                    <div className="row-actions">
                      {!m.downloaded && (
                        <button type="button" className="btn btn-primary btn-sm" disabled={isBusy || !!c} onClick={() => setConfirm({ id: m.id, action: "download" })}>
                          {busy[m.id] === "Downloading" ? "Downloading" : "Download"}
                        </button>
                      )}
                      {m.downloaded && !m.loaded && (
                        <button id={`run-${m.id}`} type="button" className="btn btn-primary btn-sm" disabled={isBusy} onClick={() => run(m)}>
                          {busy[m.id] === "Loading" ? "Loading" : "Run"}
                        </button>
                      )}
                      {m.loaded && (
                        <button type="button" className="btn btn-primary btn-sm" onClick={() => navigate("chat")}>
                          Open chat
                        </button>
                      )}
                      {m.downloaded && (
                        <button type="button" className="btn btn-danger btn-sm" disabled={isBusy || !!c} onClick={() => setConfirm({ id: m.id, action: "delete" })}>
                          Delete
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
