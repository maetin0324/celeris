import { defineConfig, devices } from "@playwright/test";

// 結合テスト。browser は host の ~/.cache/ms-playwright にある chromium を使い、download しない。
// 本番（:7700 / :7710）と staging（:7701 / :7711 / :7712）には接続しない。起動する server は
// WEB_E2E_PORT（既定 7720、docs/web/implementation-plan.md §3 H2）で待ち受ける。
const PORT = Number(process.env.WEB_E2E_PORT ?? "7720");

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  timeout: 60_000,
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    // pnpm を挟むと終了時に preview が止まらず test が終わらないので、vite を直接起動する。
    command: `node_modules/.bin/vite build && exec node_modules/.bin/vite preview --host 127.0.0.1 --strictPort --port ${PORT}`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
