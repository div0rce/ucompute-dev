import { useEffect, useState } from "react";
import { allowThroughFirewall, firewallStatus, type FirewallStatus } from "../daemon";
import { network } from "../state/live";
import { createStore } from "../state/store";

/** Windows Firewall rule state for the bundled service; null until read (or outside the app / an older shell). */
export const firewall = createStore<FirewallStatus | null>(null);
/** Re-reads it (on Devices, Settings and Dashboard mount and after Allow). */
export const refreshFirewall = () =>
  firewallStatus().then(
    (s) => firewall.set(s),
    () => firewall.set(null),
  );

/** True when the notice would show: Windows, networking on, and no rule for the service found. */
export function useFirewallMissing(): boolean {
  const fw = firewall.use();
  const on = network.use().data?.enabled ?? false;
  return !!fw?.supported && on && !fw.allowed;
}

/**
 * Windows only (src-tauri `firewall.rs`). The check cannot prove a block (a localized Windows, a non-ASCII path, a
 * broader rule, a disabled or third-party firewall all read "not allowed"), so it says what was not found, in warn
 * tone, and offers one consent step: a rule for Common's background service only, inbound UDP on Private networks,
 * through Windows' own confirmation. Never in the status bar; no error is ever relabelled as a firewall problem.
 */
export function FirewallNotice({ compact = false }: { compact?: boolean }) {
  const fw = firewall.use();
  const on = network.use().data?.enabled ?? false;
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<{ ok: boolean; text: string } | null>(null);

  useEffect(() => {
    void refreshFirewall();
  }, []);

  if (!fw?.supported || !on) return null;

  const nets = fw.public_networks ?? [];
  // The rule is for Private networks only, and Windows puts a new Wi-Fi network or hotspot on Public: say so, and leave
  // that choice to the user (the app never widens the rule).
  const publicNote = nets.length > 0 && (
    <p className="small">
      Windows treats {nets.join(", ")} as Public, where this rule does not apply, so other devices cannot reach this computer there. If you trust a network, set it to
      Private in Windows Settings &gt; Network &amp; internet.
    </p>
  );

  async function allow() {
    setBusy(true);
    setOutcome(null);
    try {
      await allowThroughFirewall();
      // netsh exited 0 inside the elevated prompt: the rule is in place. Not re-read, see firewall.rs `parse_rules`.
      firewall.set({ ...fw!, allowed: true });
      setOutcome({ ok: true, text: "Allowed on private networks." });
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
          <p role="status" className="small tone-ok">
            {outcome.text}
          </p>
        )}
        {publicNote && (
          <div className="notice tone-warn">
            <p className="notice-title">Windows Firewall still blocks other devices on this network.</p>
            {publicNote}
          </div>
        )}
      </>
    );
  return (
    <div className="notice tone-warn">
      <p className="notice-title">No Windows Firewall rule for the service was found; other devices may be unable to connect.</p>
      {!compact && <p className="small">Adds a rule for Common's background service only, on networks Windows treats as Private. Windows will ask you to confirm.</p>}
      {publicNote}
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
