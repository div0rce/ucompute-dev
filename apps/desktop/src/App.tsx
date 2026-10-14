import { useEffect } from "react";
import { api } from "./api/client";
import { startDaemonPolling } from "./daemon";
import { href, useRoute, type Route } from "./router";
import { daemon, privacy } from "./state/stores";
import { Chat } from "./views/Chat";
import { Diagnostics } from "./views/Diagnostics";
import { Hardware } from "./views/Hardware";
import { Home } from "./views/Home";
import { Models } from "./views/Models";
import { Peers } from "./views/Peers";
import { Performance } from "./views/Performance";
import { MODE_LABEL, Privacy } from "./views/Privacy";
import { Settings } from "./views/Settings";
import { Later } from "./components/Later";

const NAV: { group: string; items: { route: Route; label: string; later?: string }[] }[] = [
  {
    group: "Run",
    items: [
      { route: "home", label: "Home" },
      { route: "models", label: "Models" },
      { route: "chat", label: "Chat" },
    ],
  },
  {
    group: "This computer",
    items: [
      { route: "hardware", label: "Hardware" },
      { route: "performance", label: "Performance" },
      { route: "diagnostics", label: "Diagnostics" },
    ],
  },
  {
    group: "Devices and network",
    items: [
      { route: "peers", label: "Peers" },
      { route: "cluster", label: "Cluster", later: "M3" },
      { route: "jobs", label: "Jobs", later: "M3" },
      { route: "contributions", label: "Contributions", later: "M7" },
      { route: "privacy", label: "Privacy" },
    ],
  },
  { group: "", items: [{ route: "settings", label: "Settings" }] },
];

function View({ route }: { route: Route }) {
  switch (route) {
    case "home":
      return <Home />;
    case "models":
      return <Models />;
    case "chat":
      return <Chat />;
    case "hardware":
      return <Hardware />;
    case "performance":
      return <Performance />;
    case "settings":
      return <Settings />;
    case "diagnostics":
      return <Diagnostics />;
    case "peers":
      return <Peers />;
    case "cluster":
      return <Later title="Cluster" milestone="milestone M3" what="See paired devices as one pool and run models larger than any single device." />;
    case "jobs":
      return <Later title="Jobs" milestone="milestone M3" what="Watch requests move through the pool: where each one runs, how long it takes, what it cost." />;
    case "contributions":
      return <Later title="Contributions" milestone="milestone M7" what="Lend spare compute to others on your terms, with caps for CPU, GPU, battery and hours." />;
    case "privacy":
      return <Privacy />;
  }
}

export default function App({ poll = true }: { poll?: boolean }) {
  const route = useRoute();
  const d = daemon.use();
  const p = privacy.use();

  // The mode in the header must be the daemon's, never a cached guess: fetch it whenever the service comes back.
  useEffect(() => {
    if (!d.reachable) return;
    api.privacy().then(privacy.set, () => undefined);
  }, [d.reachable]);

  useEffect(() => {
    if (!poll) return;
    let stop: (() => void) | undefined;
    startDaemonPolling().then((s) => (stop = s));
    return () => stop?.();
  }, [poll]);

  return (
    <div className="app">
      <a className="skip" href="#main">
        Skip to content
      </a>
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true" />
          Common
        </div>
        <a className={`mode-pill${p && p.mode !== "local_only" ? " away" : ""}`} href={href("privacy")} aria-label={`Privacy mode: ${p ? MODE_LABEL[p.mode] : "unknown"}`}>
          {p ? MODE_LABEL[p.mode] : "Privacy mode unknown"}
        </a>
        <nav className="nav" aria-label="Main">
          {NAV.map((g) => (
            <div key={g.group || "settings"}>
              {g.group && <div className="nav-group">{g.group}</div>}
              {g.items.map((it) => (
                <a key={it.route} href={href(it.route)} aria-current={route === it.route ? "page" : undefined}>
                  {it.label}
                  {it.later && <span className="later">{it.later}</span>}
                </a>
              ))}
            </div>
          ))}
        </nav>
        <div className="sidebar-foot">
          <span className={`dot ${d.reachable ? "ok" : d.reachable === false ? "bad" : ""}`} aria-hidden="true" />
          {d.reachable ? `Service running ${d.version ?? ""}` : d.reachable === false ? "Service not running" : "Connecting"}
        </div>
      </aside>
      <main id="main" tabIndex={-1}>
        <View route={route} />
      </main>
    </div>
  );
}
