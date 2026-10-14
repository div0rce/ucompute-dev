import { useEffect, useState } from "react";
import { api, type PrivacyMode } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { href } from "../router";
import { daemon, privacy } from "../state/stores";

export const MODE_LABEL: Record<PrivacyMode, string> = { local_only: "Local only", trusted_group: "Trusted devices" };

/**
 * Privacy (F-10-10). Leaving local-only shows the daemon's disclosure and needs an explicit acknowledgement
 * before anything is sent; going back to local-only needs none.
 */
export function Privacy() {
  const d = daemon.use();
  const current = privacy.use();
  const [pick, setPick] = useState<PrivacyMode | null>(null);
  const [ack, setAck] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<MappedError | null>(null);
  const [saved, setSaved] = useState("");

  useEffect(() => {
    if (d.reachable === false) return;
    api.privacy().then(privacy.set, (e) => setError(explain(e)));
  }, [d.reachable]);

  const mode = current?.mode ?? "local_only";
  const selected = pick ?? mode;
  const changed = selected !== mode;
  const needsAck = changed && selected !== "local_only";

  async function apply() {
    if (!changed || (needsAck && !ack)) return;
    setSaving(true);
    setError(null);
    try {
      const p = await api.setPrivacy({ mode: selected, acknowledged: needsAck ? ack : false });
      privacy.set(p);
      setPick(null);
      setAck(false);
      setSaved(`Switched to ${MODE_LABEL[p.mode].toLowerCase()}.`);
    } catch (e) {
      setError(explain(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Privacy</h1>
        <p>Where your prompts and the model's replies may go. The current mode is shown at the top of the sidebar on every screen.</p>
      </div>

      {d.reachable === false && <div className="empty">Available once the background service is running.</div>}
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}

      {current && (
        <>
          <div className="glance" role="status">
            <span className={`dot ${mode === "local_only" ? "ok" : ""}`} aria-hidden="true" />
            <span>
              Current mode: {MODE_LABEL[mode]}. {mode === "local_only" ? "Nothing leaves this Mac." : "Requests may run on a paired device when you pick one."}
            </span>
          </div>

          <div className="section">
            <fieldset>
              <legend>
                <h2>Mode</h2>
              </legend>
              <div className="modes" role="radiogroup" aria-label="Privacy mode">
                <label className="mode">
                  <input type="radio" name="privacy" value="local_only" checked={selected === "local_only"} onChange={() => { setPick("local_only"); setSaved(""); }} />
                  <span>
                    <strong>Local only</strong>
                    <span className="small muted">Everything runs on this Mac. The network is never used for prompts or replies.</span>
                  </span>
                </label>
                <label className="mode">
                  <input type="radio" name="privacy" value="trusted_group" checked={selected === "trusted_group"} onChange={() => { setPick("trusted_group"); setSaved(""); }} />
                  <span>
                    <strong>Trusted devices</strong>
                    <span className="small muted">
                      A prompt may run on a device you paired in <a href={href("peers")}>Peers</a>, but only when you choose that device under "Run on".
                    </span>
                  </span>
                </label>
                <label className="mode disabled">
                  <input type="radio" name="privacy" value="public" disabled aria-describedby="public-help" />
                  <span>
                    <strong>Public compute</strong>
                    <span id="public-help" className="small muted">Coming later. Not available in this version and cannot be turned on.</span>
                  </span>
                </label>
              </div>
            </fieldset>

            {needsAck && (
              <div className="notice disclosure" role="region" aria-label="Before you switch">
                <div className="title">Before you switch</div>
                <p className="disclosure-text">{current.disclosure}</p>
                <label className="check" style={{ marginTop: 10 }}>
                  <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />I understand that a device I send a prompt to can see that prompt and its reply.
                </label>
              </div>
            )}

            {changed && (
              <div className="toolbar" style={{ marginTop: 14 }}>
                <button type="button" className="btn btn-primary" onClick={apply} disabled={saving || (needsAck && !ack)}>
                  {saving ? "Switching" : `Switch to ${MODE_LABEL[selected].toLowerCase()}`}
                </button>
                <button type="button" className="btn" onClick={() => { setPick(null); setAck(false); }} disabled={saving}>
                  Cancel
                </button>
              </div>
            )}
            <p role="status" className="ok small" style={{ marginTop: 10 }}>
              {saved}
            </p>
          </div>
        </>
      )}
    </section>
  );
}
