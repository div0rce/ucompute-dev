// Devices (plan §2.2): this computer, paired devices, nearby devices and revocations in one table, with an inspector
// for the selected row. Selection and the inspector tab live in the URL (`#/devices/<id>?tab=…`, replaceState); this
// computer (`self`) is selected when the URL names none. `?pair=1|join` opens the Pair dialog, `?confirm=remove|revoke`
// an inline confirmation, so the toolbar, the row menu, the Delete key and the Dashboard share one path.
import "./devices.css";
import { useEffect, useState } from "react";
import { select, useLocation } from "../../router";
import { useNow } from "../../shell/attention";
import { live, useDeviceRows, type DeviceRow } from "../../state/devices";
import { dialing, mesh, network, pairing, useNeed, type Key } from "../../state/live";
import { daemon } from "../../state/stores";
import { ui } from "../../state/ui";
import { DataTable } from "../../ui/DataTable";
import { ErrorNotice } from "../../ui/ErrorNotice";
import { FirewallNotice } from "../../ui/FirewallNotice";
import { activeTab, Inspector, openInspector } from "../../ui/Inspector";
import { EmptyState, ScopeBar, ToolButton } from "../../ui/primitives";
import { frameState, SectionFrame } from "../../ui/SectionFrame";
import { Tip } from "../../ui/Tip";
import { PairDialog } from "./PairDialog";
import {
  canPairWith,
  connectBlocked,
  connectFromRow,
  connectOutcome,
  deviceError,
  deviceMenu,
  disconnect,
  focusDevices,
  frozenWhy,
  removeDevice,
  setNetworking,
  useDeviceColumns,
} from "./table";
import { Confirm, deviceTabs, NETWORK_HELP } from "./tabs";

type Scope = "all" | "connected" | "not-connected" | "nearby" | "revoked";
const SCOPES: { id: Scope; label: string; test: (r: DeviceRow) => boolean }[] = [
  { id: "all", label: "All", test: (r) => r.trust !== "revoked" },
  { id: "connected", label: "Connected", test: (r) => !!r.peer && live(r.peer) },
  { id: "not-connected", label: "Not connected", test: (r) => !!r.peer && !live(r.peer) },
  { id: "nearby", label: "Nearby", test: (r) => r.trust === "untrusted" },
  { id: "revoked", label: "Revoked", test: (r) => r.trust === "revoked" },
];

const mmss = (ms: number) => {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
};

export default function Devices() {
  const loc = useLocation();
  const d = daemon.use();
  const netLive = network.use();
  const meshLive = mesh.use();
  const net = netLive.data;
  const rows = useDeviceRows();
  const columns = useDeviceColumns();
  const session = pairing.use();
  const dial = dialing.use();
  const error = deviceError.use();
  const now = useNow(1000, !!session);
  const [scope, setScope] = useState<Scope>("all");
  const [filter, setFilter] = useState("");
  const [netError, setNetError] = useState<ReturnType<typeof deviceError.get>>(null);
  const prefs = ui.use().inspectors.devices ?? {};

  const scoped = rows.filter(SCOPES.find((s) => s.id === scope)!.test);
  const paired = rows.filter((r) => r.trust === "trusted");
  const key = loc.id ?? "self";
  // Only a row the grid can show: a selection the scope hides is not shown in the inspector either.
  const row = scoped.find((r) => r.key === key) ?? null;
  const confirm = loc.params.get("confirm");
  const pairOpen = loc.params.has("pair");
  const up = d.reachable === true;
  const state = frameState(d, netLive, meshLive);
  const frozen = frozenWhy(state);

  // Privacy and groups decide Can run there; without them it is shown as not known. Jobs and audit at the faster
  // cadence while the inspector shows a tab built from them (the URL's, else the last one used, else the first).
  const needs: Key[] = ["network", "mesh", "groups", "privacy", "hardware", "models"];
  const shown = row && !prefs.collapsed ? activeTab(deviceTabs(row), loc.params.get("tab"), prefs.tab)?.id : null;
  if (shown === "activity" || shown === "performance") needs.push("jobs", "audit");
  useNeed(needs);
  // Outcomes belong to this visit; a later visit starts clean.
  useEffect(() => () => connectOutcome.set({}), []);
  /** The current params with `patch` applied (null removes a key). */
  const params = (patch: Record<string, string | null>) => {
    const q = new URLSearchParams(loc.params);
    for (const [k, v] of Object.entries(patch)) v == null ? q.delete(k) : q.set(k, v);
    return q;
  };

  async function toggleNetworking(on: boolean) {
    setNetError(await setNetworking({ enabled: on }));
  }

  const pairButton = (
    <button type="button" className="btn" onClick={() => select(loc.id, params({ pair: "1", confirm: null }))}>
      Pair a device…
    </button>
  );
  const empty = (() => {
    if (scope === "nearby")
      return !net?.enabled ? (
        <EmptyState
          title="Turn on networking to look for nearby devices."
          action={
            up &&
            net && (
              <button type="button" className="btn" onClick={() => void toggleNetworking(true)}>
                Turn on networking
              </button>
            )
          }
        />
      ) : !net.mdns ? (
        <EmptyState title="No nearby devices" reason="This computer is not looking for devices on the network. Pair by address instead: the other device shows its address under its code." />
      ) : (
        <EmptyState title="No unpaired devices nearby" reason="Some networks, such as many campus and office Wi-Fi networks, stop devices from finding each other; pair by address instead." />
      );
    if (scope === "revoked") return <EmptyState title="No revoked devices." />;
    if (filter) return <EmptyState title={`No devices match "${filter}"`} action={<button type="button" className="btn" onClick={() => setFilter("")}>Clear filter</button>} />;
    if (!paired.length) return <EmptyState title="No paired devices." action={pairButton} />;
    return <EmptyState title={scope === "connected" ? "No device is connected." : "Every paired device is connected."} />;
  })();

  const p = row?.peer;
  const selection = row && (
    <>
      {p && (
        <ToolButton icon="plug" label={`Connect to ${row.name}`} busy={dial === p.id} disabledReason={connectBlocked(p, row.name)} onClick={() => connectFromRow(p)} />
      )}
      {p && p.held !== undefined && (
        <ToolButton icon="unplug" label={`Disconnect ${row.name}`} disabledReason={p.connected ? null : `${row.name} is not connected`} onClick={() => void disconnect(p)} />
      )}
      {canPairWith(row) && (
        <ToolButton icon="link" label={`Pair with ${row.name}`} onClick={() => select(row.key, params({ pair: "join", confirm: null }))} />
      )}
      {p && <ToolButton icon="trash-2" label={`Remove ${row.name}`} onClick={() => select(row.key, params({ confirm: "remove" }))} />}
    </>
  );

  const banner = (
    <>
      {error && <ErrorNotice error={error} onDismiss={() => deviceError.set(null)} />}
      {netError && <ErrorNotice error={netError} onDismiss={() => setNetError(null)} />}
      {!pairOpen && <FirewallNotice />}
      {confirm === "remove" && p && !frozen && (
        <Confirm
          label={`Confirm removing ${row!.name}`}
          text={`Remove ${row!.name} from this computer? It is forgotten here; other devices keep their own pairing with it, and you can pair it again later.`}
          action="Remove"
          onConfirm={() => {
            select(null, params({ confirm: null }));
            focusDevices();
            void removeDevice(p);
          }}
          onCancel={() => select(loc.id, params({ confirm: null }))}
        />
      )}
      {scope === "all" && up && net && !net.enabled && (
        <div className="notice tone-neutral devices-hint">
          <p className="notice-title">Networking is off. Paired devices can't connect and nearby devices aren't listed.</p>
          <div className="notice-actions">
            <button type="button" className="btn" onClick={() => void toggleNetworking(true)}>
              Turn on networking
            </button>
          </div>
        </div>
      )}
      {scope === "all" && up && net?.enabled && meshLive.data && !paired.length && (
        <div className="notice tone-neutral devices-hint">
          <p className="notice-title">No paired devices.</p>
          <div className="notice-actions">{pairButton}</div>
        </div>
      )}
    </>
  );

  return (
    <>
      <SectionFrame
        title="Devices"
        state={state}
        controls={
          <span className="net-switch">
            <Tip text={NETWORK_HELP(net?.listen_addr, net?.mdns ?? false)}>
              <input
                id="devices-networking"
                type="checkbox"
                role="switch"
                className="switch"
                checked={net?.enabled ?? false}
                disabled={!up || !net}
                onChange={(e) => void toggleNetworking(e.target.checked)}
              />
            </Tip>
            <label htmlFor="devices-networking">Networking</label>
          </span>
        }
        actions={
          <ToolButton
            icon="link"
            label={session && session.expiresAt > now ? "Pairing open" : "Pair a device…"}
            detail={session && session.expiresAt > now ? mmss(session.expiresAt - now) : undefined}
            disabledReason={up ? null : "The background service is not running"}
            onClick={() => select(loc.id, params({ pair: "1", confirm: null }))}
          />
        }
        selectionActions={selection}
        scope={
          <ScopeBar
            label="Show devices"
            items={SCOPES.map((s) => ({ id: s.id, label: s.label, count: rows.filter((r) => r.trust !== "self" || s.id === "all").filter(s.test).length }))}
            value={scope}
            onChange={(s) => setScope(s as Scope)}
          />
        }
        filter={{ value: filter, onChange: setFilter, label: "Filter devices" }}
        banner={banner}
        inspector={<Inspector id="devices" label="Device details" hasSelection={!!row} empty="Select a device to see its details." defaultHeight="fit" tabs={row ? deviceTabs(row) : []} />}
      >
        <DataTable
          id="devices"
          label="Devices"
          columns={columns}
          rows={scoped}
          rowKey={(r) => r.key}
          selected={row ? key : null}
          onSelect={(k) => select(k, params({ confirm: null, pair: null }))}
          onActivate={() => openInspector()}
          onCopy={(r) => r.id}
          menu={deviceMenu}
          onDelete={(r) => r.peer && !frozen && select(r.key, params({ confirm: "remove" }))}
          pinned={(r) => r.trust === "self"}
          filter={filter}
          empty={empty}
        />
      </SectionFrame>
      <PairDialog open={pairOpen} mode={loc.params.get("pair")} row={row} onClose={() => select(loc.id, params({ pair: null }))} />
    </>
  );
}
