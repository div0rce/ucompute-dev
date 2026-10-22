// Pair dialog (plan §2.2): "Show a code" and "Enter a code" as APG tabs in a native <dialog>. The pairing window
// outlives the dialog: `pair/start` answers 409 while one is open, `pair/status` does not return the code and there is
// no cancel route (N11), so the session lives in the app-level `pairing` store and is polled here, at module level,
// until it reports a final state or expires. Closing the dialog hides it; reopening resumes the same code.
import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";
import { api, expiresAtMs, pairingAddr, shortLabel, type PeerRecord } from "../../api/client";
import { explain, mapError, type MappedError } from "../../api/errors";
import { clock } from "../../format";
import { href } from "../../router";
import { announce } from "../../shell/announcer";
import { useNow } from "../../shell/attention";
import { sentence, type DeviceRow } from "../../state/devices";
import { mesh, network, pairing, refreshMany } from "../../state/live";
import { createStore } from "../../state/store";
import { settings } from "../../state/stores";
import { Dialog } from "../../ui/Dialog";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { FirewallNotice } from "../../ui/FirewallNotice";
import { setNetworking } from "./table";

/** How often `pair/status` is polled while a window is open (tests shorten it). */
export const pairPoll = { ms: 1000 };
/** The window's port, for services that do not report `pair_addr`. */
const pairPort = createStore<number | null>(null);
/** How the last window ended, shown when the dialog is open; cleared at the next start. */
export const pairResult = createStore<{ ok: boolean; text: string } | { error: MappedError } | null>(null);
/** The last `pair/status` read failed (the window is kept and asked again next tick); null once one answers. */
const statusError = createStore<{ error: MappedError; at: number } | null>(null);

const nameOf = (p: PeerRecord) => p.name || shortLabel(p.id);
/** "123456" -> "123 456": easier to read aloud across a room. */
const spaced = (code: string) => `${code.slice(0, 3)} ${code.slice(3)}`;
/** The code field takes either the six digits or a pasted `common-pair:v1:...` payload (which carries the address). */
const isPayload = (code: string) => code.trim().startsWith("common-pair:");
/** The device id a pasted payload names, if any. */
const payloadId = (code: string) => /^common-pair:v\d+:([0-9a-f]{64}):/.exec(code.trim())?.[1] ?? null;

function finish(result: NonNullable<ReturnType<typeof pairResult.get>>) {
  statusError.set(null);
  pairing.set(null);
  pairResult.set(result);
  announce("error" in result ? `Pairing failed: ${result.error.title}` : result.text);
}

async function poll() {
  const s = pairing.get();
  if (!s) return;
  try {
    const st = await api.pairStatus();
    if (pairing.get() !== s) return;
    statusError.set(null);
    if (st.result && "peer" in st.result) {
      finish({ ok: true, text: `Paired with ${nameOf(st.result.peer)}.` });
      void refreshMany(["mesh", "groups"], true);
    } else if (st.result && "error" in st.result) {
      // The other side sent a wrong code or dropped out; the daemon reports it as a typed error.
      finish({ error: mapError(st.result.error.code, st.result.error.message, st.result.error.hint ?? null) });
    } else if (!st.active) finish({ ok: false, text: "The code expired before another device joined. Start again for a fresh one." });
  } catch (e) {
    // One failed read does not end the window: the service still holds it (pair/start answers 409 until it ends) and
    // pair/status never returns the code again. Keep it and ask again next tick, until it has expired.
    if (pairing.get() !== s) return;
    if (Date.now() > s.expiresAt) finish({ error: explain(e) });
    else statusError.set({ error: explain(e), at: Date.now() });
  }
}

let timer: ReturnType<typeof setInterval> | null = null;
pairing.subscribe(() => {
  if (pairing.get() && !timer) {
    void poll();
    timer = setInterval(() => void poll(), pairPoll.ms);
  } else if (!pairing.get() && timer) {
    clearInterval(timer);
    timer = null;
  }
});

/** Opens a pairing window (as the daemon's device name unless the user set one in Settings › Network). */
export async function startPairing(): Promise<MappedError | null> {
  pairResult.set(null);
  statusError.set(null);
  try {
    const s = await api.pairStart(settings.get().device_name.trim() || undefined);
    pairPort.set(s.port);
    pairing.set({ code: s.code, addr: s.pair_addr ?? null, payload: s.qr_payload, expiresAt: expiresAtMs(s.expires_at) });
    return null;
  } catch (e) {
    return explain(e);
  }
}

type Tab = "show" | "join";
const TABS: { id: Tab; label: string }[] = [
  { id: "show", label: "Show a code" },
  { id: "join", label: "Enter a code" },
];

/**
 * `mode` is the `pair` URL param: "join" opens Enter a code, prefilled from `row` (a nearby device's pairing window);
 * anything else opens Show a code (resuming an open window).
 */
export function PairDialog({ open, mode, row, onClose }: { open: boolean; mode: string | null; row: DeviceRow | null; onClose: () => void }) {
  const netLive = network.use();
  const net = netLive.data;
  const base = useId();
  // The tab the user picked; until then the one `mode` asks for (known on the render that opens the dialog, so the
  // right field gets the initial focus).
  const [picked, setTab] = useState<Tab | null>(null);
  const tab = picked ?? (mode === "join" ? "join" : "show");
  const tabRefs = useRef<(HTMLButtonElement | null)[]>([]);
  const prefill = mode === "join" && row?.discovered ? { addr: pairingAddr(row.discovered), id: row.id } : null;

  useEffect(() => {
    if (!open) setTab(null);
  }, [open]);

  function onTabKey(e: KeyboardEvent) {
    const i = TABS.findIndex((t) => t.id === tab);
    const j = e.key === "ArrowRight" ? i + 1 : e.key === "ArrowLeft" ? i - 1 : e.key === "Home" ? 0 : e.key === "End" ? TABS.length - 1 : null;
    if (j == null) return;
    e.preventDefault();
    const next = (j + TABS.length) % TABS.length;
    setTab(TABS[next].id);
    tabRefs.current[next]?.focus();
  }

  return (
    <Dialog open={open} label="Pair a device" onClose={onClose} className="pair-dialog">
      <h2>Pair a device</h2>
      {!net ? (
        // Not read yet is not "off": offering to turn it on would PUT a guess.
        netLive.error ? (
          <ErrorNotice error={netLive.error} />
        ) : (
          <p className="muted">Reading the network state…</p>
        )
      ) : !net.enabled ? (
        <NetworkingStep />
      ) : (
        <>
          <div role="tablist" aria-label="Pairing" className="tablist pair-tabs" onKeyDown={onTabKey}>
            {TABS.map((t, i) => (
              <button
                key={t.id}
                ref={(el) => {
                  tabRefs.current[i] = el;
                }}
                type="button"
                role="tab"
                id={`${base}-tab-${t.id}`}
                aria-selected={tab === t.id}
                aria-controls={`${base}-panel`}
                tabIndex={tab === t.id ? 0 : -1}
                onClick={() => setTab(t.id)}
              >
                {t.label}
              </button>
            ))}
          </div>
          <div role="tabpanel" id={`${base}-panel`} aria-labelledby={`${base}-tab-${tab}`} className="pair-tabpanel">
            {tab === "show" ? <ShowCode /> : <EnterCode prefill={prefill} />}
          </div>
        </>
      )}
      <div className="dialog-actions">
        <button type="button" className="btn" onClick={onClose}>
          Close
        </button>
      </div>
    </Dialog>
  );
}

function NetworkingStep() {
  const [error, setError] = useState<MappedError | null>(null);
  return (
    <div className="pair-step">
      <p className="prose">Networking is off. Turn it on so this computer can reach the other device and be reached by it.</p>
      <button
        type="button"
        className="btn btn-primary"
        data-autofocus
        onClick={async () => {
          const e = await setNetworking({ enabled: true });
          setError(e);
          // This step is replaced by the tabs: the next step's primary control takes focus.
          if (!e) setTimeout(() => document.querySelector<HTMLElement>(".pair-dialog [data-autofocus]")?.focus(), 0);
        }}
      >
        Turn on networking
      </button>
      {error && <ErrorNotice error={error} />}
    </div>
  );
}

function ShowCode() {
  const session = pairing.use();
  const result = pairResult.use();
  const port = pairPort.use();
  const failedRead = statusError.use();
  const net = network.use().data;
  const name = settings.use().device_name.trim() || net?.device_name;
  const now = useNow(1000, !!session);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<MappedError | null>(null);
  const codeRef = useRef<HTMLDivElement>(null);

  async function start() {
    setStarting(true);
    const e = await startPairing();
    setError(e);
    setStarting(false);
    // The button that had focus is gone: the code takes it, so it is read out and Esc still reaches the dialog.
    if (!e) setTimeout(() => codeRef.current?.focus(), 0);
  }

  if (session) {
    const left = Math.max(0, Math.round((session.expiresAt - now) / 1000));
    return (
      <div ref={codeRef} tabIndex={-1} className="pair-live" role="group" aria-label="Pairing code">
        <div className="pair-code">{spaced(session.code)}</div>
        <p className="small muted">
          {left > 0 ? `Expires in ${left} s.` : "Expiring."}{" "}
          {session.addr ? (
            <>
              Address <code className="pair-addr">{session.addr}</code>
            </>
          ) : (
            `Port ${port}.`
          )}
        </p>
        <details>
          <summary>Show as text for a QR app</summary>
          <code className="qr-payload">{session.payload}</code>
        </details>
        <p role="status" className="small">
          {failedRead ? `Status not read at ${clock(failedRead.at)}: ${failedRead.error.title} Trying again.` : "Waiting for the other device."}
        </p>
        <FirewallNotice compact />
      </div>
    );
  }
  return (
    <div className="pair-step">
      <p className="prose small">
        Shows a one-time code for two minutes. On the other device, open Pair a device › Enter a code and type it there.
        {name && (
          <>
            {" "}
            Other devices will see this computer as <strong>{name}</strong> (<a href={href("settings", "network")}>change in Settings › Network</a>).
          </>
        )}
      </p>
      <button type="button" className="btn btn-primary" data-autofocus disabled={starting} onClick={() => void start()}>
        Pair this device
      </button>
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}
      {result && "error" in result && <ErrorNotice error={result.error} />}
      {result && "text" in result && (
        <p role="status" className={result.ok ? "tone-ok" : "tone-warn"}>
          {result.text}
        </p>
      )}
      <FirewallNotice compact />
    </div>
  );
}

function EnterCode({ prefill }: { prefill: { addr: string; id: string } | null }) {
  const m = mesh.use().data;
  const revocations = m?.revocations ?? [];
  const [join, setJoin] = useState({ addr: prefill?.addr ?? "", code: "" });
  const [joining, setJoining] = useState(false);
  const [error, setError] = useState<MappedError | null>(null);
  const [joined, setJoined] = useState<PeerRecord | null>(null);
  const [askRevoked, setAskRevoked] = useState<string | null>(null);

  const payload = isPayload(join.code);
  const canJoin = payload || (!!join.addr.trim() && join.code.replace(/\D/g, "").length === 6);
  const prefilled = !!prefill;

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (!canJoin || joining) return;
    // Pairing again with a device revoked here trusts it again: say so first. Known from a pasted QR text, the row the
    // dialog was opened for, or a typed address that a revoked device announces nearby.
    const typed = join.addr.trim();
    const byAddr = typed ? m?.discovered.find((d) => revocations.some((r) => r.peer_id === d.id) && [pairingAddr(d), d.addr, ...(d.addrs ?? [])].includes(typed))?.id : undefined;
    const target = payloadId(join.code) ?? prefill?.id ?? byAddr ?? null;
    const rev = target ? revocations.find((r) => r.peer_id === target) : undefined;
    if (rev) return setAskRevoked(new Date(rev.at_utc).toLocaleDateString());
    await doJoin();
  }

  async function doJoin() {
    const code = payload ? join.code.trim() : join.code.replace(/\D/g, "");
    const addr = join.addr.trim();
    setJoining(true);
    setError(null);
    setJoined(null);
    try {
      const peer = await api.pairJoin({ ...(addr ? { addr } : {}), code, name: settings.get().device_name.trim() || undefined });
      setJoined(peer);
      setJoin({ addr: "", code: "" });
      void refreshMany(["mesh", "groups"], true);
    } catch (err) {
      const m = explain(err);
      // Only "nothing answered" carries a hint (pairing.rs `no_answer`): the code was never tried, so point at the
      // other device's firewall instead of the code. Wrong-code failures keep the generic advice.
      setError(m.code === "pairing_failed" && m.hint ? { ...m, next_step: sentence(m.hint), hint: null } : m);
    } finally {
      setJoining(false);
    }
  }

  return (
    <>
      <form className="pair-join" onSubmit={submit}>
        <div className="field">
          <label htmlFor="join-addr">Address</label>
          <input id="join-addr" type="text" value={join.addr} placeholder="192.168.1.20:54321" autoComplete="off" data-autofocus={prefilled ? undefined : true} onChange={(e) => setJoin({ ...join, addr: e.target.value })} />
          <span className="help small muted">Shown under the code on the other device. Not needed with a pasted QR text.</span>
        </div>
        <div className="field">
          <label htmlFor="join-code">Code</label>
          <input id="join-code" type="text" value={join.code} autoComplete="one-time-code" data-autofocus={prefilled ? true : undefined} onChange={(e) => setJoin({ ...join, code: e.target.value })} />
          <span className="help small muted">The six digits on the other device's screen, or the text behind its QR code.</span>
        </div>
        <button type="submit" className="btn btn-primary" disabled={joining || !canJoin}>
          {joining ? "Joining" : "Join"}
        </button>
      </form>
      {askRevoked && (
        <div className="confirm" role="group" aria-label="Confirm pairing a revoked device">
          <p>You revoked this device on {askRevoked}; pairing again trusts it again.</p>
          <div className="confirm-actions">
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => {
                setAskRevoked(null);
                void doJoin();
              }}
            >
              Trust it again
            </button>
            <button type="button" className="btn" onClick={() => setAskRevoked(null)}>
              Cancel
            </button>
          </div>
        </div>
      )}
      {error && <ErrorNotice error={error} onDismiss={() => setError(null)} />}
      {joined && (
        <div className="notice tone-ok" role="status">
          <p className="notice-title">Paired with {nameOf(joined)}.</p>
          <p>Add it to a group to let it run work for this computer.</p>
        </div>
      )}
    </>
  );
}
