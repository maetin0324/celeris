import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createFakeDaemon } from "../e2e/support/fake-daemon.mjs";
import { createApp } from "../server/app.js";

// daemon の token が build の出力・HTML・`/api/*` の応答・エラー本文・要求ログに出ないことを確かめる（P1-07、X5）。
// 偽 daemon と gateway は loopback の空き port。先に `pnpm -C web build` で dist/ を作っておく。
export const FIXTURE_TOKEN = "celeris-web-fixture-token-6d1f0b9e4a";
const serverOnly = ["CELERIS_API_TOKEN_FILE", "CELERIS_WEB_PASSWORD_FILE", "CELERIS_WEB_SESSION_SECRET_FILE"];
const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const file = path.join(dir, name);
    return statSync(file).isDirectory() ? files(file) : [file];
  });
}

function get(base, route) {
  const { hostname, port } = new URL(base);
  return new Promise((resolve, reject) => {
    http
      .get({ hostname, port, path: route, headers: { Accept: "application/json" } }, (res) => {
        let body = "";
        res.on("data", (chunk) => {
          body += chunk;
        });
        res.on("end", () => resolve({ status: res.statusCode, headers: JSON.stringify(res.headers), body }));
      })
      .on("error", reject);
  });
}

async function listen(app) {
  const server = app.listen(0, "127.0.0.1");
  await new Promise((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  return { server, base: `http://127.0.0.1:${server.address().port}` };
}

export async function checkSecrets({ distDir = path.join(webRoot, "dist") } = {}) {
  const errors = [];
  if (!existsSync(path.join(distDir, "index.html"))) return [`${distDir}/index.html is missing; run the build first`];
  for (const file of files(distDir)) {
    const text = readFileSync(file, "latin1");
    if (text.includes(FIXTURE_TOKEN)) errors.push(`build output contains the token: ${file}`);
    for (const name of serverOnly) if (text.includes(name)) errors.push(`build output mentions ${name}: ${file}`);
  }

  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-secrets-"));
  const tokenFile = path.join(dir, "token");
  const wrongFile = path.join(dir, "wrong");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  writeFileSync(wrongFile, "wrong-token\n");
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const daemonUrl = await daemon.start();
  const closed = await listen(http.createServer());
  await new Promise((resolve) => closed.server.close(resolve));
  const logs = [];
  const log = (entry) => logs.push(JSON.stringify(entry));
  const gateways = [];
  try {
    const good = await listen(createApp({ distDir, daemonUrl, daemonTokenFile: tokenFile, log }));
    const wrong = await listen(createApp({ distDir, daemonUrl, daemonTokenFile: wrongFile, log }));
    const down = await listen(createApp({ distDir, daemonUrl: closed.base, daemonTokenFile: tokenFile, log }));
    gateways.push(good, wrong, down);
    const html = await get(good.base, "/");
    const assets = [...html.body.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g)].map((match) => match[1]);
    const cases = [
      [good.base, "/", 200],
      [good.base, "/tasks", 200],
      ...assets.map((asset) => [good.base, asset, 200]),
      [good.base, "/api/health", 200],
      [good.base, "/api/inbox", 200],
      [good.base, "/api/missing", 404],
      [good.base, "/api/%2e%2e/x", 400],
      [wrong.base, "/api/health", 502],
      [down.base, "/api/health", 502],
    ];
    for (const [base, route, status] of cases) {
      const response = await get(base, route);
      if (response.status !== status) errors.push(`${route}: expected ${status}, got ${response.status}`);
      if (response.body.includes(FIXTURE_TOKEN) || response.headers.includes(FIXTURE_TOKEN))
        errors.push(`${route}: response contains the token`);
    }
    if (!daemon.requests.some((request) => request.authorization === `Bearer ${FIXTURE_TOKEN}`))
      errors.push("the daemon never received the token; the check would be vacuous");
    if (logs.join("\n").includes(FIXTURE_TOKEN)) errors.push("request log contains the token");
  } finally {
    for (const { server } of gateways) await new Promise((resolve) => server.close(resolve));
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
  return errors;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const errors = await checkSecrets();
  if (errors.length) {
    console.error(errors.join("\n"));
    process.exitCode = 1;
  } else process.stdout.write("check:secrets: token absent from build output, HTML, /api responses, errors and logs\n");
}
