import { getBaseUrl } from "../api/client";
import { settings } from "../state/stores";

export function Settings() {
  const s = settings.use();
  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Settings</h1>
      </div>

      <div className="section">
        <h2>Running models</h2>
        <fieldset>
          <legend>Default device</legend>
          <div className="radio-row" role="radiogroup" aria-label="Default device">
            {(["auto", "cpu", "gpu"] as const).map((v) => (
              <label key={v}>
                <input type="radio" name="device" value={v} checked={s.device === v} onChange={() => settings.set({ ...s, device: v })} />
                {v === "auto" ? "Automatic" : v.toUpperCase()}
              </label>
            ))}
          </div>
          <p className="help muted small">Automatic uses the GPU when the model fits there and tells you if it had to fall back to the CPU.</p>
        </fieldset>
        <div className="field">
          <label htmlFor="n-ctx">Context length</label>
          <input
            id="n-ctx"
            type="number"
            min={128}
            step={128}
            value={s.n_ctx ?? ""}
            placeholder="Model default"
            aria-describedby="n-ctx-help"
            onChange={(e) => settings.set({ ...s, n_ctx: e.target.value ? Number(e.target.value) : null })}
          />
          <span id="n-ctx-help" className="help">
            How much text the model can hold at once, in tokens. Larger values use more memory. Leave empty for the model's default.
          </span>
        </div>
      </div>

      <div className="section">
        <h2>Background service</h2>
        <div className="field">
          <label htmlFor="base-url">Service address</label>
          <input id="base-url" type="text" readOnly value={getBaseUrl()} />
          <span className="help">The app talks to Common's background service at this address. It only listens on this computer.</span>
        </div>
      </div>

      <div className="section">
        <h2>Privacy</h2>
        <p>All inference runs on this computer. Nothing is sent anywhere.</p>
        <fieldset style={{ marginTop: 12 }}>
          <legend>Devices and shared compute</legend>
          <p className="muted small">Coming in a later release. These controls do nothing yet.</p>
          <label className="check" style={{ marginTop: 8 }}>
            <input type="checkbox" disabled /> Allow trusted devices to pair with this computer
          </label>
          <label className="check">
            <input type="checkbox" disabled /> Contribute spare compute to a shared pool
          </label>
        </fieldset>
      </div>
    </section>
  );
}
