import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { readFileSync } from "node:fs";
import express from "express";

// 認証と session（agent-docs/web/implementation-plan.md P1-05、H3 の決まるまでの扱い）。
// - 認証が有効になるのは CELERIS_WEB_PASSWORD_FILE があるとき（非 loopback では app.js が必須にする）。
// - パスワードは SHA-256 同士を timingSafeEqual で比べる。失敗は 1 s 待つ。
// - session は HMAC-SHA256 で署名した cookie。cookie 名と署名鍵は web/ 独自で、gui/ の
//   `__celeris_gui_session` は読まない。鍵は CELERIS_WEB_SESSION_SECRET_FILE、無ければ process ごとの乱数。

export const SESSION_COOKIE_NAME = "__celeris_web_session";
export const SESSION_MAX_AGE_SECONDS = 24 * 60 * 60;
export const FAILED_LOGIN_DELAY_MS = 1_000;

function digest(value) {
  return createHash("sha256").update(value, "utf8").digest();
}

function readTrimmed(file, what) {
  const value = readFileSync(file, "utf8").trim();
  if (!value) throw new Error(`${what} is empty`);
  return value;
}

function sign(secret, payload) {
  return createHmac("sha256", secret).update(payload).digest("base64url");
}

// `next` は同一オリジンの絶対パスだけを受ける。それ以外は "/"。
export function safeNextPath(value) {
  if (typeof value !== "string" || !value.startsWith("/") || value.startsWith("//") || value.startsWith("/\\"))
    return "/";
  if (value.includes("\\") || [...value].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f)) return "/";
  try {
    const url = new URL(value, "http://celeris-web.invalid");
    if (url.origin !== "http://celeris-web.invalid") return "/";
    return `${url.pathname}${url.search}${url.hash}`;
  } catch {
    return "/";
  }
}

function readCookie(req, name) {
  const header = req.headers.cookie;
  if (typeof header !== "string") return null;
  for (const part of header.split(";")) {
    const index = part.indexOf("=");
    if (index < 0) continue;
    if (part.slice(0, index).trim() === name) return part.slice(index + 1).trim();
  }
  return null;
}

function isProtected(path) {
  return (
    path === "/api" ||
    path.startsWith("/api/") ||
    path === "/files" ||
    path.startsWith("/files/") ||
    path === "/events" ||
    path.startsWith("/events/") ||
    path === "/console" ||
    path.startsWith("/console/")
  );
}

function wantsJson(req) {
  return req.is("application/json") === "application/json" || /application\/json/.test(req.headers.accept ?? "");
}

export function createAuth({
  passwordFile = process.env.CELERIS_WEB_PASSWORD_FILE,
  secretFile = process.env.CELERIS_WEB_SESSION_SECRET_FILE,
  now = () => Date.now(),
  failedDelayMs = FAILED_LOGIN_DELAY_MS,
} = {}) {
  const passwordDigest = passwordFile ? digest(readTrimmed(passwordFile, "CELERIS_WEB_PASSWORD_FILE")) : null;
  const secret = secretFile
    ? readTrimmed(secretFile, "CELERIS_WEB_SESSION_SECRET_FILE")
    : randomBytes(32).toString("base64");
  const enabled = passwordDigest !== null;
  const logoutListeners = new Set();

  function verifyPassword(candidate) {
    if (!passwordDigest || typeof candidate !== "string") return false;
    return timingSafeEqual(passwordDigest, digest(candidate.trim()));
  }

  function issue() {
    const payload = Buffer.from(
      JSON.stringify({ iat: Math.floor(now() / 1000), id: randomBytes(16).toString("base64url") }),
    ).toString("base64url");
    return `${payload}.${sign(secret, payload)}`;
  }

  function isValid(token) {
    if (typeof token !== "string") return false;
    const [payload, mac, extra] = token.split(".");
    if (!payload || !mac || extra !== undefined) return false;
    const expected = Buffer.from(sign(secret, payload));
    const actual = Buffer.from(mac);
    if (expected.length !== actual.length || !timingSafeEqual(expected, actual)) return false;
    try {
      const { iat } = JSON.parse(Buffer.from(payload, "base64url").toString("utf8"));
      const age = Math.floor(now() / 1000) - iat;
      return Number.isInteger(iat) && age >= 0 && age < SESSION_MAX_AGE_SECONDS;
    } catch {
      return false;
    }
  }

  function authenticated(req) {
    return !enabled || isValid(readCookie(req, SESSION_COOKIE_NAME));
  }

  function sessionKey(req) {
    const token = readCookie(req, SESSION_COOKIE_NAME);
    return enabled && isValid(token) ? createHash("sha256").update(`browser-owner\0${token}`).digest("hex") : null;
  }

  function cookieOptions(req) {
    return { httpOnly: true, sameSite: "strict", secure: Boolean(req.socket.encrypted), path: "/" };
  }

  function register(app) {
    const body = [express.urlencoded({ extended: false, limit: "4kb" }), express.json({ limit: "4kb" })];

    app.get("/api/session", (req, res) => res.json({ authenticated: authenticated(req), authRequired: enabled }));

    app.post("/login", ...body, async (req, res) => {
      const next = safeNextPath(req.body?.next);
      if (!enabled) return wantsJson(req) ? res.json({ ok: true, next }) : res.redirect(303, next);
      if (!verifyPassword(req.body?.password)) {
        await new Promise((resolve) => setTimeout(resolve, failedDelayMs));
        if (wantsJson(req)) return res.status(401).json({ ok: false, error: "invalid password" });
        return res.redirect(303, `/login?error=1&next=${encodeURIComponent(next)}`);
      }
      res.cookie(SESSION_COOKIE_NAME, issue(), { ...cookieOptions(req), maxAge: SESSION_MAX_AGE_SECONDS * 1000 });
      return wantsJson(req) ? res.json({ ok: true, next }) : res.redirect(303, next);
    });

    app.post("/logout", (req, res) => {
      for (const listener of logoutListeners) listener(sessionKey(req));
      res.clearCookie(SESSION_COOKIE_NAME, cookieOptions(req));
      return wantsJson(req) ? res.json({ ok: true }) : res.redirect(303, "/login");
    });

    app.use((req, res, next) => {
      if (isProtected(req.path) && !authenticated(req)) return res.status(401).json({ error: "unauthenticated" });
      next();
    });
  }

  return {
    enabled,
    register,
    authenticated,
    sessionKey,
    issue,
    isValid,
    verifyPassword,
    onLogout: (listener) => logoutListeners.add(listener),
  };
}
