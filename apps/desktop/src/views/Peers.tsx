import { useCallback, useEffect, useRef, useState } from "react";
import { api, expiresAtMs, pairingAddr, shortLabel, type Discovered, type Group, type NetworkState, type PairingSession, type Peer, type PeerRecord } from "../api/client";
import { explain, mapError, type MappedError } from "../api/errors";
import { ErrorNotice } from "../components/ErrorNotice";
import { Groups } from "../components/Groups";
import { allowThroughFirewall, firewallStatus, type FirewallStatus } from "../daemon";
import { daemon, privacy, settings } from "../state/stores";

/** 64 hex as 8 groups of 8, the form the pairing confirmation compares. */
export const grouped = (hex: string) => (hex.match(/.{1,8}/g) ?? [hex]).join(" ");
/** "123456" -> "123 456": easier to read aloud across a room. */
const spaced = (code: string) => `${code.slice(0, 3)} ${code.slice(3)}`;

/** A paired device's connection as the daemon reports it. */
export type ConnState = "connected" | "unhealthy" | "connecting" | "reconnecting" | "unreachable" | "offline" | "disconnected";
const STATE: Record<ConnState, [word: string, tone: string]> = {
  connected: ["Connected", "ok"],
  // Still connected, but missing keep-alives; the daemon closes the connection within seconds if it stays silent.
  unhealthy: ["Not responding", "warn"],
  connecting: ["Connecting", "warn"],
  reconnecting: ["Reconnecting", "warn"],
  unreachable: ["Unreachable", "warn"],
  offline: ["Offline", "muted"],
  disconnected: ["Not connected", "muted"],
};
/**
 * Only an open connection counts as connected: a signed mDNS record (`seen`) only says the device announced itself
 * nearby, and the daemon may still hold it after the device left. `dialing` is this view's own connect request;
 * `looking` is whether mDNS runs here, without which "not seen" means nothing. The daemon redials paired devices
 * while networking is on; `last_error` is what its last try ran into. `reconnecting` stays set after a lost
 * connection until one succeeds, so a failed redial since then says more: unreachable.
 */
export function connState(p: Peer, { seen, looking, dialing }: { seen: boolean; looking: boolean; dialing: boolean }): ConnState {
  if (p.connected) return p.health === "unhealthy" ? "unhealthy" : "connected";
  if (dialing) return "connecting";
  if (p.last_error) return "unreachable";
  if (p.health === "reconnecting") return "reconnecting";
  return looking && !seen ? "offline" : "disconnected";
}
/**
 * Whether prompts sent from this computer can run on `p` now, and otherwise what is missing, from what this computer
 * knows: an open connection, one of its groups allowing prompts, and Privacy. The other device's own groups and
 * settings are not visible here.
 */
export function promptReadiness(p: Peer, groups: Group[], localOnly: boolean): { ok: boolean; text: string } {
  const steps = [
    ...(p.connected ? [] : ["connect it"]),
    ...(groups.some((g) => g.members.includes(p.id) && g.policy.allow_remote_inference)
      ? []
      : [p.groups.length ? `turn on "Members may run prompts for each other" in ${p.groups.length === 1 ? "its group" : "one of its groups"}` : "add it to a group that allows prompts"]),
    ...(localOnly ? ["allow trusted devices under Privacy"] : []),
  ];
  return steps.length ? { ok: false, text: `Cannot run prompts from this computer yet: ${steps.join(", ")}.` } : { ok: true, text: "Can run prompts from this computer." };
}
const nameOf = (p: PeerRecord & { short_label?: string }) => p.name || p.short_label || shortLabel(p.id);

/**
 * A refused connect. An address proves nothing, so the daemon reports `peer_unauthorized` both for whatever answered
 * without proving it is the paired device (another device, or an alert sent before any key: common-network
 * transport.rs `unproven`) and for the paired device itself refusing this computer. Name both; never tell the user to
 * re-pair a device that may be fine.
 */
function connectError(err: unknown, name: string): MappedError {
  const m = explain(err);
  if (m.code !== "peer_unauthorized") return m;
  return {
    ...m,
    title: `The device that answered did not prove it is ${name}, or ${name} no longer trusts this computer.`,
    next_step: `Nothing was trusted or saved. Check the address shown on ${name} under Peers > This device.`,
  };
}
/** Daemon messages and hints are lower-case clauses; this makes one a sentence without doubling its full stop. */
const sentence = (s: string) => `${s.charAt(0).toUpperCase()}${s.slice(1).replace(/\.$/, "")}.`;
/** The code field takes either the six digits or a pasted `common-pair:v1:...` payload (which carries the address). */
const isPayload = (code: string) => code.trim().startsWith("common-pair:");

type Confirm = { id: string; action: "revoke" | "remove" } | null;
/** The open "connect" panel of one paired device: an optional address typed by the user, and the outcome. */
type Link = { id: string; addr: string; busy: boolean; error: MappedError | null } | null;
type Outcome = { ok: boolean; text: string } | null;

/**
 * Windows only (src-tauri `firewall.rs`): Windows Firewall drops what other devices send this computer's pairing
 * window (a random port) and listener, and never asks about Common's background service. One consent step adds a
 * rule for that program only, inbound UDP on Private networks, through Windows' own confirmation.
 */
function FirewallNotice({ fw, busy, outcome, onAllow }: { fw: FirewallStatus; busy: boolean; outcome: Outcome; onAllow: () => void }) {
  const nets = fw.public_networks ?? [];
  // The rule is for Private networks only, and Windows puts a new Wi-Fi network or hotspot on Public: say so, and leave
  // that choice to the user (the app never widens the rule).
  const publicNote = nets.length > 0 && (
    <div className="small">
      Windows treats {nets.join(", ")} as Public, where this rule does not apply, so other devices cannot reach this computer there. If you trust a
      network, set it to Private in Windows Settings &gt; Network &amp; internet.
    </div>
  );
  if (fw.allowed || outcome?.ok)
    return (
      <>
        {outcome?.ok && (
          <p role="status" className="ok small" style={{ marginTop: 10 }}>
            {outcome.text}
          </p>
        )}
        {publicNote && (
          <div className="notice" style={{ marginTop: 10, maxWidth: "64ch" }}>
            <div className="title">Windows Firewall still blocks other devices on this network.</div>
            {publicNote}
          </div>
        )}
      </>
    );
  return (
    <div className="notice" style={{ marginTop: 10, maxWidth: "64ch" }}>
      <div className="title">Windows Firewall blocks other devices from reaching this computer, so pairing started here and incoming connections fail.</div>
      <div className="small">Adds a rule for Common's background service only, on networks Windows treats as Private. Windows will ask you to confirm.</div>
      {publicNote}
      <div className="actions">
        <button type="button" className="btn btn-primary btn-sm" disabled={busy} onClick={onAllow}>
          {busy ? "Waiting for Windows" : "Allow on private networks"}
        </button>
      </div>
      {outcome && (
        <p role="alert" className="warn small" style={{ marginTop: 6 }}>
          {outcome.text}
        </p>
      )}
    </div>
  );
}

/**
 * Peers (F-10-05): networking switch, this device's identity, pairing in both directions, the trusted list,
 * groups and nearby devices. `pollMs` is how often pairing status and the peer list refresh.
 */
export function Peers({ pollMs = 1000 }: { pollMs?: number }) {
  const d = daemon.use();
  const s = settings.use();
  const localOnly = (privacy.use()?.mode ?? "local_only") === "local_only";
  const [net, setNet] = useState<NetworkState | null>(null);
  const [peers, setPeers] = useState<Peer[]>([]);
  /** Every device mDNS sees except this one, paired or not. */
  const [nearby, setNearby] = useState<Discovered[]>([]);
  const [groups, setGroups] = useState<Group[]>([]);
  const [error, setError] = useState<MappedError | null>(null);
  const [session, setSession] = useState<PairingSession | null>(null);
  const [pairResult, setPairResult] = useState<string>("");
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [join, setJoin] = useState({ addr: "", code: "" });
  const [joining, setJoining] = useState(false);
  const [joinError, setJoinError] = useState<MappedError | null>(null);
  const [joined, setJoined] = useState<PeerRecord | null>(null);
  const [confirm, setConfirm] = useState<Confirm>(null);
  const [link, setLink] = useState<Link>(null);
  const [linked, setLinked] = useState("");
  const [fw, setFw] = useState<FirewallStatus | null>(null);
  const [fwBusy, setFwBusy] = useState(false);
  const [fwOutcome, setFwOutcome] = useState<Outcome>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);
  const linkRef = useRef<HTMLInputElement>(null);
  const codeRef = useRef<HTMLInputElement>(null);
  const linkedRef = useRef<HTMLParagraphElement>(null);

  const fail = (e: unknown) => setError(explain(e));
  /** Closing a panel removes the focused control; put focus back on the control that opened it (after the re-render enables it). */
  const refocus = (id: string) => setTimeout(() => document.getElementById(id)?.focus(), 0);

  const reload = useCallback(async () => {
    try {
      const [n, p, g] = await Promise.all([api.network(), api.peers(), api.groups()]);
      setNet(n);
      setPeers(p.peers);
      // mDNS also hears this device's own advert (and a second daemon on the same host); never list it.
      setNearby(p.discovered.filter((x) => x.id !== n.peer_id));
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

  // An older app shell has no such command: treat it like a system without the firewall step.
  useEffect(() => {
    firewallStatus().then(setFw, () => setFw(null));
  }, []);

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
        if (st.result && "peer" in st.result) {
          setPairResult(`Paired with ${nameOf(st.result.peer)}.`);
          setSession(null);
          reload();
        } else if (st.result && "error" in st.result) {
          // The other side sent a wrong code or dropped out; the daemon reports it as a typed error.
          setError(mapError(st.result.error.code, st.result.error.message, st.result.error.hint ?? null));
          setSession(null);
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

  // Focus the address field once when a panel opens, not on every keystroke.
  const linkId = link?.id;
  useEffect(() => {
    if (linkId) linkRef.current?.focus();
  }, [linkId]);

  async function setNetwork(patch: Partial<Pick<NetworkState, "enabled" | "serve_peers">>) {
    if (!net) return;
    try {
      setNet(await api.setNetwork({ enabled: net.enabled, serve_peers: net.serve_peers, ...patch }));
      setError(null);
    } catch (e) {
      fail(e);
    }
  }

  async function allowFirewall() {
    setFwBusy(true);
    setFwOutcome(null);
    try {
      await allowThroughFirewall();
      // netsh exited 0 inside the elevated prompt: the rule is in place. Not re-read, see firewall.rs `parse_rules`.
      setFw((f) => f && { ...f, allowed: true });
      setFwOutcome({ ok: true, text: "Allowed on private networks." });
    } catch (e) {
      setFwOutcome({
        ok: false,
        text:
          String(e) === "cancelled"
            ? "Nothing changed: the Windows confirmation was cancelled. Other devices still cannot reach this computer."
            : `Windows did not add the rule (${String(e)}). Nothing else changed; try again, or allow Common's background service in Windows Defender Firewall.`,
      });
    } finally {
      setFwBusy(false);
    }
  }

  async function startPairing() {
    setPairResult("");
    try {
      setSession(await api.pairStart(s.device_name.trim() || undefined));
      setError(null);
    } catch (e) {
      fail(e);
    }
  }

  const payload = isPayload(join.code);
  const canJoin = payload || (!!join.addr.trim() && join.code.replace(/\D/g, "").length === 6);

  async function submitJoin(e: React.FormEvent) {
    e.preventDefault();
    if (!canJoin) return;
    const code = payload ? join.code.trim() : join.code.replace(/\D/g, "");
    const addr = join.addr.trim();
    setJoining(true);
    setJoinError(null);
    setJoined(null);
    try {
      const peer = await api.pairJoin({ ...(addr ? { addr } : {}), code, name: s.device_name.trim() || undefined });
      setJoined(peer);
      setJoin({ addr: "", code: "" });
      reload();
    } catch (err) {
      const m = explain(err);
      // Only "nothing answered" carries a hint (pairing.rs `no_answer`): the code was never tried, so point at the
      // other device's firewall instead of the code. Wrong-code failures keep the generic advice.
      setJoinError(m.code === "pairing_failed" && m.hint ? { ...m, next_step: sentence(m.hint), hint: null } : m);
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

  /** Dials a paired device now: the typed address first when there is one, else the addresses the daemon knows. */
  async function connect(e: React.FormEvent) {
    e.preventDefault();
    if (!link || link.busy) return;
    const { id } = link;
    setLink({ ...link, busy: true, error: null });
    try {
      const rec = await api.connectPeer(id, link.addr.trim() || undefined);
      setLink(null);
      setLinked(`Connected to ${nameOf(rec)}.`);
      // The row's Connect button goes away once connected, so the outcome takes focus.
      setTimeout(() => linkedRef.current?.focus(), 0);
    } catch (err) {
      const p = peers.find((x) => x.id === id);
      setLink((l) => (l?.id === id ? { ...l, busy: false, error: connectError(err, p ? nameOf(p) : shortLabel(id)) } : l));
    }
    await reload();
  }

  function pairWith(x: Discovered) {
    setJoin({ addr: pairingAddr(x), code: "" });
    setJoined(null);
    setJoinError(null);
    setTimeout(() => codeRef.current?.focus(), 0);
  }

  const enabled = net?.enabled ?? false;
  const offline = d.reachable === false;
  const paired = new Set(peers.map((p) => p.id));
  const strangers = nearby.filter((x) => !x.trusted && !paired.has(x.id));
  const addrsOf = (x: Discovered) => (x.addrs?.length ? x.addrs : [x.addr]).join(", ");
  const firewall = fw?.supported && enabled && (!fw.allowed || !!fw.public_networks?.length || fwOutcome?.ok) && (
    <FirewallNotice fw={fw} busy={fwBusy} outcome={fwOutcome} onAllow={allowFirewall} />
  );

  return (
    <section className="view" aria-labelledby="view-title">
      <div className="view-head">
        <h1 id="view-title">Peers</h1>
        <p>
          Pair the computers you trust so they can run models for each other. Unpaired devices are refused before any data is exchanged, and
          nothing leaves this computer while Privacy is set to local only.
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
              Let this computer talk to paired devices
            </label>
            <p id="net-help" className="help muted small" style={{ maxWidth: "64ch" }}>
              Off by default. When on, Common listens{net.listen_addr ? <> on <code>{net.listen_addr}</code></> : ""} for connections from paired devices
              only, encrypted and checked against the key saved at pairing.{net.mdns ? " It also announces its id on this network so paired devices can find it." : ""}
            </p>
            <label className="check" style={{ marginTop: 10 }}>
              <input type="checkbox" checked={net.serve_peers} disabled={!net.enabled} aria-describedby="serve-help" onChange={(e) => setNetwork({ serve_peers: e.target.checked })} />
              Run models for paired devices when they ask
            </label>
            <p id="serve-help" className="help muted small" style={{ maxWidth: "64ch" }}>
              Off by default. Even when on, only devices in a group that allows it can send work here.
            </p>
            {!session && firewall}
          </div>

          <div className="section">
            <h2>This device</h2>
            <dl className="kv">
              <dt>Name</dt>
              <dd>
                <label>
                  <span className="visually-hidden">Name other devices see</span>
                  <input
                    type="text"
                    value={s.device_name}
                    placeholder={net.device_name ?? ""}
                    maxLength={64}
                    aria-describedby="name-help"
                    onChange={(e) => settings.set({ ...s, device_name: e.target.value })}
                  />
                </label>
                <span id="name-help" className="help muted small" style={{ display: "block" }}>
                  {net.device_name ? `Leave empty to use this computer's name, ${net.device_name}.` : "Leave empty to use this computer's name."}
                </span>
              </dd>
              <dt>Short label</dt>
              <dd>{net.short_label}</dd>
              <dt>Device id</dt>
              <dd>
                <code className="device-id">{grouped(net.peer_id)}</code>
              </dd>
              {net.dial_addr && (
                <>
                  <dt>Address</dt>
                  <dd>
                    <code>{net.dial_addr}</code>
                    <span className="help muted small" style={{ display: "block" }}>
                      A paired device that cannot find this one on the network can connect to it at this address.
                    </span>
                  </dd>
                </>
              )}
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
                    {secondsLeft > 0 ? `Expires in ${secondsLeft} s.` : "Expiring."}{" "}
                    {session.pair_addr ? (
                      <>
                        Address <code className="pair-addr">{session.pair_addr}</code>
                      </>
                    ) : (
                      `Port ${session.port}.`
                    )}
                  </div>
                  <details style={{ marginTop: 10 }}>
                    <summary>Show as text for a QR app</summary>
                    <code className="qr-payload">{session.qr_payload}</code>
                  </details>
                  <p role="status" className="small" style={{ marginTop: 10 }}>
                    Waiting for the other device.
                  </p>
                  {firewall}
                </div>
              )}
            </div>

            <div className="pair-panel">
              <h2>Join a device</h2>
              <form onSubmit={submitJoin}>
                <div className="field">
                  <label htmlFor="join-addr">Address</label>
                  <input id="join-addr" type="text" value={join.addr} placeholder="192.168.1.20:54321" autoComplete="off" disabled={!enabled} onChange={(e) => setJoin({ ...join, addr: e.target.value })} />
                  <span className="help">Shown under the code on the other device, or pick a nearby device below. Not needed with a pasted QR text.</span>
                </div>
                <div className="field">
                  <label htmlFor="join-code">Code</label>
                  <input
                    id="join-code"
                    ref={codeRef}
                    type="text"
                    value={join.code}
                    autoComplete="one-time-code"
                    disabled={!enabled}
                    onChange={(e) => setJoin({ ...join, code: e.target.value })}
                  />
                  <span className="help">The six digits on the other device's screen, or the text behind its QR code.</span>
                </div>
                <button type="submit" className="btn btn-primary" disabled={!enabled || joining || !canJoin}>
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
                  <div className="title">Paired with {nameOf(joined)}.</div>
                  <div>Add it to a group to let it run work for this computer.</div>
                </div>
              )}
            </div>
          </div>

          <div className="section">
            <h2>Paired devices</h2>
            <p ref={linkedRef} tabIndex={-1} role="status" className="ok small">
              {linked}
            </p>
            {peers.length === 0 ? (
              <div className="empty">No paired devices. Pair one above.</div>
            ) : (
              <table>
                <thead>
                  <tr>
                    <th scope="col">Device</th>
                    <th scope="col">State</th>
                    <th scope="col">Address</th>
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
                    const l = link?.id === p.id ? link : null;
                    const seen = nearby.find((x) => x.id === p.id);
                    const [word, tone] = STATE[connState(p, { seen: !!seen, looking: net.mdns, dialing: !!l?.busy })];
                    const ready = promptReadiness(p, groups, localOnly);
                    const name = p.name || p.short_label;
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
                                  : `Remove ${p.name || p.short_label} from this computer? It is forgotten here; other devices keep their own pairing with it.`}
                              </div>
                              <div className="actions">
                                <button ref={confirmRef} type="button" className="btn btn-danger btn-sm" onClick={() => act(p, c.action)}>
                                  {c.action === "revoke" ? "Revoke" : "Remove"}
                                </button>
                                <button
                                  type="button"
                                  className="btn btn-sm"
                                  onClick={() => {
                                    setConfirm(null);
                                    refocus(`${c.action}-${p.id}`);
                                  }}
                                >
                                  Cancel
                                </button>
                              </div>
                            </div>
                          )}
                          {l && (
                            <form className="confirm" aria-label={`Connect to ${name}`} onSubmit={connect}>
                              <div className="field" style={{ margin: 0 }}>
                                <label htmlFor={`connect-addr-${p.id}`}>Address (optional)</label>
                                <input
                                  id={`connect-addr-${p.id}`}
                                  ref={linkRef}
                                  type="text"
                                  value={l.addr}
                                  placeholder="192.168.1.20:8193"
                                  autoComplete="off"
                                  aria-describedby={`connect-help-${p.id}`}
                                  onChange={(e) => setLink({ ...l, addr: e.target.value })}
                                />
                                <span id={`connect-help-${p.id}`} className="help">
                                  Leave empty to try the addresses this computer knows, or enter the address shown under "This device" on {name}. Whatever
                                  answers there must still prove it is {name} with the key saved at pairing.
                                </span>
                              </div>
                              {l.error && <ErrorNotice error={l.error} />}
                              <div className="actions">
                                <button type="submit" className="btn btn-primary btn-sm" disabled={l.busy}>
                                  {l.busy ? "Connecting" : "Connect"}
                                </button>
                                <button
                                  type="button"
                                  className="btn btn-sm"
                                  disabled={l.busy}
                                  onClick={() => {
                                    setLink(null);
                                    refocus(`connect-${p.id}`);
                                  }}
                                >
                                  Cancel
                                </button>
                              </div>
                            </form>
                          )}
                        </td>
                        <td>
                          <span className={tone}>{word}</span>
                          {!p.connected && p.last_error && (
                            <div className="model-meta">
                              Last try at {new Date(p.last_error.at_utc).toLocaleTimeString()}: {p.last_error.message.replace(/\.$/, "")}.
                              {p.last_error.hint ? ` ${sentence(p.last_error.hint)}` : ""}
                            </div>
                          )}
                        </td>
                        <td>
                          {seen ? (
                            <>
                              <div>{addrsOf(seen)}</div>
                              <div className="model-meta">Seen on this network</div>
                            </>
                          ) : p.last_addr ? (
                            <>
                              <div>{p.last_addr}</div>
                              <div className="model-meta">{net.mdns ? "Last known, not seen on this network" : "Last known"}</div>
                            </>
                          ) : (
                            <span className="muted">Not known</span>
                          )}
                        </td>
                        <td>
                          {p.groups.length ? p.groups.join(", ") : <span className="muted">none</span>}
                          <div className="model-meta">{ready.ok ? <span className="ok">{ready.text}</span> : ready.text}</div>
                        </td>
                        <td>{p.last_seen_utc ? new Date(p.last_seen_utc).toLocaleString() : "never"}</td>
                        <td>
                          <div className="row-actions">
                            {enabled && !p.connected && (
                              <button
                                id={`connect-${p.id}`}
                                type="button"
                                className="btn btn-sm"
                                // One dial at a time: opening another panel mid-dial would swallow this one's outcome.
                                disabled={!!c || !!l || !!link?.busy}
                                aria-label={`Connect to ${name}`}
                                onClick={() => {
                                  setConfirm(null);
                                  setLinked("");
                                  setLink({ id: p.id, addr: "", busy: false, error: null });
                                }}
                              >
                                Connect
                              </button>
                            )}
                            <button id={`revoke-${p.id}`} type="button" className="btn btn-danger btn-sm" disabled={!!c} aria-label={`Revoke ${p.name || p.short_label}`} onClick={() => setConfirm({ id: p.id, action: "revoke" })}>
                              Revoke
                            </button>
                            <button id={`remove-${p.id}`} type="button" className="btn btn-sm" disabled={!!c} aria-label={`Remove ${p.name || p.short_label}`} onClick={() => setConfirm({ id: p.id, action: "remove" })}>
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
              Devices announcing themselves on this network that this computer does not trust. They cannot send or receive anything until paired.
            </p>
            {strangers.length === 0 ? (
              <div className="empty" style={{ marginTop: 10 }}>
                {!enabled
                  ? "Turn on networking to look for nearby devices."
                  : !net.mdns
                    ? "This computer is not looking for devices on the network. Pair by address instead: the other device shows its address under its code."
                    : "No unpaired devices nearby. Some networks, such as many campus and office Wi-Fi networks, stop devices from finding each other; pair by address instead."}
              </div>
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
                  {strangers.map((x) => (
                    <tr key={x.id}>
                      <td>
                        <div className="model-name">{shortLabel(x.id)}</div>
                        <div className="model-meta">Not trusted</div>
                      </td>
                      <td>{addrsOf(x)}</td>
                      <td>
                        <div className="row-actions">
                          {x.pairing ? (
                            <button type="button" className="btn btn-sm" aria-label={`Pair with ${shortLabel(x.id)}`} onClick={() => pairWith(x)}>
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
