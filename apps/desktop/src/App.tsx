import { useEffect, useRef } from "react";
import { startDaemonPolling } from "./daemon";
import { useLocation } from "./router";
import { Announcer } from "./shell/announcer";
import { startWatchers } from "./shell/attention";
import { installCopyHandler } from "./shell/copy";
import { installMenu } from "./shell/menu";
import { Rail } from "./shell/Rail";
import { SECTION_COMPONENTS } from "./shell/sections";
import { installShortcuts } from "./shell/shortcuts";
import { ShortcutsSheet } from "./shell/ShortcutsSheet";
import { StatusBar } from "./shell/StatusBar";
import { neededKeys, refreshMany } from "./state/live";
import { ALWAYS } from "./state/poll";
import { daemon } from "./state/stores";
import { applyTheme, ui } from "./state/ui";

/**
 * The shell: rail, the current section, status bar (plan §1.1). `poll={false}` (tests) skips daemon discovery and the
 * poller; the always-on stores are still read once whenever the service becomes reachable.
 */
export default function App({ poll = true }: { poll?: boolean }) {
  const { section } = useLocation();
  const reachable = daemon.use().reachable;
  const theme = ui.use().theme;
  const main = useRef<HTMLElement>(null);

  // What the status bar and rail show must be the daemon's, never a cached guess: read it whenever the service is back,
  // with whatever the mounted section needs (it fetched nothing before the service's address was known).
  useEffect(() => {
    if (reachable) void refreshMany([...new Set([...ALWAYS, ...neededKeys()])]);
  }, [reachable]);

  useEffect(() => {
    if (!poll) return;
    let stop: (() => void) | undefined;
    let gone = false;
    startDaemonPolling().then((s) => (gone ? s() : (stop = s)));
    return () => {
      gone = true;
      stop?.();
    };
  }, [poll]);

  useEffect(() => {
    const offs = [installShortcuts(), installCopyHandler(), startWatchers()];
    void installMenu();
    return () => offs.forEach((off) => off());
  }, []);

  useEffect(() => {
    void applyTheme(theme);
  }, [theme]);

  const Section = SECTION_COMPONENTS[section];
  return (
    <div className="shell">
      <a
        className="skip"
        href="#main"
        onClick={(e) => {
          // The hash is the router's: focus the main area instead of navigating.
          e.preventDefault();
          main.current?.focus();
        }}
      >
        Skip to content
      </a>
      <Rail current={section} />
      <main id="main" ref={main} tabIndex={-1} className="shell-main">
        <Section />
      </main>
      <StatusBar />
      <Announcer />
      <ShortcutsSheet />
    </div>
  );
}
