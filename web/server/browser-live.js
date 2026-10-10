import { createHash, createHmac, createPrivateKey, randomBytes, sign, timingSafeEqual } from "node:crypto";
import { readFileSync, statSync } from "node:fs";
import { request as httpRequest } from "node:http";
import { request as httpsRequest } from "node:https";
import { createServer as createSocketServer } from "node:net";
import path from "node:path";
import express from "express";
import { parseUpstream, readTokenFile } from "./relay.js";

const ID = /^[A-Za-z0-9_-]{1,64}$/;
const ENTRY = /^\/browser\/live\/([A-Za-z0-9_-]{1,64})\/([A-Za-z0-9_-]{1,64})(\/.*)?$/;
const STATIC = /^\/_next\/static\/[A-Za-z0-9._~/-]+$/;
const READ = /^\/api\/session\/(\d{1,5})\/(tabs|status)$/;
const STREAM = /^\/api\/session\/(\d{1,5})\/stream$/;
const CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'self'; base-uri 'none'; form-action 'none'";
const guardedSockets = new WeakSet();
// 信頼できる端末（ADR 2026-10-07-browser-trusted-devices D1〜D4）。値は `<device_id>.<secret>`。
// secret は web の外に出さない（daemon には hash だけ。log・応答にも出さない）。
export const DEVICE_COOKIE_NAME = "__celeris_web_device";
export const DEVICE_COOKIE_PATH = "/browser/owner-session";
const DEVICE_COOKIE = /^([0-9A-HJKMNP-TV-Z]{26})\.([A-Za-z0-9_-]{43})$/;
const DEVICE_ID = /^[0-9A-HJKMNP-TV-Z]{26}$/;
const OWNER_TTL_MS = 24 * 60 * 60 * 1000;

export function deviceSecretHash(secret) {
  return createHash("sha256").update(`celeris-device\0${secret}`).digest("hex");
}
function readDeviceCookie(req) {
  const header = req.headers.cookie;
  if (typeof header !== "string") return { present: false };
  for (const part of header.split(";")) {
    const index = part.indexOf("=");
    if (index < 0 || part.slice(0, index).trim() !== DEVICE_COOKIE_NAME) continue;
    const raw = part.slice(index + 1).trim();
    const match = raw.length <= 256 ? DEVICE_COOKIE.exec(raw) : null;
    return match ? { present: true, id: match[1], secret: match[2] } : { present: true };
  }
  return { present: false };
}
function safeDevice(device) {
  if (!device || typeof device !== "object") return null;
  const { id, name, method, created_at, last_used_at, expires_at, absolute_expires_at, revoked_at, revoked_reason } =
    device;
  return { id, name, method, created_at, last_used_at, expires_at, absolute_expires_at, revoked_at, revoked_reason };
}

export function parseLiveUpstream(raw) {
  if (!raw) return null;
  const match = /^(?:\[(::1)\]|(127\.0\.0\.1|localhost)):(\d{1,5})$/.exec(raw);
  if (!match || Number(match[3]) < 1 || Number(match[3]) > 65535)
    throw new Error("CELERIS_WEB_LIVE_VIEW_UPSTREAM must be a loopback host:port");
  return {
    host: match[1] ?? (match[2] === "localhost" ? "127.0.0.1" : match[2]),
    port: Number(match[3]),
    authority: match[1] ? `[::1]:${match[3]}` : `${match[2]}:${match[3]}`,
  };
}

// Upgrade sockets are detached from HTTP's error handling. Keep an error
// listener through teardown, including while a rejected response is in flight.
export function guardUpgradeSocket(socket) {
  if (guardedSockets.has(socket)) return;
  guardedSockets.add(socket);
  socket.on("error", () => {});
}

export function rejectUpgrade(socket, response) {
  guardUpgradeSocket(socket);
  const deadline = setTimeout(() => socket.destroy(), 1000);
  deadline.unref();
  socket.once("close", () => clearTimeout(deadline));
  socket.end(response);
}

function privateKey(file) {
  if (!file) return null;
  try {
    const st = statSync(file);
    const dir = statSync(path.dirname(file));
    if (st.mode & 0o077 || dir.mode & 0o077) return null;
    const key = createPrivateKey(readFileSync(file));
    return key.asymmetricKeyType === "ed25519" ? key : null;
  } catch {
    return null;
  }
}

function problem(res, status, code) {
  return res.status(status).json({ code });
}
function safeJson(value) {
  if (Array.isArray(value)) return value.map(safeJson);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, key === "live_view_url" ? null : safeJson(item)]),
  );
}
function isAuthWait(wait) {
  return wait.reason === "waiting_for_auth" || wait.credential != null || wait.operation?.action === "credential_use";
}
function safeRun(run) {
  return (
    run &&
    typeof run === "object" &&
    ID.test(run.run_id) &&
    (run.session_id == null || ID.test(run.session_id)) &&
    ID.test(run.task_id)
  );
}
export function liveAvailability(run, upstream) {
  if (run.frames_available === true && run.state === "RUNNING" && run.session_id)
    return { state: "link", href: `/browser/live/${run.task_id}/${run.run_id}` };
  if (run.live_reason === "launcher_protocol_no_live_frames")
    return { state: "disabled", reason: "launcher_protocol_no_live_frames" };
  if (!upstream) return { state: "disabled", reason: "relay_unavailable" };
  if (run.state !== "RUNNING") return { state: "disabled", reason: "not_running" };
  if (!run.session_id) return { state: "disabled", reason: "not_configured" };
  return { state: "link", href: `/browser/live/${run.task_id}/${run.run_id}` };
}
function pathFor(task, run, session, suffix) {
  return `/api/v1/tasks/${task}/browser/${suffix}/${run}/${session}`;
}

export function createBrowserLive({
  auth,
  daemonUrl,
  daemonTokenFile,
  liveUpstream,
  attestationKeyFile,
  ownerSocket,
  probe = false,
  ownerId = process.env.CELERIS_WEB_OWNER_ID?.trim() || "owner",
  fetchImpl = fetch,
  now = Date.now,
}) {
  const daemon = daemonUrl ? parseUpstream(daemonUrl) : null;
  const token = daemon ? readTokenFile(daemonTokenFile) : null;
  const upstream = parseLiveUpstream(liveUpstream);
  const key = attestationKeyFile ? privateKey(attestationKeyFile) : null;
  const challenges = new Map();
  let owner = null;
  const bindings = new Map();
  const guardInflight = new Map();
  const resumeInflight = new Map();
  const sockets = new Set();
  const csrfSecret = randomBytes(32);

  function ownerKey(req) {
    if (!auth.enabled) return { status: 403, code: "owner_unavailable" };
    const session = auth.sessionKey(req);
    if (!session) return { status: 401, code: "unauthenticated" };
    if (!owner || owner.session !== session || owner.expires <= now()) return { status: 403, code: "not_owner" };
    return { session };
  }
  function csrf(session) {
    return createHmac("sha256", csrfSecret).update(session).digest("hex");
  }
  function same(a, b) {
    const x = Buffer.from(typeof a === "string" ? a : "");
    const y = Buffer.from(b);
    return x.length === y.length && timingSafeEqual(x, y);
  }
  function mutation(req, session, candidate) {
    const origin = `${req.socket.encrypted ? "https" : "http"}://${req.headers.host}`;
    return req.headers.origin === origin && same(candidate, csrf(session));
  }
  function assertion(task, run, session, ownerSession) {
    if (!key) return null;
    const payload = JSON.stringify({
      task_id: task,
      run_id: run,
      browser_session_id: session,
      owner_session_id: ownerSession,
      owner_session: true,
      origin_ok: true,
      expires_at: Math.floor(now() / 1000) + 20,
    });
    return { payload, signature: sign(null, Buffer.from(payload), key).toString("hex") };
  }
  function waitAssertion(task, wait, session, decision) {
    if (!key) return null;
    const payload = JSON.stringify({
      actor_id: process.env.CELERIS_WEB_OWNER_ID?.trim() || "owner",
      owner_session_hash: session,
      task_id: task,
      wait_id: wait.wait_id,
      version: wait.version,
      decision,
      policy_hash: wait.policy_hash,
      nonce: randomBytes(16).toString("hex"),
      expires_at: Math.floor(now() / 1000) + 20,
    });
    return { payload, signature: sign(null, Buffer.from(payload), key).toString("hex") };
  }
  // DeviceClaims（crates/task-api/src/browser_trusted_devices.rs）。undefined の欄は JSON に出ない。
  function deviceAssertion(purpose, session, fields = {}) {
    if (!key) return null;
    const payload = JSON.stringify({
      purpose,
      owner_session_id: session,
      owner_session: purpose !== "device_resume",
      actor_id: ownerId,
      ...fields,
      expires_at: Math.floor(now() / 1000) + 20,
    });
    return { payload, signature: sign(null, Buffer.from(payload), key).toString("hex") };
  }
  function assertionHeaders(signed) {
    return {
      "x-celeris-assertion-payload": signed.payload,
      "x-celeris-assertion-signature": signed.signature,
    };
  }
  function deviceCookieOptions(req, expiresAtSecs) {
    return {
      httpOnly: true,
      sameSite: "strict",
      secure: Boolean(req.socket.encrypted),
      path: DEVICE_COOKIE_PATH,
      ...(expiresAtSecs === undefined ? {} : { maxAge: Math.max(0, expiresAtSecs * 1000 - now()) }),
    };
  }
  function clearDeviceCookie(req, res) {
    res.clearCookie(DEVICE_COOKIE_NAME, deviceCookieOptions(req));
  }
  function setDeviceCookie(req, res, device, secret) {
    res.cookie(DEVICE_COOKIE_NAME, `${device.id}.${secret}`, deviceCookieOptions(req, device.expires_at));
  }
  function sameOrigin(req) {
    return req.headers.origin === `${req.socket.encrypted ? "https" : "http"}://${req.headers.host}`;
  }
  function isOwner(session) {
    return Boolean(owner && owner.session === session && owner.expires > now());
  }
  // 失効した端末から作った owner はメモリからも即時に落とす（D4）。
  function dropDevice(deviceId) {
    if (deviceId && owner?.deviceId === deviceId) revoke();
  }
  // 同じ端末・同じ秘密の同時 resume は 1 回の daemon 呼び出しにまとめる（D3。旧秘密の再提示と誤判定しない）。
  function resumeDevice(cookie, session) {
    const id = `${cookie.id}\0${cookie.secret}`;
    const pending = resumeInflight.get(id);
    if (pending) return pending;
    const result = (async () => {
      const secret = randomBytes(32).toString("base64url");
      const fields = {
        device_id: cookie.id,
        presented_hash: deviceSecretHash(cookie.secret),
        next_hash: deviceSecretHash(secret),
      };
      const signed = deviceAssertion("device_resume", session, fields);
      if (!signed) return { status: 503, code: "attestation_unavailable" };
      const reply = await api("POST", "/api/v1/browser/trusted-devices/verify", { ...fields, assertion: signed });
      if (reply.error) return { status: reply.status, code: reply.code };
      if (!reply.device || reply.device.id !== cookie.id) return { status: 503, code: "celeris_unavailable" };
      return { device: reply.device, secret };
    })().finally(() => {
      if (resumeInflight.get(id) === result) resumeInflight.delete(id);
    });
    resumeInflight.set(id, result);
    return result;
  }
  async function api(method, route, body, extraHeaders = {}) {
    if (!daemon) throw new Error("daemon unavailable");
    const response = await fetchImpl(new URL(route, daemon), {
      method,
      headers: {
        ...(token ? { authorization: `Bearer ${token}` } : {}),
        ...(body ? { "content-type": "application/json" } : {}),
        ...extraHeaders,
      },
      body: body ? JSON.stringify(body) : undefined,
      redirect: "manual",
      signal: AbortSignal.timeout(10000),
    });
    if (response.status >= 300 && response.status < 400) throw new Error("daemon redirected");
    const value = await response.json().catch(() => ({}));
    if (!response.ok) {
      const validation =
        route === "/api/v1/org/browser-execution/browser-settings" && response.status === 422
          ? { errors: value.errors, detail: value.detail }
          : {};
      return { error: true, status: response.status, code: value.code ?? "daemon_rejected", ...validation };
    }
    return value;
  }
  async function runs(task) {
    const latest = new Map();
    let after = -1;
    for (let page = 0; page < 20; page++) {
      const result = await api(
        "GET",
        `/api/v1/tasks/${task}/events?types=browser_updated&after_seq=${after}&limit=5000`,
      );
      if (result.error) throw new Error("events unavailable");
      for (const row of result.items ?? []) {
        if (
          row.task_id === task &&
          row.seq > after &&
          row.event?.type === "browser_updated" &&
          safeRun(row.event.browser) &&
          row.event.browser.task_id === task
        )
          latest.set(row.event.browser.run_id, row.event.browser);
      }
      if (!result.has_more) return [...latest.values()];
      const next = result.items?.at(-1)?.seq;
      if (!Number.isInteger(next) || next <= after) throw new Error("incomplete browser history");
      after = next;
    }
    throw new Error("browser history limit");
  }
  function guard(task, run) {
    const id = `${task}\0${run}`;
    const pending = guardInflight.get(id);
    if (pending) return pending;
    const result = guardFresh(task, run).finally(() => {
      if (guardInflight.get(id) === result) guardInflight.delete(id);
    });
    guardInflight.set(id, result);
    return result;
  }
  async function guardFresh(task, run) {
    let detail, found, waits;
    try {
      [detail, found, waits] = await Promise.all([
        api("GET", `/api/v1/tasks/${task}`),
        runs(task),
        api("GET", `/api/v1/tasks/${task}/browser/waits`),
      ]);
    } catch {
      return { status: 404, code: "not_found" };
    }
    if (detail.error || waits.error || !Array.isArray(waits.items)) return { status: 404, code: "not_found" };
    const item = found.find((r) => r.run_id === run);
    if (!item) return { status: 404, code: "not_found" };
    if (
      item.state !== "RUNNING" ||
      detail.task?.status !== "running" ||
      !detail.runs?.some((r) => r.run_id === run && !r.finished_at && !r.end && !r.outcome)
    )
      return { status: 409, code: "not_running" };
    if ((waits.items ?? []).some((w) => w.run_id === run && isAuthWait(w)))
      return { status: 409, code: "auth_interval" };
    // Dashboard shares a namespace. Missing task/wait history must close observation.
    let cursor;
    for (let page = 0; page < 20; page++) {
      const query = new URLSearchParams({
        limit: "200",
        status: "draft,ready,running,blocked,reviewing",
        archived: "true",
        order: "created_desc",
      });
      if (cursor) query.set("cursor", cursor);
      const tasks = await api("GET", `/api/v1/tasks?${query}`);
      if (tasks.error || !Array.isArray(tasks.items)) return { status: 503, code: "live_view_guard_unavailable" };
      for (const t of tasks.items ?? []) {
        if (["done", "failed", "cancelled"].includes(t.status)) continue;
        const check = await api("GET", `/api/v1/tasks/${t.id}/browser/waits`);
        if (check.error || !Array.isArray(check.items)) return { status: 503, code: "live_view_guard_unavailable" };
        if ((check.items ?? []).some(isAuthWait)) return { status: 409, code: "auth_interval" };
      }
      if (!tasks.next_cursor) return { run: item };
      if (tasks.next_cursor === cursor) break;
      cursor = tasks.next_cursor;
    }
    return { status: 503, code: "live_view_guard_unavailable" };
  }
  async function checked(binding, session) {
    const check = await guard(binding.task, binding.run);
    if (!check.run) return check;
    const signed = assertion(binding.task, binding.run, binding.browserSession, session);
    if (!signed) return { status: 503, code: "attestation_unavailable" };
    if (Number(binding.expires) - Math.floor(now() / 1000) <= 15) {
      const renewed = await api("POST", `${pathFor(binding.task, binding.run, binding.browserSession, "live")}/grant`, {
        assertion: signed,
      });
      if (renewed.error || !renewed.grant_id)
        return { status: renewed.status ?? 503, code: renewed.code ?? "grant_expired" };
      binding.grant = renewed.grant_id;
      binding.expires = renewed.expires_at;
    }
    const reply = await api("POST", `${pathFor(binding.task, binding.run, binding.browserSession, "live")}/check`, {
      assertion: signed,
      grant_id: binding.grant,
    });
    if (reply.error || reply.connected !== true)
      return { status: reply.status ?? 403, code: reply.code ?? "grant_expired" };
    return { run: check.run };
  }
  function classify(raw) {
    const path = raw.split("?")[0];
    const match = ENTRY.exec(path);
    if (!match) return null;
    const rest = match[3];
    if (!rest || rest === "/") return { task: match[1], run: match[2], sub: "/", entry: true };
    return { task: match[1], run: match[2], sub: rest, entry: false };
  }
  function allowed(sub, stream = false) {
    if (sub.includes("..") || sub.includes("%") || sub.includes("\\")) return false;
    if (stream) {
      const m = STREAM.exec(sub);
      return !!m && Number(m[1]) > 0 && Number(m[1]) <= 65535;
    }
    if (STATIC.test(sub) && !sub.split("/").includes(".")) return true;
    if (sub === "/api/sessions" || sub === "/api/chat/status") return true;
    const m = READ.exec(sub);
    return !!m && Number(m[1]) > 0 && Number(m[1]) <= 65535;
  }
  async function upstreamGet(sub, query = "") {
    return new Promise((resolve, reject) => {
      const request = httpRequest(
        {
          host: upstream.host,
          port: upstream.port,
          path: `${sub}${query}`,
          method: "GET",
          timeout: 15000,
          headers: { Host: upstream.authority, Origin: `http://${upstream.authority}`, Accept: "*/*" },
        },
        (reply) => {
          const parts = [];
          let bytes = 0;
          reply.on("data", (chunk) => {
            bytes += chunk.length;
            if (bytes > 32 * 1024 * 1024) request.destroy();
            else parts.push(chunk);
          });
          reply.on("end", () =>
            resolve({ status: reply.statusCode, type: reply.headers["content-type"], body: Buffer.concat(parts) }),
          );
        },
      );
      request.on("timeout", () => request.destroy());
      request.on("error", reject);
      request.end();
    });
  }
  async function live(req, res) {
    const who = ownerKey(req);
    if (!who.session) return problem(res, who.status, who.code);
    let target = classify(req.originalUrl);
    const originalPath = req.originalUrl.split("?")[0];
    if (!target && (originalPath.startsWith("/_next/") || originalPath.startsWith("/api/"))) {
      const binding = bindings.get(who.session);
      if (!binding) return problem(res, 404, "live_view_not_bound");
      target = { task: binding.task, run: binding.run, sub: originalPath, entry: false };
    }
    if (!target || req.originalUrl.includes("%") || !["GET", "HEAD"].includes(req.method))
      return problem(res, 404, "not_found");
    if (req.query.url || req.query.live_view_url) return problem(res, 400, "invalid_query");
    if (!upstream) return problem(res, 503, "live_view_relay_unavailable");
    if (!target.entry && !allowed(target.sub)) return problem(res, 403, "live_view_action_denied");
    const state = await guard(target.task, target.run);
    if (!state.run) return problem(res, state.status, state.code);
    if (target.entry) {
      const signed = assertion(target.task, target.run, state.run.session_id, who.session);
      if (!signed) return problem(res, 503, "attestation_unavailable");
      const grant = await api("POST", `${pathFor(target.task, target.run, state.run.session_id, "live")}/grant`, {
        assertion: signed,
      });
      if (grant.error || !grant.grant_id) return problem(res, grant.status ?? 503, grant.code ?? "grant_denied");
      bindings.set(who.session, {
        task: target.task,
        run: target.run,
        browserSession: state.run.session_id,
        grant: grant.grant_id,
        expires: grant.expires_at,
      });
    } else {
      const binding = bindings.get(who.session);
      if (!binding || binding.task !== target.task || binding.run !== target.run)
        return problem(res, 404, "live_view_not_bound");
      const check = await checked(binding, who.session);
      if (!check.run) return problem(res, check.status, check.code);
    }
    const question = req.originalUrl.indexOf("?");
    const rawQuery = question >= 0 ? req.originalUrl.slice(question) : "";
    const query =
      target.entry &&
      /^\?port=\d{1,5}$/.test(rawQuery) &&
      Number(rawQuery.slice(6)) <= 65535 &&
      Number(rawQuery.slice(6)) > 0
        ? rawQuery
        : "";
    const response = await upstreamGet(target.sub, query);
    const binding = bindings.get(who.session);
    if (!binding || binding.task !== target.task || binding.run !== target.run || !ownerKey(req).session)
      return problem(res, 403, "not_owner");
    const rechecked = await checked(binding, who.session);
    if (!rechecked.run) return problem(res, rechecked.status, rechecked.code);
    if ((response.status >= 300 && response.status < 400) || response.status >= 500)
      return problem(res, 502, "live_view_upstream_error");
    if (target.entry && !/^text\/html\b/i.test(response.type ?? ""))
      return problem(res, 502, "live_view_upstream_error");
    res.set("Content-Security-Policy", CSP).set("X-Frame-Options", "SAMEORIGIN");
    res
      .status(response.status)
      .type(response.type ?? "application/octet-stream")
      .send(req.method === "HEAD" ? "" : response.body);
  }
  function revoke() {
    owner = null;
    challenges.clear();
    bindings.clear();
    for (const socket of sockets) wsClose(socket, 1001);
    sockets.clear();
  }
  auth.onLogout((session) => {
    for (const [key, item] of challenges) if (item.session === session) challenges.delete(key);
    if (session && owner?.session === session) revoke();
  });
  function approve(challenge) {
    const item = challenges.get(challenge);
    challenges.delete(challenge);
    if (!item || item.expires <= now()) return false;
    revoke();
    owner = { session: item.session, deviceId: null, expires: now() + OWNER_TTL_MS };
    return true;
  }
  function startSocket() {
    // probe の web は本番の owner socket を開かない（D7）。
    if (!ownerSocket || probe) return null;
    const dir = statSync(path.dirname(ownerSocket));
    if (dir.mode & 0o077 || dir.uid !== process.getuid())
      throw new Error("browser owner socket directory must be private");
    const server = createSocketServer((conn) => {
      let line = "";
      conn.on("data", (data) => {
        line += data.toString();
        if (line.length > 1024) return conn.destroy();
        if (!line.includes("\n")) return;
        let request;
        try {
          request = JSON.parse(line.slice(0, line.indexOf("\n")));
        } catch {
          request = null;
        }
        const ok =
          request?.op === "approve" &&
          /^[A-Fa-f0-9]{12}$/.test(request.challenge ?? "") &&
          approve(request.challenge.toUpperCase());
        conn.end(`${JSON.stringify({ ok, code: ok ? "approved" : "unknown_challenge" })}\n`);
      });
    });
    server.listen(ownerSocket, () => {
      import("node:fs").then(({ chmodSync }) => chmodSync(ownerSocket, 0o600));
    });
    return server;
  }
  function register(app) {
    // D3: site policy / grant edits require the same owner and CSRF proof as approvals.
    for (const [route, upstream, methods] of [
      ["/browser/site-policies", "/api/v1/browser/site-policies", ["PUT", "DELETE"]],
      ["/browser/settings", "/api/v1/org/browser-execution/browser-settings", ["PATCH"]],
    ]) {
      app.use(route, express.json({ limit: "32kb" }), async (req, res, next) => {
        if (route === "/browser/settings" && ["GET", "HEAD"].includes(req.method)) return next();
        try {
          const who = ownerKey(req);
          if (!who.session) return problem(res, who.status, who.code);
          const suffix = req.path === "/" ? "" : req.path;
          if (suffix && (route !== "/browser/site-policies" || !/^\/[A-Za-z0-9._-]{1,64}$/.test(suffix)))
            return problem(res, 404, "not_found");
          if (!methods.includes(req.method) || (route === "/browser/site-policies" && !suffix))
            return problem(res, 405, "method_not_allowed");
          if (!mutation(req, who.session, req.body?.csrf)) return problem(res, 403, "csrf_failed");
          const { csrf: _csrf, ...payload } = req.body ?? {};
          const result = await api(req.method, upstream + suffix, req.method === "DELETE" ? undefined : payload);
          if (result.error) {
            if (route === "/browser/settings" && result.status === 422)
              return res.status(422).json({ code: result.code, errors: result.errors, detail: result.detail });
            return problem(res, result.status, result.code);
          }
          return res.json(result);
        } catch {
          return problem(res, 503, "celeris_unavailable");
        }
      });
    }

    app.get("/browser/owner-session", (req, res) => {
      if (!auth.enabled) return problem(res, 403, "owner_unavailable");
      const session = auth.sessionKey(req);
      if (!session) return problem(res, 401, "unauthenticated");
      const device = readDeviceCookie(req);
      const owned = isOwner(session);
      res.json({
        available: true,
        isOwner: owned,
        csrfToken: owner?.session === session ? csrf(session) : null,
        trustedDevice: device.present,
        resumable: !owned && !probe && Boolean(device.id),
        deviceId: owned ? (owner.deviceId ?? null) : null,
      });
    });
    // 登録端末からの復帰（D3）: password login 済み session ＋ 端末 cookie ＋ daemon の検証・回転。challenge は要らない。
    app.post("/browser/owner-session/resume", async (req, res) => {
      try {
        if (!auth.enabled) return problem(res, 403, "owner_unavailable");
        const session = auth.sessionKey(req);
        if (!session) return problem(res, 401, "unauthenticated");
        if (!sameOrigin(req)) return problem(res, 403, "origin_mismatch");
        if (probe) return problem(res, 503, "probe_mode");
        const cookie = readDeviceCookie(req);
        if (!cookie.id) {
          if (cookie.present) clearDeviceCookie(req, res);
          return problem(res, 403, "device_rejected");
        }
        if (isOwner(session) && owner.deviceId === cookie.id)
          return res.json({ ok: true, isOwner: true, csrfToken: csrf(session), deviceId: cookie.id });
        const result = await resumeDevice(cookie, session);
        if (!result.device) {
          if (result.code === "device_rejected") {
            // 旧秘密の再提示なら daemon が端末を失効させている。その端末由来の owner も落とす。
            dropDevice(cookie.id);
            clearDeviceCookie(req, res);
            return problem(res, 403, "device_rejected");
          }
          // daemon が使えない等。cookie は残し、次の読み込みで試し直せるようにする。
          return problem(res, 503, result.code === "attestation_unavailable" ? result.code : "celeris_unavailable");
        }
        // 待つ間に logout・別の失効が起きていたら作らない。
        const current = auth.sessionKey(req);
        if (current !== session) return problem(res, 401, "unauthenticated");
        setDeviceCookie(req, res, result.device, result.secret);
        const loginExpires = auth.sessionExpiresAt?.(req) ?? now() + OWNER_TTL_MS;
        const expires = Math.min(now() + OWNER_TTL_MS, loginExpires, result.device.expires_at * 1000);
        if (!(owner?.session === session && owner.deviceId === result.device.id)) revoke();
        owner = { session, deviceId: result.device.id, expires };
        return res.json({ ok: true, isOwner: true, csrfToken: csrf(session), deviceId: result.device.id });
      } catch {
        return problem(res, 503, "celeris_unavailable");
      }
    });
    // 端末の登録（D3）・一覧・失効（D4）。登録と失効は owner の CSRF と Origin を要する。
    async function registerDevice(req, res) {
      const who = ownerKey(req);
      if (!who.session) return problem(res, who.status, who.code);
      if (!mutation(req, who.session, req.body?.csrf ?? req.headers["x-csrf-token"]))
        return problem(res, 403, "csrf_failed");
      if (probe) return problem(res, 503, "probe_mode");
      const name = typeof req.body?.name === "string" ? req.body.name.trim() : "";
      if (!name || [...name].length > 64 || [...name].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f))
        return problem(res, 422, "invalid_input");
      const secret = randomBytes(32).toString("base64url");
      const secretHash = deviceSecretHash(secret);
      const signed = deviceAssertion("device_register", who.session, { name, presented_hash: secretHash });
      if (!signed) return problem(res, 503, "attestation_unavailable");
      const reply = await api("POST", "/api/v1/browser/trusted-devices", {
        name,
        secret_hash: secretHash,
        assertion: signed,
      });
      if (reply.error) return problem(res, reply.status, reply.code);
      if (!reply.device || !DEVICE_ID.test(reply.device.id ?? "")) return problem(res, 503, "celeris_unavailable");
      setDeviceCookie(req, res, reply.device, secret);
      return res.status(201).json({ ok: true, device: safeDevice(reply.device) });
    }
    const deviceBody = [express.json({ limit: "4kb" }), express.urlencoded({ extended: false, limit: "4kb" })];
    app.post("/browser/owner-session/device", ...deviceBody, (req, res) => {
      registerDevice(req, res).catch(() => problem(res, 503, "celeris_unavailable"));
    });
    app.use("/browser/trusted-devices", ...deviceBody, async (req, res) => {
      try {
        const who = ownerKey(req);
        if (!who.session) return problem(res, who.status, who.code);
        const match = /^\/browser\/trusted-devices(?:\/([0-9A-HJKMNP-TV-Z]{26})(\/revoke)?)?$/.exec(
          req.originalUrl.split("?")[0],
        );
        if (!match) return problem(res, 404, "not_found");
        if (!match[1] && req.method === "POST") return await registerDevice(req, res);
        if (probe) return problem(res, 503, "probe_mode");
        if (!match[1] && req.method === "GET") {
          const signed = deviceAssertion("device_list", who.session);
          if (!signed) return problem(res, 503, "attestation_unavailable");
          const reply = await api("GET", "/api/v1/browser/trusted-devices", undefined, assertionHeaders(signed));
          if (reply.error) return problem(res, reply.status, reply.code);
          return res.json({
            devices: (reply.devices ?? []).map(safeDevice),
            limit: reply.limit,
            now: reply.now,
            currentDeviceId: owner?.deviceId ?? null,
          });
        }
        if (match[1] && ((req.method === "DELETE" && !match[2]) || (req.method === "POST" && match[2]))) {
          if (!mutation(req, who.session, req.body?.csrf ?? req.headers["x-csrf-token"]))
            return problem(res, 403, "csrf_failed");
          const signed = deviceAssertion("device_revoke", who.session, { device_id: match[1] });
          if (!signed) return problem(res, 503, "attestation_unavailable");
          const reply = await api(
            "DELETE",
            `/api/v1/browser/trusted-devices/${match[1]}`,
            undefined,
            assertionHeaders(signed),
          );
          if (reply.error) return problem(res, reply.status, reply.code);
          if (owner?.deviceId === match[1]) clearDeviceCookie(req, res);
          dropDevice(match[1]);
          return res.json({ ok: true, revoked: reply.revoked === true, device: safeDevice(reply.device) });
        }
        return problem(res, 405, "method_not_allowed");
      } catch {
        return problem(res, 503, "celeris_unavailable");
      }
    });
    app.post("/browser/owner-session", (req, res) => {
      if (!auth.enabled) return problem(res, 403, "owner_unavailable");
      const session = auth.sessionKey(req);
      if (!session) return problem(res, 401, "unauthenticated");
      if (req.headers.origin !== `${req.socket.encrypted ? "https" : "http"}://${req.headers.host}`)
        return problem(res, 403, "origin_mismatch");
      for (const [key, item] of challenges)
        if (item.expires <= now() || item.session === session) challenges.delete(key);
      const challenge = randomBytes(6).toString("hex").toUpperCase();
      challenges.set(challenge, { session, expires: now() + 300000 });
      res.json({ challenge });
    });
    app.get("/browser/runs", async (req, res) => {
      const who = ownerKey(req);
      if (!who.session) return problem(res, who.status, who.code);
      try {
        const task = req.query.task_id;
        if (task !== undefined && !ID.test(task)) return problem(res, 422, "invalid_input");
        let tasks;
        if (task) tasks = [task];
        else {
          const list = await api("GET", "/api/v1/tasks?limit=200&archived=true&order=created_desc");
          if (list.error) return problem(res, 503, "celeris_unavailable");
          tasks = [];
          for (const item of (list.items ?? []).slice(0, 200)) {
            if (tasks.length >= 20) break;
            if (!ID.test(item.id ?? "")) continue;
            const detail = await api("GET", `/api/v1/tasks/${item.id}`);
            if (detail.error) throw new Error("task detail unavailable");
            if (detail.task?.skills?.includes("browser-enabled")) tasks.push(item.id);
          }
        }
        const items = [];
        for (const id of tasks)
          for (const run of await runs(id)) {
            let enrichedRun = run;
            if (run.state === "RUNNING" && run.session_id) {
              const signed = assertion(run.task_id, run.run_id, run.session_id, who.session);
              if (signed) {
                const grant = await api("POST", `${pathFor(run.task_id, run.run_id, run.session_id, "live")}/grant`, {
                  assertion: signed,
                });
                if (!grant.error && grant.grant_id && grant.frames_available === true) {
                  bindings.set(who.session, {
                    task: run.task_id,
                    run: run.run_id,
                    browserSession: run.session_id,
                    grant: grant.grant_id,
                    expires: grant.expires_at,
                  });
                  enrichedRun = { ...run, frames_available: true };
                } else if (!grant.error && grant.live_reason) enrichedRun = { ...run, live_reason: grant.live_reason };
              }
            }
            const live = enrichedRun.live_reason
              ? { state: "disabled", reason: enrichedRun.live_reason }
              : liveAvailability(enrichedRun, upstream);
            items.push({
              ...safeJson(enrichedRun),
              live,
              ...(enrichedRun.live_reason ? { live_reason: enrichedRun.live_reason } : {}),
              ...(live.state === "link" ? { live_path: live.href } : {}),
            });
          }
        res.json({ items });
      } catch {
        problem(res, 503, "celeris_unavailable");
      }
    });
    app.use("/browser/live", (req, res) => {
      live(req, res).catch(() => problem(res, 503, "live_view_guard_unavailable"));
    });
    app.use("/_next", (req, res) => {
      live(req, res).catch(() => problem(res, 503, "live_view_guard_unavailable"));
    });
    app.use("/api", (req, res, next) => {
      const path = req.originalUrl.split("?")[0];
      if (
        path === "/api/sessions" ||
        path.startsWith("/api/session/") ||
        path === "/api/chat/status" ||
        ["/api/exec", "/api/kill", "/api/chat", "/api/models"].includes(path)
      )
        live(req, res).catch(() => problem(res, 503, "live_view_guard_unavailable"));
      else next();
    });
    app.use(
      "/browser/control",
      express.json({ limit: "4kb" }),
      express.urlencoded({ extended: false, limit: "4kb" }),
      async (req, res) => {
        try {
          const who = ownerKey(req);
          if (!who.session) return problem(res, who.status, who.code);
          const m =
            /^\/browser\/control\/([A-Za-z0-9_-]{1,64})\/([A-Za-z0-9_-]{1,64})\/([A-Za-z0-9_-]{1,64})(\/release)?$/.exec(
              req.originalUrl,
            );
          if (!m) return problem(res, 404, "not_found");
          const state = await guard(m[1], m[2]);
          if (!state.run || state.run.session_id !== m[3]) return problem(res, 404, "not_found");
          const route = pathFor(m[1], m[2], m[3], "control");
          if (req.method === "GET" && !m[4]) {
            const result = await api("GET", route);
            return result.error
              ? problem(res, result.status, result.code)
              : res.json({ ok: true, status: safeJson(result) });
          }
          if (req.method !== "POST" || !mutation(req, who.session, req.body?.csrf))
            return problem(res, 403, "csrf_failed");
          const signed = assertion(m[1], m[2], m[3], who.session);
          if (!signed) return problem(res, 503, "attestation_unavailable");
          if (m[4]) {
            const reply = await api("POST", `${route}/disconnect`, { assertion: signed });
            return reply.error ? problem(res, reply.status, reply.code) : res.json({ ok: true });
          }
          if (
            !Number.isSafeInteger(req.body?.expected_version) ||
            req.body.expected_version < 0 ||
            !ID.test(req.body?.idempotency_key ?? "") ||
            !["pause", "takeover", "renew", "resume", "stop"].includes(req.body?.command?.kind)
          )
            return problem(res, 422, "invalid_input");
          const current = await api("GET", route);
          if (current.error || current.auth_section) return problem(res, 409, "auth_section_active");
          const reply = await api("POST", route, {
            assertion: signed,
            command: req.body.command,
            expected_version: req.body.expected_version,
            idempotency_key: req.body.idempotency_key,
          });
          if (reply.error) return problem(res, reply.status, reply.code);
          const updated = await api("GET", route);
          return updated.error
            ? problem(res, updated.status, updated.code)
            : res.json({ ok: true, status: safeJson(updated) });
        } catch {
          return problem(res, 503, "celeris_unavailable");
        }
      },
    );
    app.use(
      "/browser/waits",
      express.json({ limit: "8kb" }),
      express.urlencoded({ extended: false, limit: "8kb" }),
      async (req, res) => {
        try {
          const who = ownerKey(req);
          if (!who.session) return problem(res, who.status, who.code);
          const match = /^\/browser\/waits\/([A-Za-z0-9_-]{1,64})\/(decision|credential)$/.exec(req.originalUrl);
          if (!match) return problem(res, 404, "not_found");
          if (req.method !== "POST") return problem(res, 405, "method_not_allowed");
          if (!mutation(req, who.session, req.body?.csrf)) return problem(res, 403, "csrf_failed");
          const task = req.body?.task_id;
          if (!ID.test(task ?? "") || !Number.isSafeInteger(req.body?.expected_version))
            return problem(res, 422, "invalid_input");
          const list = await api("GET", `/api/v1/tasks/${task}/browser/waits`);
          if (list.error) return problem(res, 503, "celeris_unavailable");
          const wait = list.items?.find((w) => w.wait_id === match[1] && w.task_id === task);
          if (!wait) return problem(res, 404, "wait_not_found");
          if (
            wait.state !== "pending" ||
            wait.reason !== (match[2] === "decision" ? "waiting_for_approval" : "waiting_for_auth")
          )
            return problem(res, 409, "wait_not_actionable");
          if (Date.parse(wait.deadline) <= now()) return problem(res, 410, "wait_gone");
          if (wait.version !== req.body.expected_version) return problem(res, 409, "version_conflict");
          let payload;
          if (match[2] === "decision") {
            const decision = req.body?.decision;
            if (!["approve_once", "deny"].includes(decision)) return problem(res, 422, "invalid_input");
            const attestation = waitAssertion(task, wait, who.session, decision);
            if (!attestation) return problem(res, 503, "attestation_unavailable");
            const idempotency_key = createHash("sha256")
              .update(`${wait.wait_id}\0${wait.version}\0${decision}\0${who.session}`)
              .digest("hex")
              .slice(0, 32);
            payload = { decision, expected_version: wait.version, idempotency_key, attestation };
          } else {
            const username = req.body?.username;
            const password = req.body?.password;
            if (
              typeof username !== "string" ||
              typeof password !== "string" ||
              Buffer.byteLength(username) < 1 ||
              Buffer.byteLength(username) > 256 ||
              Buffer.byteLength(password) < 1 ||
              Buffer.byteLength(password) > 1024
            )
              return problem(res, 422, "invalid_input");
            const attestation = waitAssertion(task, wait, who.session, "register");
            if (!attestation) return problem(res, 503, "attestation_unavailable");
            payload = { expected_version: wait.version, username, password, attestation };
          }
          const result = await api("POST", `/api/v1/tasks/${task}/browser/waits/${match[1]}/${match[2]}`, payload);
          if (result.error) return problem(res, result.status, result.code);
          return res.json({
            ok: true,
            code: match[2] === "credential" ? "registered" : req.body.decision === "deny" ? "denied" : "approved",
            task_status: result.task_status,
          });
        } catch {
          return problem(res, 503, "celeris_unavailable");
        }
      },
    );
    app.use("/browser/identities", express.json({ limit: "256kb" }), async (req, res) => {
      try {
        const who = ownerKey(req);
        if (!who.session) return problem(res, who.status, who.code);
        const match = /^\/browser\/identities(?:\/([A-Za-z0-9_-]{1,64})(?:\/(revoke|restore))?)?$/.exec(
          req.originalUrl.split("?")[0],
        );
        if (!match) return problem(res, 404, "not_found");
        if (req.method === "GET" && !match[1]) {
          const project = req.query.project_id;
          if (!ID.test(project ?? "")) return problem(res, 422, "invalid_input");
          const result = await api("GET", `/api/v1/browser/identities?project_id=${project}`);
          return result.error ? problem(res, result.status, result.code) : res.json(safeJson(result));
        }
        if (!mutation(req, who.session, req.body?.csrf)) return problem(res, 403, "csrf_failed");
        if (req.method === "POST" && !match[1]) {
          const input = req.body;
          if (
            !ID.test(input?.identity_id ?? "") ||
            !ID.test(input?.project_id ?? "") ||
            !/^https:\/\/[^\s/?#]+$/.test(input?.origin ?? "") ||
            typeof input.demand_confirmed_by !== "string" ||
            !input.demand_confirmed_by.trim() ||
            !Array.isArray(input.state?.entries)
          )
            return problem(res, 422, "invalid_input");
          const result = await api("POST", "/api/v1/browser/identities", {
            identity_id: input.identity_id,
            project_id: input.project_id,
            origin: input.origin,
            demand_confirmed_by: input.demand_confirmed_by.trim(),
            ttl_secs: 604800,
            state: input.state,
          });
          return result.error
            ? problem(res, result.status, result.code)
            : res.status(201).json({ ok: true, identity: safeJson(result.identity) });
        }
        if (!match[1] || !ID.test(req.body?.project_id ?? "")) return problem(res, 422, "invalid_input");
        const list = await api("GET", `/api/v1/browser/identities?project_id=${req.body.project_id}`);
        if (
          list.error ||
          !list.identities?.some((item) => item.identity_id === match[1] && item.project_id === req.body.project_id)
        )
          return problem(res, 404, "identity_not_found");
        const route = `/api/v1/browser/identities/${match[1]}`;
        if (req.method === "DELETE" && !match[2]) {
          const result = await api("DELETE", route);
          return result.error ? problem(res, result.status, result.code) : res.json({ ok: true });
        }
        if (req.method === "POST" && match[2] === "revoke") {
          const result = await api("POST", `${route}/revoke`, {});
          return result.error
            ? problem(res, result.status, result.code)
            : res.json({ ok: true, identity: safeJson(result.identity) });
        }
        if (req.method === "POST" && match[2] === "restore") {
          if (!/^https:\/\/[^\s/?#]+$/.test(req.body?.origin ?? "")) return problem(res, 422, "invalid_input");
          const result = await api("POST", `${route}/restore`, {
            project_id: req.body.project_id,
            origin: req.body.origin,
            session_id: req.body.session_id ?? null,
          });
          return result.error ? problem(res, result.status, result.code) : res.json({ ok: true });
        }
        return problem(res, 405, "method_not_allowed");
      } catch {
        return problem(res, 503, "celeris_unavailable");
      }
    });
  }
  async function upgrade(req, client, head) {
    guardUpgradeSocket(client);
    const reject = (status, code) => {
      rejectUpgrade(
        client,
        `HTTP/1.1 ${status} Rejected\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n${JSON.stringify({ code })}`,
      );
    };
    const origin = `${req.socket.encrypted ? "https" : "http"}://${req.headers.host}`;
    if (req.headers.origin !== origin) return reject(403, "origin_mismatch");
    const who = ownerKey(req);
    if (!who.session) return reject(who.status, who.code);
    const rawPath = req.url?.split("?")[0] ?? "";
    const frameTarget = /^\/browser\/live\/([A-Za-z0-9_-]{1,64})\/([A-Za-z0-9_-]{1,64})\/frames$/.exec(rawPath);
    if (frameTarget) return upgradeFrames(req, client, head, who, frameTarget[1], frameTarget[2]);
    const target = classify(rawPath);
    const binding = bindings.get(who.session);
    if (!binding) return reject(404, "live_view_not_bound");
    const task = target?.task ?? binding.task;
    const run = target?.run ?? binding.run;
    const sub = target?.sub ?? rawPath;
    if (
      !upstream ||
      target?.entry ||
      !allowed(sub, true) ||
      task !== binding.task ||
      run !== binding.run ||
      !/^\?last_seen=\d{1,20}$/.test(req.url?.slice(rawPath.length) || "?last_seen=0")
    )
      return reject(404, "not_found");
    const check = await checked(binding, who.session);
    if (!check.run) return reject(check.status, check.code);
    const clientKey = req.headers["sec-websocket-key"];
    if (
      req.headers.upgrade?.toLowerCase() !== "websocket" ||
      typeof clientKey !== "string" ||
      !/^[A-Za-z0-9+/]{22}==$/.test(clientKey) ||
      Buffer.from(clientKey, "base64").length !== 16
    )
      return reject(400, "invalid_websocket");
    const upstreamKey = randomBytes(16).toString("base64");
    const upstreamRequest = httpRequest({
      host: upstream.host,
      port: upstream.port,
      path: `${sub}${req.url.slice(rawPath.length)}`,
      headers: {
        Host: upstream.authority,
        Origin: `http://${upstream.authority}`,
        Connection: "Upgrade",
        Upgrade: "websocket",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": upstreamKey,
      },
      timeout: 10000,
    });
    upstreamRequest.on("socket", guardUpgradeSocket);
    const upstreamSocket = await new Promise((resolve) => {
      upstreamRequest.once("upgrade", (response, socket, extra) => {
        guardUpgradeSocket(socket);
        const expected = createHash("sha1")
          .update(upstreamKey + WS_MAGIC)
          .digest("base64");
        if (response.headers["sec-websocket-accept"] !== expected) {
          socket.destroy();
          return resolve(null);
        }
        if (extra.length) socket.unshift(extra);
        resolve(socket);
      });
      upstreamRequest.once("error", () => resolve(null));
      upstreamRequest.once("response", (response) => {
        response.resume();
        resolve(null);
      });
      upstreamRequest.once("timeout", () => {
        upstreamRequest.destroy();
        resolve(null);
      });
      upstreamRequest.end();
    });
    if (!upstreamSocket) return reject(502, "live_view_upstream_error");
    const accept = createHash("sha1")
      .update(clientKey + WS_MAGIC)
      .digest("base64");
    client.write(
      `HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`,
    );
    sockets.add(client);
    let clientBuffer = Buffer.from(head);
    let upstreamBuffer = Buffer.alloc(0);
    let work = Promise.resolve();
    let closed = false;
    const close = (code = 1008) => {
      if (closed) return;
      closed = true;
      clearInterval(interval);
      sockets.delete(client);
      wsClose(client, code);
      upstreamSocket.destroy();
    };
    const interval = setInterval(() => {
      work = work
        .then(async () => {
          const current = ownerKey(req);
          if (!current.session) return close(1001);
          const result = await checked(binding, who.session);
          if (!result.run) close(1008);
        })
        .catch(() => close(1008));
    }, 5000);
    interval.unref();
    client.on("close", () => close(1001));
    upstreamSocket.on("close", () => close(1001));
    client.on("error", () => close(1001));
    upstreamSocket.on("error", () => close(1008));
    async function clientData(data) {
      clientBuffer = Buffer.concat([clientBuffer, data]);
      if (clientBuffer.length > 65536 + 14) return close(1008);
      for (;;) {
        const frame = wsDecode(clientBuffer, true, 65536);
        if (!frame) return;
        clientBuffer = clientBuffer.subarray(frame.consumed);
        if (frame.opcode === 8) return close(1001);
        if (frame.opcode === 9) {
          client.write(wsFrame(10, frame.payload));
          continue;
        }
        if (frame.opcode !== 1) {
          process.stderr.write(`${JSON.stringify({ browser_live: "input_denied", code: "unsupported_frame" })}\n`);
          client.write(wsFrame(1, JSON.stringify({ type: "input_denied", code: "unsupported_frame" })));
          continue;
        }
        const current = ownerKey(req);
        if (!current.session) return close(1001);
        const valid = await checked(binding, who.session);
        if (!valid.run) return close(1008);
        let message;
        try {
          message = JSON.parse(frame.payload.toString("utf8"));
        } catch {
          message = null;
        }
        if (!message || typeof message.type !== "string") continue;
        if (!["ack", "config", "input_mouse", "input_keyboard", "input_touch"].includes(message.type)) continue;
        if (message.type.startsWith("input_")) {
          const state = await api("GET", pathFor(binding.task, binding.run, binding.browserSession, "control"));
          if (
            state.error ||
            state.phase !== "human_control" ||
            state.lease_holder !== who.session ||
            state.auth_section ||
            Number(state.lease_expires_at) <= Math.floor(now() / 1000)
          ) {
            process.stderr.write(
              `${JSON.stringify({ browser_live: "input_denied", code: state.error ? "control_unavailable" : "lease_required" })}\n`,
            );
            client.write(
              wsFrame(
                1,
                JSON.stringify({ type: "input_denied", code: state.error ? "control_unavailable" : "lease_required" }),
              ),
            );
            continue;
          }
        }
        // Preserve payload bytes. The dashboard and browser action gate validate the command itself.
        const mask = randomBytes(4);
        const source = frame.payload;
        const header =
          source.length < 126 ? Buffer.alloc(2) : source.length <= 65535 ? Buffer.alloc(4) : Buffer.alloc(10);
        header[0] = 0x81;
        if (header.length === 2) header[1] = 0x80 | source.length;
        else if (header.length === 4) {
          header[1] = 0xfe;
          header.writeUInt16BE(source.length, 2);
        } else {
          header[1] = 0xff;
          header.writeBigUInt64BE(BigInt(source.length), 2);
        }
        const encoded = Buffer.from(source);
        for (let i = 0; i < encoded.length; i++) encoded[i] ^= mask[i % 4];
        upstreamSocket.write(Buffer.concat([header, mask, encoded]));
      }
    }
    async function upstreamData(data) {
      upstreamBuffer = Buffer.concat([upstreamBuffer, data]);
      if (upstreamBuffer.length > 32 * 1024 * 1024 + 14) return close(1008);
      for (;;) {
        const frame = wsDecode(upstreamBuffer, false, 32 * 1024 * 1024);
        if (!frame) return;
        upstreamBuffer = upstreamBuffer.subarray(frame.consumed);
        const current = ownerKey(req);
        if (!current.session) return close(1001);
        const valid = await checked(binding, who.session);
        if (!valid.run) return close(1008);
        if (frame.opcode === 8) return close(1001);
        if (frame.opcode === 1 || frame.opcode === 2) client.write(wsFrame(frame.opcode, frame.payload));
      }
    }
    client.on("data", (data) => {
      work = work.then(() => clientData(data)).catch(() => close(1008));
    });
    upstreamSocket.on("data", (data) => {
      work = work.then(() => upstreamData(data)).catch(() => close(1008));
    });
    if (head.length) {
      clientBuffer = Buffer.alloc(0);
      work = work.then(() => clientData(head)).catch(() => close(1008));
    }
  }
  async function upgradeFrames(req, client, head, who, task, run) {
    const reject = (status, code) =>
      rejectUpgrade(
        client,
        `HTTP/1.1 ${status} Rejected\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n${JSON.stringify({ code })}`,
      );
    if (head.length) return reject(400, "invalid_websocket");
    if (!daemon || !token) return reject(503, "live_frame_stream_unavailable");
    const origin = `${req.socket.encrypted ? "https" : "http"}://${req.headers.host}`;
    if (req.headers.origin !== origin) return reject(403, "origin_mismatch");
    let binding = bindings.get(who.session);
    if (!binding) {
      const state = await guard(task, run);
      if (!state.run?.session_id) return reject(state.status ?? 404, state.code ?? "not_found");
      const signed = assertion(task, run, state.run.session_id, who.session);
      if (!signed) return reject(503, "attestation_unavailable");
      const grant = await api("POST", `${pathFor(task, run, state.run.session_id, "live")}/grant`, {
        assertion: signed,
      });
      if (grant.error || !grant.grant_id) return reject(grant.status ?? 503, grant.code ?? "grant_denied");
      binding = { task, run, browserSession: state.run.session_id, grant: grant.grant_id, expires: grant.expires_at };
      bindings.set(who.session, binding);
    }
    if (binding.task !== task || binding.run !== run) return reject(404, "live_view_not_bound");
    const key = req.headers["sec-websocket-key"];
    if (
      req.headers.upgrade?.toLowerCase() !== "websocket" ||
      typeof key !== "string" ||
      !/^[A-Za-z0-9+/]{22}==$/.test(key) ||
      Buffer.from(key, "base64").length !== 16
    )
      return reject(400, "invalid_websocket");
    const check = await checked(binding, who.session);
    if (!check.run) return reject(check.status, check.code);
    const signed = assertion(task, run, binding.browserSession, who.session);
    if (!signed) return reject(503, "attestation_unavailable");
    const payload = JSON.stringify({ assertion: signed, grant_id: binding.grant });
    const daemonAddress = new URL(daemon);
    const frameReq = (daemonAddress.protocol === "https:" ? httpsRequest : httpRequest)({
      host: daemonAddress.hostname,
      port: daemonAddress.port || (daemonAddress.protocol === "https:" ? 443 : 80),
      path: `/api/v1/tasks/${task}/browser/live/${run}/${binding.browserSession}/frames`,
      method: "POST",
      headers: {
        Authorization: `Bearer ${token}`,
        "Content-Type": "application/json",
        "Content-Length": Buffer.byteLength(payload),
        Accept: "application/octet-stream",
      },
    });
    frameReq.on("socket", guardUpgradeSocket);
    const response = await new Promise((resolve) => {
      frameReq.setTimeout(10000, () => {
        frameReq.destroy();
        resolve(null);
      });
      frameReq.once("response", resolve);
      frameReq.once("error", () => resolve(null));
      frameReq.end(payload);
    });
    if (response?.statusCode !== 200 || response.headers["cache-control"] !== "no-store") {
      response?.resume();
      return reject(response ? 502 : 503, "live_frame_stream_unavailable");
    }
    const accept = createHash("sha1")
      .update(key + WS_MAGIC)
      .digest("base64");
    client.write(
      `HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: ${accept}\r\nCache-Control: no-store\r\n\r\n`,
    );
    sockets.add(client);
    let buf = Buffer.alloc(0),
      closed = false,
      frameQueue = null,
      sending = false;
    const close = (code = 1008) => {
      if (closed) return;
      closed = true;
      clearInterval(interval);
      sockets.delete(client);
      response.destroy();
      wsClose(client, code);
    };
    const interval = setInterval(() => {
      const current = ownerKey(req);
      if (!current.session) return close(1001);
      checked(binding, who.session)
        .then((result) => {
          if (!result.run) close(1008);
        })
        .catch(() => close(1008));
    }, 1000);
    interval.unref();
    // No viewer messages are accepted, including control, input, and application pings.
    client.on("data", () => close(1008));
    client.on("close", () => close(1001));
    client.on("error", () => close(1001));
    response.on("close", () => close(1001));
    response.on("error", () => close(1008));
    function sendLatest() {
      if (sending || !frameQueue || closed) return;
      sending = true;
      const frame = frameQueue;
      frameQueue = null;
      if (!client.write(wsFrame(2, frame)))
        client.once("drain", () => {
          sending = false;
          sendLatest();
        });
      else {
        sending = false;
        sendLatest();
      }
    }
    response.on("data", (chunk) => {
      buf = Buffer.concat([buf, chunk]);
      if (buf.length > 4 * 1024 * 1024 + 8) return close(1009);
      while (buf.length >= 4) {
        const size = buf.readUInt32BE(0);
        if (!size || size > 2 * 1024 * 1024) return close(1009);
        if (buf.length < size + 4) break;
        frameQueue = Buffer.from(buf.subarray(4, size + 4)); // capacity one: replace any unsent frame
        buf = buf.subarray(size + 4);
        sendLatest();
      }
    });
  }
  return { register, startSocket, approve, upgrade, ownerKey, checked, classify, allowed, upstream, sockets };
}

// Minimal RFC 6455 framing for the pinned dashboard stream. Client frames must be masked;
// fragmented and binary input is denied rather than reconstructed across authorization checks.
const WS_MAGIC = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
function wsFrame(opcode, payload = Buffer.alloc(0)) {
  const body = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
  const size = body.length < 126 ? 2 : body.length <= 65535 ? 4 : 10;
  const out = Buffer.alloc(size + body.length);
  out[0] = 0x80 | opcode;
  if (size === 2) out[1] = body.length;
  else if (size === 4) {
    out[1] = 126;
    out.writeUInt16BE(body.length, 2);
  } else {
    out[1] = 127;
    out.writeBigUInt64BE(BigInt(body.length), 2);
  }
  body.copy(out, size);
  return out;
}
function wsDecode(buffer, requireMask, max) {
  if (buffer.length < 2) return null;
  const fin = !!(buffer[0] & 0x80);
  const opcode = buffer[0] & 0x0f;
  const masked = !!(buffer[1] & 0x80);
  if (masked !== requireMask || !fin || buffer[0] & 0x70) throw new Error("invalid frame");
  let length = buffer[1] & 0x7f;
  let offset = 2;
  if (length === 126) {
    if (buffer.length < 4) return null;
    length = buffer.readUInt16BE(2);
    offset = 4;
  } else if (length === 127) {
    if (buffer.length < 10) return null;
    const n = buffer.readBigUInt64BE(2);
    if (n > BigInt(max)) throw new Error("frame too large");
    length = Number(n);
    offset = 10;
  }
  if (length > max) throw new Error("frame too large");
  if (buffer.length < offset + (masked ? 4 : 0) + length) return null;
  const mask = masked ? buffer.subarray(offset, offset + 4) : null;
  if (masked) offset += 4;
  const payload = Buffer.from(buffer.subarray(offset, offset + length));
  if (mask) for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
  return { opcode, payload, consumed: offset + length };
}
function wsClose(socket, code = 1008) {
  if (socket.destroyed) return;
  const payload = Buffer.alloc(2);
  payload.writeUInt16BE(code);
  socket.end(wsFrame(8, payload));
}
