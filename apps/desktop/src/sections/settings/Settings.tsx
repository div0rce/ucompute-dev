import "./settings.css";
import { useState, type ReactNode } from "react";
// Named import: Vite tree-shakes JSON, so only the version reaches the bundle (not the dependency list).
import { version as appVersion } from "../../../package.json";
import { api, getBaseUrl, shortLabel, type PrivacyMode } from "../../api/client";
import { explain, type MappedError } from "../../api/errors";
import { devDaemonUrl, refreshStatus } from "../../daemon";
import { clock, humanBytes, num, plural } from "../../format";
import { href, useLocation } from "../../router";
import { groups as groupsStore, hardware, mesh, models, network, refreshFresh, useNeed } from "../../state/live";
import { chatHistory, daemon, privacy, settings } from "../../state/stores";
import { ui, type Theme } from "../../state/ui";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { FirewallNotice } from "../../ui/FirewallNotice";
import { GroupsEditor } from "../../ui/Groups";
import { KV, StateBadge, Unavailable } from "../../ui/primitives";
import { frameState, SectionFrame } from "../../ui/SectionFrame";
import { session } from "../chat/session";

export const PAGES = [
  { id: "general", label: "General" },
  { id: "execution", label: "Execution" },
  { id: "network", label: "Network" },
  { id: "sharing", label: "Sharing & permissions" },
  { id: "privacy", label: "Privacy" },
  { id: "security", label: "Security" },
  { id: "storage", label: "Storage" },
] as const;
type Page = (typeof PAGES)[number]["id"];

const READ_ONLY = "Configured in the service settings file; applies after restart.";
const MODE_LABEL: Record<PrivacyMode, string> = { local_only: "Local only", trusted_group: "Trusted devices" };
/** The "what is kept" statement (plan §2.7); tested verbatim. */
export const KEPT =
  "The service never stores prompts or replies. It keeps request details (model, device, tokens, timing) in memory until it restarts. It also keeps a log of pairing, connections and requests to and from other devices (audit.jsonl). Your conversation is stored only in this app on this computer.";

/** A switch (`role=switch`) that calls its route; the outcome is an inline status. */
function Switch({ label, checked, disabledReason, onChange, help }: { label: string; checked: boolean; disabledReason?: string | null; onChange: (on: boolean) => void; help?: string }) {
  const id = `sw-${label.replace(/\W+/g, "-").toLowerCase()}`;
  return (
    <div className="settings-row">
      <label className="check">
        <input type="checkbox" role="switch" className="switch" checked={checked} disabled={!!disabledReason} aria-describedby={`${id}-help`} onChange={(e) => onChange(e.target.checked)} />
        {label}
      </label>
      <p id={`${id}-help`} className="note settings-help">
        {disabledReason ?? help}
      </p>
    </div>
  );
}

/** PUT /api/network with an inline outcome. */
function useNetworkPut() {
  const [status, setStatus] = useState("");
  const [error, setError] = useState<MappedError | null>(null);
  const [busy, setBusy] = useState(false);
  async function put(body: { enabled: boolean; serve_peers: boolean }, done: string) {
    setBusy(true);
    setError(null);
    setStatus("");
    try {
      network.set({ data: await api.setNetwork(body), at: Date.now(), error: null });
      setStatus(done);
      void refreshFresh("mesh");
    } catch (e) {
      setError(explain(e));
    } finally {
      setBusy(false);
    }
  }
  const outcome = (
    <>
      <p role="status" className="small tone-ok settings-status">
        {status}
      </p>
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}
    </>
  );
  return { put, busy, outcome };
}

function General() {
  const theme = ui.use().theme;
  const d = daemon.use();
  const dev = devDaemonUrl(window.location.search);
  return (
    <>
      <fieldset className="settings-fieldset">
        <legend>Appearance</legend>
        <div role="radiogroup" aria-label="Appearance" className="settings-radios">
          {(["system", "light", "dark"] as Theme[]).map((t) => (
            <label key={t} className="check">
              <input type="radio" name="theme" value={t} checked={theme === t} onChange={() => ui.set((u) => ({ ...u, theme: t }))} />
              {t === "system" ? "System" : t === "light" ? "Light" : "Dark"}
            </label>
          ))}
        </div>
      </fieldset>
      <h3>About</h3>
      <KV
        items={[
          { k: "App version", v: appVersion },
          {
            k: "Service",
            v:
              d.reachable === null ? (
                <StateBadge tone="active" icon="loader" label="Checking" />
              ) : d.reachable === false ? (
                <StateBadge tone="danger" icon="circle-x" label="Not running" />
              ) : d.responding === false ? (
                <StateBadge tone="warn" icon="circle-alert" label="Busy (not answering)" />
              ) : (
                <StateBadge tone="ok" icon="circle-check" label="Running" />
              ),
          },
          { k: "Service version", v: d.status?.version ?? d.version ?? <Unavailable reason="not reported yet" /> },
          { k: "Service address", v: <code>{getBaseUrl()}</code>, note: dev ? "(chosen with ?daemon=, development only)" : "Only listens on this computer." },
          { k: "Started by", v: d.spawned ? "This app" : "Something else (already running when the app opened)" },
        ]}
      />
    </>
  );
}

function Execution() {
  const s = settings.use();
  const q = daemon.use().status?.queue;
  return (
    <>
      <fieldset className="settings-fieldset">
        <legend>Default device</legend>
        <div className="settings-radios" role="radiogroup" aria-label="Default device">
          {(["auto", "cpu", "gpu"] as const).map((v) => (
            <label key={v} className="check">
              <input type="radio" name="device" value={v} checked={s.device === v} onChange={() => settings.set({ ...s, device: v })} />
              {v === "auto" ? "Automatic" : v.toUpperCase()}
            </label>
          ))}
        </div>
        <p className="note settings-help">Automatic uses the GPU when the model fits there and says so if it had to fall back to the CPU.</p>
      </fieldset>
      <div className="settings-field">
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
        <p id="n-ctx-help" className="note settings-help">
          How much text the model can hold at once, in tokens. Larger values use more memory. Leave empty for the model's default.
        </p>
      </div>
      <p className="prose small">Applies to models this app loads; loads requested by other programs or paired devices use the service defaults.</p>
      <h3>Admission limits</h3>
      <KV
        items={[
          { k: "Requests at once", v: q ? num(q.max_concurrent) : <Unavailable reason="not reported yet" />, note: READ_ONLY },
          { k: "Waiting requests", v: q ? num(q.max_queue) : <Unavailable reason="not reported yet" />, note: READ_ONLY },
        ]}
      />
    </>
  );
}

/** Why a service value is not shown. The store keeps the last good value, but a stopped service has no current one. */
const unknownWhy = (reachable: boolean | null) => (reachable === false ? "Not known while the background service is not running." : "Not read yet.");

/** A switch-shaped row whose value is not known: the label, and why there is no value (never a last known position). */
function UnknownSwitch({ label, reachable }: { label: string; reachable: boolean | null }) {
  return (
    <div className="settings-row">
      <span>{label}</span> <Unavailable reason={unknownWhy(reachable)} />
    </div>
  );
}

function Network() {
  const reachable = daemon.use().reachable;
  const netData = network.use().data;
  const net = reachable === true ? netData : null;
  const s = settings.use();
  const { put, busy, outcome } = useNetworkPut();
  const unknown = <Unavailable reason={unknownWhy(reachable)} />;
  return (
    <>
      {net ? (
        <Switch
          label="Networking"
          checked={net.enabled}
          disabledReason={busy ? "Applying…" : null}
          help="Lets paired devices connect and nearby devices be listed. Turning it off also stops sharing."
          // Turning networking off also clears serve_peers (existing rule).
          onChange={(on) => void put({ enabled: on, serve_peers: on ? net.serve_peers : false }, on ? "Networking is on." : "Networking is off.")}
        />
      ) : (
        <UnknownSwitch label="Networking" reachable={reachable} />
      )}
      {outcome}
      <div className="settings-field">
        <label htmlFor="device-name">Name other devices see when pairing</label>
        <input id="device-name" type="text" maxLength={64} value={s.device_name} placeholder={net?.device_name ?? ""} onChange={(e) => settings.set({ ...s, device_name: e.target.value })} />
        <p className="note settings-help">Leave empty to use this computer's own name.</p>
      </div>
      <KV
        items={[
          { k: "Discovery (mDNS)", v: net ? (net.mdns ? "On" : "Off") : unknown, note: READ_ONLY },
          { k: "Listening on", v: !net ? unknown : net.listen_addr ? <code>{net.listen_addr}</code> : "Not listening" },
          { k: "Address for other devices", v: !net ? unknown : net.dial_addr ? <code>{net.dial_addr}</code> : <Unavailable reason="only while networking is on" /> },
        ]}
      />
      <FirewallNotice />
    </>
  );
}

function Sharing() {
  const reachable = daemon.use().reachable;
  const netData = network.use().data;
  const net = reachable === true ? netData : null;
  const { put, busy, outcome } = useNetworkPut();
  const quota = net?.peer_quota;
  const unknown = <Unavailable reason={unknownWhy(reachable)} />;
  return (
    <>
      {net ? (
        <Switch
          label="Run requests for paired devices"
          checked={net.serve_peers}
          disabledReason={!net.enabled ? "Turn on networking first (Settings › Network)." : busy ? "Applying…" : null}
          help="Only devices in a group that allows prompts, within the quota below."
          onChange={(on) => void put({ enabled: net.enabled, serve_peers: on }, on ? "Sharing is on." : "Sharing is off.")}
        />
      ) : (
        <UnknownSwitch label="Run requests for paired devices" reachable={reachable} />
      )}
      {outcome}
      <KV
        items={[
          {
            k: "Device quota",
            v: quota ? `${num(quota.requests_per_minute)} requests/min · ${num(quota.tokens_per_minute)} tokens/min · ${num(quota.max_concurrent)} at once` : net ? <Unavailable reason="not reported yet" /> : unknown,
            note: READ_ONLY,
          },
          { k: "Slots for devices", v: net ? num(net.peer_slots) : unknown, note: READ_ONLY },
        ]}
      />
      <GroupsEditor />
    </>
  );
}

function PrivacyPage() {
  const d = daemon.use();
  const privLive = privacy.use();
  // Only what the running service reported is shown as current. The controls stay (inert) while it is down or starting,
  // so the one with focus is not removed and the status line below announces the change.
  const live = d.reachable === true;
  const current = privLive.data;
  const { messages } = chatHistory.use();
  const streaming = !!session.use().streaming;
  const [pick, setPick] = useState<PrivacyMode | null>(null);
  const [ack, setAck] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<MappedError | null>(null);
  const [saved, setSaved] = useState("");
  const [confirmDelete, setConfirmDelete] = useState(false);

  const mode = current?.mode ?? "local_only";
  // No mode checked while it is not known, unless the user picked one.
  const selected = pick ?? (live ? mode : null);
  const changed = selected !== null && selected !== mode;
  const needsAck = changed && selected !== "local_only";
  const inert = live ? undefined : ("true" as const);
  const choose = (m: PrivacyMode) => live && (setPick(m), setSaved(""));

  async function apply() {
    if (!live || !selected || !changed || (needsAck && !ack)) return;
    setSaving(true);
    setError(null);
    try {
      const p = await api.setPrivacy({ mode: selected, acknowledged: needsAck ? ack : false });
      privacy.set({ data: p, at: Date.now(), error: null });
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
    <>
      {d.reachable === false && !current && <p className="empty">The mode is available once the background service is running.</p>}
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}
      {current && (
        <>
          <p className="settings-current" role="status">
            {live ? (
              <>
                Current mode: {MODE_LABEL[mode]}. {mode === "local_only" ? "Nothing leaves this computer." : "Requests may run on a paired device when you pick one."}
                {privLive.error && <span className="note"> Not updated since {clock(privLive.at)}.</span>}
              </>
            ) : (
              `Current mode: not known ${d.reachable === false ? "while the background service is not running" : "until the background service answers"}.`
            )}
          </p>
          <fieldset className="settings-fieldset">
            <legend>Mode</legend>
            <div className="settings-modes" role="radiogroup" aria-label="Privacy mode">
              <label className="settings-mode">
                <input type="radio" name="privacy" value="local_only" checked={selected === "local_only"} aria-disabled={inert} onChange={() => choose("local_only")} />
                <span>
                  <strong>Local only</strong>
                  <span className="note">Everything runs on this computer. The network is never used for prompts or replies.</span>
                </span>
              </label>
              <label className="settings-mode">
                <input type="radio" name="privacy" value="trusted_group" checked={selected === "trusted_group"} aria-disabled={inert} onChange={() => choose("trusted_group")} />
                <span>
                  <strong>Trusted devices</strong>
                  <span className="note">
                    A prompt may run on a device you paired under <a href={href("devices")}>Devices</a>, but only when you choose it as the Destination in Chat.
                  </span>
                </span>
              </label>
            </div>
          </fieldset>
          {needsAck && (
            <div className="notice tone-warn" role="region" aria-label="Before you switch">
              <p className="notice-title">Before you switch</p>
              <p className="prose">{current.disclosure}</p>
              <label className="check settings-ack">
                <input type="checkbox" checked={ack} aria-disabled={inert} onChange={(e) => live && setAck(e.target.checked)} />I understand that a device I send a prompt to can see that prompt and its reply.
              </label>
            </div>
          )}
          {changed && (
            <div className="settings-actions">
              <button type="button" className="btn btn-primary" onClick={() => void apply()} disabled={saving || (needsAck && !ack)} aria-disabled={inert}>
                {saving ? "Switching" : `Switch to ${MODE_LABEL[selected ?? mode].toLowerCase()}`}
              </button>
              <button type="button" className="btn" onClick={() => (setPick(null), setAck(false))} disabled={saving}>
                Cancel
              </button>
            </div>
          )}
          <p role="status" className="small tone-ok settings-status">
            {saved}
          </p>
        </>
      )}
      <h3>What is kept</h3>
      <p className="prose">{KEPT}</p>
      <div className="settings-actions">
        {confirmDelete ? (
          <div role="group" aria-label="Delete chat history?" className="notice tone-warn">
            <p>Deletes the conversation stored in this app on this computer. It cannot be undone.</p>
            <div className="notice-actions">
              <button type="button" className="btn" autoFocus onClick={() => setConfirmDelete(false)}>
                Cancel
              </button>{" "}
              <button
                type="button"
                className="btn btn-danger"
                onClick={() => {
                  chatHistory.set({ messages: [] });
                  setConfirmDelete(false);
                  setSaved("Chat history deleted from this computer.");
                }}
              >
                Delete chat history
              </button>
            </div>
          </div>
        ) : (
          <button type="button" className="btn" disabled={messages.length === 0 || streaming} onClick={() => setConfirmDelete(true)} aria-describedby="delete-chat-help">
            Delete chat history on this computer
          </button>
        )}
        <span id="delete-chat-help" className="note">
          {streaming ? "A reply is being generated; stop it first." : messages.length === 0 ? "No chat history is stored." : plural(messages.length, "message") + " stored."}
        </span>
      </div>
    </>
  );
}

function Security() {
  const net = network.use().data;
  const m = mesh.use().data;
  const nameOf = (id: string) => (id === net?.peer_id ? "This computer" : (m?.peers.find((p) => p.id === id)?.name ?? shortLabel(id)));
  const revoked = m?.revocations ?? [];
  return (
    <>
      <h3>This device</h3>
      <KV
        items={[
          { k: "Device ID", v: net ? <code className="settings-id">{net.peer_id.match(/.{1,8}/g)!.join(" ")}</code> : <Unavailable reason="not read yet" />, note: "Compare it with what the other device shows when pairing." },
          { k: "Short label", v: net ? <code>{net.short_label}</code> : "—" },
        ]}
      />
      <h3>Revoked devices</h3>
      {revoked.length === 0 ? (
        <p className="empty">{m ? "No device has been revoked." : "Not read yet."}</p>
      ) : (
        <table className="settings-table">
          <thead>
            <tr>
              <th scope="col">Device</th>
              <th scope="col">Revoked at</th>
              <th scope="col">By</th>
              <th scope="col" className="num">
                Seq
              </th>
            </tr>
          </thead>
          <tbody>
            {revoked.map((r) => (
              <tr key={`${r.peer_id}-${r.seq}`}>
                <td>
                  <code>{shortLabel(r.peer_id)}</code>
                </td>
                <td>
                  {new Date(r.at_utc).toLocaleDateString()} {clock(r.at_utc)}
                </td>
                <td>{nameOf(r.revoked_by)}</td>
                <td className="num">{r.seq}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </>
  );
}

function Storage() {
  const disk = hardware.use().data?.disk_free_bytes;
  const list = models.use().data;
  const downloaded = list?.filter((m) => m.downloaded) ?? null;
  return (
    <KV
      items={[
        { k: "Disk free", v: disk != null ? humanBytes(disk) : <Unavailable reason="not reported by the service" /> },
        {
          k: "Downloaded models",
          v: downloaded ? `${plural(downloaded.length, "model")}, ${humanBytes(downloaded.reduce((a, m) => a + m.bytes, 0))}` : <Unavailable reason="not read yet" />,
        },
        { k: "Model folder", v: <Unavailable reason="not reported by the service" /> },
      ]}
    />
  );
}

const RENDER: Record<Page, () => ReactNode> = { general: General, execution: Execution, network: Network, sharing: Sharing, privacy: PrivacyPage, security: Security, storage: Storage };

/** Settings (plan §2.7): a page list and one form at a time. Controls apply at once; privacy needs acknowledgement. */
export default function Settings() {
  const d = daemon.use();
  useNeed(["network", "mesh", "groups", "hardware", "models", "privacy"]);
  const g = groupsStore.use();
  const { id } = useLocation();
  const page = PAGES.find((p) => p.id === id) ?? PAGES[0];
  const Body = RENDER[page.id];
  const fs = frameState(d, ...(page.id === "sharing" ? [g] : []));
  return (
    // Every page reads local preferences too, so a service that is down never empties Settings: pages say what waits for it.
    // A first read that failed is a banner here (pages keep their local preferences), not an emptied page.
    <SectionFrame title="Settings" state={typeof fs !== "object" ? "ok" : "failed" in fs ? { stale: { since: null, error: fs.failed } } : fs}>
      <div className="settings">
        <nav aria-label="Settings pages" className="settings-nav">
          <ul>
            {PAGES.map((p) => (
              <li key={p.id}>
                <a href={href("settings", p.id)} aria-current={p.id === page.id ? "page" : undefined}>
                  {p.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>
        <div className="settings-page">
          <h2>{page.label}</h2>
          {d.reachable === false && page.id !== "general" && page.id !== "privacy" && (
            <div className="notice tone-danger">
              <p className="notice-title">The background service is not running.</p>
              <p>Values from it are unavailable until it runs again.</p>
              <div className="notice-actions">
                <button type="button" className="btn" onClick={() => void refreshStatus()}>
                  Try again
                </button>
              </div>
            </div>
          )}
          <Body />
        </div>
      </div>
    </SectionFrame>
  );
}
