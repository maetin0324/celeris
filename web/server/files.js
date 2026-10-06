import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { DEFAULT_RELAY_TIMEOUT_MS, fail, parseUpstream, readTokenFile, validSegment } from "./relay.js";

// file の中継（agent-docs/web/implementation-plan.md P1-08、feature-parity R40・R41・X6、H8）。
// - `/files/tasks/:id/runs/:runId/:name` → daemon の `/api/v1/tasks/{id}/runs/{run_id}/{name}`、
//   `/files/tasks/:id/artifacts/:idx` → `/api/v1/tasks/{id}/artifacts/{idx}`。path の解釈は daemon が行い、
//   gateway は segment を検査する（`..`・区切り文字・NUL を拒む。`:idx` は 10 進の整数だけ）。
// - `Range`・`offset`・`length`・`download` を検査して転送し、206 / 416 と `Content-Range` をそのまま返す。
// - daemon の応答ヘッダは許可リストだけ通し、`nosniff` と `CSP sandbox` を付け直す。HTML・SVG・XML の本文は
//   同一オリジンで実行させず、`Content-Disposition: attachment` にする（H8）。
// - `view=1`（gateway だけが解釈し daemon へは送らない）は画面内・新しいタブでの表示。拡張子で HTML・SVG・PDF の
//   Content-Type を補い、`inline` にする。HTML・SVG は `CSP sandbox`（allow-* なし: script・form・popup を止め、
//   opaque origin）のまま、PDF は browser の viewer が sandbox で止まるので sandbox を外し `default-src 'none'` で
//   埋め込み元を自 origin に限る。どちらも `frame-ancestors 'self'`（ADR 2026-10-05-web-artifact-inline-view）。
// - 本文は buffer に貯めず流す。timeout は応答ヘッダが届くまでだけ。ブラウザが切断したら upstream を abort する。

const responseHeaders = [
  "content-type",
  "content-disposition",
  "content-length",
  "content-range",
  "accept-ranges",
  "etag",
  "last-modified",
  "x-celeris-sha256",
  "x-celeris-sha256-current",
  "x-celeris-size",
];
const fileCsp = "sandbox; default-src 'none'; frame-ancestors 'none'";
// `view=1` の応答。HTML・SVG は opaque origin で script を走らせず、inline の style と data: の画像・font だけ許す。
const viewCsp =
  "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";
// PDF は sandbox だと browser の viewer が表示を拒むので外す。本文は application/pdf と nosniff で HTML として解釈されない。
const pdfViewCsp = "default-src 'none'; object-src 'self'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'";
// daemon が application/octet-stream で返す拡張子のうち、`view=1` で表示する種類（api.md §3.8 の表に無いもの）。
const viewTypes = {
  html: "text/html; charset=utf-8",
  htm: "text/html; charset=utf-8",
  svg: "image/svg+xml",
  pdf: "application/pdf",
};
const activeType =
  /^\s*(text\/html|application\/xhtml\+xml|image\/svg\+xml|text\/xml|application\/xml|[^;]*\+xml)\s*(;|$)/i;
const range = /^bytes=\d*-\d*(\s*,\s*\d*-\d*)*$/;
const integer = /^\d{1,15}$/;

// `/files` より後ろの path を daemon の path にする。受けられなければ null。
export function filePath(rest) {
  if (typeof rest !== "string" || !rest.startsWith("/")) return null;
  const parts = rest.slice(1).split("/");
  if (!parts.every(validSegment) || parts[0] !== "tasks") return null;
  if (parts.length === 5 && parts[2] === "runs") return `/api/v1/${parts.join("/")}`;
  if (parts.length === 4 && parts[2] === "artifacts" && integer.test(parts[3])) return `/api/v1/${parts.join("/")}`;
  return null;
}

// query の `offset`・`length`・`download` を検査する。受けられなければ null。
export function fileQuery(search) {
  const params = new URLSearchParams(search);
  const out = new URLSearchParams();
  for (const [key, value] of params) {
    if (out.has(key)) return null;
    if (key === "offset" || key === "length") {
      if (!integer.test(value)) return null;
      out.set(key, value);
    } else if (key === "download" || key === "view") {
      if (value !== "1") return null;
      out.set(key, "1");
    } else return null;
  }
  if (out.has("download") && out.has("view")) return null;
  return out;
}

// Content-Disposition の file 名（RFC 8187 の `filename*` を優先）。無ければ空文字。
export function filenameOf(disposition) {
  if (!disposition) return "";
  const extended = /filename\*\s*=\s*UTF-8''([^;\s]+)/i.exec(disposition);
  if (extended) {
    try {
      return decodeURIComponent(extended[1]);
    } catch {
      // 壊れた符号化は fallback の filename を使う。
    }
  }
  return /filename\s*=\s*"([^"]*)"/i.exec(disposition)?.[1] ?? /filename\s*=\s*([^;\s]+)/i.exec(disposition)?.[1] ?? "";
}

// `view=1` の Content-Type。daemon が種類を決めなかった（octet-stream・無し）ときだけ拡張子で補う。
export function viewTypeFor(contentType, disposition) {
  if (contentType && !/^\s*application\/octet-stream\s*(;|$)/i.test(contentType)) return contentType;
  const ext = /\.([a-z0-9]+)$/i.exec(filenameOf(disposition))?.[1]?.toLowerCase();
  return (ext && viewTypes[ext]) || contentType || "application/octet-stream";
}

// `view=1` の Content-Disposition（inline。file 名の指定は残す）。
export function viewDisposition(disposition) {
  if (!disposition) return "inline";
  return /^\s*(inline|attachment)\b/i.test(disposition)
    ? disposition.replace(/^\s*(inline|attachment)\b/i, "inline")
    : `inline; ${disposition}`;
}

// `view=1` の CSP。PDF だけ sandbox を外す（上の pdfViewCsp）。
export function viewCspFor(contentType) {
  return /^\s*application\/pdf\s*(;|$)/i.test(contentType ?? "") ? pdfViewCsp : viewCsp;
}

// 応答が同一オリジンで実行されうる種類なら attachment にする。
export function dispositionFor(contentType, disposition) {
  if (!activeType.test(contentType ?? "")) return disposition;
  if (!disposition) return "attachment";
  return /^\s*inline\b/i.test(disposition) ? disposition.replace(/^\s*inline\b/i, "attachment") : disposition;
}

export function createFiles({ upstream, tokenFile, timeoutMs = DEFAULT_RELAY_TIMEOUT_MS, fetchImpl = fetch }) {
  const origin = parseUpstream(upstream);
  const token = readTokenFile(tokenFile);

  async function relay(req, res) {
    if (req.method !== "GET" && req.method !== "HEAD") return fail(res, 405, "method_not_allowed");
    const queryIndex = req.originalUrl.indexOf("?");
    const rawPath = (queryIndex < 0 ? req.originalUrl : req.originalUrl.slice(0, queryIndex)).slice("/files".length);
    const target = filePath(rawPath);
    if (!target) return fail(res, 400, "invalid_path");
    const query = fileQuery(queryIndex < 0 ? "" : req.originalUrl.slice(queryIndex + 1));
    if (!query) return fail(res, 400, "invalid_query");
    const view = query.has("view");
    query.delete("view");
    const url = new URL(`${target}${query.size ? `?${query}` : ""}`, origin);
    if (url.origin !== origin) return fail(res, 400, "invalid_path");

    const headers = new Headers();
    const rangeHeader = req.headers.range;
    if (rangeHeader !== undefined) {
      if (typeof rangeHeader !== "string" || !range.test(rangeHeader.trim())) return fail(res, 400, "invalid_range");
      headers.set("range", rangeHeader.trim());
    }
    if (token) headers.set("authorization", `Bearer ${token}`);

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
          redirect: "manual",
          signal: controller.signal,
        });
      } catch {
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
      res.status(status);
      for (const name of responseHeaders) {
        const value = upstreamResponse.headers.get(name);
        if (value !== null && !(token && value.includes(token))) res.set(name, value);
      }
      const upstreamType = upstreamResponse.headers.get("content-type");
      const upstreamDisposition = upstreamResponse.headers.get("content-disposition");
      if (view) {
        const type = viewTypeFor(upstreamType, upstreamDisposition);
        res.set("Content-Type", type);
        res.set("Content-Disposition", viewDisposition(upstreamDisposition));
        res.set("Content-Security-Policy", viewCspFor(type));
        res.set("X-Frame-Options", "SAMEORIGIN");
      } else {
        const disposition = dispositionFor(upstreamType, upstreamDisposition);
        if (disposition) res.set("Content-Disposition", disposition);
        if (!upstreamType) res.set("Content-Type", "application/octet-stream");
        res.set("Content-Security-Policy", fileCsp);
      }
      res.set("X-Content-Type-Options", "nosniff");
      res.set("Cache-Control", "no-store");
      if (!upstreamResponse.body || req.method === "HEAD") {
        await upstreamResponse.body?.cancel().catch(() => {});
        return res.end();
      }
      res.flushHeaders();
      try {
        await pipeline(Readable.fromWeb(upstreamResponse.body), res);
      } catch {
        controller.abort();
        if (!res.destroyed) res.destroy();
      }
    } finally {
      clearTimeout(timer);
      res.off("close", onClose);
    }
  }

  function register(app) {
    app.use("/files", (req, res, next) => {
      relay(req, res).catch(next);
    });
  }

  return { origin, register };
}
