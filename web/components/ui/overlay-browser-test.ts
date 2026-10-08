import { randomUUID } from "node:crypto";
import { rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { join } from "node:path";
import { type Browser, chromium, type Page } from "@playwright/test";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { createServer, type ViteDevServer } from "vite";

const axePath = createRequire(import.meta.url).resolve("axe-core/axe.min.js");

// Fixture startup and browser operations can be slow when the UI test files run together.
export const OVERLAY_FIXTURE_TIMEOUT = 120_000;

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

export async function openOverlayFixture(
  fixturePath = "/components/ui/fixtures/overlay.html",
): Promise<{ page: Page; close: () => Promise<void> }> {
  // UI test files run concurrently. Each fixture server gets its own deps cache so that one server's
  // re-optimization does not invalidate the optimized deps another server's page is loading (the page
  // would then never render and the hook would hang until it times out).
  const cacheDir = join(process.cwd(), "node_modules", ".vite", `overlay-${randomUUID()}`);
  const server: ViteDevServer = await createServer({
    configFile: false,
    root: process.cwd(),
    cacheDir,
    plugins: [react(), tailwindcss()],
    server: { host: "127.0.0.1", port: 0, strictPort: false },
  });
  let browser: Browser | undefined;
  try {
    await server.listen();
    const address = server.httpServer?.address();
    if (!address || typeof address === "string") throw new Error("Vite の port を取得できません");
    browser = await chromium.launch({ headless: true, timeout: OVERLAY_FIXTURE_TIMEOUT });
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${address.port}${fixturePath}`, {
      waitUntil: "domcontentloaded",
      timeout: OVERLAY_FIXTURE_TIMEOUT,
    });
    await page.getByRole("heading", { level: 1 }).waitFor({ timeout: OVERLAY_FIXTURE_TIMEOUT });
    return {
      page,
      close: async () => {
        await browser?.close();
        await server.close();
        await rm(cacheDir, { recursive: true, force: true });
      },
    };
  } catch (error) {
    await browser?.close();
    await server.close();
    await rm(cacheDir, { recursive: true, force: true });
    throw error;
  }
}
