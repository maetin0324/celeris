import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import type { Browser, BrowserContext, Page, WebSocket } from "@playwright/test";
import { expect, test } from "./test";

// ADR-0080 D4〜D6 の端から端（実 celeris daemon・実 GUI（パスワード認証）・実 agent-browser dashboard・実ブラウザ。LLM なし）。
// **scripts/browser-live-e2e.sh からだけ回す**（daemon・credentiald・dashboard・GUI をそのスクリプトが起動し、
// 下の G14_* を渡す）。単体で `playwright test` しても前提が無いので最初の test が理由を書いて落ちる。
//
//  (a) 未登録 credential → WAITING_FOR_AUTH（サイト + 用途）→ 本人 session の登録（celerisctl で承認）→
//      GUI の登録フォーム → task が Ready に戻り、新しい run が始まる
//  (b) WAITING_FOR_APPROVAL → GUI で承認 → 再開 / 別の wait → GUI で拒否 → task は failed（approval_denied）
//  (c) Live View: 本人は `/browser/live/<task>/<run>` で dashboard を見る。別 context（同じパスワードでログイン、
//      本人ではない）は 403、未認証は 401、別 run は 404、`POST /api/exec` は 403
//  (d) 登録フォームに入れた sentinel が DB・WAL・events API・daemon / GUI のログ・artifacts に無い
//
// wait は dispatcher の browser supervisor の代わりに admin token で開く（fake ワーカーは browser を持たない）。
// Live View の `browser_updated` も supervisor の代わりに scratch DB へ直接追記する（scripts/browser-live-e2e.sh の注記）。

const env = (name: string): string => {
  const v = process.env[name];
  if (!v) throw new Error(`${name} is not set; run this spec via gui/scripts/browser-live-e2e.sh`);
  return v;
};

const RUN_DIR = process.env.G14_RUN_DIR ?? "";
const ARTIFACTS = process.env.E2E_ARTIFACTS_DIR ?? path.join(RUN_DIR, "artifacts");
const API = process.env.CELERIS_API_URL ?? "";
const GUI = `http://${process.env.CELERIS_GUI_BIND ?? ""}`;
const ORIGIN = "https://login.example.org";
const LIVE_VIEW_URL = "https://browser.example.org/";
const CREDENTIAL_POLICY = "pol-g14-login";
const PURPOSE_AUTH = "Sign in to example.org to read the build dashboard (g14 e2e)";
const PURPOSE_APPROVE = "Use the registered example.org login once (g14 e2e)";
const PURPOSE_DENY = "Click the publish button on example.org (g14 e2e, will be denied)";

const POLICY = {
  policy_id: "g14-login",
  revision: 1,
  domain_mode: "common_hosts",
  network_domains: ["example.org", "login.example.org"],
  allowed_actions: ["navigate", "snapshot", "click", "credential_use"],
  approval_actions: ["click"],
  credential_policy_ids: [CREDENTIAL_POLICY],
};
// wait の policy_hash は store では固定語の token（`sha256:<hex>` 形式）。ここでは task policy の JSON の digest を使う。
const POLICY_HASH = `sha256:${
  execFileSync("sha256sum", { input: JSON.stringify(POLICY) })
    .toString()
    .split(" ")[0]
}`;

interface RunSummary {
  run_id: string;
  finished_at: string | null;
  outcome: unknown;
  end?: unknown;
}
interface TaskDetail {
  task: { id: string; status: string; title: string };
  runs: RunSummary[];
}
interface BrowserWait {
  wait_id: string;
  run_id: string;
  reason: string;
  state: string;
  version: number;
  resolution_code?: string;
  credential?: { credential_id: string; provider: string; policy_id: string };
}

let token = "";
let password = "";
let sentinel = "";

async function api<T>(method: string, p: string, body?: unknown): Promise<{ status: number; json: T; text: string }> {
  const res = await fetch(`${API}/api/v1${p}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body === undefined ? {} : { "Content-Type": "application/json" }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json: unknown = null;
  try {
    json = JSON.parse(text);
  } catch {
    // 本文が JSON でない（エラー）
  }
  return { status: res.status, json: json as T, text };
}

async function detail(id: string): Promise<TaskDetail> {
  const r = await api<TaskDetail>("GET", `/tasks/${id}`);
  expect(r.status, r.text).toBe(200);
  return r.json;
}

async function waits(id: string): Promise<BrowserWait[]> {
  const r = await api<{ items: BrowserWait[] }>("GET", `/tasks/${id}/browser/waits`);
  expect(r.status, r.text).toBe(200);
  return r.json.items;
}

function activeRun(d: TaskDetail): string | null {
  if (d.task.status !== "running") return null;
  const open = d.runs.filter((r) => !r.finished_at && !r.outcome && !r.end);
  return open.length > 0 ? open[open.length - 1].run_id : null;
}

/** fake ワーカーが task について起動した回数（`markers/<task_id>.<pid>`）。 */
function workerInvocations(taskId: string): number {
  const dir = path.join(RUN_DIR, "markers");
  if (!fs.existsSync(dir)) return 0;
  return fs.readdirSync(dir).filter((f) => f.startsWith(`${taskId}.`)).length;
}

/** task が running になり、別の run（`notRun` 以外）が走り出すまで待つ。run_id を返す。 */
async function waitRunning(id: string, opts: { notRun?: string; minInvocations?: number } = {}): Promise<string> {
  let seen = "";
  await expect
    .poll(
      async () => {
        const d = await detail(id);
        const run = activeRun(d);
        seen = `status=${d.task.status} activeRun=${run} invocations=${workerInvocations(id)}`;
        if (!run || run === opts.notRun) return seen;
        if (workerInvocations(id) < (opts.minInvocations ?? 1)) return seen;
        return `ok:${run}`;
      },
      { timeout: 60_000, intervals: [250, 500, 1000] },
    )
    .toMatch(/^ok:/);
  const d = await detail(id);
  const run = activeRun(d);
  if (!run) throw new Error(`task ${id} is not running any more (${seen})`);
  return run;
}

/** browser skill を持たない fake の task を作り（draft で policy を付けてから Go）、run が始まるまで待つ。 */
async function createRunningTask(title: string): Promise<{ id: string; runId: string }> {
  const created = await api<{ id?: string; task?: { id: string } }>("POST", "/tasks", {
    title,
    objective: `${title}: fake worker stays running while the e2e opens browser waits`,
    acceptance: [{ type: "command", cmd: "true" }],
    status: "draft",
  });
  expect(created.status, created.text).toBe(201);
  const id = created.json.id ?? created.json.task?.id;
  if (!id) throw new Error(`POST /tasks returned no id: ${created.text}`);
  const policy = await api("PUT", `/tasks/${id}/browser/policy`, POLICY);
  expect(policy.status, policy.text).toBe(200);
  const approved = await api("POST", `/tasks/${id}/approve`, {});
  expect(approved.status, approved.text).toBe(200);
  const runId = await waitRunning(id);
  return { id, runId };
}

async function openWait(
  taskId: string,
  body: Record<string, unknown>,
): Promise<{ wait: BrowserWait; created: boolean }> {
  const r = await api<{ wait: BrowserWait; created: boolean }>("POST", `/tasks/${taskId}/browser/requests`, {
    origin: ORIGIN,
    policy_revision: POLICY.revision,
    policy_hash: POLICY_HASH,
    owner_id: "owner",
    resume_key: `g14-${randomBytes(6).toString("hex")}`,
    ...body,
  });
  expect(r.status, r.text).toBe(201);
  expect(r.json.created).toBe(true);
  return r.json;
}

async function expectStatus(id: string, re: RegExp, timeout = 30_000): Promise<void> {
  await expect.poll(async () => (await detail(id)).task.status, { timeout, intervals: [250, 500, 1000] }).toMatch(re);
}

async function shot(page: Page, name: string): Promise<void> {
  await page.screenshot({ path: path.join(ARTIFACTS, name), fullPage: true });
}

async function login(page: Page): Promise<void> {
  await page.goto(`${GUI}/login`);
  await page.fill("#password", password);
  await page.click('[data-testid="login-submit"]');
  await expect(page).not.toHaveURL(/\/login/);
}

function waitCard(page: Page, reason: string) {
  return page.locator(`[data-testid="browser-wait"][data-wait-reason="${reason}"]`).filter({ hasText: "pending" });
}

// ---- 共有状態（serial） ----
let browser: Browser;
let owner: BrowserContext;
let page: Page;
const tasks: string[] = [];
let taskA = "";
let credentialRef: BrowserWait["credential"];

// serial にしない: (c)（relay が無い間は 503 で落ちる）が落ちても (d) を必ず回す。worker は 1 つなので順序は保たれる
// （失敗の後は新しい worker で beforeAll からやり直すので、(d) は task を markers と API の一覧から集める）。
test.setTimeout(180_000);
// Credential input must never enter traces, HAR or videos, including failed runs.
test.use({ trace: "off", video: "off", screenshot: "off" });

test.beforeAll(async ({ browser: b }) => {
  env("G14_RUN_DIR");
  env("G14_SENTINEL");
  env("CELERIS_API_URL");
  env("CELERIS_GUI_BIND");
  env("G14_OWNER_SOCKET");
  env("G14_CELERISCTL");
  fs.mkdirSync(ARTIFACTS, { recursive: true });
  token = fs.readFileSync(path.join(RUN_DIR, "api.token"), "utf8").trim();
  password = fs.readFileSync(path.join(RUN_DIR, "gui-password"), "utf8").trim();
  sentinel = env("G14_SENTINEL");
  if (!/^SENTINEL-[0-9a-f]{24}$/.test(sentinel)) throw new Error("G14_SENTINEL has an unexpected shape");
  browser = b;
  owner = await browser.newContext();
  page = await owner.newPage();
  await login(page);
});

test.afterAll(async () => {
  await owner?.close();
});

test("(a) 未登録 credential: WAITING_FOR_AUTH → 本人の登録 → 手動登録 → Ready に戻り新しい run で再開", async () => {
  const a = await createRunningTask("g14 browser auth + approval");
  taskA = a.id;
  tasks.push(a.id);
  const opened = await openWait(a.id, {
    run_id: a.runId,
    session_id: "g14-session-auth",
    reason: "waiting_for_auth",
    purpose: PURPOSE_AUTH,
    credential_policy_id: CREDENTIAL_POLICY,
  });
  await expectStatus(a.id, /^blocked$/);

  await page.goto(`${GUI}/tasks/${a.id}`);
  const card = waitCard(page, "waiting_for_auth");
  await expect(card).toBeVisible({ timeout: 15_000 });
  await expect(card).toContainText("WAITING_FOR_AUTH");
  await expect(card).toContainText(ORIGIN);
  await expect(card).toContainText(PURPOSE_AUTH);
  // 本人として登録する前はフォームを出さない
  await expect(page.locator('[data-testid="browser-credential-form"]')).toHaveCount(0);
  await shot(page, "01-waiting-for-auth.png");

  // 本人 session の登録（GUI が challenge を出し、ローカルの celerisctl が owner socket で確定する）
  const request = page.locator('[data-testid="browser-owner-request"]');
  await expect(request).toBeVisible();
  await request.getByRole("button", { name: "このセッションを本人として登録" }).click();
  const code = request.locator("code");
  await expect(code).toContainText(/owner-session approve [0-9A-F]{12}/, { timeout: 15_000 });
  const challenge = /approve ([0-9A-F]{12})/.exec((await code.textContent()) ?? "")?.[1];
  if (!challenge) throw new Error("no owner challenge shown");
  await shot(page, "02-owner-challenge.png");
  const out = execFileSync(
    env("G14_CELERISCTL"),
    [
      "--db",
      path.join(RUN_DIR, "celeris.sqlite3"),
      "browser",
      "owner-session",
      "approve",
      challenge,
      "--socket",
      env("G14_OWNER_SOCKET"),
    ],
    { encoding: "utf8" },
  );
  expect(out).toContain("approved");
  await page.reload();
  const form = page.locator('[data-testid="browser-credential-form"]');
  await expect(form).toBeVisible({ timeout: 15_000 });
  await expect(page.locator('[data-testid="browser-owner-request"]')).toHaveCount(0);
  await shot(page, "03-owner-approved.png");

  // 手動登録（パスワードは sentinel）
  await form.locator('input[name="username"]').fill("g14-user");
  await form.locator('input[name="password"]').fill(sentinel);
  const [registerRes] = await Promise.all([
    page.waitForResponse((r) => r.request().method() === "POST" && /\/browser\/waits\/[^/]+\/credential/.test(r.url())),
    form.getByRole("button", { name: "登録する" }).click(),
  ]);
  // BFF の応答は固定コードだけ（入力を反射しない）。成功後は loader の再検証で wait が registered になり、
  // フォーム（と、その中の結果表示）は消える。
  expect(registerRes.status()).toBe(200);
  // fetcher の応答は turbo-stream（React Router の single fetch）。固定コードの語だけを見る。
  expect(await registerRes.text()).toMatch(/"ok",true,"code","registered"/);
  await expect(page.locator('[data-testid="browser-credential-form"]')).toHaveCount(0, { timeout: 15_000 });
  await expect(page.locator('[data-testid="browser-wait"][data-wait-reason="waiting_for_auth"]')).toContainText(
    "registered",
  );
  await shot(page, "04-credential-registered.png");

  const after = (await waits(a.id)).find((w) => w.wait_id === opened.wait.wait_id);
  expect(after?.state).toBe("registered");
  credentialRef = after?.credential;
  expect(credentialRef?.policy_id).toBe(CREDENTIAL_POLICY);

  // Ready に戻り、dispatcher が新しい run を始める（fake ワーカーの 2 回目の起動）
  const resumedRun = await waitRunning(a.id, { notRun: a.runId, minInvocations: 2 });
  expect(resumedRun).not.toBe(a.runId);
  expect(workerInvocations(a.id)).toBeGreaterThanOrEqual(2);
  await page.reload();
  await expect(page.locator('[data-testid="browser-wait"][data-wait-reason="waiting_for_auth"]')).toContainText(
    "registered",
  );
  await shot(page, "05-resumed.png");
});

test("(b) WAITING_FOR_APPROVAL: GUI で承認 → 再開、別の wait を GUI で拒否 → failed（approval_denied）", async () => {
  if (!taskA || !credentialRef) throw new Error("(a) did not complete; (b) needs the registered credential of task A");
  // 承認: task A の再開した run で、登録した credential の一回だけの使用承認を求める
  const d = await detail(taskA);
  const runA = activeRun(d);
  if (!runA) throw new Error(`task A is not running (status ${d.task.status})`);
  const before = workerInvocations(taskA);
  await openWait(taskA, {
    run_id: runA,
    session_id: "g14-session-auth",
    reason: "waiting_for_approval",
    purpose: PURPOSE_APPROVE,
    credential: credentialRef,
    operation: { intent_id: "g14-intent-login", action: "credential_use" },
  });
  await expectStatus(taskA, /^blocked$/);
  await page.goto(`${GUI}/tasks/${taskA}`);
  const card = waitCard(page, "waiting_for_approval");
  await expect(card).toBeVisible({ timeout: 15_000 });
  await expect(card).toContainText("WAITING_FOR_APPROVAL");
  await expect(card).toContainText(ORIGIN);
  await expect(card).toContainText(PURPOSE_APPROVE);
  await expect(card.locator('[data-testid="browser-approval-target"]')).toContainText("credential_use");
  await shot(page, "06-waiting-for-approval.png");
  const decision = card.locator('[data-testid="browser-decision-form"]');
  const [approveRes] = await Promise.all([
    page.waitForResponse((r) => r.request().method() === "POST" && /\/browser\/waits\/[^/]+\/decision/.test(r.url())),
    decision.getByRole("button", { name: "一回だけ承認" }).click(),
  ]);
  expect(approveRes.status()).toBe(200);
  expect(await approveRes.text()).toMatch(/"ok",true,"code","approved"/);
  await expect(page.locator('[data-testid="browser-decision-form"]')).toHaveCount(0, { timeout: 15_000 });
  await shot(page, "07-approved.png");
  await waitRunning(taskA, { notRun: runA, minInvocations: before + 1 });
  const approvedWait = (await waits(taskA)).find((w) => w.reason === "waiting_for_approval");
  expect(approvedWait?.state).toMatch(/^(approved|resumed)$/);
  await page.reload();
  await shot(page, "08-approval-resumed.png");

  // 拒否: 別の task の高リスク操作（click）の承認待ちを GUI で拒否する
  const b = await createRunningTask("g14 browser deny");
  tasks.push(b.id);
  const denyWait = await openWait(b.id, {
    run_id: b.runId,
    session_id: "g14-session-deny",
    reason: "waiting_for_approval",
    purpose: PURPOSE_DENY,
    operation: { intent_id: "g14-intent-publish", action: "click", args_digest: "sha256:g14publish" },
  });
  await expectStatus(b.id, /^blocked$/);
  await page.goto(`${GUI}/tasks/${b.id}`);
  const denyCard = waitCard(page, "waiting_for_approval");
  await expect(denyCard).toBeVisible({ timeout: 15_000 });
  await expect(denyCard).toContainText(PURPOSE_DENY);
  await shot(page, "09-deny-waiting.png");
  const denyForm = denyCard.locator('[data-testid="browser-decision-form"]');
  const [denyRes] = await Promise.all([
    page.waitForResponse((r) => r.request().method() === "POST" && /\/browser\/waits\/[^/]+\/decision/.test(r.url())),
    denyForm.getByRole("button", { name: "拒否" }).click(),
  ]);
  expect(denyRes.status()).toBe(200);
  expect(await denyRes.text()).toMatch(/"ok",true,"code","denied","task_status","failed"/);
  await expect(page.locator('[data-testid="browser-decision-form"]')).toHaveCount(0, { timeout: 15_000 });
  await shot(page, "10-denied.png");
  await expectStatus(b.id, /^failed$/);
  const denied = (await waits(b.id)).find((w) => w.wait_id === denyWait.wait.wait_id);
  expect(denied?.state).toBe("denied");
  expect(denied?.resolution_code).toBe("approval_denied");
  const events = await api<{ items: Array<{ event: Record<string, unknown> }> }>("GET", `/tasks/${b.id}/events`);
  expect(events.status, events.text).toBe(200);
  const transitionedToFailed = events.json.items.filter(
    (e) => e.event.type === "transitioned" && e.event.to === "failed",
  );
  expect(JSON.stringify(transitionedToFailed)).toContain("approval_denied");
  await page.reload();
  await shot(page, "11-task-failed.png");
});

test("(c) Live View: 本人は dashboard、本人でないログイン済みは 403、未認証は 401", async () => {
  const c = await createRunningTask("g14 browser live view");
  tasks.push(c.id);
  // dispatcher の browser supervisor の代わり: この run の `browser_updated`（RUNNING + https の live_view_url）を追記する
  const event = {
    type: "browser_updated",
    browser: {
      task_id: c.id,
      run_id: c.runId,
      session_id: "g14-session-live",
      state: "RUNNING",
      live_view_url: LIVE_VIEW_URL,
    },
  };
  const q = (s: string) => `'${s.replaceAll("'", "''")}'`;
  execFileSync("sqlite3", [
    "-cmd",
    ".timeout 10000",
    path.join(RUN_DIR, "celeris.sqlite3"),
    `INSERT INTO events (task_id, seq, ts, json) SELECT ${q(c.id)}, COALESCE(MAX(seq), -1) + 1, ` +
      `strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ${q(JSON.stringify(event))} FROM events WHERE task_id = ${q(c.id)};`,
  ]);
  const seeded = await api<{ items: Array<{ event: { browser?: { run_id: string; state: string } } }> }>(
    "GET",
    `/tasks/${c.id}/events?types=browser_updated`,
  );
  expect(seeded.status, seeded.text).toBe(200);
  expect(seeded.json.items.map((i) => `${i.event.browser?.run_id}:${i.event.browser?.state}`)).toContain(
    `${c.runId}:RUNNING`,
  );
  expect(activeRun(await detail(c.id))).toBe(c.runId);

  const livePath = `/browser/live/${c.id}/${c.runId}`;
  // The dashboard lists all sessions: another task's credential interval must close the entire relay.
  expect((await page.request.get(`${GUI}${livePath}`)).status()).toBe(409);
  const cancelled = await api("POST", `/tasks/${taskA}/cancel`, {});
  expect(cancelled.status, cancelled.text).toBe(200);
  await expectStatus(taskA, /^cancelled$/);
  await page.goto(`${GUI}/tasks/${c.id}`);
  const runs = page.locator('[data-testid="browser-runs"]');
  await expect(runs).toBeVisible({ timeout: 15_000 });
  await expect(runs).toContainText(c.runId);
  await shot(page, "12-live-view-panel.png");
  const link = runs.getByRole("link", { name: "Open Browser Live View" });
  const panelText = (await runs.innerText()).replace(/\s+/g, " ");
  expect.soft(await link.count(), `Live View link on the task page (panel: ${panelText})`).toBe(1);
  if ((await link.count()) === 1) expect.soft(await link.getAttribute("href")).toBe(livePath);

  // 本人: dashboard の HTML が GUI の origin で開く
  const livePage = await owner.newPage();
  const streams: WebSocket[] = [];
  let frames = 0;
  livePage.on("websocket", (ws) => {
    streams.push(ws);
    ws.on("framereceived", ({ payload }) => {
      if (typeof payload === "string" && payload.includes('"type":"frame"')) frames++;
      if (Buffer.isBuffer(payload) && payload.length > 1024) frames++;
    });
  });
  const ownerRes = await livePage.goto(`${GUI}${livePath}`);
  const ownerStatus = ownerRes?.status();
  const ownerBody = ((await ownerRes?.text()) ?? "").slice(0, 200).replace(/\s+/g, " ");
  await livePage.waitForTimeout(3_000);
  await livePage.screenshot({ path: path.join(ARTIFACTS, "live-view-owner.png"), fullPage: true });
  expect.soft(ownerStatus, `owner GET ${livePath} (body starts: ${ownerBody})`).toBe(200);
  expect.soft(ownerRes?.headers()["content-type"] ?? "", "owner Live View content-type").toContain("text/html");
  if (ownerStatus === 200) {
    // dashboard の画面が描画された（agent-browser の dashboard は開いている session の名前を一覧に出す）
    const sessions = Number(process.env.G14_DASHBOARD_SESSIONS ?? "0");
    const html = await livePage.content();
    expect.soft(html.length, "dashboard HTML rendered").toBeGreaterThan(500);
    expect.soft(html, "dashboard URL must not leak").not.toContain(`127.0.0.1:${process.env.G14_DASHBOARD_PORT}`);
    if (sessions > 0) {
      await expect.soft(livePage.getByText(/g14-live-/).first()).toBeVisible({ timeout: 20_000 });
      await expect.poll(() => frames, { timeout: 20_000 }).toBeGreaterThan(0);
      expect(streams.every((ws) => new URL(ws.url()).host === new URL(GUI).host)).toBe(true);
      await livePage.screenshot({ path: path.join(ARTIFACTS, "live-view-owner.png"), fullPage: true });
    }
  }
  // 別の run は 404、`POST /api/exec`（dashboard の操作 API）は 403
  const otherRun = await livePage.request.get(`${GUI}/browser/live/${c.id}/g14-no-such-run`, { maxRedirects: 0 });
  expect.soft(otherRun.status(), "owner GET for another run").toBe(404);
  const exec = await livePage.request.post(`${GUI}/api/exec`, {
    data: { command: "noop" },
    headers: { Origin: GUI },
    maxRedirects: 0,
  });
  expect.soft(exec.status(), `owner POST /api/exec (body: ${(await exec.text()).slice(0, 120)})`).toBe(403);

  // 本人でないログイン済みの context: 403
  const other = await browser.newContext();
  try {
    const otherPage = await other.newPage();
    await login(otherPage);
    const res = await otherPage.goto(`${GUI}${livePath}`);
    await otherPage.screenshot({ path: path.join(ARTIFACTS, "live-view-non-owner.png"), fullPage: true });
    expect.soft(res?.status(), `non-owner GET ${livePath} (body: ${((await res?.text()) ?? "").trim()})`).toBe(403);
  } finally {
    await other.close();
  }

  // 未認証の context: 401（/login への 302 ではない）
  const anon = await browser.newContext();
  try {
    const anonPage = await anon.newPage();
    const res = await anonPage.goto(`${GUI}${livePath}`);
    await anonPage.screenshot({ path: path.join(ARTIFACTS, "live-view-unauthenticated.png"), fullPage: true });
    expect.soft(anonPage.url(), "unauthenticated request must not be redirected").toBe(`${GUI}${livePath}`);
    expect
      .soft(res?.status(), `unauthenticated GET ${livePath} (body: ${((await res?.text()) ?? "").trim()})`)
      .toBe(401);
  } finally {
    await anon.close();
  }
  const activeStreams = streams.filter((ws) => !ws.isClosed());
  expect(activeStreams.length).toBeGreaterThan(0);
  const logout = await page.request.post(`${GUI}/logout`, { headers: { Origin: GUI }, maxRedirects: 0 });
  expect(logout.status()).toBe(302);
  await expect.poll(() => activeStreams.every((ws) => ws.isClosed())).toBe(true);
  expect((await page.request.get(`${GUI}${livePath}`, { maxRedirects: 0 })).status()).toBe(401);
  fs.writeFileSync(
    path.join(ARTIFACTS, "live-view-ws.json"),
    JSON.stringify({ frames, streams: streams.length, closedAfterLogout: true }),
  );
  await livePage.close();
});

test("(d) sentinel は DB・WAL・events API・daemon / GUI のログ・artifacts に無い", async () => {
  // events API の全件（全 task）を artifacts に残し、その dump も走査対象にする
  const dumps: string[] = [];
  const all = await api<{ items: Array<{ id?: string; task?: { id: string } }> }>("GET", "/tasks?limit=200");
  expect(all.status, all.text).toBe(200);
  const markerIds = fs.existsSync(path.join(RUN_DIR, "markers"))
    ? fs.readdirSync(path.join(RUN_DIR, "markers")).map((f) => f.split(".")[0])
    : [];
  const listed = all.json.items.map((t) => t.id ?? t.task?.id).filter((id): id is string => Boolean(id));
  const ids = new Set([...tasks, ...markerIds, ...listed]);
  expect(ids.size, "earlier tests must have created tasks").toBeGreaterThan(0);
  for (const id of ids) {
    const r = await api("GET", `/tasks/${id}/events`);
    expect(r.status, r.text).toBe(200);
    const w = await api("GET", `/tasks/${id}/browser/waits`);
    const file = path.join(ARTIFACTS, `events-${id}.json`);
    fs.writeFileSync(file, `${r.text}\n${w.text}\n`);
    dumps.push(file);
  }
  const inbox = await api("GET", "/browser/waits");
  fs.writeFileSync(path.join(ARTIFACTS, "browser-waits-inbox.json"), `${inbox.text}\n`);

  const needle = Buffer.from(sentinel, "utf8");
  const scanned: string[] = [];
  const hits: string[] = [];
  const walk = (p: string) => {
    if (!fs.existsSync(p)) return;
    const st = fs.lstatSync(p);
    if (st.isDirectory()) {
      for (const e of fs.readdirSync(p)) walk(path.join(p, e));
    } else if (st.isFile()) {
      scanned.push(p);
      if (fs.readFileSync(p).includes(needle)) hits.push(p);
    }
  };
  walk(RUN_DIR); // celeris.sqlite3 / -wal / -shm、celeris.log、gui.log、credentiald の vault、workspaces
  walk(ARTIFACTS);
  for (const must of ["celeris.sqlite3", "celeris.log", "gui.log"]) {
    expect(
      scanned.some((f) => path.basename(f) === must),
      `scan must cover ${must}`,
    ).toBe(true);
  }
  for (const f of dumps) expect(scanned).toContain(f);
  fs.writeFileSync(
    path.join(ARTIFACTS, "sentinel-scan.txt"),
    `scanned ${scanned.length} files (run dir + artifacts, incl. ${dumps.length} events dumps)\nhits: ${hits.length}\n${hits.join("\n")}\n`,
  );
  expect(hits, "the credential sentinel must not appear anywhere").toEqual([]);
});
