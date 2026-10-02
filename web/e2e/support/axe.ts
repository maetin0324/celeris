import { createRequire } from "node:module";
import type { Page } from "@playwright/test";

// axe-core を page に注入し、critical / serious の違反だけを返す（S4）。注入には bypassCSP: true が要る。
const axePath = createRequire(import.meta.url).resolve("axe-core/axe.min.js");

export async function seriousViolations(page: Page) {
  await page.addScriptTag({ path: axePath });
  return page.evaluate(async () => {
    const axe = (
      window as unknown as Window & {
        axe: { run: () => Promise<{ violations: Array<{ id: string; impact: string; nodes: unknown[] }> }> };
      }
    ).axe;
    return (await axe.run()).violations
      .filter((item) => item.impact === "critical" || item.impact === "serious")
      .map((item) => ({ id: item.id, impact: item.impact, nodes: item.nodes.length }));
  });
}
