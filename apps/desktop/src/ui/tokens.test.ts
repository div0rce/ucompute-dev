// WCAG 2.x contrast of the design tokens (plan §3.1), read from tokens.css so a value change is caught here.
// jsdom has no layout: axe color-contrast runs in the real-browser pass on top of this.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const css = readFileSync(join(__dirname, "tokens.css"), "utf8");

function block(selector: RegExp): Record<string, string> {
  const m = css.match(selector);
  if (!m) throw new Error(`no block ${selector}`);
  return Object.fromEntries([...m[1].matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})\s*;/g)].map((x) => [x[1], x[2]]));
}
const light = block(/^:root \{([\s\S]*?)\n\}/m);
const darkMedia = block(/:root:not\(\[data-theme="light"\]\) \{([\s\S]*?)\n {2}\}/);
const darkAttr = block(/:root\[data-theme="dark"\] \{([\s\S]*?)\n\}/);
const dark = { ...light, ...darkMedia };

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

const SURFACES = ["bg", "surface", "surface-2", "hover", "selected"];

describe.each([
  ["light", light],
  ["dark", dark],
])("%s tokens", (_, t) => {
  it.each(["text", "muted", "ok", "active", "warn", "danger"])("--%s is at least 4.5:1 on every surface", (fg) => {
    for (const s of SURFACES) expect(contrast(t[fg], t[s]), `${fg} on ${s}`).toBeGreaterThanOrEqual(4.5);
  });
  it("--accent-ink on --accent is at least 4.5:1", () => {
    expect(contrast(t["accent-ink"], t.accent)).toBeGreaterThanOrEqual(4.5);
  });
  it("the focus ring (--accent) is at least 3:1 on surface and selected", () => {
    for (const s of ["surface", "selected"]) expect(contrast(t.accent, t[s]), s).toBeGreaterThanOrEqual(3);
  });
  it("--line-input (control borders) is at least 3:1 on every surface, including selected", () => {
    for (const s of SURFACES) expect(contrast(t["line-input"], t[s]), s).toBeGreaterThanOrEqual(3);
  });
  it("count badges: --lime-ink on --lime is at least 4.5:1", () => {
    expect(contrast(t["lime-ink"], t.lime)).toBeGreaterThanOrEqual(4.5);
  });
});

describe("dark tokens are the same under the media query and data-theme", () => {
  it("defines every dark override twice, identically", () => {
    expect(darkAttr).toEqual(darkMedia);
    expect(Object.keys(darkMedia).length).toBeGreaterThan(10);
  });
});
