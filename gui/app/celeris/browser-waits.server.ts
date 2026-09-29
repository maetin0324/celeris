import { createHash } from "node:crypto";
import { type AttestationDecision, signAttestation } from "~/browser-attestation.server";
import {
  checkOwner,
  exactSameOrigin,
  issueOwnerChallenge,
  readOwnerSession,
  verifyOwnerCsrfToken,
} from "~/browser-owner.server";
import { isOpenBrowserWait, loadTaskBrowserWaits } from "./browser";
import type { CelerisClient } from "./client.server";
import { CelerisError } from "./errors";
import type { BrowserWait, BrowserWaitResult, Status } from "./types";

/**
 * ADR-0080 D5: 本人の credential 登録・一回だけの承認/拒否を daemon API へ中継する BFF。
 * - 本人（owner grant）+ exact Origin + session 束縛の CSRF token を毎回要求する
 * - 秘密（username/password）は daemon へ渡すだけ。応答・ログ・loader data・URL に書かない
 * - 応答は固定コードだけ（入力や daemon の文言を反射しない）。`Cache-Control: no-store`
 * - origin・policy・version の正本は保存済みの wait（フォームからの差し替えは使わない）
 */

export const USERNAME_MAX_BYTES = 256;
export const PASSWORD_MAX_BYTES = 1024;
/** フォーム本文の上限（bytes）。 */
export const BROWSER_FORM_MAX_BYTES = 8 * 1024;

export type BrowserActionCode =
  | "registered"
  | "approved"
  | "denied"
  | "challenge_issued"
  | "method_not_allowed"
  | "csrf_failed"
  | "unauthenticated"
  | "owner_unavailable"
  | "not_owner"
  | "payload_too_large"
  | "invalid_input"
  | "wait_not_found"
  | "wait_not_actionable"
  | "version_conflict"
  | "wait_gone"
  | "attestation_unavailable"
  | "rejected"
  | "celeris_unavailable";

export interface BrowserActionBody {
  ok: boolean;
  code: BrowserActionCode;
  /** 成功時の task の状態（登録で ready、承認で ready/running、拒否で failed 等） */
  task_status?: Status;
  /** 本人確認の challenge（非秘密。`celerisctl browser owner-session approve <challenge>`） */
  challenge?: string;
}

const STATUS: Record<BrowserActionCode, number> = {
  registered: 200,
  approved: 200,
  denied: 200,
  challenge_issued: 200,
  method_not_allowed: 405,
  csrf_failed: 403,
  unauthenticated: 401,
  owner_unavailable: 403,
  not_owner: 403,
  payload_too_large: 413,
  invalid_input: 422,
  wait_not_found: 404,
  wait_not_actionable: 409,
  version_conflict: 409,
  wait_gone: 410,
  attestation_unavailable: 503,
  rejected: 422,
  celeris_unavailable: 503,
};

export function browserActionResponse(body: BrowserActionBody): Response {
  return Response.json(body, {
    status: STATUS[body.code],
    headers: {
      "Cache-Control": "no-store",
      "Referrer-Policy": "no-referrer",
      "X-Content-Type-Options": "nosniff",
    },
  });
}

function fail(code: BrowserActionCode): Response {
  return browserActionResponse({ ok: false, code });
}

function daemonFailure(e: unknown): BrowserActionCode {
  if (!(e instanceof CelerisError)) return "celeris_unavailable";
  switch (e.status) {
    case 401:
    case 403:
      return "rejected";
    case 404:
      return "wait_not_found";
    case 409:
      return e.code === "version_conflict" ? "version_conflict" : "wait_not_actionable";
    case 410:
      return "wait_gone";
    case 422:
      return "rejected";
    case 503:
      return "celeris_unavailable";
    default:
      return "celeris_unavailable";
  }
}

const ID_RE = /^[0-9A-Za-z_-]{1,64}$/;

function byteLength(s: string): number {
  return Buffer.byteLength(s, "utf8");
}

type Guarded =
  | { ok: true; form: FormData; sessionHash: string; taskId: string; version: number; wait: BrowserWait }
  | { ok: false; response: Response };

/** 変更系の共通検査: method → Origin → owner → 大きさ → CSRF → 保存済みの wait。 */
async function guard(
  client: CelerisClient,
  request: Request,
  waitId: string | undefined,
  reason: BrowserWait["reason"],
): Promise<Guarded> {
  const no = (code: BrowserActionCode): Guarded => ({ ok: false, response: fail(code) });
  if (request.method.toUpperCase() !== "POST") return no("method_not_allowed");
  if (!exactSameOrigin(request)) return no("csrf_failed");
  const owner = await checkOwner(request);
  if (!owner.ok) return no(owner.code);
  const length = Number(request.headers.get("content-length") ?? "0");
  if (!Number.isFinite(length) || length > BROWSER_FORM_MAX_BYTES) return no("payload_too_large");
  let form: FormData;
  try {
    const raw = await request.arrayBuffer();
    if (raw.byteLength > BROWSER_FORM_MAX_BYTES) return no("payload_too_large");
    form = await new Response(raw, {
      headers: { "Content-Type": request.headers.get("content-type") ?? "" },
    }).formData();
  } catch {
    return no("invalid_input");
  }
  if (!verifyOwnerCsrfToken(owner.config, owner.sessionHash, form.get("csrf"))) return no("csrf_failed");
  const taskId = form.get("task_id");
  const versionRaw = form.get("expected_version");
  if (!waitId || !ID_RE.test(waitId) || typeof taskId !== "string" || !ID_RE.test(taskId)) return no("invalid_input");
  if (typeof versionRaw !== "string" || !/^\d{1,15}$/.test(versionRaw)) return no("invalid_input");
  const version = Number(versionRaw);
  let waits: BrowserWait[];
  try {
    waits = await loadTaskBrowserWaits(client, taskId, request.signal);
  } catch (e) {
    return no(daemonFailure(e));
  }
  const wait = waits.find((w) => w.wait_id === waitId);
  if (!wait) return no("wait_not_found");
  if (wait.reason !== reason || !isOpenBrowserWait(wait)) return no("wait_not_actionable");
  if (Date.parse(wait.deadline) <= Date.now()) return no("wait_gone");
  if (wait.version !== version) return no("version_conflict");
  return { ok: true, form, sessionHash: owner.sessionHash, taskId, version, wait };
}

function attest(g: Extract<Guarded, { ok: true }>, decision: AttestationDecision) {
  return signAttestation({
    ownerSessionHash: g.sessionHash,
    taskId: g.taskId,
    waitId: g.wait.wait_id,
    version: g.version,
    decision,
    policyHash: g.wait.policy_hash,
  });
}

/** `POST /browser/waits/:waitId/credential`: 本人の手動登録（username/password）。 */
export async function runCredentialAction(
  client: CelerisClient,
  request: Request,
  waitId: string | undefined,
): Promise<Response> {
  const g = await guard(client, request, waitId, "waiting_for_auth");
  if (!g.ok) return g.response;
  const username = g.form.get("username");
  const password = g.form.get("password");
  if (typeof username !== "string" || typeof password !== "string") return fail("invalid_input");
  const ul = byteLength(username);
  const pl = byteLength(password);
  if (ul === 0 || ul > USERNAME_MAX_BYTES || pl === 0 || pl > PASSWORD_MAX_BYTES) return fail("invalid_input");
  const attestation = attest(g, "register");
  if (!attestation) return fail("attestation_unavailable");
  try {
    const result = await client.post<BrowserWaitResult>(
      `/tasks/${encodeURIComponent(g.taskId)}/browser/waits/${encodeURIComponent(g.wait.wait_id)}/credential`,
      { expected_version: g.version, username, password, attestation },
      { signal: request.signal },
    );
    return browserActionResponse({ ok: true, code: "registered", task_status: result.task_status });
  } catch (e) {
    // 例外（CelerisError の detail を含む）は記録しない。固定コードへ写すだけ。
    return fail(daemonFailure(e));
  }
}

/** `POST /browser/waits/:waitId/decision`: `approve_once` / `deny`。 */
export async function runDecisionAction(
  client: CelerisClient,
  request: Request,
  waitId: string | undefined,
): Promise<Response> {
  const g = await guard(client, request, waitId, "waiting_for_approval");
  if (!g.ok) return g.response;
  const decision = g.form.get("decision");
  if (decision !== "approve_once" && decision !== "deny") return fail("invalid_input");
  const attestation = attest(g, decision);
  if (!attestation) return fail("attestation_unavailable");
  // 同じ wait/version/決定の再送は同じ key（daemon 側で冪等）。
  const idempotencyKey = createHash("sha256")
    .update(`${g.wait.wait_id}\0${g.version}\0${decision}\0${g.sessionHash}`)
    .digest("hex")
    .slice(0, 32);
  try {
    const result = await client.post<BrowserWaitResult>(
      `/tasks/${encodeURIComponent(g.taskId)}/browser/waits/${encodeURIComponent(g.wait.wait_id)}/decision`,
      { decision, expected_version: g.version, idempotency_key: idempotencyKey, attestation },
      { signal: request.signal },
    );
    return browserActionResponse({
      ok: true,
      code: decision === "approve_once" ? "approved" : "denied",
      task_status: result.task_status,
    });
  } catch (e) {
    return fail(daemonFailure(e));
  }
}

/** `POST /browser/owner-session`: この session の本人確認 challenge を発行する。 */
export async function runOwnerChallengeAction(request: Request): Promise<Response> {
  if (request.method.toUpperCase() !== "POST") return fail("method_not_allowed");
  if (!exactSameOrigin(request)) return fail("csrf_failed");
  const s = await readOwnerSession(request);
  if (!s.ok) return fail(s.code);
  const challenge = issueOwnerChallenge(s.sessionHash, s.expiresAtMs);
  return browserActionResponse({ ok: true, code: "challenge_issued", challenge });
}
