import { readFileSync } from "node:fs";
import express from "express";

// JSON API の中継（agent-docs/web/implementation-plan.md P1-07、feature-parity X5）。
// - same-origin の `/api/*` を daemon の `/api/v1/*` へ送る。upstream は起動設定の origin に固定し、要求から
//   URL・host を決めない。path は segment ごとに検査し、`..`・区切り文字の符号化・制御文字を拒む。
// - daemon の token はファイルから読み、gateway → daemon の Authorization にだけ付ける。ブラウザの Authorization・
//   Cookie は転送しない。要求・応答のヘッダは許可リストだけ通す。
// - 応答で原因を区別する: gateway の障害 500 `gateway_error`、daemon に届かない 502 `daemon_unreachable`、
//   時間切れ 504 `daemon_timeout`、daemon の認証エラー 502 `daemon_auth`（ブラウザの session 切れ 401 と混ぜない）。
//   どれも `X-Celeris-Web-Error` に同じ値を付ける。daemon の他の応答（4xx/5xx を含む）はそのまま返す。
// - token が応答本文に紛れても `[redacted]` に置き換える。ログには path と status だけが出る（app.js）。

export const DEFAULT_RELAY_TIMEOUT_MS = 30_000;
const BODY_LIMIT = "1mb";
const methods = new Set(["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE"]);
const requestHeaders = ["accept", "content-type", "if-match", "if-none-match"];
const responseHeaders = ["content-type", "etag", "last-modified"];
const segment = /^[A-Za-z0-9._~:@!$&'()*+,;=-]+$/;

export function parseUpstream(value) {
  let url;
  try {
    url = new URL(String(value));
  } catch {
    throw new Error("CELERIS_API_URL must be an http(s) URL");
  }
  if (!["http:", "https:"].includes(url.protocol) || url.username || url.password || url.search || url.hash)
    throw new Error("CELERIS_API_URL must be an http(s) origin without credentials, query or fragment");
  if (url.pathname !== "/" && url.pathname !== "") throw new Error("CELERIS_API_URL must not have a path");
  return url.origin;
}

export function readTokenFile(file) {
  if (!file) return null;
  const token = readFileSync(file, "utf8").trim();
  if (!token) throw new Error("CELERIS_API_TOKEN_FILE is empty");
  return token;
}

// `/api` より後ろの path を daemon の `/api/v1/...` にする。受けられなければ null。
export function upstreamPath(rest) {
  if (typeof rest !== "string" || !rest.startsWith("/") || rest === "/") return null;
  return rest.slice(1).split("/").every(validSegment) ? `/api/v1${rest}` : null;
}

// URL の path segment 1 つ（符号化のまま）を検査する。`.`・`..`・区切り文字（`/` `\`、その符号化）・
// NUL を含む制御文字・不正な % 符号化を拒む。file と SSE の中継（files.js・events.js）も使う。
export function validSegment(part) {
  if (typeof part !== "string" || !segment.test(part.replace(/%[0-9A-Fa-f]{2}/g, "_"))) return false;
  let decoded;
  try {
    decoded = decodeURIComponent(part);
  } catch {
    return false;
  }
  if (!decoded || decoded === "." || decoded === ".." || /[/\\]/.test(decoded)) return false;
  return ![...decoded].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f);
}

function redact(text, token) {
  const clean = token ? text.split(token).join("[redacted]") : text;
  if (!clean.includes("live_view_url")) return clean;
  try {
    return JSON.stringify(redactLiveUrls(JSON.parse(clean)));
  } catch {
    return clean.replace(/"live_view_url"\s*:\s*"(?:[^"\\]|\\.)*"/g, '"live_view_url":null');
  }
}

function redactLiveUrls(value) {
  if (Array.isArray(value)) return value.map(redactLiveUrls);
  if (!value || typeof value !== "object") return value;
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, key === "live_view_url" ? null : redactLiveUrls(item)]),
  );
}

function browserRouteRequired(path, method) {
  path = decodeURIComponent(path);
  if (
    method !== "GET" &&
    (/^\/api\/v1\/browser\/site-policies(?:\/|$)/.test(path) || /^\/api\/v1\/org\/[^/]+\/browser-settings$/.test(path))
  )
    return true;
  if (/^\/api\/v1\/browser\/credentials(?:\/|$)/.test(path)) return true;
  if (/^\/api\/v1\/browser\/identities(?:\/|$)/.test(path)) return true;
  if (!/^\/api\/v1\/tasks\/[^/]+\/browser(?:\/|$)/.test(path)) return false;
  if (/\/browser\/live(?:\/|$)/.test(path)) return true;
  if (/\/browser\/control(?:\/|$)/.test(path)) return true;
  return method !== "GET";
}

export function fail(res, status, code) {
  if (res.headersSent) return res.destroy();
  res.set("X-Celeris-Web-Error", code).status(status).json({ error: code });
}

export function createRelay({ upstream, tokenFile, timeoutMs = DEFAULT_RELAY_TIMEOUT_MS, fetchImpl = fetch }) {
  const origin = parseUpstream(upstream);
  const token = readTokenFile(tokenFile);
  const body = express.raw({ type: () => true, limit: BODY_LIMIT });

  async function relay(req, res) {
    if (!methods.has(req.method)) return fail(res, 405, "method_not_allowed");
    const queryIndex = req.originalUrl.indexOf("?");
    const rawPath = (queryIndex < 0 ? req.originalUrl : req.originalUrl.slice(0, queryIndex)).slice("/api".length);
    const target = upstreamPath(rawPath);
    if (!target) return fail(res, 400, "invalid_path");
    if (browserRouteRequired(target, req.method)) return fail(res, 403, "browser_route_required");
    const url = new URL(`${target}${queryIndex < 0 ? "" : req.originalUrl.slice(queryIndex)}`, origin);
    if (url.origin !== origin || !url.pathname.startsWith("/api/v1/")) return fail(res, 400, "invalid_path");

    const headers = new Headers();
    for (const name of requestHeaders) {
      const value = req.headers[name];
      if (typeof value === "string") headers.set(name, value);
    }
    if (token) headers.set("authorization", `Bearer ${token}`);
    const hasBody = !["GET", "HEAD"].includes(req.method) && Buffer.isBuffer(req.body) && req.body.length > 0;

    const controller = new AbortController();
    let reason = null;
    const timer = setTimeout(() => {
      reason = "timeout";
      controller.abort();
    }, timeoutMs);
    const onClose = () => {
      if (!res.writableFinished) {
        reason ??= "client";
        controller.abort();
      }
    };
    res.on("close", onClose);
    try {
      let upstreamResponse;
      try {
        upstreamResponse = await fetchImpl(url, {
          method: req.method,
          headers,
          body: hasBody ? req.body : undefined,
          redirect: "manual",
          signal: controller.signal,
        });
      } catch {
        if (reason === "client") return;
        return reason === "timeout" ? fail(res, 504, "daemon_timeout") : fail(res, 502, "daemon_unreachable");
      }
      if (upstreamResponse.status === 401 || upstreamResponse.status === 403) {
        await upstreamResponse.body?.cancel().catch(() => {});
        return fail(res, 502, "daemon_auth");
      }
      let text;
      try {
        text = req.method === "HEAD" ? "" : await upstreamResponse.text();
      } catch {
        if (reason === "client") return;
        return reason === "timeout" ? fail(res, 504, "daemon_timeout") : fail(res, 502, "daemon_unreachable");
      }
      if (upstreamResponse.status >= 300 && upstreamResponse.status < 400) return fail(res, 502, "daemon_redirect");
      res.status(upstreamResponse.status);
      for (const name of responseHeaders) {
        const value = upstreamResponse.headers.get(name);
        if (value !== null) res.set(name, redact(value, token));
      }
      if (!upstreamResponse.headers.has("content-type")) res.type("text/plain");
      return res.send(redact(text, token));
    } finally {
      clearTimeout(timer);
      res.off("close", onClose);
    }
  }

  function register(app) {
    app.use("/api", body, (req, res, next) => {
      relay(req, res).catch(next);
    });
    app.use("/api", (error, _req, res, _next) => {
      if (error?.type === "entity.too.large") return fail(res, 413, "request_too_large");
      fail(res, 500, "gateway_error");
    });
  }

  return { origin, register };
}
