import { createHash, createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { chmodSync, existsSync, lstatSync, mkdirSync, rmSync, statSync } from "node:fs";
import { createServer, type Server } from "node:net";
import path from "node:path";
import { type AuthConfig, getAuthConfig, readValidSession } from "~/auth.server";
import type { BrowserOwnerView } from "~/lib/browser";

/**
 * ADR-0080 D6: browser の本人（owner）session。
 * - 単一所有者の専用 instance だけ。認証が無効（loopback の既定）なら本人を区別できないので全て拒否する
 * - GUI の `POST /browser/owner-session` が非秘密の challenge を発行し、本人がローカルの
 *   `celerisctl browser owner-session approve <challenge>` で確定すると、その cookie session に grant が付く
 * - challenge は一回限り・5 分。grant は cookie の期限を超えない。再登録で旧 grant は失効。
 *   logout・cookie 期限・GUI 再起動（メモリだけに持つ）で失効する
 * - cookie 自体・cookie ID は外へ出さない（ここで扱うのは ID のハッシュだけ）
 */

export const OWNER_CHALLENGE_TTL_MS = 5 * 60 * 1000;
/** CLI が接続する Unix control socket（runtime directory 0700 / socket 0600）。 */
export const OWNER_SOCKET_ENV = "CELERIS_GUI_OWNER_SOCKET";

interface Challenge {
  sessionHash: string;
  expiresAtMs: number;
  sessionExpiresAtMs: number;
}

interface Grant {
  sessionHash: string;
  expiresAtMs: number;
}

interface OwnerStore {
  challenges: Map<string, Challenge>;
  grant: Grant | null;
  /** grant の失効を購読する（Live View の既存接続を閉じる）。 */
  listeners: Set<() => void>;
}

const store: OwnerStore = { challenges: new Map(), grant: null, listeners: new Set() };

/** テスト用: 状態を空に戻す。 */
export function resetOwnerStoreForTest(): void {
  store.challenges.clear();
  store.grant = null;
  store.listeners.clear();
}

/** cookie session の ID のハッシュ（ID 自体は保持・記録しない）。 */
export function ownerSessionHash(sessionId: string): string {
  return createHash("sha256").update(`celeris-browser-owner\0${sessionId}`, "utf8").digest("hex");
}

function sameHash(a: string, b: string): boolean {
  const x = Buffer.from(a, "utf8");
  const y = Buffer.from(b, "utf8");
  return x.length === y.length && timingSafeEqual(x, y);
}

function pruneChallenges(now: number): void {
  for (const [key, c] of store.challenges) if (c.expiresAtMs <= now) store.challenges.delete(key);
}

function notifyRevoked(): void {
  for (const fn of [...store.listeners]) {
    try {
      fn();
    } catch {
      // 購読側の失敗で他の失効を止めない
    }
  }
}

/** 本人確認の challenge を発行する。同じ session の古い challenge は捨てる。 */
export function issueOwnerChallenge(sessionHash: string, sessionExpiresAtMs: number, now = Date.now()): string {
  pruneChallenges(now);
  for (const [key, c] of store.challenges) if (sameHash(c.sessionHash, sessionHash)) store.challenges.delete(key);
  // 人がターミナルへ写すので短い大文字 hex（非秘密。一回限り・5 分）。
  const challenge = randomBytes(6).toString("hex").toUpperCase();
  store.challenges.set(challenge, {
    sessionHash,
    expiresAtMs: Math.min(now + OWNER_CHALLENGE_TTL_MS, sessionExpiresAtMs),
    sessionExpiresAtMs,
  });
  return challenge;
}

export type ApproveOutcome = "approved" | "unknown_challenge" | "expired";

/** control socket から: challenge を消費して、その session を唯一の owner にする（旧 grant は失効）。 */
export function approveOwnerChallenge(challenge: string, now = Date.now()): ApproveOutcome {
  const key = challenge.trim().toUpperCase();
  const c = store.challenges.get(key);
  store.challenges.delete(key);
  if (!c) return "unknown_challenge";
  if (c.expiresAtMs <= now || c.sessionExpiresAtMs <= now) return "expired";
  const hadGrant = store.grant !== null;
  store.grant = { sessionHash: c.sessionHash, expiresAtMs: c.sessionExpiresAtMs };
  if (hadGrant) notifyRevoked();
  return "approved";
}

/** この session の grant を失効させる（logout）。 */
export function revokeOwnerSession(sessionHash: string): void {
  for (const [key, c] of store.challenges) if (sameHash(c.sessionHash, sessionHash)) store.challenges.delete(key);
  if (store.grant && sameHash(store.grant.sessionHash, sessionHash)) {
    store.grant = null;
    notifyRevoked();
  }
}

export function onOwnerRevoked(fn: () => void): () => void {
  store.listeners.add(fn);
  return () => store.listeners.delete(fn);
}

function hasGrant(sessionHash: string, now: number): boolean {
  const g = store.grant;
  if (!g) return false;
  if (g.expiresAtMs <= now) {
    store.grant = null;
    notifyRevoked();
    return false;
  }
  return sameHash(g.sessionHash, sessionHash);
}

function hasPendingChallenge(sessionHash: string, now: number): boolean {
  pruneChallenges(now);
  for (const c of store.challenges.values()) if (sameHash(c.sessionHash, sessionHash)) return true;
  return false;
}

export type { BrowserOwnerView };

export type OwnerCheck =
  | { ok: true; sessionHash: string; config: AuthConfig; expiresAtMs: number }
  | { ok: false; status: 401 | 403; code: "unauthenticated" | "owner_unavailable" | "not_owner" };

/** 認証の有無と session を見る（owner かどうかは問わない）。 */
export async function readOwnerSession(
  request: Request,
  now = Date.now(),
): Promise<
  | { ok: true; sessionHash: string; config: AuthConfig; expiresAtMs: number }
  | { ok: false; status: 401 | 403; code: "unauthenticated" | "owner_unavailable" }
> {
  const config = getAuthConfig();
  // 認証なし（loopback 既定）では本人を識別できない。`authenticated=true` だけでは許可しない（ADR-0080 D6）。
  if (!config.enabled) return { ok: false, status: 403, code: "owner_unavailable" };
  const session = await readValidSession(config, request, now);
  if (!session) return { ok: false, status: 401, code: "unauthenticated" };
  return { ok: true, sessionHash: ownerSessionHash(session.id), config, expiresAtMs: session.expiresAtMs };
}

/** 本人の session か（毎回照合する）。 */
export async function checkOwner(request: Request, now = Date.now()): Promise<OwnerCheck> {
  const s = await readOwnerSession(request, now);
  if (!s.ok) return s;
  if (!hasGrant(s.sessionHash, now)) return { ok: false, status: 403, code: "not_owner" };
  return s;
}

/** session に束縛した CSRF token（cookie の署名鍵で HMAC）。 */
export function ownerCsrfToken(config: AuthConfig, sessionHash: string): string {
  return createHmac("sha256", config.secret).update(`browser-csrf\0${sessionHash}`, "utf8").digest("hex");
}

export function verifyOwnerCsrfToken(config: AuthConfig, sessionHash: string, candidate: unknown): boolean {
  if (typeof candidate !== "string" || candidate.length !== 64) return false;
  return sameHash(ownerCsrfToken(config, sessionHash), candidate);
}

/** 変更系は `Origin` が自分と完全一致することを必須にする（無い要求も拒否。ADR-0080 D5）。 */
export function exactSameOrigin(request: Request): boolean {
  const origin = request.headers.get("origin");
  if (origin === null) return false;
  return origin === new URL(request.url).origin;
}

export async function browserOwnerView(request: Request, now = Date.now()): Promise<BrowserOwnerView> {
  const s = await readOwnerSession(request, now);
  if (!s.ok)
    return { available: s.code !== "owner_unavailable", isOwner: false, challengePending: false, csrfToken: null };
  const isOwner = hasGrant(s.sessionHash, now);
  return {
    available: true,
    isOwner,
    challengePending: !isOwner && hasPendingChallenge(s.sessionHash, now),
    csrfToken: isOwner ? ownerCsrfToken(s.config, s.sessionHash) : null,
  };
}

// ---- local control socket（`celerisctl browser owner-session approve <challenge>`） ----

/** control socket の 1 行の要求を処理する（純粋。単体テスト用）。 */
export function handleOwnerControlLine(line: string, now = Date.now()): string {
  let op: unknown;
  let challenge: unknown;
  try {
    ({ op, challenge } = JSON.parse(line) as { op?: unknown; challenge?: unknown });
  } catch {
    return JSON.stringify({ ok: false, code: "bad_request" });
  }
  if (op !== "approve" || typeof challenge !== "string" || !/^[0-9A-Fa-f]{12}$/.test(challenge.trim())) {
    return JSON.stringify({ ok: false, code: "bad_request" });
  }
  const outcome = approveOwnerChallenge(challenge, now);
  return JSON.stringify(outcome === "approved" ? { ok: true, code: outcome } : { ok: false, code: outcome });
}

let controlServer: Server | null = null;

/**
 * control socket を開く。runtime directory は 0700、socket は 0600。
 * 注: Node は SO_PEERCRED を読めないので peer UID の照合はできない。同一 UID の相手は区別できない
 * （ADR-0080 D7: file mode で同一 UID の worker を隔離したとは主張しない）。
 */
export function startOwnerControlSocket(socketPath = process.env[OWNER_SOCKET_ENV]): Server | null {
  if (!socketPath || controlServer) return controlServer;
  const dir = path.dirname(socketPath);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  const st = statSync(dir);
  if (typeof process.getuid === "function" && st.uid !== process.getuid()) {
    throw new Error(`${OWNER_SOCKET_ENV}: directory ${dir} is not owned by this user`);
  }
  chmodSync(dir, 0o700);
  if (existsSync(socketPath)) {
    if (!lstatSync(socketPath).isSocket())
      throw new Error(`${OWNER_SOCKET_ENV}: ${socketPath} exists and is not a socket`);
    rmSync(socketPath);
  }
  const server = createServer((conn) => {
    let buf = "";
    conn.setEncoding("utf8");
    conn.setTimeout(5_000, () => conn.destroy());
    conn.on("data", (chunk: string) => {
      buf += chunk;
      if (buf.length > 1024) {
        conn.end(`${JSON.stringify({ ok: false, code: "bad_request" })}\n`);
        return;
      }
      const nl = buf.indexOf("\n");
      if (nl < 0) return;
      conn.end(`${handleOwnerControlLine(buf.slice(0, nl))}\n`);
    });
    conn.on("error", () => conn.destroy());
  });
  const umask = process.umask(0o177);
  try {
    server.listen(socketPath);
  } finally {
    process.umask(umask);
  }
  server.on("listening", () => chmodSync(socketPath, 0o600));
  server.unref();
  server.on("close", () => {
    if (controlServer === server) controlServer = null;
  });
  controlServer = server;
  return server;
}
