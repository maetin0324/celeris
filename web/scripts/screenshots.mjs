import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { screens } from "../e2e/support/screens.ts";
import { applyStateRoute, states } from "../e2e/support/states.ts";
import { startFixtureGateway } from "./fixture-gateway.mjs";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const arg = (name) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? null : process.argv[index + 1];
};
const only = arg("--only");
const out = arg("--out");
const withStates = process.argv.includes("--states");
if (!out) throw new Error("--out <directory> is required");
const selected = only ? screens.filter((screen) => screen.fixture === only.split("?")[0]) : screens;
if (!withStates && !selected.length) throw new Error(`unknown screen: ${only}`);
if (!existsSync(path.join(webRoot, "dist/index.html"))) {
  const built = spawnSync(path.join(webRoot, "node_modules/.bin/vite"), ["build"], { cwd: webRoot, stdio: "inherit" });
  if (built.status !== 0) throw new Error("vite build failed");
}
mkdirSync(out, { recursive: true });
const browser = await chromium.launch();
let screenshotCount = 0;
try {
  if (withStates) {
    for (const state of states) {
      for (const target of state.screens) {
        const gateway = await startFixtureGateway(state.daemon);
        try {
          for (const width of [360, 390, 412, 1440]) {
            const page = await browser.newPage({ viewport: { width, height: 800 } });
            try {
              await applyStateRoute(page, state);
              await page.goto(`${gateway.base}${target}`);
              // loading は状態を撮り終えるまで保留し、撮影後に必ず解放する。
              await page.screenshot({
                path: path.join(out, `${state.key}-${target.replace(/[^a-z0-9]+/gi, "_") || "root"}-${width}.png`),
                fullPage: true,
              });
              screenshotCount += 1;
            } finally {
              await page.close();
            }
          }
        } finally {
          gateway.daemon.releaseHeld();
          await gateway.close();
        }
      }
    }
  } else {
    const gateway = await startFixtureGateway();
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
          screenshotCount += 1;
          await page.close();
        }
      }
    } finally {
      await gateway.close();
    }
  }
} finally {
  await browser.close();
}
process.stdout.write(
  `screenshots: ${screenshotCount} image(s)${withStates ? ` across ${states.length} states` : ` from ${selected.length} screen(s)`} -> ${out}\n`,
);
