import assert from "node:assert/strict";
import { chmodSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, test } from "node:test";
import { WebSocket } from "ws";
import { createApp } from "./app.js";
import { terminalConfig } from "./terminal.js";

// ADR 2026-10-27-web-terminal の gateway 側（D1〜D4）の決定的な試験。偽の helper（frame を解く node script）と
// 偽の daemon で、開く・入出力・リサイズ・終了の理由・helper の消滅・上限・認可・入口を確かめる。
// 待ちは出来事（daemon への終了の記録・WebSocket の close）で行う。外部ネットワークに出ない。

const dir = mkdtempSync(path.join(tmpdir(), "celeris-terminal-"));
const passwordFile = path.join(dir, "password");
const tokenFile = path.join(dir, "token");
writeFileSync(passwordFile, "pw\n", { mode: 0o600 });
writeFileSync(tokenFile, "test-daemon-token", { mode: 0o600 });
writeFileSync(path.join(dir, "index.html"), "<!doctype html><title>web</title>");
const helperScript = path.join(dir, "fake-helper.mjs");
writeFileSync(
  helperScript,
  `import { writeFileSync } from "node:fs";
const args = process.argv.slice(2);
const at = args.indexOf("--");
const opt = (name) => args[args.indexOf(name) + 1];
writeFileSync(args[at + 1], String(process.pid));
process.stdout.write("start:" + opt("--cols") + "x" + opt("--rows") + "\\n");
let buf = Buffer.alloc(0);
process.stdin.on("data", (chunk) => {
  buf = Buffer.concat([buf, chunk]);
  while (buf.length >= 5 && buf.length >= 5 + buf.readUInt32BE(1)) {
    const kind = buf[0];
    const body = buf.subarray(5, 5 + buf.readUInt32BE(1));
    buf = buf.subarray(5 + body.length);
    if (kind === 1) process.stdout.write("resize:" + body.readUInt16BE(0) + "x" + body.readUInt16BE(2) + "\\n");
    else if (body.toString() === "exit") process.exit(7);
    else process.stdout.write("echo:" + body.toString());
  }
});
process.stdin.on("end", () => process.exit(0));
`,
);
const helper = path.join(dir, "fake-helper");
writeFileSync(helper, `#!/bin/sh\nexec "${process.execPath}" "${helperScript}" "$@"\n`);
chmodSync(helper, 0o755);

const opens = [];
const closes = [];
const closeWaiters = [];
let pidSeq = 0;
const daemon = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => {
    body += chunk;
  });
  req.on("end", () => {
    assert.equal(req.headers.authorization, "Bearer test-daemon-token");
    const json = JSON.parse(body || "{}");
    res.setHeader("content-type", "application/json");
    if (req.method === "POST" && req.url === "/api/v1/terminals") {
      opens.push(json);
      if (json.target === "cluster:down") {
        res.statusCode = 409;
        return res.end(JSON.stringify({ code: "cluster_not_connected" }));
      }
      pidSeq += 1;
      res.statusCode = 201;
      return res.end(
        JSON.stringify({
          session_id: `S${pidSeq}`,
          target: json.target,
          argv: [path.join(dir, `pid-${pidSeq}`)],
          cwd: dir,
        }),
      );
    }
    const match = /^\/api\/v1\/terminals\/([^/]+)\/close$/.exec(req.url ?? "");
    if (req.method === "POST" && match) {
      closes.push({ id: match[1], ...json });
      res.statusCode = 204;
      res.end();
      for (const waiter of closeWaiters.splice(0)) waiter();
      return;
    }
    res.statusCode = 404;
    res.end("{}");
  });
});

let daemonUrl;
const servers = [];

before(async () => {
  await new Promise((resolve) => daemon.listen(0, "127.0.0.1", resolve));
  daemonUrl = `http://127.0.0.1:${daemon.address().port}`;
});
after(async () => {
  for (const server of servers) {
    server.closeAllConnections();
    server.close();
  }
  daemon.closeAllConnections();
  daemon.close();
});

async function startWeb(overrides = {}, { password = true, extraEnv = {} } = {}) {
  const config = {
    ...terminalConfig({
      CELERIS_WEB_TERMINAL_LAUNCHER: "inline",
      CELERIS_WEB_TERMINAL_HELPER: helper,
      ...extraEnv,
    }),
    ...overrides,
  };
  const app = createApp({
    bind: { host: "127.0.0.1", port: 0 },
    passwordFile: password ? passwordFile : undefined,
    allowedHosts: "192.168.1.103,celeris.example.ts.net",
    distDir: dir,
    daemonUrl,
    daemonTokenFile: tokenFile,
    failedLoginDelayMs: 0,
    log: () => {},
    terminal: { config },
  });
  const server = http.createServer(app);
  server.on("upgrade", app.locals.browserLiveUpgrade);
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  servers.push(server);
  return { port: server.address().port, terminal: app.locals.terminal };
}

function request(port, method, pathname, { headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      {
        host: "127.0.0.1",
        port,
        method,
        path: pathname,
        headers: { host: `127.0.0.1:${port}`, ...headers, ...(body ? { "content-type": "application/json" } : {}) },
      },
      (res) => {
        let text = "";
        res.on("data", (chunk) => {
          text += chunk;
        });
        res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, text }));
      },
    );
    req.on("error", reject);
    req.end(body ? JSON.stringify(body) : undefined);
  });
}

async function login(port, host = `127.0.0.1:${port}`) {
  const res = await request(port, "POST", "/login", {
    headers: { host, accept: "application/json" },
    body: { password: "pw" },
  });
  assert.equal(res.status, 200);
  return res.headers["set-cookie"][0].split(";")[0];
}

async function stepUp(port, cookie, host = `127.0.0.1:${port}`, password = "pw") {
  return request(port, "POST", "/terminal/step-up", { headers: { host, cookie }, body: { password } });
}

async function ready(port) {
  const cookie = await login(port);
  const res = await stepUp(port, cookie);
  assert.equal(res.status, 200, res.text);
  return cookie;
}

// WebSocket を開く。upgrade の拒否は { rejected: status, code }、開けば { ws, messages, text }。
function connect(port, cookie, { target = "local", host = `127.0.0.1:${port}`, origin = `http://${host}` } = {}) {
  return new Promise((resolve) => {
    const headers = { host };
    if (cookie) headers.cookie = cookie;
    if (origin !== null) headers.origin = origin;
    const ws = new WebSocket(`ws://127.0.0.1:${port}/terminal/ws?target=${target}&cols=90&rows=30`, { headers });
    const state = { ws, messages: [], text: "", waiters: [] };
    const wake = () => {
      for (const waiter of state.waiters.splice(0)) waiter();
    };
    ws.on("unexpected-response", (_req, res) =>
      resolve({ rejected: res.statusCode, code: res.headers["x-celeris-terminal-error"] }),
    );
    ws.on("error", () => {});
    ws.on("open", () => resolve(state));
    ws.on("message", (data, isBinary) => {
      if (isBinary) state.text += data.toString();
      else state.messages.push(JSON.parse(String(data)));
      wake();
    });
    ws.on("close", (code) => {
      state.closeCode = code;
      wake();
    });
  });
}

function until(state, predicate) {
  return new Promise((resolve) => {
    const check = () => (predicate() ? resolve() : state.waiters.push(check));
    check();
  });
}

function closedRecord(id) {
  return new Promise((resolve) => {
    const check = () => {
      const found = closes.find((c) => c.id === id);
      if (found) resolve(found);
      else closeWaiters.push(check);
    };
    check();
  });
}

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

async function opened(state) {
  await until(state, () => state.messages.some((m) => m.type === "ready") && state.text.includes("start:"));
  const id = state.messages.find((m) => m.type === "ready").session_id;
  const pid = Number(readFileSync(path.join(dir, `pid-${id.slice(1)}`), "utf8"));
  return { id, pid };
}

test("terminal: open, input, resize and explicit close end the helper", async () => {
  const { port } = await startWeb();
  const cookie = await ready(port);
  const state = await connect(port, cookie);
  assert.ok(state.ws, `rejected: ${state.code}`);
  const { id, pid } = await opened(state);
  await until(state, () => state.text.includes("start:90x30"));
  state.ws.send(Buffer.from("ls 日本語\n"), { binary: true });
  await until(state, () => state.text.includes("echo:ls 日本語"));
  state.ws.send(JSON.stringify({ type: "resize", cols: 120, rows: 40 }));
  await until(state, () => state.text.includes("resize:120x40"));
  state.ws.send(JSON.stringify({ type: "close" }));
  const record = await closedRecord(id);
  assert.equal(record.reason, "user");
  assert.equal(alive(pid), false);
  await until(state, () => state.closeCode !== undefined);
  assert.equal(state.closeCode, 1000);
  const open = opens.at(-1);
  assert.equal(open.target, "local");
  assert.match(open.client, /^127\.0\.0\.1:\d+ /);
});

test("terminal: disconnect, child exit, idle timeout, max duration and logout are recorded", async () => {
  const { port } = await startWeb({ idleMs: 60_000, maxMs: 60_000 });
  const cookie = await ready(port);

  let state = await connect(port, cookie);
  let { id, pid } = await opened(state);
  state.ws.terminate();
  assert.equal((await closedRecord(id)).reason, "disconnect");
  assert.equal(alive(pid), false);

  state = await connect(port, cookie);
  ({ id, pid } = await opened(state));
  state.ws.send(Buffer.from("exit"), { binary: true });
  const exited = await closedRecord(id);
  assert.equal(exited.reason, "exited");
  assert.equal(exited.exit_code, 7);
  await until(state, () => state.messages.some((m) => m.type === "exit"));

  state = await connect(port, cookie);
  ({ id, pid } = await opened(state));
  await request(port, "POST", "/logout", { headers: { cookie, accept: "application/json" } });
  assert.equal((await closedRecord(id)).reason, "logout");
  assert.equal(alive(pid), false);

  const short = await startWeb({ idleMs: 100, maxMs: 60_000 });
  const c2 = await ready(short.port);
  state = await connect(short.port, c2);
  ({ id, pid } = await opened(state));
  assert.equal((await closedRecord(id)).reason, "idle_timeout");
  assert.equal(alive(pid), false);

  const capped = await startWeb({ idleMs: 60_000, maxMs: 100 });
  const c3 = await ready(capped.port);
  state = await connect(capped.port, c3);
  ({ id, pid } = await opened(state));
  assert.equal((await closedRecord(id)).reason, "max_duration");
  assert.equal(alive(pid), false);
});

test("terminal: the session limit rejects extra terminals and gateway shutdown closes the rest", async () => {
  const { port, terminal } = await startWeb({ maxSessions: 1 });
  const cookie = await ready(port);
  const state = await connect(port, cookie);
  const { id, pid } = await opened(state);
  const extra = await connect(port, cookie);
  assert.deepEqual(extra, { rejected: 429, code: "too_many_terminals" });
  await terminal.shutdown();
  assert.equal((await closedRecord(id)).reason, "gateway_shutdown");
  assert.equal(alive(pid), false);
});

test("terminal: upgrade requires login, step-up, same origin and an allowed entry", async () => {
  const { port } = await startWeb({}, { extraEnv: { CELERIS_WEB_TERMINAL_HOSTS: "celeris.example.ts.net" } });
  const before = opens.length;
  // login していない。
  assert.deepEqual(await connect(port, null), { rejected: 401, code: "unauthenticated" });
  const cookie = await login(port);
  // step-up が無い。間違ったパスワードの step-up は 401。
  assert.deepEqual(await connect(port, cookie), { rejected: 403, code: "step_up_required" });
  assert.equal((await stepUp(port, cookie, undefined, "nope")).status, 401);
  assert.deepEqual(await connect(port, cookie), { rejected: 403, code: "step_up_required" });
  assert.equal((await stepUp(port, cookie)).status, 200);
  // 別 origin・Origin 無し。
  assert.deepEqual(await connect(port, cookie, { origin: "http://evil.example" }), {
    rejected: 403,
    code: "origin_mismatch",
  });
  assert.deepEqual(await connect(port, cookie, { origin: null }), { rejected: 403, code: "origin_mismatch" });
  // LAN の平文入口（Host 192.168.1.103:7721）は step-up も端末も拒む。
  const lan = "192.168.1.103:7721";
  const lanCookie = await login(port, lan);
  assert.equal((await stepUp(port, lanCookie, lan)).status, 403);
  assert.deepEqual(await connect(port, lanCookie, { host: lan }), {
    rejected: 403,
    code: "terminal_entry_not_allowed",
  });
  // 許可した tailscale の名前は通る。
  const ts = "celeris.example.ts.net";
  const tsCookie = await login(port, ts);
  assert.equal((await stepUp(port, tsCookie, ts)).status, 200);
  const state = await connect(port, tsCookie, { host: ts });
  assert.ok(state.ws, `rejected: ${state.code}`);
  const { id } = await opened(state);
  state.ws.close();
  await closedRecord(id);
  // 拒否した upgrade は daemon に届かない（許可した 1 件だけ）。
  assert.equal(opens.length, before + 1);
  // 形の違う target。
  assert.deepEqual(await connect(port, cookie, { target: "cluster:a%20b" }), { rejected: 403, code: "invalid_target" });
});

test("terminal: a disconnected cluster is not opened and the screen is told why", async () => {
  const { port } = await startWeb();
  const cookie = await ready(port);
  const state = await connect(port, cookie, { target: "cluster:down" });
  await until(state, () => state.closeCode !== undefined);
  assert.deepEqual(state.messages, [{ type: "error", code: "cluster_not_connected" }]);
  assert.equal(state.closeCode, 4000);
});

test("terminal: password-less config, off switch and the API relay never open terminals", async () => {
  const open = await startWeb({}, { password: false });
  const status = await request(open.port, "GET", "/terminal/status");
  assert.equal(JSON.parse(status.text).reason, "terminal_requires_password");
  assert.deepEqual(await connect(open.port, null), { rejected: 403, code: "terminal_requires_password" });

  const off = await startWeb({}, { extraEnv: { CELERIS_WEB_TERMINAL: "off" } });
  const cookie = await login(off.port);
  assert.deepEqual(await connect(off.port, cookie), { rejected: 403, code: "terminal_disabled" });

  // `/api/terminals` の中継は daemon へ送らない（web の token で許可を取らせない）。
  const { port } = await startWeb();
  const c = await login(port);
  const before = opens.length;
  const res = await request(port, "POST", "/api/terminals", { headers: { cookie: c }, body: { target: "local" } });
  assert.equal(res.status, 403);
  assert.equal(opens.length, before);
  const s = JSON.parse((await request(port, "GET", "/terminal/status", { headers: { cookie: c } })).text);
  assert.equal(s.enabled, true);
  assert.equal(s.step_up_expires_at, null);
  assert.deepEqual(s.limits, { max_sessions: 4, idle_secs: 1800, max_secs: 28800, step_up_secs: 600 });
});
