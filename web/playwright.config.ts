import { execFileSync } from "node:child_process";
import os from "node:os";
import { defineConfig, devices, type Project } from "@playwright/test";

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
// 通常は本番・staging に接続しない。P6-02 の WEB_E2E_REAL_BASE_URL 指定時だけ既存の
// staging gateway を使う。通常起動する server は WEB_E2E_PORT（既定 7720）で待ち受ける。
// 7720 が他の作業ツリーの e2e に使われているときは空き port に移る。決めた port は env に残し、
// config を読み直す worker も同じ port を使う。
const realBaseUrl = process.env.WEB_E2E_REAL_BASE_URL;
if (!realBaseUrl && !process.env.WEB_E2E_PORT) {
  process.env.WEB_E2E_PORT = execFileSync(process.execPath, ["-e", PICK_PORT], { encoding: "utf8" }).trim();
}
const PORT = Number(process.env.WEB_E2E_PORT);

// 機能（functional）と非機能（nfr）を project で分ける。`pnpm e2e` は functional、`pnpm e2e:nfr` は nfr、
// `pnpm e2e:all` は両方（WEB_E2E_SCOPE で選ぶ）。WU の check は対象画面の functional spec を基本にし、
// nfr は visual-qa・最終の受け入れ・release gate の段で流す。
// - nfr: 全画面の axe（a11y/axe・parity/mobile-gate）と、関係のない SSE の全画面掃引（realtime/refetch-scope）。
// - nfr-latency: 時間を測る gate（latency/transition の S1・parity/latency-gate）。他の project が終わってから
//   少ない worker で流す（他の試験の負荷で時間の閾値を揺らさない。retries 0 のまま）。
// - release: parity/cutover は scripts/release.sh が dist/ を build し直すので、dist/ を配る他の試験と重ねない。
//   最後に 1 worker で流す。
// 試験の状態（偽 daemon・gateway）は試験ごとに loopback の空き port（port 0）で起こすので、worker 間で共有しない。
// webServer の preview は dist/ を配るだけの状態を持たない server。
const NFR = ["a11y/axe.spec.ts", "parity/mobile-gate.spec.ts", "realtime/refetch-scope.spec.ts"];
const LATENCY = ["latency/transition.spec.ts", "parity/latency-gate.spec.ts"];
const RELEASE = ["parity/cutover.spec.ts"];
const scope = process.env.WEB_E2E_SCOPE ?? "all";
if (!["functional", "nfr", "all"].includes(scope))
  throw new Error(`WEB_E2E_SCOPE must be functional, nfr or all: ${scope}`);

// 既定の worker 数は CPU 数の半分（上限 8）。CI などは WEB_E2E_WORKERS で上書きする（数か "50%"）。
const workersEnv = process.env.WEB_E2E_WORKERS;
const workers = workersEnv
  ? /^\d+$/.test(workersEnv)
    ? Number(workersEnv)
    : workersEnv
  : Math.max(1, Math.min(8, Math.floor(os.availableParallelism() / 2)));

// nfr-latency は他の project が終わってから流すので、時間を測る試験どうしだけが並ぶ。既定 4（S1 の実測は
// 閾値 300 ms に対し 40〜90 ms）。WEB_E2E_LATENCY_WORKERS=1 で 1 本ずつにできる。
const latencyWorkers = Number(process.env.WEB_E2E_LATENCY_WORKERS ?? 4);

const chrome = { ...devices["Desktop Chrome"] };
const parallel: Project[] = [];
if (scope !== "nfr") parallel.push({ name: "functional", testIgnore: [...NFR, ...LATENCY, ...RELEASE], use: chrome });
if (scope !== "functional") parallel.push({ name: "nfr", testMatch: NFR, use: chrome });
const projects: Project[] = [...parallel];
let last = parallel.map((project) => project.name as string);
if (scope !== "functional") {
  projects.push({ name: "nfr-latency", testMatch: LATENCY, workers: latencyWorkers, dependencies: last, use: chrome });
  last = ["nfr-latency"];
}
if (scope !== "nfr")
  projects.push({ name: "release", testMatch: RELEASE, workers: 1, dependencies: last, use: chrome });

export default defineConfig({
  testDir: "./e2e",
  // e2e/support/*.test.ts は vitest の単体テストなので拾わない。
  testMatch: "**/*.spec.ts",
  fullyParallel: true,
  workers,
  retries: 0,
  reporter: [["list"]],
  timeout: 60_000,
  use: {
    baseURL: realBaseUrl ?? `http://127.0.0.1:${PORT}`,
    trace: "retain-on-failure",
  },
  projects,
  webServer: realBaseUrl
    ? undefined
    : {
        // dist/ が無いか古いときだけ build し、preview する（`pnpm build && pnpm e2e` で二重に build しない）。
        // pnpm を挟むと終了時に preview が止まらず test が終わらないので、vite を直接起動する。
        command: `node e2e/support/ensure-dist.mjs && exec node_modules/.bin/vite preview --host 127.0.0.1 --strictPort --port ${PORT}`,
        url: `http://127.0.0.1:${PORT}/`,
        reuseExistingServer: false,
        timeout: 120_000,
      },
});
