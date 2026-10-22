import { href, SECTIONS, type Section } from "../router";
import { useDeviceRows } from "../state/devices";
import { mesh, network, ownStreams, pulls } from "../state/live";
import { daemon } from "../state/stores";
import { BrandMark, Icon } from "../ui/icons";
import { SECTION_META } from "./sections";

interface Badge {
  text: string;
  /** Read as the link's description. */
  says: string;
  warn?: boolean;
}

/** Rail badges, computed from live stores on every render (never cached). */
function useBadges(): Partial<Record<Section, Badge>> {
  const rows = useDeviceRows();
  const p = pulls.use();
  const d = daemon.use();
  const meshLive = mesh.use();
  const netLive = network.use();
  const q = d.status?.queue;
  // The device count only from a running, answering service whose last mesh and network reads worked: the stores keep
  // their last good data, and connections of a stopped service are not connections (as the status bar's Mesh item).
  const fresh = d.reachable === true && d.responding !== false && !meshLive.error && !netLive.error;
  const streams = ownStreams.use();
  const paired = rows.filter((r) => r.trust === "trusted");
  const connected = paired.filter((r) => r.peer && r.peer.connected && r.peer.health !== "unhealthy").length;
  // The warn dot only for Not responding and Unreachable, never for Not seen nearby, Not connected or Disconnected.
  const trouble = paired.some((r) => r.state.word === "Not responding" || r.state.word === "Unreachable");
  const downloading = Object.values(p).filter((x) => x.state === "downloading").length;
  const jobs = q ? q.inflight + q.queued : 0;
  const out: Partial<Record<Section, Badge>> = {};
  if (fresh && paired.length)
    out.devices = {
      text: `${connected}/${paired.length}`,
      says: `${connected} of ${paired.length} paired devices connected${trouble ? "; one is not responding or unreachable" : ""}`,
      warn: trouble,
    };
  if (downloading) out.models = { text: `↓${downloading}`, says: `${downloading} ${downloading === 1 ? "download" : "downloads"} running` };
  if (jobs) out.jobs = { text: String(jobs), says: `${jobs} ${jobs === 1 ? "request" : "requests"} in use or waiting. Includes requests served for paired devices.` };
  if (Object.keys(streams).length) out.chat = { text: "", says: "A reply is being generated" };
  return out;
}

/** `<nav aria-label="Sections">`: brand, then the seven sections with live badges. Below 900 px: icons only (CSS). */
export function Rail({ current }: { current: Section }) {
  const badges = useBadges();
  return (
    <nav className="rail" aria-label="Sections" data-region="rail">
      <div className="rail-brand">
        <BrandMark />
        <span className="rail-label">UCompute</span>
      </div>
      <ul>
        {SECTIONS.map((s) => {
          const meta = SECTION_META[s];
          const b = badges[s];
          return (
            <li key={s}>
              <a className="rail-item" href={href(s)} aria-current={current === s ? "page" : undefined} aria-label={meta.label} aria-describedby={b ? `rail-badge-${s}` : undefined}>
                <Icon name={meta.icon} />
                <span className="rail-label">{meta.label}</span>
                {b && (
                  <span className={`rail-badge${b.text ? "" : " dot"}${b.warn ? " warn" : ""}`} aria-hidden="true">
                    {b.text}
                  </span>
                )}
              </a>
              {b && (
                <span id={`rail-badge-${s}`} hidden>
                  {b.says}
                </span>
              )}
            </li>
          );
        })}
      </ul>
    </nav>
  );
}
