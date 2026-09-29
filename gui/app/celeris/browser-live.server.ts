import { request as httpRequest } from "node:http";
import { signLiveAssertion } from "~/browser-attestation.server";
import { checkOwner, onOwnerRevoked } from "~/browser-owner.server";
import { activeBrowserRunIds, inAuthInterval, safeBrowserLiveUrl } from "~/lib/browser";
import { loadBrowserRuns, loadTaskBrowserWaits } from "./browser";
import { type CelerisClient, getCelerisClient } from "./client.server";
import { CelerisError } from "./errors";
import type { BrowserRun, BrowserWait, BrowserWaitList, TaskDetail, TaskList } from "./types";

/**
 * ADR-0080 D6: `/browser/live/:taskId/:runId`。毎回 owner grant・task/run 対応・active・RUNNING・
 * 認証区間外を照合し、通った本人の session にだけ固定版 dashboard（agent-browser 0.38.1）を同一 origin で
 * 読み取り専用に relay する。
 * - upstream は起動時に固定した loopback の `CELERIS_GUI_LIVE_VIEW_UPSTREAM`（`host:port`）だけ。
 *   client からの URL/port は upstream の宛先にならない。run の `live_view_url` は「設定済みか」の gate にだけ使い、
 *   宛先にも応答にも使わない
 * - 未設定なら relay は開かず、guard を通った本人にも `503 live_view_relay_unavailable`
 * - HTML を開いた本人の session に「見ている run」を束縛する（メモリだけ）。dashboard が絶対 URL で取りに来る
 *   `/_next/*`・読み取り API・stream は、その束縛の run が今も guard を通るときだけ relay する
 *   （`browser-live-relay.server.ts`）
 * - upstream の URL・token・cookie・session ID は応答・Location・ログに出さない
 */

const ID_RE = /^[0-9A-Za-z_-]{1,64}$/;

export const LIVE_VIEW_UPSTREAM_ENV = "CELERIS_GUI_LIVE_VIEW_UPSTREAM";

export interface LiveViewUpstream {
  /** 接続先（`127.0.0.1` / `::1` / `localhost`）。 */
  host: string;
  port: number;
  /** `Host` / `Origin` に使う authority（`127.0.0.1:27849` / `[::1]:27849`）。 */
  authority: string;
}

/** `host:port`（host は loopback だけ）を読む。不正なら null。 */
export function parseLiveViewUpstream(value: string): LiveViewUpstream | null {
  const m = /^(?:\[(::1)\]|(127\.0\.0\.1|localhost)):(\d{1,5})$/.exec(value.trim());
  if (!m) return null;
  const port = Number(m[3]);
  if (!(port > 0 && port < 65536)) return null;
  if (m[1]) return { host: "::1", port, authority: `[::1]:${port}` };
  const host = m[2] ?? "127.0.0.1";
  return { host, port, authority: `${host}:${port}` };
}

let upstreamOverride: { value: LiveViewUpstream | null } | null = null;
let clientOverride: CelerisClient | null = null;
let upstreamFromEnv: { raw: string | undefined; value: LiveViewUpstream | null } | null = null;

/** 設定された upstream（未設定・不正なら null。不正な値は server.js が起動時に exit 2 で止める）。 */
export function liveViewUpstream(): LiveViewUpstream | null {
  if (upstreamOverride) return upstreamOverride.value;
  const raw = process.env[LIVE_VIEW_UPSTREAM_ENV];
  if (!upstreamFromEnv || upstreamFromEnv.raw !== raw) {
    upstreamFromEnv = { raw, value: raw ? parseLiveViewUpstream(raw) : null };
  }
  return upstreamFromEnv.value;
}

/** Live View の relay が使える構成か（画面の導線の表示に使う）。 */
export function liveViewRelayAvailable(): boolean {
  return liveViewUpstream() !== null;
}

/** relay が daemon に照会するときの client。 */
export function liveViewClient(): CelerisClient {
  return clientOverride ?? getCelerisClient();
}

/** テスト用: upstream（`host:port`、null で未設定）と daemon client を差し替える。undefined で元に戻す。 */
export function setLiveViewRelayForTest(opts: { upstream?: string | null; client?: CelerisClient | null } | undefined) {
  if (!opts) {
    upstreamOverride = null;
    clientOverride = null;
    resetLiveViewStateForTest();
    return;
  }
  if (opts.upstream !== undefined) {
    upstreamOverride = { value: opts.upstream === null ? null : parseLiveViewUpstream(opts.upstream) };
  }
  if (opts.client !== undefined) clientOverride = opts.client;
}

// ---- 応答 ----

const BASE_HEADERS: Record<string, string> = {
  "Cache-Control": "no-store",
  "Referrer-Policy": "no-referrer",
  "X-Content-Type-Options": "nosniff",
  "X-Frame-Options": "DENY",
};

/** relay した dashboard の CSP（同一 origin で動くのに必要な分だけ。inline script は dashboard の export が使う）。 */
export const LIVE_VIEW_CSP =
  "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/** 拒否・失敗の応答（固定コードだけ）。 */
export function liveViewPlain(status: number, code: string): Response {
  return new Response(`${code}\n`, {
    status,
    headers: {
      ...BASE_HEADERS,
      "Content-Type": "text/plain; charset=utf-8",
      "Content-Security-Policy": "default-src 'none'; frame-ancestors 'none'; base-uri 'none'",
      ...(status === 401 ? { "WWW-Authenticate": "Cookie" } : {}),
    },
  });
}

// ---- run の guard ----

export type LiveViewGuard = { ok: true; run: BrowserRun } | { ok: false; status: number; code: string };

/** The dashboard lists the entire namespace. A public run cannot bypass another task's auth interval.
 * Conservatively keep the relay closed until every task that used credentials is terminal. Read the
 * strict wait endpoint here: missing/partial history must never mean that the namespace is safe.
 */
async function namespaceInAuthInterval(client: CelerisClient, signal: AbortSignal): Promise<boolean> {
  let cursor: string | undefined;
  const seen = new Set<string>();
  for (let page = 0; page < 20; page++) {
    const tasks = await client.get<TaskList>("/tasks", {
      query: {
        limit: 200,
        status: "draft,ready,running,blocked,reviewing",
        archived: true,
        order: "created_desc",
        ...(cursor ? { cursor } : {}),
      },
      signal,
    });
    for (const task of tasks.items) {
      if (["done", "failed", "cancelled"].includes(task.status)) continue;
      const waits = await client.get<BrowserWaitList>(`/tasks/${encodeURIComponent(task.id)}/browser/waits`, {
        signal,
      });
      if (waits.items.some((w) => inAuthInterval([w], w.run_id))) return true;
    }
    if (!tasks.next_cursor) return false;
    if (seen.has(tasks.next_cursor)) throw new Error("Incomplete task history");
    cursor = tasks.next_cursor;
    seen.add(cursor);
  }
  throw new Error("Task history exceeds page limit");
}

/** task/run 対応・active controller・RUNNING・設定済み・認証区間外（owner の照合は呼び出し側）。 */
export async function checkLiveViewRun(
  client: CelerisClient,
  taskId: string,
  runId: string,
  signal: AbortSignal,
): Promise<LiveViewGuard> {
  if (!ID_RE.test(taskId) || !ID_RE.test(runId)) return { ok: false, status: 404, code: "not_found" };
  let detail: TaskDetail;
  let runs: BrowserRun[];
  let waits: BrowserWait[];
  try {
    [detail, runs, waits] = await Promise.all([
      client.get<TaskDetail>(`/tasks/${encodeURIComponent(taskId)}`, { signal }),
      loadBrowserRuns(client, taskId, signal),
      loadTaskBrowserWaits(client, taskId, signal),
    ]);
  } catch {
    return { ok: false, status: 404, code: "not_found" };
  }
  const run = runs.find((r) => r.run_id === runId && r.task_id === taskId);
  if (!run) return { ok: false, status: 404, code: "not_found" };
  const active = activeBrowserRunIds(detail.runs, detail.task.status).includes(runId);
  if (run.state !== "RUNNING" || !active) return { ok: false, status: 409, code: "not_running" };
  if (!safeBrowserLiveUrl(run.live_view_url)) return { ok: false, status: 404, code: "not_configured" };
  if (inAuthInterval(waits, runId)) return { ok: false, status: 409, code: "auth_interval" };
  try {
    if (await namespaceInAuthInterval(client, signal)) return { ok: false, status: 409, code: "auth_interval" };
  } catch {
    return { ok: false, status: 503, code: "live_view_guard_unavailable" };
  }
  return { ok: true, run };
}

/** Coalesce concurrent checks only. Never reuse a completed authorization decision. */
const guardCache = new Map<string, Promise<LiveViewGuard>>();

export function cachedLiveViewGuard(taskId: string, runId: string): Promise<LiveViewGuard> {
  const key = `${taskId}\0${runId}`;
  const hit = guardCache.get(key);
  if (hit) return hit;
  const result = checkLiveViewRun(liveViewClient(), taskId, runId, AbortSignal.timeout(10_000)).finally(() => {
    if (guardCache.get(key) === result) guardCache.delete(key);
  });
  guardCache.set(key, result);
  return result;
}

// ---- session と run の束縛 ----

export interface LiveViewBinding {
  taskId: string;
  runId: string;
  browserSessionId: string;
  grantId: string;
  boundAtMs: number;
}

const bindings = new Map<string, LiveViewBinding>();

function clearBindingsOnRevoke(): void {
  bindings.clear();
  guardCache.clear();
}

/** owner grant の失効（logout・期限・再登録）で束縛を全て捨てる。購読は冪等（同じ関数）。 */
function ensureRevokeSubscription(): void {
  onOwnerRevoked(clearBindingsOnRevoke);
}

export function bindLiveView(
  sessionHash: string,
  taskId: string,
  runId: string,
  browserSessionId: string,
  grantId: string,
  now = Date.now(),
): void {
  ensureRevokeSubscription();
  bindings.set(sessionHash, { taskId, runId, browserSessionId, grantId, boundAtMs: now });
}

export type LiveApiResult = { ok: true } | { ok: false; status: number; code: string };

function liveApiPath(binding: Pick<LiveViewBinding, "taskId" | "runId" | "browserSessionId">): string {
  return `/tasks/${encodeURIComponent(binding.taskId)}/browser/live/${encodeURIComponent(binding.runId)}/${encodeURIComponent(binding.browserSessionId)}`;
}

function liveApiFailure(error: unknown): LiveApiResult {
  if (error instanceof CelerisError) {
    const allowed = new Set([
      "other_task",
      "grant_expired",
      "run_ended",
      "observation_stopped",
      "not_owner_session",
      "origin_mismatch",
      "live_view_disabled",
    ]);
    return { ok: false, status: error.status, code: allowed.has(error.code) ? error.code : "live_view_denied" };
  }
  return { ok: false, status: 503, code: "live_view_guard_unavailable" };
}

function liveAssertion(binding: Pick<LiveViewBinding, "taskId" | "runId" | "browserSessionId">, sessionHash: string) {
  return signLiveAssertion({
    taskId: binding.taskId,
    runId: binding.runId,
    browserSessionId: binding.browserSessionId,
    ownerSessionId: sessionHash,
    originOk: true,
  });
}

export async function grantLiveView(
  client: CelerisClient,
  run: BrowserRun,
  sessionHash: string,
): Promise<{ ok: true; grantId: string } | Exclude<LiveApiResult, { ok: true }>> {
  const binding = { taskId: run.task_id, runId: run.run_id, browserSessionId: run.session_id };
  const assertion = liveAssertion(binding, sessionHash);
  if (!assertion) return { ok: false, status: 503, code: "attestation_unavailable" };
  try {
    const result = await client.post<{ grant_id: string }>(`${liveApiPath(binding)}/grant`, { assertion });
    return { ok: true, grantId: result.grant_id };
  } catch (error) {
    return liveApiFailure(error) as Exclude<LiveApiResult, { ok: true }>;
  }
}

export async function checkLiveGrant(binding: LiveViewBinding, sessionHash: string): Promise<LiveApiResult> {
  const assertion = liveAssertion(binding, sessionHash);
  if (!assertion) return { ok: false, status: 503, code: "attestation_unavailable" };
  try {
    await liveViewClient().post(`${liveApiPath(binding)}/check`, { assertion, grant_id: binding.grantId });
    return { ok: true };
  } catch (error) {
    return liveApiFailure(error);
  }
}

export interface LiveReadPage {
  plan: { kind: "replay"; after_seq: number } | { kind: "reset"; latest_seq: number };
  events: Array<{ seq: number; body: Record<string, unknown> }>;
}

export async function readLiveEvents(
  binding: LiveViewBinding,
  sessionHash: string,
  after: number,
): Promise<LiveReadPage | null> {
  const assertion = liveAssertion(binding, sessionHash);
  if (!assertion) return null;
  try {
    return await liveViewClient().post<LiveReadPage>(
      `${liveApiPath(binding)}/read`,
      { assertion, grant_id: binding.grantId },
      { query: { after } },
    );
  } catch {
    return null;
  }
}

export function liveViewBinding(sessionHash: string): LiveViewBinding | null {
  return bindings.get(sessionHash) ?? null;
}

export function unbindLiveView(sessionHash: string): void {
  bindings.delete(sessionHash);
}

/** guard の cache を捨てる（テスト用。次の照合で daemon に問い合わせ直す）。 */
export function clearLiveViewGuardCache(): void {
  guardCache.clear();
}

/** テスト用: 束縛と guard の cache を空にする。 */
export function resetLiveViewStateForTest(): void {
  bindings.clear();
  guardCache.clear();
}

// ---- upstream の HTTP ----

const UPSTREAM_MAX_BYTES = 32 * 1024 * 1024;

export interface UpstreamResult {
  status: number;
  contentType: string | null;
  body: Buffer;
}

/** 固定の upstream に GET する（`path` は relay が組み立てた検証済みのものだけ）。 */
export function upstreamGet(upstream: LiveViewUpstream, path: string, timeoutMs = 15_000): Promise<UpstreamResult> {
  return new Promise((resolve, reject) => {
    const req = httpRequest(
      {
        host: upstream.host,
        port: upstream.port,
        method: "GET",
        path,
        headers: {
          Host: upstream.authority,
          // dashboard の `/api/*` は同一 origin の Origin を要求する（loopback の Host + loopback の Origin なら token 不要）
          Origin: `http://${upstream.authority}`,
          Accept: "*/*",
          Connection: "close",
        },
        timeout: timeoutMs,
      },
      (res) => {
        const chunks: Buffer[] = [];
        let total = 0;
        res.on("data", (chunk: Buffer) => {
          total += chunk.length;
          if (total > UPSTREAM_MAX_BYTES) {
            req.destroy(new Error("upstream_too_large"));
            return;
          }
          chunks.push(chunk);
        });
        res.on("end", () => {
          const ct = res.headers["content-type"];
          resolve({
            status: res.statusCode ?? 502,
            contentType: typeof ct === "string" ? ct : null,
            body: Buffer.concat(chunks),
          });
        });
        res.on("error", () => reject(new Error("upstream_error")));
      },
    );
    req.on("timeout", () => req.destroy(new Error("upstream_timeout")));
    req.on("error", () => reject(new Error("upstream_unavailable")));
    req.end();
  });
}

/**
 * upstream の応答を relay の応答にする。持ち越すのは status と Content-Type だけ
 * （Set-Cookie・Location・token を運びうるヘッダは全て捨てる）。
 */
export function relayResponse(result: UpstreamResult, opts: { html?: boolean } = {}): Response {
  if (result.status >= 500) return liveViewPlain(502, "live_view_upstream_error");
  if (result.status >= 300 && result.status < 400) return liveViewPlain(502, "live_view_upstream_error");
  const contentType = result.contentType ?? "application/octet-stream";
  if (opts.html && result.status === 200 && !/^text\/html\b/i.test(contentType)) {
    return liveViewPlain(502, "live_view_upstream_error");
  }
  return new Response(new Uint8Array(result.body), {
    status: result.status,
    headers: { ...BASE_HEADERS, "Content-Type": contentType, "Content-Security-Policy": LIVE_VIEW_CSP },
  });
}

/** `GET /browser/live/:taskId/:runId` の `?port=` だけを 1〜5 桁のときに引き継ぐ（それ以外の query は捨てる）。 */
function entryUpstreamPath(search: string): string {
  const port = new URLSearchParams(search).get("port");
  return port && /^\d{1,5}$/.test(port) ? `/?port=${port}` : "/";
}

/** The dashboard remains read-only; this small companion presents persisted live events beside it. */
function withLiveEvents(result: UpstreamResult, taskId: string, runId: string, search: string): UpstreamResult {
  if (result.status !== 200 || !/^text\/html\b/i.test(result.contentType ?? "")) return result;
  const port = new URLSearchParams(search).get("port");
  if (!port || !/^\d{1,5}$/.test(port) || Number(port) < 1 || Number(port) > 65535) return result;
  const html = result.body.toString("utf8");
  if (!/<\/body>/i.test(html)) return result;
  const script = `<aside id="celeris-live-events" aria-label="Live events" style="position:fixed;right:0;top:0;z-index:2147483647;width:240px;max-height:45vh;overflow:auto;background:#111;color:#eee;font:12px sans-serif;padding:8px"><strong>Live events</strong><div id="celeris-live-status"></div><div id="celeris-live-tabs"></div><div id="celeris-live-url"></div><div id="celeris-live-console"></div></aside><script>(()=>{const base="/browser/live/${taskId}/${runId}/api/session/${port}/stream";const key="celeris-live:${taskId}:${runId}";let seen=Number(sessionStorage.getItem(key)||0);let delay=250;function put(id,value){document.getElementById("celeris-live-"+id).textContent=String(value??"")}function connect(){const ws=new WebSocket(base+"?last_seen="+seen);ws.onmessage=e=>{let m;try{m=JSON.parse(e.data)}catch{return}if(m.type==="live_reset"){seen=m.latest_seq;sessionStorage.setItem(key,String(seen));put("status","Reset: current state")}if(m.type==="live_snapshot"){put(m.path,JSON.stringify(m.value))}if(m.type==="live_event"&&m.seq>seen){seen=m.seq;sessionStorage.setItem(key,String(seen));const b=m.body||{};if(b.kind==="status")put("status",b.state);if(b.kind==="tabs")put("tabs",b.count+" tabs: "+(b.origins||[]).join(", "));if(b.kind==="url")put("url",b.url);if(b.kind==="console")put("console",b.level+": "+b.text)}};ws.onopen=()=>{delay=250};ws.onclose=()=>{setTimeout(connect,delay);delay=Math.min(delay*2,5000)}}connect()})()</script>`;
  return { ...result, body: Buffer.from(html.replace(/<\/body>/i, `${script}</body>`), "utf8") };
}

/**
 * `/browser/live/:taskId/:runId`（他の method・upgrade も同じ guard を通して拒否する）。
 * express の relay と React Router の route の両方がこれを使う。
 */
export async function runLiveViewRoute(
  client: CelerisClient,
  request: Request,
  taskId: string | undefined,
  runId: string | undefined,
): Promise<Response> {
  const owner = await checkOwner(request);
  if (!owner.ok) return liveViewPlain(owner.status, owner.code);
  if (request.method.toUpperCase() !== "GET") return liveViewPlain(405, "method_not_allowed");
  // WebSocket は `/api/session/:port/stream` の upgrade だけ（browser-live-relay.server.ts）。ここへは来させない。
  if (request.headers.get("upgrade")) return liveViewPlain(400, "live_view_upgrade_not_here");
  if (!taskId || !runId || !ID_RE.test(taskId) || !ID_RE.test(runId)) return liveViewPlain(404, "not_found");
  const guard = await checkLiveViewRun(client, taskId, runId, request.signal);
  if (!guard.ok) return liveViewPlain(guard.status, guard.code);
  const grant = await grantLiveView(client, guard.run, owner.sessionHash);
  if (!grant.ok) return liveViewPlain(grant.status, grant.code);
  const upstream = liveViewUpstream();
  // 未設定なら、guard を通った本人にも relay を開かない。
  if (!upstream) return liveViewPlain(503, "live_view_relay_unavailable");
  bindLiveView(owner.sessionHash, taskId, runId, guard.run.session_id, grant.grantId);
  let result: UpstreamResult;
  try {
    result = await upstreamGet(upstream, entryUpstreamPath(new URL(request.url).search));
  } catch {
    return liveViewPlain(502, "live_view_upstream_unavailable");
  }
  return relayResponse(withLiveEvents(result, taskId, runId, new URL(request.url).search), { html: true });
}
