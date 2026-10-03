// @ts-nocheck — ブラウザ内評価とモックを含む検査スクリプト（型検査の対象外。check-resume-recovery.mjs と同じ）
// gui/scripts/check-run-log.mjs — run ログ画面（`/tasks/:id/runs/:runId`）を、本番で使っている harness の
// 実ログから作った fixture（`test/fixtures/run-log/*.jsonl`）で開き、幅 360 / 390 / 412 CSS px とデスクトップ幅で
// スクリーンショットを撮って表示を確かめる。偽の celeris（fixture）だけを使い、実 celeris・認証・LLM・外部
// ネットワークには出ない。
// 使い方: pnpm build && node scripts/check-run-log.mjs [--out <dir>] [--no-checks]
//   --out        スクリーンショットの出力先（既定 gui/docs/gui/run-log。human-review と同じ置き場所）
//   --no-checks  画面の要素の検査をせずに撮るだけ（変更前の画面を撮るとき用）
// 失敗（期待の要素が無い・横スクロールが出る・実行中の run の追記が表示されない）は exit 1。
import { spawn } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { getFreePort, MOBILE_DEVICE, setupMockCeleris, TASK_ID, waitForHealth } from "./lib/celeris-fixture.mjs";

const GUI_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(path.join(GUI_DIR, "package.json"));
const { chromium } = require("@playwright/test");

const args = process.argv.slice(2);
const outIdx = args.indexOf("--out");
const OUT_DIR = outIdx >= 0 ? path.resolve(args[outIdx + 1]) : path.join(GUI_DIR, "docs", "gui", "run-log");
const CHECKS = !args.includes("--no-checks");
fs.mkdirSync(OUT_DIR, { recursive: true });

const FIXTURES = path.join(GUI_DIR, "test/fixtures/run-log");
const fixture = (name) => fs.readFileSync(path.join(FIXTURES, name), "utf-8");

const RUNS = [
  {
    run_id: "01RUNLOGCLAUDE00000000001",
    adapter: "claude-code",
    model: "claude-opus-5-5",
    file: "claude-code-real.jsonl",
  },
  { run_id: "01RUNLOGCODEX000000000001", adapter: "codex", model: "gpt-5.5", file: "codex-real.jsonl" },
  { run_id: "01RUNLOGACP00000000000001", adapter: "opencode", model: "qwen3.8-27b", file: "opencode-acp-real.jsonl" },
];
const LIVE_RUN = "01RUNLOGLIVE0000000000001";
// 実行中の run: 最初は 2 行、以後 stdout を取りに来るたびに 1 行ずつ増える（celeris の `?offset=` 追尾の再現）。
const liveLines = fixture("claude-code-real.jsonl").split("\n").filter(Boolean);
let liveCount = 2;
const liveContent = () => `${liveLines.slice(0, liveCount).join("\n")}\n`;

const send = (res, body, type = "application/json; charset=utf-8") => {
  res.writeHead(200, { "content-type": type, "cache-control": "no-store" });
  res.end(typeof body === "string" ? body : JSON.stringify(body));
};
const runSummary = (r, finished = true) => ({
  run_id: r.run_id,
  role: "worker",
  adapter: r.adapter,
  model: r.model,
  provider: r.adapter,
  started_at: "2026-09-28T00:00:00Z",
  finished_at: finished ? "2026-09-28T00:14:03Z" : null,
  outcome: finished ? "done" : null,
  work_unit: r.adapter === "codex" ? "review" : null,
  progress: 0,
  artifacts: 0,
  verdicts: 0,
  reviewer_deferrals: 0,
  files: { stdout: true, stderr: false, result: false },
});

const mock = await setupMockCeleris();
mock.on("GET", `/api/v1/tasks/${TASK_ID}/runs`, (_req, res) =>
  send(res, {
    runs: [
      ...RUNS.map((r) => runSummary(r)),
      runSummary({ run_id: LIVE_RUN, adapter: "claude-code", model: "claude-sonnet-5" }, false),
    ],
  }),
);
for (const r of RUNS) {
  mock.on("GET", `/api/v1/tasks/${TASK_ID}/runs/${r.run_id}/stdout`, (_req, res) =>
    send(res, fixture(r.file), "application/x-ndjson"),
  );
}
mock.on("GET", `/api/v1/tasks/${TASK_ID}/runs/${LIVE_RUN}/stdout`, (req, res) => {
  const offset = Number(new URL(req.url ?? "/", "http://x").searchParams.get("offset") ?? "0");
  const bytes = Buffer.from(liveContent(), "utf-8");
  if (offset > 0 && liveCount < liveLines.length) liveCount += 1;
  send(res, bytes.subarray(Math.min(offset, bytes.length)).toString("utf-8"), "application/x-ndjson");
});

const guiPort = await getFreePort();
const guiBind = `127.0.0.1:${guiPort}`;
const gui = spawn(process.execPath, ["server.js"], {
  cwd: GUI_DIR,
  env: {
    ...process.env,
    NODE_ENV: "production",
    CELERIS_API_URL: mock.baseUrl,
    CELERIS_GUI_BIND: guiBind,
  },
  stdio: ["ignore", "inherit", "inherit"],
});

const DEVICES = [
  ["desktop", { viewport: { width: 1440, height: 900 } }],
  ["w360", { ...MOBILE_DEVICE, viewport: { width: 360, height: 780 } }],
  ["w390", { ...MOBILE_DEVICE, viewport: { width: 390, height: 844 } }],
  ["w412", { ...MOBILE_DEVICE, viewport: { width: 412, height: 915 } }],
];

let failed = false;
const results = [];
let browser;
try {
  await waitForHealth(`http://${guiBind}/healthz`);
  browser = await chromium.launch({ headless: true });
  for (const [device, opts] of DEVICES) {
    const context = await browser.newContext(opts);
    const page = await context.newPage();
    for (const r of RUNS) {
      await page.goto(`http://${guiBind}/tasks/${TASK_ID}/runs/${r.run_id}`, { waitUntil: "load" });
      await page.waitForTimeout(400);
      const shot = path.join(OUT_DIR, `${r.adapter}-${device}.png`);
      await page.screenshot({ path: shot, fullPage: true });
      if (device !== "desktop") {
        // 全体図は縮小されて読めないので、1 画面ぶん（実寸）も撮る: 先頭と、最初のツール / コマンドの位置。
        await page.screenshot({ path: path.join(OUT_DIR, `${r.adapter}-${device}-view-top.png`) });
        const first = page.locator('[data-event-kind="tool"], [data-event-kind="command"]').first();
        if ((await first.count()) > 0) {
          await first.scrollIntoViewIfNeeded();
          await page.screenshot({ path: path.join(OUT_DIR, `${r.adapter}-${device}-view-tool.png`) });
        }
      }
      const result = { device, adapter: r.adapter, shot: path.basename(shot) };
      if (CHECKS) {
        const kinds = await page
          .locator('[data-testid="run-log-event"]')
          .evaluateAll((els) => [...new Set(els.map((e) => e.getAttribute("data-event-kind")))]);
        // 既定表示に生の JSON 行（旧 `stdout-line` の raw）が出ていないこと。
        const rawVisible = await page.locator('[data-testid="event-raw-json"]:visible').count();
        const checks = {
          hasEvents: kinds.length > 0,
          noRawJsonByDefault: rawVisible === 0,
          noPageHScroll: await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1),
        };
        // 1 件目のイベントの「JSON」を開くと元の行が出る。
        await page.locator('[data-testid="event-json-toggle"]').first().click();
        checks.eventJsonOpens = (await page.locator('[data-testid="event-raw-json"]:visible').count()) === 1;
        if (device === "desktop" || device === "w360") {
          await page.screenshot({ path: path.join(OUT_DIR, `${r.adapter}-${device}-json-open.png`), fullPage: false });
        }
        // run 全体の元の JSON に切り替えられる。
        await page.locator('[data-testid="stdout-toggle-raw"]').click();
        checks.wholeRawShows = (await page.locator('[data-testid="run-log-raw"]').count()) === 1;
        result.kinds = kinds;
        result.checks = checks;
        if (Object.values(checks).some((v) => !v)) failed = true;
      }
      results.push(result);
    }
    if (device === "desktop" || device === "w390") {
      // 実行中の run は既存の `?offset=` 追尾（1 秒ごと）で追記表示される。
      await page.goto(`http://${guiBind}/tasks/${TASK_ID}/runs/${LIVE_RUN}`, { waitUntil: "load" });
      const count = () => page.locator('[data-testid="run-log-event"], [data-testid="stdout-line"]').count();
      const before = await count();
      await page.waitForTimeout(3500);
      const after = await count();
      await page.screenshot({ path: path.join(OUT_DIR, `live-${device}.png`), fullPage: true });
      const result = { device, adapter: "live", before, after };
      if (CHECKS) {
        result.checks = { appended: after > before };
        if (!result.checks.appended) failed = true;
      }
      results.push(result);
    }
    await context.close();
  }
} finally {
  await browser?.close();
  gui.kill("SIGTERM");
  await mock.close();
}
process.stdout.write(`${JSON.stringify({ ok: !failed, out: OUT_DIR, results })}\n`);
process.exit(failed ? 1 : 0);
