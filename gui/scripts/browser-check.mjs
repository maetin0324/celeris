// Offline browser capability smoke: built GUI + existing API fixture, no external dashboard request.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";
import { getFreePort, setupMockCeleris, TASK_ID, waitForHealth } from "./lib/celeris-fixture.mjs";

const out = process.argv[2];
if (!out || !path.isAbsolute(out)) throw new Error("Usage: node scripts/browser-check.mjs /absolute/artifacts/path");
fs.mkdirSync(out, { recursive: true });
const guiDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const mock = await setupMockCeleris();
const detail = /** @type {{task: Record<string, unknown>}} */ (
  await (await fetch(`${mock.baseUrl}/api/v1/tasks/${TASK_ID}`)).json()
);
const run = {
  run_id: "browser-mvp-r1",
  adapter: "opencode",
  model: "fixture",
  role: "worker",
  started_at: "2026-09-28T00:00:00Z",
  artifacts: 0,
  progress: 0,
  verdicts: 0,
  reviewer_deferrals: 0,
};
let state = "RUNNING";
/** @type {string | null} */
let liveViewUrl = "https://browser.example/dashboard";
let taskStatus = "running";
/** @param {import("node:http").ServerResponse} res @param {unknown} body */
function json(res, body) {
  res.writeHead(200, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
}
mock.on("GET", `/api/v1/tasks/${TASK_ID}`, (_req, res) =>
  json(res, { ...detail, task: { ...detail.task, status: taskStatus }, runs: [run] }),
);
mock.on("GET", `/api/v1/tasks/${TASK_ID}/runs`, (_req, res) => json(res, { runs: [run] }));
mock.on("GET", `/api/v1/tasks/${TASK_ID}/events`, (_req, res) =>
  json(res, {
    has_more: false,
    items: [
      {
        id: 1,
        seq: 1,
        task_id: TASK_ID,
        ts: "2026-09-28T00:00:00Z",
        event: {
          type: "browser_updated",
          browser: {
            task_id: TASK_ID,
            run_id: run.run_id,
            session_id: "celeris-isolated-browser-mvp-r1",
            state,
            live_view_url: liveViewUrl,
          },
        },
      },
    ],
  }),
);
const port = await getFreePort();
const base = `http://127.0.0.1:${port}`;
const server = spawn(process.execPath, ["server.js"], {
  cwd: guiDir,
  env: { ...process.env, CELERIS_API_URL: mock.baseUrl, CELERIS_GUI_BIND: `127.0.0.1:${port}` },
  stdio: "ignore",
});
let browser;
try {
  await waitForHealth(`${base}/healthz`, 30000);
  browser = await chromium.launch();
  /** @type {[string, {width: number, height: number}][]} */
  const viewports = [
    ["desktop", { width: 1280, height: 900 }],
    ["mobile", { width: 393, height: 851 }],
  ];
  for (const [name, viewport] of viewports) {
    const page = await browser.newPage({ viewport });
    await page.goto(`${base}/tasks/${TASK_ID}?types=created`);
    const panel = page.getByTestId("browser-runs");
    await panel.waitFor();
    const link = panel.getByRole("link", { name: "Open Browser Live View" });
    assert.equal(await link.getAttribute("href"), liveViewUrl);
    assert.equal(await link.getAttribute("rel"), "noopener noreferrer");
    await panel.screenshot({ path: path.join(out, `browser-ui-${name}.png`) });
    assert.equal(await page.evaluate("document.documentElement.scrollWidth > window.innerWidth"), false);
    await page.goto(`${base}/tasks/${TASK_ID}/runs/${run.run_id}`);
    await page.getByTestId("browser-runs").getByRole("link", { name: "Open Browser Live View" }).waitFor();
    await page.close();
  }
  const page = await browser.newPage();
  for (const scenario of ["completed", "unsafe-url", "task-cancelled", "unconfigured"]) {
    state = scenario === "completed" ? "COMPLETED" : "RUNNING";
    liveViewUrl =
      scenario === "unsafe-url"
        ? "javascript:alert(1)"
        : scenario === "unconfigured"
          ? null
          : "https://browser.example/dashboard";
    taskStatus = scenario === "task-cancelled" ? "cancelled" : "running";
    await page.goto(`${base}/tasks/${TASK_ID}`);
    await page.getByTestId("browser-runs").waitFor();
    assert.equal(await page.getByTestId("browser-runs").locator("a").count(), 0, scenario);
  }
  fs.writeFileSync(
    path.join(out, "browser-ui-check.json"),
    JSON.stringify(
      {
        ok: true,
        checks: [
          "desktop/mobile live view",
          "task and run pages",
          "independent from timeline filter",
          "no horizontal overflow",
          "completed/unsafe/cancelled/unconfigured link suppression",
        ],
      },
      null,
      2,
    ),
  );
  process.stdout.write("Browser UI smoke passed (desktop/mobile, task/run, lifecycle and URL guards).\n");
} finally {
  await browser?.close();
  server.kill("SIGTERM");
  await mock.close();
}
