import { useEffect, useState } from "react";

export const ROUTES = [
  "home",
  "models",
  "chat",
  "hardware",
  "performance",
  "settings",
  "diagnostics",
  "peers",
  "cluster",
  "jobs",
  "contributions",
  "privacy",
] as const;
export type Route = (typeof ROUTES)[number];

function parse(hash: string): Route {
  const r = hash.replace(/^#\/?/, "").split(/[/?]/)[0];
  return (ROUTES as readonly string[]).includes(r) ? (r as Route) : "home";
}

export const href = (r: Route) => `#/${r}`;
export function navigate(r: Route): void {
  window.location.hash = href(r);
}

/** Hash router: `#/models`. Deep links survive reloads; no dependency. */
export function useRoute(): Route {
  const [route, setRoute] = useState<Route>(() => parse(window.location.hash));
  useEffect(() => {
    const on = () => setRoute(parse(window.location.hash));
    window.addEventListener("hashchange", on);
    return () => window.removeEventListener("hashchange", on);
  }, []);
  return route;
}
