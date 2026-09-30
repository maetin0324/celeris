import { readFileSync } from "node:fs";
import { isIP } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";
import express from "express";
import packageInfo from "../package.json" with { type: "json" };
import { createAuth } from "./auth.js";
import { createEvents } from "./events.js";
import { createFiles } from "./files.js";
import { createRelay } from "./relay.js";
import { isSpaRoute } from "./spa-routes.js";

const webRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const unsafeMethods = new Set(["POST", "PUT", "PATCH", "DELETE"]);
const defaultCsp = "default-src 'none'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'";
const htmlCsp = `${defaultCsp}; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; font-src 'self'`;
const fingerprint = /-[a-zA-Z0-9_-]{8,}\.[a-zA-Z0-9]+$/;

export function parseBind(value = "127.0.0.1:7720") {
  const match = /^(?:\[([^\]]+)\]|([^:]+)):(\d{1,5})$/.exec(value.trim());
  if (!match) throw new Error("CELERIS_WEB_BIND must be host:port");
  const host = match[1] ?? match[2];
  const port = Number(match[3]);
  if (!host || port < 1 || port > 65535) throw new Error("CELERIS_WEB_BIND has an invalid host or port");
  return { host, port };
}

export function isLoopback(host) {
  if (host.toLowerCase() === "localhost") return true;
  const kind = isIP(host);
  return (kind === 4 && host.startsWith("127.")) || (kind === 6 && host.toLowerCase() === "::1");
}

export function readPasswordFile(file) {
  if (!file) return null;
  const password = readFileSync(file, "utf8").trim();
  if (!password) throw new Error("CELERIS_WEB_PASSWORD_FILE is empty");
  return password;
}

export function validateConfig({ bind, passwordFile }) {
  if (!isLoopback(bind.host) && !passwordFile)
    throw new Error("CELERIS_WEB_PASSWORD_FILE is required for non-loopback bind");
  if (passwordFile) readPasswordFile(passwordFile);
}

function hostName(authority) {
  if (typeof authority !== "string" || !authority || /[\s/@?#\\]/.test(authority)) return null;
  const match = /^(?:\[([0-9a-fA-F:]+)\]|([a-zA-Z0-9.-]+))(?::(\d{1,5}))?$/.exec(authority);
  if (!match) return null;
  const port = match[3] === undefined ? null : Number(match[3]);
  if (port !== null && (port < 1 || port > 65535)) return null;
  if (match[1] && isIP(match[1]) !== 6) return null;
  return (match[1] ?? match[2]).toLowerCase();
}

function selfOrigin(req) {
  return `${req.socket.encrypted ? "https" : "http"}://${req.headers.host}`;
}

export function createApp({
  bind = parseBind(process.env.CELERIS_WEB_BIND),
  passwordFile = process.env.CELERIS_WEB_PASSWORD_FILE,
  allowedHosts = process.env.CELERIS_WEB_ALLOWED_HOSTS ?? "",
  distDir = path.join(webRoot, "dist"),
  release = process.env.CELERIS_WEB_RELEASE ?? "dev",
  log = (entry) => process.stderr.write(`${JSON.stringify(entry)}\n`),
  secretFile = process.env.CELERIS_WEB_SESSION_SECRET_FILE,
  failedLoginDelayMs,
  daemonUrl,
  daemonTokenFile,
  relayTimeoutMs,
  registerRoutes = () => {},
} = {}) {
  validateConfig({ bind, passwordFile });
  const auth = createAuth({ passwordFile, secretFile, failedDelayMs: failedLoginDelayMs });
  // daemonUrl が無ければ中継しない（/api/* は 404）。起動時の既定は index.js が与える。
  // `/files/*` と `/events` も同じ daemon へ中継する（P1-08・P1-09）。
  const relays = daemonUrl
    ? [
        createRelay({ upstream: daemonUrl, tokenFile: daemonTokenFile, timeoutMs: relayTimeoutMs }),
        createFiles({ upstream: daemonUrl, tokenFile: daemonTokenFile, timeoutMs: relayTimeoutMs }),
        createEvents({ upstream: daemonUrl, tokenFile: daemonTokenFile }),
      ]
    : [];
  const allowed = new Set(["localhost", "127.0.0.1", "::1", bind.host.toLowerCase()]);
  for (const value of allowedHosts.split(",")) {
    const host = hostName(value.trim());
    if (host) allowed.add(host);
  }
  const app = express();
  app.disable("x-powered-by");
  app.set("trust proxy", false);

  app.use((req, res, next) => {
    const started = process.hrtime.bigint();
    res.on("finish", () =>
      log({
        path: req.path,
        status: res.statusCode,
        ms: Math.round(Number(process.hrtime.bigint() - started) / 1e5) / 10,
      }),
    );
    res.set({
      "X-Content-Type-Options": "nosniff",
      "Referrer-Policy": "no-referrer",
      "X-Frame-Options": "DENY",
      "Content-Security-Policy": defaultCsp,
      "Cache-Control": "no-store",
    });
    const host = hostName(req.headers.host);
    if (!host || !allowed.has(host)) return res.status(400).type("text/plain").send("host not allowed");
    if (unsafeMethods.has(req.method)) {
      const origin = req.headers.origin;
      const site = req.headers["sec-fetch-site"];
      if (
        (origin !== undefined &&
          (typeof origin !== "string" || origin.trim().toLowerCase() !== selfOrigin(req).toLowerCase())) ||
        (site !== undefined &&
          (typeof site !== "string" || !["same-origin", "none"].includes(site.trim().toLowerCase())))
      )
        return res.status(403).type("text/plain").send("csrf rejected");
    }
    next();
  });

  app.get("/healthz", (_req, res) =>
    res.json({ ok: true, name: "celeris-web", version: packageInfo.version, release }),
  );
  app.use(
    "/assets",
    express.static(path.join(distDir, "assets"), {
      fallthrough: false,
      setHeaders(res, file) {
        if (fingerprint.test(path.basename(file)))
          res.setHeader("Cache-Control", "public, max-age=31536000, immutable");
      },
    }),
  );
  auth.register(app);
  registerRoutes(app);
  for (const relay of relays) relay.register(app);
  app.use((req, res, next) => {
    if (
      req.path.startsWith("/api/") ||
      req.path === "/api" ||
      req.path.startsWith("/files/") ||
      req.path === "/files" ||
      req.path.startsWith("/events/") ||
      req.path === "/events" ||
      path.extname(req.path)
    )
      return res.status(404).type("text/plain").send("not found");
    if (req.method !== "GET" && req.method !== "HEAD") return res.status(404).type("text/plain").send("not found");
    res.set("Content-Security-Policy", htmlCsp);
    res.set("Cache-Control", "no-store");
    // 未定義の path も同じ middleware を通したうえで 404。本文は SPA で、shell の中の 404 を出す（R42）。
    if (!isSpaRoute(req.path)) res.status(404);
    res.sendFile(path.join(distDir, "index.html"), (error) => {
      if (error && !res.headersSent) next(error);
    });
  });
  app.use((error, _req, res, _next) => {
    if (res.headersSent) return;
    res
      .status(error.status === 404 ? 404 : 500)
      .type("text/plain")
      .send(error.status === 404 ? "not found" : "internal error");
  });
  return app;
}
