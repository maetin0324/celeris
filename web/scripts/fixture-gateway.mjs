import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { createFakeDaemon } from "../e2e/support/fake-daemon.mjs";
import { createApp } from "../server/app.js";
import { FIXTURE_TOKEN } from "./check-secrets.mjs";

// mobile-audit と screenshots の共通。偽 daemon と gateway を loopback の空き port で起こし、
// 画面を偽 daemon の fixture（T1 / R1 / cos / P1）で描く。:7700 / :7710 / staging には接続しない。
export async function startFixtureGateway() {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-audit-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, profile: "rich" });
  const daemonUrl = await daemon.start();
  const server = createApp({ log: () => {}, daemonUrl, daemonTokenFile: tokenFile }).listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  return {
    base: `http://127.0.0.1:${server.address().port}`,
    async close() {
      await new Promise((resolve) => {
        server.close(resolve);
        server.closeAllConnections(); // SSE の接続を待たない
      });
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    },
  };
}
