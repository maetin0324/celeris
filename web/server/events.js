import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fail, parseUpstream, readTokenFile } from "./relay.js";

// SSE の中継（docs/web/implementation-plan.md P1-09、feature-parity R37 の中継の部分）。
// - `/events` → daemon の `/api/v1/stream`。バイト列は加工しない。
// - `Last-Event-ID`・`after_id` は event id（10 進の整数）、`task_id` は id の文字だけを受け、他の query は拒む。
// - `text/event-stream`・`no-store`・`X-Accel-Buffering: no` で返し、ヘッダをすぐ送る。
// - JSON 中継の timeout は掛けない（stream は何分でも続く）。ブラウザが切断したら upstream を abort する。
// - daemon の 401 / 503 を含む 4xx/5xx は status と本文を保つ。redirect は upstream を追わない。

const eventId = /^\d{1,20}$/;
const taskId = /^[A-Za-z0-9_-]{1,128}$/;

// query と `Last-Event-ID` を検査して daemon への query とヘッダの値にする。受けられなければ null。
export function streamParams(search, lastEventId) {
  const params = new URLSearchParams(search);
  const out = new URLSearchParams();
  for (const [key, value] of params) {
    if (out.has(key)) return null;
    if (key === "after_id" && eventId.test(value)) out.set(key, value);
    else if (key === "task_id" && taskId.test(value)) out.set(key, value);
    else return null;
  }
  if (lastEventId !== undefined && (typeof lastEventId !== "string" || !eventId.test(lastEventId.trim()))) return null;
  return { query: out, lastEventId: lastEventId?.trim() };
}

export function createEvents({ upstream, tokenFile, fetchImpl = fetch }) {
  const origin = parseUpstream(upstream);
  const token = readTokenFile(tokenFile);

  async function relay(req, res) {
    if (req.method !== "GET") return fail(res, 405, "method_not_allowed");
    const queryIndex = req.originalUrl.indexOf("?");
    const pathname = queryIndex < 0 ? req.originalUrl : req.originalUrl.slice(0, queryIndex);
    if (pathname !== "/events") return fail(res, 404, "not_found");
    const params = streamParams(
      queryIndex < 0 ? "" : req.originalUrl.slice(queryIndex + 1),
      req.headers["last-event-id"],
    );
    if (!params) return fail(res, 400, "invalid_query");
    const url = new URL(`/api/v1/stream${params.query.size ? `?${params.query}` : ""}`, origin);

    const headers = new Headers({ accept: "text/event-stream" });
    if (params.lastEventId) headers.set("last-event-id", params.lastEventId);
    if (token) headers.set("authorization", `Bearer ${token}`);

    const controller = new AbortController();
    const onClose = () => {
      if (!res.writableFinished) controller.abort();
    };
    res.on("close", onClose);
    try {
      let upstreamResponse;
      try {
        upstreamResponse = await fetchImpl(url, { headers, redirect: "manual", signal: controller.signal });
      } catch {
        if (controller.signal.aborted) return;
        return fail(res, 502, "daemon_unreachable");
      }
      const status = upstreamResponse.status;
      if (status >= 300 && status < 400) {
        await upstreamResponse.body?.cancel().catch(() => {});
        return fail(res, 502, "daemon_redirect");
      }
      if (status !== 200) {
        const text = await upstreamResponse.text().catch(() => "");
        res.status(status).type(upstreamResponse.headers.get("content-type") ?? "text/plain");
        return res.send(token ? text.split(token).join("[redacted]") : text);
      }
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
        controller.abort();
        if (!res.destroyed) res.destroy();
      }
    } finally {
      res.off("close", onClose);
    }
  }

  function register(app) {
    app.use("/events", (req, res, next) => {
      relay(req, res).catch(next);
    });
  }

  return { origin, register };
}
