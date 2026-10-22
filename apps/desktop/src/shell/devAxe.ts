// Development only (plan §5.5 step 3): `?axe=1` on the Vite dev server runs axe-core, colour contrast included (jsdom
// cannot check it), on every section in light and dark, then leaves the results on `window.__ucomputeAxe` and in the
// console for the evidence folder. main.tsx imports this only under `import.meta.env.DEV`, so production builds never
// contain it (nor axe-core).
import axe from "axe-core";
import { go, SECTIONS, type Section } from "../router";
import { applyTheme, ui } from "../state/ui";

export interface AxeSweepResult {
  section: Section;
  theme: "light" | "dark";
  violations: { id: string; impact: string | null | undefined; help: string; targets: string[] }[];
  passes: number;
  incomplete: number;
}

export async function runAxeSweep({
  sections = SECTIONS,
  themes = ["light", "dark"],
  settleMs = 1500,
}: { sections?: readonly Section[]; themes?: readonly ("light" | "dark")[]; settleMs?: number } = {}): Promise<AxeSweepResult[]> {
  const out: AxeSweepResult[] = [];
  for (const theme of themes) {
    await applyTheme(theme);
    for (const section of sections) {
      go(section);
      // ponytail: a fixed wait for the section's first data; raise `settleMs` on a slow service.
      await new Promise((r) => setTimeout(r, settleMs));
      const r = await axe.run(document);
      out.push({
        section,
        theme,
        violations: r.violations.map((v) => ({ id: v.id, impact: v.impact, help: v.help, targets: v.nodes.map((n) => n.target.join(" ")) })),
        passes: r.passes.length,
        incomplete: r.incomplete.length,
      });
    }
  }
  await applyTheme(ui.get().theme);
  (window as unknown as { __ucomputeAxe?: AxeSweepResult[] }).__ucomputeAxe = out;
  console.info("[ucompute axe]", JSON.stringify(out));
  return out;
}
