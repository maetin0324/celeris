import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { screens } from "../e2e/support/screens.ts";
import { startFixtureGateway } from "./fixture-gateway.mjs";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const arg = (name) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? null : process.argv[index + 1];
};
const only = arg("--only");
const out = arg("--out");
if (!out) throw new Error("--out <directory> is required");
const selected = only ? screens.filter((screen) => screen.fixture === only.split("?")[0]) : screens;
if (!selected.length) throw new Error(`unknown screen: ${only}`);
if (!existsSync(path.join(webRoot, "dist/index.html"))) {
  const built = spawnSync(path.join(webRoot, "node_modules/.bin/vite"), ["build"], { cwd: webRoot, stdio: "inherit" });
  if (built.status !== 0) throw new Error("vite build failed");
}
mkdirSync(out, { recursive: true });
const gateway = await startFixtureGateway();
const browser = await chromium.launch();
try {
  for (const screen of selected) {
    // --only は台帳の fixture に ?tab= などの query を付けてもよい（P3-13 の tab）。
    const target = only ?? screen.fixture;
    for (const width of [360, 390, 412, 1440]) {
      const page = await browser.newPage({ viewport: { width, height: 800 } });
      await page.goto(`${gateway.base}${target}`);
      await page.screenshot({
        path: path.join(out, `${target.replace(/[^a-z0-9]+/gi, "_") || "root"}-${width}.png`),
        fullPage: true,
      });
      await page.close();
    }
  }
} finally {
  await browser.close();
  await gateway.close();
}
process.stdout.write(`screenshots: ${selected.length} screen(s) x 4 widths -> ${out}\n`);
