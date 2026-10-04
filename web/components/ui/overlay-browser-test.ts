import { createRequire } from "node:module";
import { type Browser, chromium, type Page } from "@playwright/test";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { createServer, type ViteDevServer } from "vite";

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

export async function openOverlayFixture(
  fixturePath = "/components/ui/fixtures/overlay.html",
): Promise<{ page: Page; close: () => Promise<void> }> {
  const server: ViteDevServer = await createServer({
    configFile: false,
    root: process.cwd(),
    plugins: [react(), tailwindcss()],
    server: { host: "127.0.0.1", port: 0, strictPort: false },
  });
  let browser: Browser | undefined;
  try {
    await server.listen();
    const address = server.httpServer?.address();
    if (!address || typeof address === "string") throw new Error("Vite の port を取得できません");
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${address.port}${fixturePath}`);
    await page.getByRole("heading", { level: 1 }).waitFor();
    return {
      page,
      close: async () => {
        await browser?.close();
        await server.close();
      },
    };
  } catch (error) {
    await browser?.close();
    await server.close();
    throw error;
  }
}
