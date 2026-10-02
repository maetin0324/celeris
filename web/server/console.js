import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fail, parseUpstream, readTokenFile } from "./relay.js";

// Console の block の SSE 中継（docs/web/implementation-plan.md P3-01、feature-parity R38）。
// - `/console/stream` → daemon の `/api/v1/console/stream`。`scope`・`since` だけを保ち、バイト列は加工しない。
// - 未認証は auth.js が 401 にする（302 にしない）。切断で upstream を abort する。JSON 中継の timeout は掛けない。

const scopeRe = /^(?:all|(?:project|node):[A-Za-z0-9._~-]{1,128})$/;
const sinceRe = /^[A-Za-z0-9._~:=-]{1,256}$/;

// query を検査して daemon への query にする。`scope`（all / project:<id> / node:<id>）と `since`（不透明な cursor）だけ受ける。
export function consoleParams(search) {
  const out = new URLSearchParams();
  for (const [key, value] of new URLSearchParams(search)) {
    if (out.has(key)) return null;
    if (key === "scope" && scopeRe.test(value)) out.set(key, value);
    else if (key === "since" && sinceRe.test(value)) out.set(key, value);
    else return null;
  }
  return out;
}

export function createConsole({ upstream, tokenFile, fetchImpl = fetch }) {
  const origin = parseUpstream(upstream);
  const token = readTokenFile(tokenFile);

  async function relay(req, res) {
    if (req.method !== "GET") return fail(res, 405, "method_not_allowed");
    const queryIndex = req.originalUrl.indexOf("?");
    const pathname = queryIndex < 0 ? req.originalUrl : req.originalUrl.slice(0, queryIndex);
    if (pathname !== "/console/stream") return fail(res, 404, "not_found");
    const query = consoleParams(queryIndex < 0 ? "" : req.originalUrl.slice(queryIndex + 1));
    if (!query) return fail(res, 400, "invalid_query");
    const url = new URL(`/api/v1/console/stream${query.size ? `?${query}` : ""}`, origin);

    const headers = new Headers({ accept: "text/event-stream" });
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
    app.use("/console/stream", (req, res, next) => {
      relay(req, res).catch(next);
    });
  }

  return { origin, register };
}
