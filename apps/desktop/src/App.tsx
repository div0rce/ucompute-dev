import { useEffect } from "react";
import { startDaemonPolling } from "./daemon";
import { href, useRoute, type Route } from "./router";
import { daemon } from "./state/stores";
import { Chat } from "./views/Chat";
import { Diagnostics } from "./views/Diagnostics";
import { Hardware } from "./views/Hardware";
import { Home } from "./views/Home";
import { Models } from "./views/Models";
import { Performance } from "./views/Performance";
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
      { route: "peers", label: "Peers", later: "M3" },
      { route: "cluster", label: "Cluster", later: "M3" },
      { route: "jobs", label: "Jobs", later: "M3" },
      { route: "contributions", label: "Contributions", later: "M7" },
      { route: "privacy", label: "Privacy", later: "M3" },
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
      return <Later title="Peers" milestone="milestone M3" what="Pair trusted devices with a short code so they can share models and compute." />;
    case "cluster":
      return <Later title="Cluster" milestone="milestone M3" what="See paired devices as one pool and run models larger than any single device." />;
    case "jobs":
      return <Later title="Jobs" milestone="milestone M3" what="Watch requests move through the pool: where each one runs, how long it takes, what it cost." />;
    case "contributions":
      return <Later title="Contributions" milestone="milestone M7" what="Lend spare compute to others on your terms, with caps for CPU, GPU, battery and hours." />;
    case "privacy":
      return <Later title="Privacy" milestone="milestone M3" what="Choose local-only, trusted devices or public compute, with a clear disclosure before anything leaves this machine. Today everything runs locally." />;
  }
}

export default function App({ poll = true }: { poll?: boolean }) {
  const route = useRoute();
  const d = daemon.use();

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
