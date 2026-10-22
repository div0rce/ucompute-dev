import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./ui/tokens.css";
import "./ui/base.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

// Development only: `?axe=1` runs the accessibility sweep on every section in both themes (plan §5.5 step 3).
// `import.meta.env.DEV` is the constant false in production builds, which drops this and the axe-core chunk.
if (import.meta.env.DEV && new URLSearchParams(window.location.search).has("axe")) void import("./shell/devAxe").then((m) => m.runAxeSweep());
