import { useCallback, useEffect, useRef, useState } from "react";
import { api, expiresAtMs, type Discovered, type Group, type NetworkState, type PairingSession, type Peer } from "../api/client";
import { explain, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { Groups } from "../components/Groups";
import { daemon, settings } from "../state/stores";

/** 64 hex as 8 groups of 8, the form the pairing confirmation compares. */
export const grouped = (hex: string) => (hex.match(/.{1,8}/g) ?? [hex]).join(" ");
/** "123456" -> "123 456": easier to read aloud across a room. */
const spaced = (code: string) => `${code.slice(0, 3)} ${code.slice(3)}`;

const HEALTH: Record<Peer["health"], string> = {
  connected: "Connected",
  unhealthy: "Not responding",
  reconnecting: "Reconnecting",
  disconnected: "Offline",
};

type Confirm = { id: string; action: "revoke" | "remove" } | null;

/**
 * Peers (F-10-05): networking switch, this device's identity, pairing in both directions, the trusted list,
 * groups and nearby devices. `pollMs` is how often pairing status and the peer list refresh.
 */
export function Peers({ pollMs = 1000 }: { pollMs?: number }) {
  const d = daemon.use();
  const s = settings.use();
  const [net, setNet] = useState<NetworkState | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  const [discovered, setDiscovered] = useState<Discovered[]>([]);
  const [groups, setGroups] = useState<Group[]>([]);
  const [error, setError] = useState<MappedError | null>(null);
  const [session, setSession] = useState<PairingSession | null>(null);
  const [pairResult, setPairResult] = useState<string>("");
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [join, setJoin] = useState({ addr: "", code: "" });
  const [joining, setJoining] = useState(false);
  const [joinError, setJoinError] = useState<MappedError | null>(null);
  const [joined, setJoined] = useState<Peer | null>(null);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const codeRef = useRef<HTMLInputElement>(null);

  const fail = (e: unknown) => setError(explain(e));

  const reload = useCallback(async () => {
    try {
      const [n, p, g] = await Promise.all([api.network(), api.peers(), api.groups()]);
      setNet(n);
      setPeers(p.peers);
      setDiscovered(p.discovered.filter((x) => !x.trusted));
      setGroups(g);
      setError(null);
    } catch (e) {
      setError(explain(e));
    }
  }, []);

  useEffect(() => {
    if (d.reachable === false) return;
    reload();
  }, [reload, d.reachable]);

  // Health and nearby devices change on their own; refresh while networking is on.
  useEffect(() => {
    if (!net?.enabled) return;
    const t = setInterval(reload, pollMs * 5);
    return () => clearInterval(t);
  }, [net?.enabled, reload, pollMs]);

  // While a code is shown: poll the daemon for the outcome and tick the countdown.
  useEffect(() => {
    if (!session) return;
    let stopped = false;
    const tick = () => setSecondsLeft(Math.max(0, Math.round((expiresAtMs(session.expires_at) - Date.now()) / 1000)));
    tick();
    const poll = async () => {
      try {
        const st = await api.pairStatus();
        if (stopped) return;
        if (st.result) {
          setPairResult(`Paired with ${st.result.name || st.result.short_label}.`);
          setSession(null);
          reload();
        } else if (!st.active) {
          setPairResult("The code expired before another device joined. Start again for a fresh one.");
          setSession(null);
        }
      } catch (e) {
        if (stopped) return;
        fail(e);
        setSession(null);
      }
    };
    poll();
    const p = setInterval(poll, pollMs);
    const c = setInterval(tick, 1000);
    return () => {
      stopped = true;
      clearInterval(p);
      clearInterval(c);
    };
  }, [session, pollMs, reload]);

  useEffect(() => {
    if (confirm) confirmRef.current?.focus();
  }, [confirm]);

  async function setNetwork(patch: Partial<Pick<NetworkState, "enabled" | "serve_peers">>) {
    if (!net) return;
    try {
      setNet(await api.setNetwork({ enabled: net.enabled, serve_peers: net.serve_peers, ...patch }));
      setError(null);
    } catch (e) {
      fail(e);
    }
  }

  async function startPairing() {
    setPairResult("");
    try {
      setSession(await api.pairStart(s.device_name.trim() || "This Mac"));
      setError(null);
    } catch (e) {
      fail(e);
    }
  }

  async function submitJoin(e: React.FormEvent) {
    e.preventDefault();
    const code = join.code.replace(/\D/g, "");
    if (!join.addr.trim() || code.length !== 6) return;
    setJoining(true);
    setJoinError(null);
    setJoined(null);
    try {
      const peer = await api.pairJoin({ addr: join.addr.trim(), code, name: s.device_name.trim() || "This Mac" });
      setJoined(peer);
      setJoin({ addr: "", code: "" });
      reload();
    } catch (err) {
      setJoinError(explain(err));
    } finally {
      setJoining(false);
    }
  }

  async function act(p: Peer, action: "revoke" | "remove") {
    setConfirm(null);
    try {
      if (action === "revoke") await api.revokePeer(p.id);
      else await api.removePeer(p.id);
      await reload();
    } catch (e) {
      fail(e);
    }
  }

  function pairWith(x: Discovered) {
    setJoin({ addr: x.addr, code: "" });
    setJoined(null);
    setJoinError(null);
    setTimeout(() => codeRef.current?.focus(), 0);
  }

  const enabled = net?.enabled ?? false;
  const offline = d.reachable === false;

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Peers</h1>
        <p>
          Pair the computers you trust so they can run models for each other. Unpaired devices are refused before any data is exchanged, and
          nothing leaves this Mac while Privacy is set to local only.
        </p>
      </div>

      {offline && <div className="empty">Available once the background service is running.</div>}
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}

      {net && (
        <>
          <div className="section">
            <h2>Networking</h2>
            <label className="check">
              <input type="checkbox" role="switch" checked={net.enabled} aria-describedby="net-help" onChange={(e) => setNetwork({ enabled: e.target.checked, ...(e.target.checked ? {} : { serve_peers: false }) })} />
              Let this Mac talk to paired devices
            </label>
            <p id="net-help" className="help muted small" style={{ maxWidth: "64ch" }}>
              Off by default. When on, Common listens on <code>{net.listen_addr}</code> for connections from paired devices only, encrypted and
              checked against the key saved at pairing.{net.mdns ? " It also announces its id on this network so paired devices can find it." : ""}
            </p>
            <label className="check" style={{ marginTop: 10 }}>
              <input type="checkbox" checked={net.serve_peers} disabled={!net.enabled} aria-describedby="serve-help" onChange={(e) => setNetwork({ serve_peers: e.target.checked })} />
              Run models for paired devices when they ask
            </label>
            <p id="serve-help" className="help muted small" style={{ maxWidth: "64ch" }}>
              Off by default. Even when on, only devices in a group that allows it can send work here.
            </p>
          </div>

          <div className="section">
            <h2>This device</h2>
            <dl className="kv">
              <dt>Name</dt>
              <dd>
                <label>
                  <span className="visually-hidden">Name other devices see</span>
                  <input type="text" value={s.device_name} maxLength={64} onChange={(e) => settings.set({ ...s, device_name: e.target.value })} />
                </label>
              </dd>
              <dt>Short label</dt>
              <dd>{net.short_label}</dd>
              <dt>Device id</dt>
              <dd>
                <code className="device-id">{grouped(net.peer_id)}</code>
              </dd>
            </dl>
            <p className="small muted" style={{ marginTop: 8, maxWidth: "64ch" }}>
              Compare the full id on both screens when pairing. The short label only tells devices apart; it is not proof of identity.
            </p>
          </div>

          <div className="section pair-grid">
            <div className="pair-panel">
              <h2>Pair this device</h2>
              {!session ? (
                <>
                  <p className="muted small">Shows a one-time code for two minutes. Enter it on the other device under "Join a device".</p>
                  <button type="button" className="btn btn-primary" onClick={startPairing} disabled={!enabled} style={{ marginTop: 10 }}>
                    Pair this device
                  </button>
                  {!enabled && <p className="small muted" style={{ marginTop: 6 }}>Turn on networking first.</p>}
                  {pairResult && (
                    <p role="status" className={pairResult.startsWith("Paired") ? "ok" : "warn"} style={{ marginTop: 10 }}>
                      {pairResult}
                    </p>
                  )}
                </>
              ) : (
                <div className="pair-live" role="group" aria-label="Pairing code">
                  <div className="pair-code">{spaced(session.code)}</div>
                  <div className="small muted">
                    {secondsLeft > 0 ? `Expires in ${secondsLeft} s.` : "Expiring."} Port {session.port}.
                  </div>
                  <details style={{ marginTop: 10 }}>
                    <summary>Show as text for a QR app</summary>
                    <code className="qr-payload">{session.qr_payload}</code>
                  </details>
                  <p role="status" className="small" style={{ marginTop: 10 }}>
                    Waiting for the other device.
                  </p>
                </div>
              )}
            </div>

            <div className="pair-panel">
              <h2>Join a device</h2>
              <form onSubmit={submitJoin}>
                <div className="field">
                  <label htmlFor="join-addr">Address</label>
                  <input id="join-addr" type="text" value={join.addr} placeholder="192.168.1.20:54321" autoComplete="off" disabled={!enabled} onChange={(e) => setJoin({ ...join, addr: e.target.value })} />
                  <span className="help">Shown under the code on the other device, or pick a nearby device below.</span>
                </div>
                <div className="field">
                  <label htmlFor="join-code">Code</label>
                  <input
                    id="join-code"
                    ref={codeRef}
                    type="text"
                    inputMode="numeric"
                    pattern="[0-9 ]*"
                    maxLength={7}
                    value={join.code}
                    autoComplete="one-time-code"
                    disabled={!enabled}
                    onChange={(e) => setJoin({ ...join, code: e.target.value })}
                  />
                  <span className="help">The six digits on the other device's screen.</span>
                </div>
                <button type="submit" className="btn btn-primary" disabled={!enabled || joining || !join.addr.trim() || join.code.replace(/\D/g, "").length !== 6}>
                  {joining ? "Joining" : "Join"}
                </button>
              </form>
              {joinError && (
                <div style={{ marginTop: 10 }}>
                  <ErrorNotice error={joinError} onDismiss={() => setJoinError(null)} />
                </div>
              )}
              {joined && (
                <div className="notice ok" role="status" style={{ marginTop: 10 }}>
                  <div className="title">Paired with {joined.name || joined.short_label}.</div>
                  <div>Add it to a group to let it run work for this Mac.</div>
                </div>
              )}
            </div>
          </div>

          <div className="section">
            <h2>Paired devices</h2>
            {peers.length === 0 ? (
              <div className="empty">No paired devices. Pair one above.</div>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th scope="col">Device</th>
                    <th scope="col">Health</th>
                    <th scope="col">Groups</th>
                    <th scope="col">Last seen</th>
                    <th scope="col">
                      <span className="visually-hidden">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {peers.map((p) => {
                    const c = confirm?.id === p.id ? confirm : null;
                    return (
                      <tr key={p.id}>
                        <td>
                          <div className="model-name">{p.name || p.short_label}</div>
                          <div className="model-meta">{p.short_label}</div>
                          {c && (
                            <div className="confirm" role="group" aria-label={`Confirm ${c.action === "revoke" ? "revoking" : "removing"} ${p.name || p.short_label}`}>
                              <div>
                                {c.action === "revoke"
                                  ? `Revoke ${p.name || p.short_label}? Its connections close now, every device in its groups is told, and it cannot reconnect until it is paired again.`
                                  : `Remove ${p.name || p.short_label} from this Mac? It is forgotten here; other devices keep their own pairing with it.`}
                              </div>
                              <div className="actions">
                                <button ref={confirmRef} type="button" className="btn btn-danger btn-sm" onClick={() => act(p, c.action)}>
                                  {c.action === "revoke" ? "Revoke" : "Remove"}
                                </button>
                                <button type="button" className="btn btn-sm" onClick={() => setConfirm(null)}>
                                  Cancel
                                </button>
                              </div>
                            </div>
                          )}
                        </td>
                        <td>
                          <span className={p.health === "connected" ? "ok" : p.health === "disconnected" ? "muted" : "warn"}>{HEALTH[p.health] ?? p.health}</span>
                        </td>
                        <td>{p.groups.length ? p.groups.join(", ") : <span className="muted">none</span>}</td>
                        <td>{p.last_seen_utc ? new Date(p.last_seen_utc).toLocaleString() : "never"}</td>
                        <td>
                          <div className="row-actions">
                            <button type="button" className="btn btn-danger btn-sm" disabled={!!c} aria-label={`Revoke ${p.name || p.short_label}`} onClick={() => setConfirm({ id: p.id, action: "revoke" })}>
                              Revoke
                            </button>
                            <button type="button" className="btn btn-sm" disabled={!!c} aria-label={`Remove ${p.name || p.short_label}`} onClick={() => setConfirm({ id: p.id, action: "remove" })}>
                              Remove
                            </button>
                          </div>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </div>

          <Groups groups={groups} peers={peers} onChange={setGroups} onError={fail} />

          <div className="section">
            <h2>Nearby, not paired</h2>
            <p className="muted small" style={{ maxWidth: "64ch" }}>
              Devices announcing themselves on this network that this Mac does not trust. They cannot send or receive anything until paired.
            </p>
            {discovered.length === 0 ? (
              <div className="empty" style={{ marginTop: 10 }}>{enabled ? "No unpaired devices nearby." : "Turn on networking to look for nearby devices."}</div>
            ) : (
              <table style={{ marginTop: 10 }}>
                <thead>
                  <tr>
                    <th scope="col">Device</th>
                    <th scope="col">Address</th>
                    <th scope="col">
                      <span className="visually-hidden">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {discovered.map((x) => (
                    <tr key={x.id}>
                      <td>
                        <div className="model-name">{x.short_label}</div>
                        <div className="model-meta">Not trusted</div>
                      </td>
                      <td>{x.addr}</td>
                      <td>
                        <div className="row-actions">
                          {x.pairing ? (
                            <button type="button" className="btn btn-sm" aria-label={`Pair with ${x.short_label}`} onClick={() => pairWith(x)}>
                              Pair
                            </button>
                          ) : (
                            <span className="small muted">Not in pairing mode</span>
                          )}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </>
      )}
    </section>
  );
}
