import { execFileSync } from "node:child_process";
import os from "node:os";
import path from "node:path";
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
// `pnpm e2e:all` は両方（WEB_E2E_SCOPE で選ぶ）。spec file を名指しすると、その file が属する project
// （nfr / nfr-latency / release）だけで走る（functional の全試験は巻き込まない）。WU の check は対象画面の
// functional spec を基本にし、nfr は visual-qa・最終の受け入れ・release gate の段で流す。
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

// spec file を名指ししたとき（位置指定の引数が NFR/LATENCY/RELEASE の file に当たるとき）、その file を
// 属する project（nfr / nfr-latency / release）で走らせる。名指し無し（grep だけの指定を含む）は
// env（`pnpm e2e` は functional、`pnpm e2e:nfr` は nfr、`pnpm e2e:all` は all）で決めたまま。
// config を読み直す worker は argv ではなく env（WEB_E2E_SCOPE）を見るので、main process が決めた
// scope を env に書き戻す（PORT と同じ機構）。名指しで project だけを選ぶときは dependencies を作らず、
// 依存 project（functional・nfr）の全試験が巻き込まれない。
const positional = process.argv.slice(2).filter((arg) => arg.length > 1 && !arg.startsWith("-") && !arg.includes("="));
const namedProject = (file: string): string | null =>
  RELEASE.includes(file) ? "release" : LATENCY.includes(file) ? "nfr-latency" : NFR.includes(file) ? "nfr" : null;
const named = positional
  .map(path.normalize)
  .map(namedProject)
  .filter((p): p is string => p !== null);
const namedSet = new Set(named);
let scope = process.env.WEB_E2E_SCOPE ?? "all";
if (namedSet.size === 1) {
  scope = [...namedSet][0] as "nfr" | "nfr-latency" | "release";
} else if (namedSet.size > 1) {
  if (namedSet.has("release") && namedSet.has("nfr-latency")) scope = "latency";
  else throw new Error(`cannot run mixed spec projects in one e2e: ${named.join(", ")}`);
}
process.env.WEB_E2E_SCOPE = scope;
if (!["functional", "nfr", "nfr-latency", "release", "latency", "all"].includes(scope))
  throw new Error(`WEB_E2E_SCOPE must be functional, nfr, nfr-latency, release, latency or all: ${scope}`);

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
const projects: Project[] = [];
let last: string[] = [];
if (scope === "functional") {
  projects.push({ name: "functional", testIgnore: [...NFR, ...LATENCY, ...RELEASE], use: chrome });
} else if (scope === "nfr") {
  projects.push({ name: "nfr", testMatch: NFR, use: chrome });
} else if (scope === "nfr-latency") {
  // 名指しで latency の spec を走らせる。dependencies を作らず、functional・nfr の全試験を巻き込まない。
  projects.push({ name: "nfr-latency", testMatch: LATENCY, workers: latencyWorkers, use: chrome });
} else if (scope === "release") {
  projects.push({ name: "release", testMatch: RELEASE, workers: 1, use: chrome });
} else if (scope === "latency") {
  // 名指しで latency と release の spec が混ざるとき。release は dist/ を build し直すので最後に。
  projects.push({ name: "nfr-latency", testMatch: LATENCY, workers: latencyWorkers, use: chrome });
  projects.push({ name: "release", testMatch: RELEASE, workers: 1, dependencies: ["nfr-latency"], use: chrome });
} else {
  projects.push({ name: "functional", testIgnore: [...NFR, ...LATENCY, ...RELEASE], use: chrome });
  projects.push({ name: "nfr", testMatch: NFR, use: chrome });
  last = ["functional", "nfr"];
  projects.push({ name: "nfr-latency", testMatch: LATENCY, workers: latencyWorkers, dependencies: last, use: chrome });
  last = ["nfr-latency"];
  projects.push({ name: "release", testMatch: RELEASE, workers: 1, dependencies: last, use: chrome });
}

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
