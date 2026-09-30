import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { screens } from "../e2e/support/screens.ts";
import { createApp } from "../server/app.js";

// S3 の最小の監査（docs/web/implementation-plan.md §2）。幅 360 / 390 / 412 / 1440 px で、ページ全体の横溢れが 0、
// 見えている操作要素が 44×44 px 以上かを見る。gateway は空き port の loopback で起こし、daemon には接続しない。
// 使い方: node scripts/mobile-audit.mjs [--only "/login"] [--screenshots <dir>]
const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const WIDTHS = [360, 390, 412, 1440];
const DEFAULT_PATHS = screens.map((screen) => screen.fixture);

function argValue(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? null : process.argv[index + 1];
}

const only = argValue("--only");
const screenshots = argValue("--screenshots");
const paths = only ? [only] : DEFAULT_PATHS;
if (only && !screens.some((screen) => screen.fixture === only)) throw new Error(`unknown screen: ${only}`);

if (!existsSync(path.join(webRoot, "dist/index.html"))) {
  const built = spawnSync(path.join(webRoot, "node_modules/.bin/vite"), ["build"], { cwd: webRoot, stdio: "inherit" });
  if (built.status !== 0) throw new Error("vite build failed");
}

const server = createApp({ log: () => {} }).listen(0, "127.0.0.1");
await new Promise((resolve, reject) => {
  server.once("listening", resolve);
  server.once("error", reject);
});
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch();
const failures = [];
try {
  for (const route of paths) {
    for (const width of WIDTHS) {
      const page = await browser.newPage({ viewport: { width, height: 800 } });
      await page.goto(`${base}${route}`, { waitUntil: "networkidle" });
      const result = await page.evaluate(() => {
        const doc = document.documentElement;
        const small = [];
        const unnamed = [];
        const badFocus = [];
        for (const el of document.querySelectorAll(
          "a, button, input:not([type=hidden]), select, textarea, [role=button]",
        )) {
          const box = el.getBoundingClientRect();
          if (box.width === 0 && box.height === 0) continue;
          const label =
            el.getAttribute("aria-label") ||
            (el.getAttribute("aria-labelledby") &&
              document.getElementById(el.getAttribute("aria-labelledby"))?.textContent) ||
            (el instanceof HTMLInputElement && el.labels?.[0]?.textContent) ||
            el.textContent;
          if (!label?.trim()) unnamed.push(el.outerHTML.slice(0, 120));
          if (el.tabIndex > 0) badFocus.push(el.outerHTML.slice(0, 120));
          if (box.width < 44 || box.height < 44)
            small.push(
              `${el.tagName.toLowerCase()}#${el.id || "-"} ${Math.round(box.width)}x${Math.round(box.height)}`,
            );
        }
        return {
          overflow: doc.scrollWidth - doc.clientWidth,
          small,
          unnamed,
          badFocus,
          main: document.querySelectorAll("main").length,
          headings: document.querySelectorAll("h1").length,
        };
      });
      if (result.overflow > 0) failures.push(`${route} @${width}: horizontal overflow ${result.overflow}px`);
      for (const item of result.small) failures.push(`${route} @${width}: tap target ${item} < 44x44`);
      for (const item of result.unnamed) failures.push(`${route} @${width}: unnamed control ${item}`);
      for (const item of result.badFocus) failures.push(`${route} @${width}: positive tabindex ${item}`);
      if (result.main !== 1 || result.headings !== 1)
        failures.push(`${route} @${width}: expected one main and one h1; got ${result.main}/${result.headings}`);
      if (screenshots) {
        mkdirSync(screenshots, { recursive: true });
        await page.screenshot({
          path: path.join(screenshots, `${route.replace(/[^a-z0-9]+/gi, "_") || "root"}-${width}.png`),
          fullPage: true,
        });
      }
      await page.close();
    }
  }
} finally {
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
if (failures.length) {
  console.error(failures.join("\n"));
  process.exitCode = 1;
} else {
  process.stdout.write(`mobile-audit: ${paths.length} path(s) x ${WIDTHS.length} widths ok\n`);
}
