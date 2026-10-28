import { spawn, spawnSync } from "node:child_process";
import { isIP } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import express from "express";
import { WebSocketServer } from "ws";
import { guardUpgradeSocket, rejectUpgrade } from "./browser-live.js";
import { createOriginPolicy } from "./origin.js";
import { parseUpstream, readTokenFile } from "./relay.js";

// web 端末（ADR 2026-10-27-web-terminal）。
// - 開けるのは login 済みで step-up（パスワードの再入力）が有効な session だけ（D3）。
// - 入口は loopback の Host と CELERIS_WEB_TERMINAL_HOSTS だけ。LAN の 7721（平文）の Host は既定で拒む（D3）。
// - WebSocket の upgrade で Host・Origin・session・step-up・同時数を検査する。外れたら upgrade を拒否する。
// - 対象の解決と開始・終了の記録は daemon（`POST /api/v1/terminals`、主体 human:web）。gateway は argv を組まない。
// - PTY は helper（`celerisctl terminal-pty`）が持ち、`systemd-run --user --scope` の中で動く（D1）。
// - 切断・終了・idle・最大継続・logout で helper の stdin を閉じる。helper は session の全 process を止めて終わる（D1・D2）。
// - 入出力・cookie・パスワード・token を log に出さない。出すのは session id・対象・理由だけ（D4）。

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const TARGET = /^(local|cluster:[A-Za-z0-9._-]{1,64})$/;
const LOOPBACK_HOSTS = ["localhost", "127.0.0.1", "::1"];
const FRAME_DATA = 0;
const FRAME_RESIZE = 1;
const MAX_MESSAGE = 64 * 1024;
const SEND_BACKLOG = 1 << 20;
const ENV_KEEP = [
  "HOME",
  "USER",
  "LOGNAME",
  "PATH",
  "SHELL",
  "LANG",
  "LC_ALL",
  "LC_CTYPE",
  "XDG_RUNTIME_DIR",
  "DBUS_SESSION_BUS_ADDRESS",
  "SSH_AUTH_SOCK",
];

function positive(value, fallback, what) {
  if (value === undefined || value === "") return fallback;
  const n = Number(value);
  if (!Number.isInteger(n) || n <= 0) throw new Error(`${what} must be a positive integer`);
  return n;
}

function hostOf(authority) {
  if (typeof authority !== "string") return null;
  const match = /^(?:\[([0-9a-fA-F:]+)\]|([a-zA-Z0-9.-]+))(?::(\d{1,5}))?$/.exec(authority.trim());
  if (!match) return null;
  if (match[1] && isIP(match[1]) !== 6) return null;
  return (match[1] ?? match[2]).toLowerCase();
}

export function terminalConfig(env = process.env) {
  const enabled = (env.CELERIS_WEB_TERMINAL ?? "on").trim();
  if (!["on", "off"].includes(enabled)) throw new Error("CELERIS_WEB_TERMINAL must be on or off");
  const launcher = (env.CELERIS_WEB_TERMINAL_LAUNCHER ?? "auto").trim();
  if (!["auto", "systemd-run", "inline"].includes(launcher))
    throw new Error("CELERIS_WEB_TERMINAL_LAUNCHER must be auto, systemd-run or inline");
  const hosts = new Set(LOOPBACK_HOSTS);
  for (const value of (env.CELERIS_WEB_TERMINAL_HOSTS ?? "").split(",")) {
    if (!value.trim()) continue;
    const host = hostOf(value.trim());
    if (!host) throw new Error("CELERIS_WEB_TERMINAL_HOSTS has an invalid host");
    hosts.add(host);
  }
  return {
    enabled: enabled === "on",
    hosts,
    launcher,
    helper: env.CELERIS_WEB_TERMINAL_HELPER || path.resolve(webRoot, "../../bin/celerisctl"),
    maxSessions: positive(env.CELERIS_WEB_TERMINAL_MAX_SESSIONS, 4, "CELERIS_WEB_TERMINAL_MAX_SESSIONS"),
    idleMs: positive(env.CELERIS_WEB_TERMINAL_IDLE_SECS, 1800, "CELERIS_WEB_TERMINAL_IDLE_SECS") * 1000,
    maxMs: positive(env.CELERIS_WEB_TERMINAL_MAX_SECS, 28800, "CELERIS_WEB_TERMINAL_MAX_SECS") * 1000,
    stepUpMs: positive(env.CELERIS_WEB_TERMINAL_STEP_UP_SECS, 600, "CELERIS_WEB_TERMINAL_STEP_UP_SECS") * 1000,
    memoryMax: env.CELERIS_WEB_TERMINAL_MEMORY_MAX || "2G",
    cpuQuota: env.CELERIS_WEB_TERMINAL_CPU_QUOTA || "200%",
    tasksMax: positive(env.CELERIS_WEB_TERMINAL_TASKS_MAX, 512, "CELERIS_WEB_TERMINAL_TASKS_MAX"),
  };
}

function systemdRunAvailable() {
  if (!process.env.XDG_RUNTIME_DIR) return false;
  return spawnSync("systemd-run", ["--version"], { stdio: "ignore" }).status === 0;
}

function frame(kind, body) {
  const head = Buffer.alloc(5);
  head[0] = kind;
  head.writeUInt32BE(body.length, 1);
  return Buffer.concat([head, body]);
}

function size(value, fallback, max) {
  const n = Number(value);
  return Number.isInteger(n) && n >= 1 && n <= max ? n : fallback;
}

export function createTerminal({
  auth,
  origins = createOriginPolicy(),
  daemonUrl,
  daemonTokenFile,
  config = terminalConfig(),
  fetchImpl = fetch,
  now = Date.now,
  log = () => {},
  failedDelayMs = 1000,
}) {
  const daemon = daemonUrl ? parseUpstream(daemonUrl) : null;
  const token = daemon ? readTokenFile(daemonTokenFile) : null;
  const stepUps = new Map();
  const sessions = new Map();
  let reserved = 0;
  let launcher = null;
  const wss = new WebSocketServer({ noServer: true, maxPayload: MAX_MESSAGE, perMessageDeflate: false });

  function resolveLauncher() {
    if (launcher) return launcher;
    launcher = config.launcher === "auto" ? (systemdRunAvailable() ? "systemd-run" : "inline") : config.launcher;
    return launcher;
  }

  function entryAllowed(req) {
    const host = hostOf(req.headers.host);
    return Boolean(host && config.hosts.has(host));
  }

  function stepUpUntil(key) {
    const until = key ? stepUps.get(key) : undefined;
    if (until === undefined) return null;
    if (until <= now()) {
      stepUps.delete(key);
      return null;
    }
    return until;
  }

  // 開ける状態か。{ status, code } か { key }。
  function admission(req) {
    if (!config.enabled) return { status: 403, code: "terminal_disabled" };
    if (!auth.enabled) return { status: 403, code: "terminal_requires_password" };
    if (!daemon) return { status: 403, code: "terminal_unavailable" };
    if (!entryAllowed(req)) return { status: 403, code: "terminal_entry_not_allowed" };
    const key = auth.sessionKey(req);
    if (!key) return { status: 401, code: "unauthenticated" };
    return { key };
  }

  async function daemonPost(pathname, body) {
    const response = await fetchImpl(new URL(pathname, daemon), {
      method: "POST",
      headers: { "content-type": "application/json", authorization: `Bearer ${token}` },
      body: JSON.stringify(body),
      redirect: "manual",
      signal: AbortSignal.timeout(15000),
    });
    let json = null;
    try {
      json = await response.json();
    } catch {
      json = null;
    }
    return { status: response.status, json };
  }

  function helperArgv(id, opened, cols, rows) {
    const pty = [
      config.helper,
      "terminal-pty",
      "--cwd",
      opened.cwd,
      "--cols",
      String(cols),
      "--rows",
      String(rows),
      "--",
      ...opened.argv,
    ];
    if (resolveLauncher() !== "systemd-run") return pty;
    return [
      "systemd-run",
      "--user",
      "--scope",
      "--quiet",
      "--collect",
      `--unit=celeris-terminal-${id.toLowerCase()}`,
      "-p",
      `MemoryMax=${config.memoryMax}`,
      "-p",
      `CPUQuota=${config.cpuQuota}`,
      "-p",
      `TasksMax=${config.tasksMax}`,
      "--",
      ...pty,
    ];
  }

  function childEnv() {
    const env = {};
    for (const name of ENV_KEEP) if (process.env[name] !== undefined) env[name] = process.env[name];
    return env;
  }

  function start(ws, req, key, target, cols, rows) {
    const client = `${req.headers.host} ${req.socket.remoteAddress ?? "-"}`;
    const session = {
      id: null,
      key,
      target,
      ws,
      child: null,
      openedAt: now(),
      ending: null,
      idle: null,
      max: null,
      exitCode: null,
      backlog: 0,
    };
    let released = false;
    const release = () => {
      if (!released) {
        released = true;
        reserved -= 1;
      }
    };
    const fail = (code) => {
      release();
      if (ws.readyState === ws.OPEN) {
        ws.send(JSON.stringify({ type: "error", code }));
        ws.close(4000, code);
      }
    };
    ws.on("error", () => {});
    ws.once("close", () => {
      if (!session.child) release();
      end(session, "disconnect");
    });
    daemonPost("/api/v1/terminals", { target, client })
      .then(({ status, json }) => {
        if (status !== 201 || !json?.session_id || !Array.isArray(json.argv)) {
          log({ op: "terminal_open", target, status });
          return fail(json?.code ?? "daemon_error");
        }
        if (ws.readyState !== ws.OPEN) {
          release();
          return record(json.session_id, "disconnect", 0, null);
        }
        session.id = json.session_id;
        const argv = helperArgv(session.id, json, cols, rows);
        let child;
        try {
          child = spawn(argv[0], argv.slice(1), {
            cwd: json.cwd,
            env: childEnv(),
            stdio: ["pipe", "pipe", "ignore"],
          });
        } catch {
          release();
          record(session.id, "spawn_failed", 0, null);
          return fail("spawn_failed");
        }
        session.child = child;
        sessions.set(session.id, session);
        release();
        log({ op: "terminal_open", session: session.id, target });
        child.stdin.on("error", () => {});
        child.on("error", () => {
          session.exitCode = null;
          end(session, "spawn_failed");
        });
        child.stdout.on("data", (chunk) => {
          touch(session);
          if (ws.readyState !== ws.OPEN) return;
          session.backlog += chunk.length;
          if (session.backlog > SEND_BACKLOG) child.stdout.pause();
          ws.send(chunk, { binary: true }, () => {
            session.backlog -= chunk.length;
            if (session.backlog <= SEND_BACKLOG / 2 && child.stdout.isPaused()) child.stdout.resume();
          });
        });
        child.on("exit", (code, signal) => {
          session.exitCode = code ?? (signal ? 128 : null);
          session.exited = true;
          end(session, "exited");
          finish(session);
        });
        ws.on("message", (data, isBinary) => {
          touch(session);
          if (isBinary) {
            if (!child.stdin.destroyed) child.stdin.write(frame(FRAME_DATA, Buffer.from(data)));
            return;
          }
          let msg;
          try {
            msg = JSON.parse(String(data));
          } catch {
            return;
          }
          if (msg?.type === "resize") {
            const body = Buffer.alloc(4);
            body.writeUInt16BE(size(msg.cols, 80, 1000), 0);
            body.writeUInt16BE(size(msg.rows, 24, 1000), 2);
            if (!child.stdin.destroyed) child.stdin.write(frame(FRAME_RESIZE, body));
          } else if (msg?.type === "close") end(session, "user");
        });
        session.max = setTimeout(() => end(session, "max_duration"), config.maxMs);
        touch(session);
        ws.send(
          JSON.stringify({
            type: "ready",
            session_id: session.id,
            target,
            remote_dir: json.remote_dir ?? null,
            idle_secs: config.idleMs / 1000,
            max_secs: config.maxMs / 1000,
          }),
        );
      })
      .catch(() => fail("daemon_unreachable"));
  }

  function touch(session) {
    if (session.ending) return;
    clearTimeout(session.idle);
    session.idle = setTimeout(() => end(session, "idle_timeout"), config.idleMs);
  }

  function record(id, reason, durationMs, exitCode) {
    return daemonPost(`/api/v1/terminals/${encodeURIComponent(id)}/close`, {
      reason,
      duration_secs: Math.round(durationMs / 1000),
      exit_code: exitCode,
    })
      .then(({ status }) => {
        if (status !== 204) log({ op: "terminal_close_record", session: id, status });
      })
      .catch(() => log({ op: "terminal_close_record", session: id, status: "unreachable" }));
  }

  // 端末を終わらせる（最初の理由だけを記録する）。helper の stdin を閉じ、終わらなければ signal で止める。
  function end(session, reason) {
    if (session.ending) return;
    session.ending = reason;
    clearTimeout(session.idle);
    clearTimeout(session.max);
    if (!session.child) return;
    if (!session.exited) {
      session.child.stdin.end();
      session.term = setTimeout(() => session.child.kill("SIGTERM"), 5000);
      session.kill = setTimeout(() => session.child.kill("SIGKILL"), 10000);
    }
    if (session.ws.readyState === session.ws.OPEN && reason !== "exited")
      session.ws.send(JSON.stringify({ type: "closing", reason }));
    if (session.exited) finish(session);
  }

  function finish(session) {
    if (session.finished) return;
    session.finished = true;
    clearTimeout(session.term);
    clearTimeout(session.kill);
    sessions.delete(session.id);
    log({ op: "terminal_close", session: session.id, reason: session.ending });
    if (session.ws.readyState === session.ws.OPEN) {
      session.ws.send(JSON.stringify({ type: "exit", reason: session.ending, code: session.exitCode }));
      session.ws.close(1000, session.ending);
    }
    session.closed = record(session.id, session.ending, now() - session.openedAt, session.exitCode);
  }

  auth.onLogout((key) => {
    if (!key) return;
    stepUps.delete(key);
    for (const session of sessions.values()) if (session.key === key) end(session, "logout");
  });

  function register(app) {
    const json = express.json({ limit: "4kb" });
    app.get("/terminal/status", (req, res) => {
      const verdict = admission(req);
      if (verdict.status === 401) return res.status(401).json({ code: verdict.code });
      const key = verdict.key ?? null;
      res.json({
        enabled: !verdict.code,
        reason: verdict.code ?? null,
        step_up_expires_at: key && stepUpUntil(key) ? new Date(stepUpUntil(key)).toISOString() : null,
        sessions: [...sessions.values()]
          .filter((s) => s.key === key)
          .map((s) => ({ id: s.id, target: s.target, opened_at: new Date(s.openedAt).toISOString() })),
        limits: {
          max_sessions: config.maxSessions,
          idle_secs: config.idleMs / 1000,
          max_secs: config.maxMs / 1000,
          step_up_secs: config.stepUpMs / 1000,
        },
      });
    });
    app.post("/terminal/step-up", json, async (req, res) => {
      const verdict = admission(req);
      if (verdict.code) return res.status(verdict.status).json({ code: verdict.code });
      if (!auth.verifyPassword(req.body?.password)) {
        await new Promise((resolve) => setTimeout(resolve, failedDelayMs));
        return res.status(401).json({ code: "invalid_password" });
      }
      const until = now() + config.stepUpMs;
      stepUps.set(verdict.key, until);
      res.json({ step_up_expires_at: new Date(until).toISOString() });
    });
  }

  function reject(socket, status, code) {
    const text = status === 401 ? "Unauthorized" : status === 429 ? "Too Many Requests" : "Forbidden";
    rejectUpgrade(
      socket,
      `HTTP/1.1 ${status} ${text}\r\nConnection: close\r\nContent-Type: text/plain\r\nX-Celeris-Terminal-Error: ${code}\r\nContent-Length: ${code.length}\r\n\r\n${code}`,
    );
  }

  // `/terminal/ws?target=local|cluster:<id>&cols=&rows=` の upgrade。
  function upgrade(req, socket, head) {
    guardUpgradeSocket(socket);
    const url = new URL(req.url, "http://celeris-web.invalid");
    if (url.pathname !== "/terminal/ws") return reject(socket, 403, "not_found");
    const verdict = admission(req);
    if (verdict.code) return reject(socket, verdict.status, verdict.code);
    const self = origins.selfOrigin(req).toLowerCase();
    const origin = req.headers.origin;
    if (typeof origin !== "string" || origin.toLowerCase() !== self) return reject(socket, 403, "origin_mismatch");
    if (!stepUpUntil(verdict.key)) return reject(socket, 403, "step_up_required");
    const target = url.searchParams.get("target") ?? "";
    if (!TARGET.test(target)) return reject(socket, 403, "invalid_target");
    if (sessions.size + reserved >= config.maxSessions) return reject(socket, 429, "too_many_terminals");
    reserved += 1;
    const cols = size(url.searchParams.get("cols"), 80, 1000);
    const rows = size(url.searchParams.get("rows"), 24, 1000);
    wss.handleUpgrade(req, socket, head, (ws) => start(ws, req, verdict.key, target, cols, rows));
  }

  // gateway の終了（D2）: 全端末を閉じ、終了の記録を待つ（上限つき）。
  async function shutdown(limitMs = 3000) {
    const all = [...sessions.values()];
    for (const session of all) end(session, "gateway_shutdown");
    const records = Promise.all(
      all.map(
        (s) =>
          new Promise((resolve) => {
            if (s.finished) return resolve(s.closed);
            s.child.once("exit", () => setImmediate(() => resolve(s.closed)));
          }),
      ),
    );
    await Promise.race([records, new Promise((resolve) => setTimeout(resolve, limitMs).unref())]);
  }

  return { register, upgrade, shutdown, sessions, config };
}
