import type { ReactNode } from "react";
import type { PrivacyMode } from "../api/client";
import { byteRate, clock, humanBytes, humanDuration, plural } from "../format";
import { go, href } from "../router";
import { live, useDeviceRows } from "../state/devices";
import { hardware, jobs, mesh, models, netRate, network, ready, samples } from "../state/live";
import { daemon, privacy } from "../state/stores";
import { Icon, type IconName } from "../ui/icons";
import type { Tone } from "../ui/primitives";
import { Tip } from "../ui/Tip";
import { useAttention } from "./attention";

export const MODE_LABEL: Record<PrivacyMode, string> = { local_only: "Local only", trusted_group: "Trusted devices" };

/**
 * One status bar item: a link or button when it has a destination, else text (focusable only to reach its Tip).
 * `drop` is the narrow-window drop order (1 drops first: Disk, Network rate, Sharing, Slots); undropped items stay.
 */
function Item({ tone = "neutral", icon, dot, tip, drop, to, onClick, label, stale, children }: { tone?: Tone; icon?: IconName; dot?: boolean; tip?: string; drop?: 1 | 2 | 3 | 4; to?: string; onClick?: () => void; label?: string; stale?: string | null; children: ReactNode }) {
  // A value that was not refreshed is never shown as live: no state colour, a visible "not updated" and the time in the tip.
  const cls = `sb-item tone-${stale ? "neutral" : tone}${drop ? ` sb-drop-${drop}` : ""}`;
  tip = stale ? [stale, tip].filter(Boolean).join(" ") : tip;
  const inner = (
    <>
      {dot && <span className="sb-dot" aria-hidden="true" />}
      {icon && <Icon name={icon} />}
      <span className="sb-text">{children}</span>
      {stale && <span className="note">not updated</span>}
    </>
  );
  const el = to ? (
    <a className={cls} href={to} aria-label={label}>
      {inner}
    </a>
  ) : onClick ? (
    <button type="button" className={cls} onClick={onClick} aria-label={label}>
      {inner}
    </button>
  ) : (
    <span className={cls} tabIndex={tip ? 0 : undefined}>
      {inner}
    </span>
  );
  return tip ? <Tip text={tip}>{el}</Tip> : el;
}

/** `<footer aria-label="Status">` (plan §1.2). Never a live region; the shell announcer speaks transitions. */
export function StatusBar() {
  const d = daemon.use();
  const netLive = network.use();
  const meshLive = mesh.use();
  const jobsLive = jobs.use();
  const conn = samples.use().conn;
  const mode = privacy.use()?.mode;
  const hw = hardware.use();
  const disk = hw.data?.disk_free_bytes;
  const loadingStreak = ready.use().loadingStreak;
  const catalog = models.use().data;
  const attention = useAttention();
  const rows = useDeviceRows();

  const nameOfModel = (id: string | null | undefined) => (id ? (catalog?.find((x) => x.id === id)?.name ?? id) : "a model");
  const nameOfPeer = (target: string) => {
    const id = target.replace(/^peer:/, "");
    const p = m?.peers.find((x) => x.id === id);
    return p ? p.name || p.short_label : `${id.slice(0, 4)}-${id.slice(4, 8)}-${id.slice(8, 12)}`;
  };
  // Live items show only what the running service reported: nothing while it is down or starting, and a store whose
  // last refresh failed (or a status poll that timed out) is marked not updated.
  const up = d.reachable === true;
  const unknownTip = d.reachable === null ? "Not known until the service answers." : "Not known while the service is not running.";
  const notUpdated = (at: number | null | undefined) => `Not updated since ${clock(at)}.`;
  const net = up ? netLive.data : null;
  const m = up ? meshLive.data : null;
  const j = up ? jobsLive.data : null;
  // A status poll that timed out (busy) or answered with an HTTP error leaves the last good status: not live.
  const statusStale = up && (d.responding === false || d.statusError) ? notUpdated(d.at) : null;
  const meshFailed = [netLive, meshLive].find((x) => x.error);
  const meshStale = up && meshFailed ? notUpdated(meshFailed.at) : null;
  const s = up ? d.status : null;

  // 1. Attention
  const first = attention[0];
  const attentionItem = first && (
    <Item
      tone={first.tone}
      icon={first.tone === "danger" ? "circle-x" : "circle-alert"}
      onClick={() => (first.action ? first.action.run() : go("dashboard"))}
      tip={attention.length > 1 ? `${attention.length} items need attention; the Dashboard lists them all.` : first.action?.label}
    >
      {first.text}
      {attention.length > 1 ? ` · +${attention.length - 1}` : ""}
    </Item>
  );

  // 2. Service
  const service =
    d.reachable === null
      ? { tone: "active" as const, text: "Starting" }
      : d.reachable === false
        ? { tone: "danger" as const, text: "Not running" }
        : d.responding === false
          ? { tone: "warn" as const, text: "Busy (not answering)" }
          : { tone: "ok" as const, text: "Running" };
  const serviceTip = d.status ? `Service ${d.status.version}, up ${humanDuration(d.status.uptime_seconds)}${d.spawned ? "" : ", started outside this app"}.` : d.spawned ? undefined : "Started outside this app.";

  // 3. Engine
  const loaded = s?.loaded ?? null;
  const engine = s?.engine;
  let engineText = s ? "No model loaded" : "Engine —";
  let engineTone: Tone = "neutral";
  let engineTip: string | undefined = s ? undefined : unknownTip;
  let engineModel = loaded?.model_id ?? null;
  if (engine?.state === "loading") {
    engineText = `Loading ${nameOfModel(engine.loading_model_id)}${engine.loading_for_ms != null ? ` · ${Math.round(engine.loading_for_ms / 1000)} s` : ""}`;
    engineTone = "active";
    engineModel = engine.loading_model_id;
  } else if (s && !engine && loadingStreak >= 2) {
    engineText = "Loading a model";
    engineTone = "active";
  } else if (loaded) {
    engineText = `${nameOfModel(loaded.model_id)} · ${loaded.device_actual}`;
    const when = loaded.loaded_at_utc ? ` at ${clock(loaded.loaded_at_utc)}` : "";
    engineTip = [
      loaded.loaded_for?.startsWith("peer:") ? `Loaded for a request from ${nameOfPeer(loaded.loaded_for)}${when}` : loaded.loaded_for ? `Loaded by this computer${when}` : null,
      loaded.fallback_reason ? `Running on CPU: ${loaded.fallback_reason}` : null,
    ]
      .filter(Boolean)
      .join(". ") || undefined;
  }

  // 4. Slots ("In use", not "Running": inflight includes requests still waiting for a load)
  const q = s?.queue;
  const slotsWarn = !!q && (q.queued > 0 || q.inflight >= q.max_concurrent);

  // 5. Mesh: distinct trusted devices; discovered entries and second connections never count.
  const peers = m?.peers ?? [];
  const connected = peers.filter(live).length;
  const notResponding = peers.filter((p) => p.connected && p.health === "unhealthy").length;
  let meshText: string;
  let meshTone: Tone = "neutral";
  if (!net) meshText = "Networking —";
  else if (!net.enabled) meshText = "Networking off";
  else if (!net.listen_addr) {
    meshText = "Not listening";
    meshTone = "warn";
  } else {
    meshText = connected ? `${plural(connected, "device")} connected` : "Listening · no devices connected";
    if (notResponding) meshText += ` · ${notResponding} not responding`;
    meshTone = notResponding ? "warn" : connected ? "ok" : "neutral";
  }
  const meshTip = !up
    ? unknownTip
    : net?.enabled
      ? `${[plural(net.connections, "connection"), ...rows.filter((r) => r.trust === "trusted").map((r) => `${r.name}: ${r.state.word}`)].join(". ")}.`
      : undefined;

  // 6. Sharing (only while networking is on)
  const serving = j && j !== "unsupported" ? j.active.filter((x) => x.kind === "served_for_peer") : [];
  const servedFor = [...new Set(serving.map((x) => x.origin))];
  const sharing = net?.enabled
    ? serving.length
      ? { text: `Serving ${plural(serving.length, "request")} for ${servedFor.length === 1 ? nameOfPeer(servedFor[0]) : plural(servedFor.length, "device")}`, tone: "active" as const, to: href("jobs", null, { scope: "served" }) }
      : { text: net.serve_peers ? "Sharing on" : "Not sharing", tone: "neutral" as const, to: href("settings", "sharing") }
    : null;

  // 7. Network rate (E1 byte counters; a gap is unknown, never 0)
  const rate = up ? netRate(conn) : null;
  const [prevConn, lastConn] = conn.slice(-2);
  // The real window between the two samples (about 6 s at the 2 s connected poll), never a nominal one.
  const rateTip = rate
    ? `Received ${Math.round(rate.rx)} B/s, sent ${Math.round(rate.tx)} B/s, over the ${Math.round((lastConn.at - prevConn.at) / 1000)} s to ${clock(lastConn.at)}. Includes 1 s keep-alives.`
    : "Not measured: needs one connection counted twice, about 5 s apart. Includes 1 s keep-alives.";

  const modeLabel = mode ? MODE_LABEL[mode] : "unknown";

  return (
    <footer className="statusbar" aria-label="Status" data-region="statusbar">
      <div className="sb-group">
        {attentionItem}
        <Item tone={service.tone} dot tip={[statusStale, serviceTip].filter(Boolean).join(" ") || undefined}>
          {service.text}
        </Item>
        <Item tone={engineTone} icon={loaded?.fallback_reason ? "circle-alert" : undefined} to={href("models", engineModel)} tip={engineTip} stale={statusStale}>
          {engineText}
        </Item>
        {q && (
          <Item
            tone={slotsWarn ? "warn" : "neutral"}
            drop={4}
            to={href("jobs")}
            stale={statusStale}
            tip={`${q.inflight} of ${q.max_concurrent} engine slots in use; ${q.queued} waiting (room for ${q.max_queue}). Includes requests served for paired devices.`}
          >
            In use {q.inflight}/{q.max_concurrent} · Waiting {q.queued}
          </Item>
        )}
      </div>
      <div className="sb-group sb-right">
        <Item tone={meshTone} to={href("devices")} tip={meshTip} stale={meshStale}>
          {meshText}
        </Item>
        {sharing && (
          <Item
            tone={sharing.tone}
            drop={3}
            to={sharing.to}
            stale={up && jobsLive.error ? notUpdated(jobsLive.at) : meshStale}
            tip={net ? `Paired devices may use up to ${plural(net.peer_slots, "engine slot")} here.` : undefined}
          >
            {sharing.text}
          </Item>
        )}
        <Item drop={2} tip={rateTip}>
          <Icon name="arrow-down" />
          <span className="visually-hidden">Received </span>
          {rate ? byteRate(rate.rx) : "—"} <Icon name="arrow-up" />
          <span className="visually-hidden">Sent </span>
          {rate ? byteRate(rate.tx) : "—"}
        </Item>
        <Item icon="shield-check" to={href("settings", "privacy")} label={`Privacy mode: ${modeLabel}`}>
          {mode ? modeLabel : "Privacy mode unknown"}
        </Item>
        <Item drop={1} icon="hard-drive" to={href("settings", "storage")} tip={disk != null ? `${disk.toLocaleString()} bytes free, measured at ${clock(hw.at)}.` : "Free disk space was not reported by the hardware probe."}>
          Disk {disk != null ? `${humanBytes(disk)} free` : "—"}
        </Item>
      </div>
    </footer>
  );
}
