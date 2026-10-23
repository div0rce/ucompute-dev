import { useEffect, useRef, useState } from "react";
import { allowThroughFirewall, firewallStatus, type FirewallStatus } from "../daemon";
import { network } from "../state/live";
import { createStore } from "../state/store";

/** Windows Firewall rule state for the bundled service; null until read (or outside the app / an older shell). */
export const firewall = createStore<FirewallStatus | null>(null);
export const ADOPTED = "The running background service was not started by this app, so the app cannot check or add its Windows Firewall rule.";

/** Re-reads it (on Devices, Settings and Dashboard mount and after Allow). */
export const refreshFirewall = () =>
  firewallStatus().then(
    (s) => firewall.set(s),
    () => firewall.set(null),
  );

/** True when the notice would show: Windows, networking on, and no rule for the service this app started found. */
export function useFirewallMissing(): boolean {
  const fw = firewall.use();
  const on = network.use().data?.enabled ?? false;
  return !!fw?.supported && on && !fw.adopted && !fw.allowed;
}

/**
 * Windows only (src-tauri `firewall.rs`). The check cannot prove a block (a broader rule, a disabled or third-party
 * firewall all read "not allowed"), so it says what was not found, in warn tone, and offers one consent step: a rule
 * for Common's background service only, inbound UDP on Private networks, through Windows' own confirmation. A read that
 * failed shows nothing, and a service this app did not start is not checked. Never in the status bar; no error is
 * ever relabelled as a firewall problem.
 */
export function FirewallNotice({ compact = false }: { compact?: boolean }) {
  const fw = firewall.use();
  const on = network.use().data?.enabled ?? false;
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<{ ok: boolean; text: string } | null>(null);
  const done = useRef<HTMLParagraphElement>(null);

  useEffect(() => {
    void refreshFirewall();
  }, []);

  if (!fw?.supported || !on) return null;
  if (fw.adopted) return <p className="small muted">{ADOPTED}</p>;

  const nets = fw.public_networks ?? [];
  const domain = fw.domain_networks ?? [];
  // The rule is for Private networks only, and Windows puts a new Wi-Fi network or hotspot on Public: say so, and leave
  // that choice to the user (the app never widens the rule).
  const publicNote = nets.length > 0 && (
    <p className="small">
      Windows treats {nets.join(", ")} as Public, where this rule does not apply, so other devices cannot reach this computer there. If you trust a network, set it to
      Private in Windows Settings &gt; Network &amp; internet.
    </p>
  );
  // A domain network cannot be switched to Private by the user: the organization's firewall settings decide.
  const domainNote = domain.length > 0 && (
    <p className="small">
      Windows treats {domain.join(", ")} as a domain network, where this rule does not apply either, so other devices may be unable to reach this computer there. Ask
      your administrator to allow Common's background service on it.
    </p>
  );

  async function allow() {
    setBusy(true);
    setOutcome(null);
    try {
      await allowThroughFirewall();
      // netsh exited 0 inside the elevated prompt: the rule is in place (the next read confirms it).
      firewall.set({ ...fw!, allowed: true });
      setOutcome({ ok: true, text: "Allowed on private networks." });
      // The button just used goes away with the warning: the outcome takes focus, so it is read and focus is not lost.
      setTimeout(() => done.current?.focus(), 0);
    } catch (e) {
      setOutcome({
        ok: false,
        text:
          String(e) === "cancelled"
            ? "Nothing changed: the Windows confirmation was cancelled. Other devices may still be unable to connect."
            : `Windows did not add the rule (${String(e)}). Nothing else changed; try again, or allow Common's background service in Windows Defender Firewall.`,
      });
    } finally {
      setBusy(false);
    }
  }

  if (fw.allowed)
    return (
      <>
        {outcome?.ok && (
          <p ref={done} tabIndex={-1} role="status" className="small tone-ok">
            {outcome.text}
          </p>
        )}
        {(publicNote || domainNote) && (
          <div className="notice tone-warn">
            <p className="notice-title">{publicNote ? "Windows Firewall still blocks other devices on this network." : "Windows Firewall may still block other devices on this network."}</p>
            {publicNote}
            {domainNote}
          </div>
        )}
      </>
    );
  return (
    <div className="notice tone-warn">
      <p className="notice-title">No Windows Firewall rule for the service was found; other devices may be unable to connect.</p>
      {!compact && <p className="small">Adds a rule for Common's background service only, on networks Windows treats as Private. Windows will ask you to confirm.</p>}
      {publicNote}
      {domainNote}
      <div className="notice-actions">
        <button type="button" className="btn" disabled={busy} onClick={() => void allow()}>
          {busy ? "Waiting for Windows" : "Allow on private networks"}
        </button>
      </div>
      {outcome && (
        <p role="alert" className="small tone-warn">
          {outcome.text}
        </p>
      )}
    </div>
  );
}
