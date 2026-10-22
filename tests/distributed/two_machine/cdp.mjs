#!/usr/bin/env node
// Tiny Chrome DevTools Protocol client for the Windows app's WebView2, reached through an SSH tunnel to
// its debugging port on the laptop's loopback (the app is launched with
// WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 --remote-debugging-address=127.0.0.1).
//   node cdp.mjs eval '<js expression or async IIFE>'   prints the JSON value
//   node cdp.mjs text                                   prints the page's visible text
//   node cdp.mjs click '<visible text>'                 clicks the first button/link/tab whose text matches
//   node cdp.mjs shot <out.png>                         screenshot of the app window's page
// CDP_PORT selects the local tunnel port (default 19222). Stdlib only (Node 22+: global WebSocket).
import { writeFileSync } from "node:fs";

const PORT = process.env.CDP_PORT || "19222";
const [cmd, arg] = process.argv.slice(2);
const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find((t) => t.type === "page");
if (!page) throw new Error("no page target");
const ws = new WebSocket(page.webSocketDebuggerUrl);
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
};
await new Promise((r, j) => {
  ws.onopen = r;
  ws.onerror = j;
});
const send = (method, params = {}) =>
  new Promise((resolve) => {
    const i = ++id;
    pending.set(i, resolve);
    ws.send(JSON.stringify({ id: i, method, params }));
  });
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.result?.exceptionDetails) {
    throw new Error(r.result.exceptionDetails.exception?.description || JSON.stringify(r.result.exceptionDetails));
  }
  return r.result?.result?.value ?? null;
};

try {
  if (cmd === "eval") {
    console.log(JSON.stringify(await evaluate(arg), null, 1));
  } else if (cmd === "text") {
    console.log(await evaluate("document.body.innerText"));
  } else if (cmd === "click") {
    const js = `(() => { const want = ${JSON.stringify(arg)};
      const el = [...document.querySelectorAll('button, a, [role=tab], [role=menuitem], summary')]
        .find(e => e.offsetParent !== null && (e.innerText || e.getAttribute('aria-label') || '').trim().startsWith(want));
      if (!el) return 'not found: ' + want; el.click(); return 'clicked: ' + (el.innerText || el.getAttribute('aria-label')).trim(); })()`;
    console.log(await evaluate(js));
  } else if (cmd === "shot") {
    const r = await send("Page.captureScreenshot", { format: "png" });
    writeFileSync(arg, Buffer.from(r.result.data, "base64"));
    console.log(`saved ${arg}`);
  } else {
    console.error("usage: cdp.mjs eval '<js>' | text | click '<visible text>' | shot <file.png>");
    process.exitCode = 2;
  }
} catch (e) {
  console.error(String(e));
  process.exitCode = 1;
} finally {
  ws.close();
}
