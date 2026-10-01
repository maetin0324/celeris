import { execFileSync } from "node:child_process";
import { defineConfig, devices } from "@playwright/test";

const PICK_PORT = `
const net = require("node:net");
const listen = (port) => new Promise((resolve, reject) => {
  const server = net.createServer();
  server.once("error", reject);
  server.listen(port, "127.0.0.1", () => {
    const picked = server.address().port;
    server.close(() => resolve(picked));
  });
});
listen(7720).catch(() => listen(0)).then((port) => process.stdout.write(String(port)));
`;

// 結合テスト。browser は host の ~/.cache/ms-playwright にある chromium を使い、download しない。
// 本番（:7700 / :7710）と staging（:7701 / :7711 / :7712）には接続しない。起動する server は
// WEB_E2E_PORT（既定 7720、docs/web/implementation-plan.md §3 H2）で待ち受ける。
// 7720 が他の作業ツリーの e2e に使われているときは空き port に移る。決めた port は env に残し、
// config を読み直す worker も同じ port を使う。
const realBaseUrl = process.env.WEB_E2E_REAL_BASE_URL;
if (!realBaseUrl && !process.env.WEB_E2E_PORT) {
  process.env.WEB_E2E_PORT = execFileSync(process.execPath, ["-e", PICK_PORT], { encoding: "utf8" }).trim();
}
const PORT = Number(process.env.WEB_E2E_PORT);

export default defineConfig({
  testDir: "./e2e",
  // e2e/support/*.test.ts は vitest の単体テストなので拾わない。
  testMatch: "**/*.spec.ts",
  fullyParallel: false,
  workers: 1,
  retries: 0,
  reporter: [["list"]],
  timeout: 60_000,
  use: {
    baseURL: realBaseUrl ?? `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: realBaseUrl ? undefined : {
    // pnpm を挟むと終了時に preview が止まらず test が終わらないので、vite を直接起動する。
    command: `node_modules/.bin/vite build && exec node_modules/.bin/vite preview --host 127.0.0.1 --strictPort --port ${PORT}`,
    url: `http://127.0.0.1:${PORT}/`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
