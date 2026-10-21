import { useEffect, useState } from "react";
import { api, type Hardware, type Model } from "../api/client";
import { explain } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { FitBar } from "../components/FitBar";
import { RunOn } from "../components/RunOn";
import { refreshStatus } from "../daemon";
import { fitText, humanBytes } from "../format";
import { href, navigate } from "../router";
import { daemon, lastError, privacy, settings } from "../state/stores";

export function Home() {
  const d = daemon.use();
  const err = lastError.use();
  const localOnly = (privacy.use()?.mode ?? "local_only") === "local_only";
  const [hw, setHw] = useState<Hardware | null>(null);
  const [models, setModels] = useState<Model[] | null>(null);
  const [running, setRunning] = useState(false);

  useEffect(() => {
    if (!d.reachable) return;
    api.hardware().then(setHw, () => undefined);
    api.models().then(setModels, () => undefined);
  }, [d.reachable]);

  const loaded = d.status?.loaded ?? null;
  const anyDownloaded = models?.some((m) => m.downloaded) ?? false;
  const step = loaded ? 3 : anyDownloaded ? 2 : 1;
  const recommended = models?.find((m) => m.tags.includes("recommended")) ?? null;

  const glance =
    d.reachable === null
      ? "Connecting to Common's background service."
      : d.reachable === false
        ? "Common's background service is not running."
        : loaded
          ? `${loaded.model_id} is loaded on ${loaded.device_actual}.`
          : "Common is running. No model is loaded.";
  const hwLine = hw ? `${hw.host.chip}, ${humanBytes(hw.memory.total_bytes)} memory, ${hw.gpus.length ? hw.gpus[0].backend : "no GPU backend"}.` : "";

  async function runRecommended(m: Model) {
    setRunning(true);
    try {
      const s = settings.get();
      await api.load(m.id, { device: s.device, ...(s.n_ctx ? { n_ctx: s.n_ctx } : {}) });
      await refreshStatus();
      navigate("chat");
    } catch (e) {
      lastError.set(explain(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Home</h1>
        <p>{localOnly ? "Run AI models on this computer. Nothing you type leaves it." : "Run AI models on this computer or on a device you paired. A prompt leaves this computer only for the device you pick."}</p>
      </div>

      <div className="glance" role="status">
        <span className={`dot ${d.reachable ? "ok" : d.reachable === false ? "bad" : ""}`} aria-hidden="true" />
        <span>
          {glance} {hwLine}
        </span>
      </div>
      {d.reachable && (
        <div className="toolbar" style={{ marginTop: 12, marginBottom: 0 }}>
          <RunOn />
        </div>
      )}

      <ol className="journey" aria-label="Getting started">
        {[
          ["Choose a model", "Pick one that fits in memory and download it.", "models"],
          ["Run", "Load it onto the CPU or GPU.", "models"],
          ["Chat", "Talk to it, right here.", "chat"],
        ].map(([t, desc, to], i) => {
          const n = i + 1;
          return (
            <li key={t} className={`journey-step${n < step ? " done" : ""}`} aria-current={n === step ? "step" : undefined}>
              <div className="n">Step {n}{n < step ? ", done" : n === step ? ", you are here" : ""}</div>
              <div className="t">
                <a href={href(to as "models" | "chat")} style={{ color: "inherit", textDecoration: "none" }}>
                  {t}
                </a>
              </div>
              <div className="d">{desc}</div>
            </li>
          );
        })}
      </ol>

      <div className="section">
        <h2>Recommended for this computer</h2>
        {recommended ? (
          <div className="model-hero">
            <div className="row">
              <div>
                <div className="model-name">{recommended.name}</div>
                <div className="model-meta">
                  {humanBytes(recommended.bytes)}, license {recommended.license || "unknown"}, {recommended.quantization}
                  {recommended.chat ? ", chat" : ""}
                </div>
              </div>
              {recommended.loaded ? (
                <a className="btn btn-primary" href={href("chat")}>
                  Open chat
                </a>
              ) : recommended.downloaded ? (
                <button type="button" className="btn btn-primary" disabled={running} onClick={() => runRecommended(recommended)}>
                  {running ? "Loading" : "Run"}
                </button>
              ) : (
                <a className="btn btn-primary" href={`${href("models")}?confirm=${encodeURIComponent(recommended.id)}`}>
                  Download
                </a>
              )}
            </div>
            <div className={recommended.fit.verdict === "fits" ? "ok" : "warn"}>{fitText(recommended.fit)}</div>
            <FitBar fit={recommended.fit} estimate={recommended.estimate} label={recommended.name} />
          </div>
        ) : (
          <div className="empty">
            {d.reachable === false ? "Available once the background service is running." : models === null ? "Waiting for the catalog." : "The catalog has no recommended model for this computer yet."}{" "}
            <a href={href("models")}>Browse all models</a>.
          </div>
        )}
      </div>

      {err && (
        <div className="section">
          <h2>Last problem</h2>
          <ErrorNotice error={err} onDismiss={() => lastError.set(null)} />
        </div>
      )}
    </section>
  );
}
