import http from "node:http";
import https from "node:https";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { dispositionFor } from "./files.js";
import { DEFAULT_RELAY_TIMEOUT_MS, fail, parseUpstream, readTokenFile, validSegment } from "./relay.js";

// CoS チャットの添付・stream・ダウンロードの中継（agent-docs/adr/2026-10-05-cos-chat-home.md D2・D4）。
// JSON relay（relay.js）より前に登録し、次の 3 種だけを受ける。他の `/api/chat/*` は next() で JSON relay に渡す。
// - POST `/api/chat/threads/{t}/attachments`: multipart の本文を buffer に貯めず daemon へ流す。上限（既定 100 MiB、
//   起動設定で変える）は Content-Length を信じず受信したバイト数を数え、超えたら upstream を abort して 413
//   `request_too_large`。宣言の Content-Length が上限を超えるなら daemon へ送らずに 413。timeout は本文を
//   送り終えてから応答ヘッダが届くまでだけ。
// - GET `/api/chat/threads/{t}/stream`: SSE を timeout 無しで中継する。`after`・`Last-Event-ID`（event id の 10 進）
//   だけを通し、両方の一致の検査は daemon が行う。ブラウザが切断したら upstream を abort する。200 以外の応答
//   （410 chat-cursor-expired の problem+json を含む）は status・Content-Type・本文をそのまま返す。
// - GET/HEAD `/api/chat/attachments/{a}/content|preview`: 本文を流し、Content-Type・Content-Disposition を保つ。
//   content は Disposition が無ければ attachment、HTML・SVG などは attachment に直す。nosniff と CSP sandbox を付ける。
// daemon の 401 / 403 は JSON relay と同じく 502 `daemon_auth`、redirect は 502 `daemon_redirect`。

export const DEFAULT_CHAT_UPLOAD_LIMIT_BYTES = 100 * 1024 * 1024;
const eventId = /^\d{1,20}$/;
const uploadRequestHeaders = ["accept", "content-type"];
const jsonResponseHeaders = ["content-type", "etag", "last-modified"];
const fileResponseHeaders = ["content-type", "content-disposition", "content-length", "etag", "last-modified"];
const fileCsp = "sandbox; default-src 'none'; frame-ancestors 'none'";

// 起動設定の上限（バイト数）。未指定・空は既定値、正の整数でなければ起動を止める。
export function parseUploadLimit(value) {
  if (value === undefined || value === null || value === "") return DEFAULT_CHAT_UPLOAD_LIMIT_BYTES;
  const text = String(value).trim();
  if (!/^\d{1,15}$/.test(text) || Number(text) < 1)
    throw new Error("CELERIS_WEB_CHAT_UPLOAD_LIMIT_BYTES must be a positive integer");
  return Number(text);
}

// `/api` より後ろの path を中継の種類と daemon の path にする。この module の対象でなければ null。
export function chatRoute(method, rest) {
  if (typeof rest !== "string" || !rest.startsWith("/")) return null;
  const parts = rest.slice(1).split("/");
  if (parts[0] !== "chat" || parts.length !== 4 || !parts.every(validSegment)) return null;
  const target = `/api/v1/${parts.join("/")}`;
  if (parts[1] === "threads" && parts[3] === "attachments" && method === "POST") return { kind: "upload", target };
  if (parts[1] === "threads" && parts[3] === "stream" && method === "GET") return { kind: "stream", target };
  if (parts[1] === "attachments" && ["content", "preview"].includes(parts[3]) && ["GET", "HEAD"].includes(method))
    return { kind: parts[3], target };
  return null;
}

// stream の query と `Last-Event-ID` を検査する。受けられなければ null。
export function chatStreamParams(search, lastEventId) {
  const out = new URLSearchParams();
  for (const [key, value] of new URLSearchParams(search)) {
    if (out.has(key) || key !== "after" || !eventId.test(value)) return null;
    out.set(key, value);
  }
  if (lastEventId !== undefined && (typeof lastEventId !== "string" || !eventId.test(lastEventId.trim()))) return null;
  return { query: out, lastEventId: lastEventId?.trim() };
}

function redact(text, token) {
  return token ? text.split(token).join("[redacted]") : text;
}

export function createChat({
  upstream,
  tokenFile,
  timeoutMs = DEFAULT_RELAY_TIMEOUT_MS,
  uploadLimitBytes = DEFAULT_CHAT_UPLOAD_LIMIT_BYTES,
  fetchImpl = fetch,
}) {
  const origin = parseUpstream(upstream);
  const token = readTokenFile(tokenFile);
  const limit = parseUploadLimit(uploadLimitBytes);

  function authHeaders() {
    const headers = new Headers();
    if (token) headers.set("authorization", `Bearer ${token}`);
    return headers;
  }

  // 応答の close で upstream を abort する。返り値の reason() は abort の原因（client / timeout / too_large）。
  function watch(res) {
    const controller = new AbortController();
    let reason = null;
    const abort = (why) => {
      reason ??= why;
      controller.abort();
    };
    const onClose = () => {
      if (!res.writableFinished) abort("client");
    };
    res.on("close", onClose);
    return { controller, abort, reason: () => reason, done: () => res.off("close", onClose) };
  }

  // 本文は node の http request で流す（fetch は stream の本文で 401 を受けると応答を返さず失敗する）。
  // ブラウザからの受信は upstream の書込みが詰まったら pause し、貯めるのは socket の buffer 分だけ。
  async function upload(req, res, url) {
    const declared = req.headers["content-length"];
    if (declared !== undefined && /^\d+$/.test(declared) && Number(declared) > limit) {
      res.set("Connection", "close");
      return fail(res, 413, "request_too_large");
    }
    const headers = {};
    for (const name of uploadRequestHeaders) {
      const value = req.headers[name];
      if (typeof value === "string") headers[name] = value;
    }
    if (token) headers.authorization = `Bearer ${token}`;
    const watcher = watch(res);
    let timer = null;
    let received = 0;
    try {
      const upstreamResponse = await new Promise((resolve, reject) => {
        const client = url.protocol === "https:" ? https : http;
        const outgoing = client.request(url, { method: "POST", headers, signal: watcher.controller.signal });
        outgoing.on("response", resolve);
        outgoing.on("error", reject);
        outgoing.on("drain", () => req.resume());
        req.on("data", (chunk) => {
          if (watcher.reason()) return;
          received += chunk.length;
          if (received > limit) {
            watcher.abort("too_large");
            req.pause();
            res.set("Connection", "close");
            return fail(res, 413, "request_too_large");
          }
          if (!outgoing.write(chunk)) req.pause();
        });
        req.on("end", () => {
          if (watcher.reason()) return;
          outgoing.end();
          timer = setTimeout(() => watcher.abort("timeout"), timeoutMs);
        });
        req.on("error", () => watcher.abort("client"));
      }).catch(() => null);
      if (!upstreamResponse) {
        const reason = watcher.reason();
        if (reason === "client" || reason === "too_large") return;
        return reason === "timeout" ? fail(res, 504, "daemon_timeout") : fail(res, 502, "daemon_unreachable");
      }
      const status = upstreamResponse.statusCode ?? 502;
      const chunks = [];
      try {
        for await (const chunk of upstreamResponse) chunks.push(chunk);
      } catch {
        const reason = watcher.reason();
        if (reason === "client" || reason === "too_large") return;
        return reason === "timeout" ? fail(res, 504, "daemon_timeout") : fail(res, 502, "daemon_unreachable");
      }
      if (watcher.reason() === "too_large") return;
      const responseHeaders = new Headers();
      for (const name of jsonResponseHeaders) {
        const value = upstreamResponse.headers[name];
        if (typeof value === "string") responseHeaders.set(name, value);
      }
      await sendJson(
        req,
        res,
        new Response([204, 205, 304].includes(status) ? null : Buffer.concat(chunks), {
          status,
          headers: responseHeaders,
        }),
        watcher,
      );
    } finally {
      clearTimeout(timer);
      watcher.done();
    }
  }

  // JSON 応答（problem+json を含む）を relay.js と同じ規則で返す。
  async function sendJson(req, res, upstreamResponse, watcher) {
    const status = upstreamResponse.status;
    if (status === 401 || status === 403 || (status >= 300 && status < 400)) {
      await upstreamResponse.body?.cancel().catch(() => {});
      return fail(res, 502, status < 400 ? "daemon_redirect" : "daemon_auth");
    }
    let text;
    try {
      text = req.method === "HEAD" ? "" : await upstreamResponse.text();
    } catch {
      const reason = watcher.reason();
      if (reason === "client" || reason === "too_large") return;
      return reason === "timeout" ? fail(res, 504, "daemon_timeout") : fail(res, 502, "daemon_unreachable");
    }
    if (res.headersSent) return;
    res.status(status);
    for (const name of jsonResponseHeaders) {
      const value = upstreamResponse.headers.get(name);
      if (value !== null) res.setHeader(name, redact(value, token));
    }
    if (!upstreamResponse.headers.has("content-type")) res.type("text/plain");
    // Buffer で送り、daemon の Content-Type（problem+json）に charset を足さない。
    return res.send(Buffer.from(redact(text, token)));
  }

  async function stream(req, res, url, search) {
    const params = chatStreamParams(search, req.headers["last-event-id"]);
    if (!params) return fail(res, 400, "invalid_query");
    if (params.query.size) url.search = `?${params.query}`;
    const headers = authHeaders();
    headers.set("accept", "text/event-stream");
    if (params.lastEventId) headers.set("last-event-id", params.lastEventId);
    const watcher = watch(res);
    try {
      let upstreamResponse;
      try {
        upstreamResponse = await fetchImpl(url, { headers, redirect: "manual", signal: watcher.controller.signal });
      } catch {
        if (watcher.reason()) return;
        return fail(res, 502, "daemon_unreachable");
      }
      if (upstreamResponse.status !== 200) return await sendJson(req, res, upstreamResponse, watcher);
      // Express の res.set は Content-Type に charset を足すので、node の setHeader で付ける。
      res.statusCode = 200;
      res.setHeader("Content-Type", "text/event-stream");
      res.setHeader("Cache-Control", "no-store");
      res.setHeader("X-Accel-Buffering", "no");
      req.socket.setNoDelay(true);
      req.socket.setTimeout(0);
      res.flushHeaders();
      if (!upstreamResponse.body) return res.end();
      try {
        await pipeline(Readable.fromWeb(upstreamResponse.body), res);
      } catch {
        watcher.abort("client");
        if (!res.destroyed) res.destroy();
      }
    } finally {
      watcher.done();
    }
  }

  async function download(req, res, url, kind) {
    const watcher = watch(res);
    const timer = setTimeout(() => watcher.abort("timeout"), timeoutMs);
    try {
      let upstreamResponse;
      try {
        upstreamResponse = await fetchImpl(url, {
          method: req.method,
          headers: authHeaders(),
          redirect: "manual",
          signal: watcher.controller.signal,
        });
      } catch {
        const reason = watcher.reason();
        if (reason === "client") return;
        return reason === "timeout" ? fail(res, 504, "daemon_timeout") : fail(res, 502, "daemon_unreachable");
      } finally {
        clearTimeout(timer);
      }
      const status = upstreamResponse.status;
      if (status === 401 || status === 403 || (status >= 300 && status < 400)) {
        await upstreamResponse.body?.cancel().catch(() => {});
        return fail(res, 502, status < 400 ? "daemon_redirect" : "daemon_auth");
      }
      if (status !== 200) return await sendJson(req, res, upstreamResponse, watcher);
      res.status(200);
      for (const name of fileResponseHeaders) {
        const value = upstreamResponse.headers.get(name);
        if (value !== null && !(token && value.includes(token))) res.setHeader(name, value);
      }
      const type = upstreamResponse.headers.get("content-type");
      const upstreamDisposition = upstreamResponse.headers.get("content-disposition");
      const disposition = dispositionFor(type, upstreamDisposition ?? (kind === "content" ? "attachment" : null));
      if (disposition) res.setHeader("Content-Disposition", disposition);
      if (!type) res.setHeader("Content-Type", "application/octet-stream");
      res.setHeader("Content-Security-Policy", fileCsp);
      res.setHeader("X-Content-Type-Options", "nosniff");
      res.setHeader("Cache-Control", "no-store");
      if (!upstreamResponse.body || req.method === "HEAD") {
        await upstreamResponse.body?.cancel().catch(() => {});
        return res.end();
      }
      res.flushHeaders();
      try {
        await pipeline(Readable.fromWeb(upstreamResponse.body), res);
      } catch {
        watcher.abort("client");
        if (!res.destroyed) res.destroy();
      }
    } finally {
      clearTimeout(timer);
      watcher.done();
    }
  }

  async function relay(req, res, next) {
    const queryIndex = req.originalUrl.indexOf("?");
    const rawPath = (queryIndex < 0 ? req.originalUrl : req.originalUrl.slice(0, queryIndex)).slice("/api".length);
    const route = chatRoute(req.method, rawPath);
    if (!route) return next();
    const search = queryIndex < 0 ? "" : req.originalUrl.slice(queryIndex + 1);
    const url = new URL(route.target, origin);
    if (url.origin !== origin) return fail(res, 400, "invalid_path");
    if (route.kind === "upload") {
      if (search) return fail(res, 400, "invalid_query");
      return upload(req, res, url);
    }
    if (route.kind === "stream") return stream(req, res, url, search);
    if (search) return fail(res, 400, "invalid_query");
    return download(req, res, url, route.kind);
  }

  function register(app) {
    app.use("/api/chat", (req, res, next) => {
      relay(req, res, next).catch(next);
    });
  }

  return { origin, limit, register };
}
