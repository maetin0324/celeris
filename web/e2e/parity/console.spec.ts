/// <reference types="node" />
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { QueryClient } from "@tanstack/react-query";
import type { ConsoleBlock } from "../../api/generated/types";
import { applyStreamBlock, type ConsoleCache, consoleQueryKey } from "../../features/console/cache";
import { type EventSourceLike, openConsoleStream } from "../../features/console/stream";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createApp } from "../../server/app.js";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { NodeEventSource } from "../support/node-event-source";

// Console（P3-01・P3-02）。実 gateway と偽 daemon（loopback の空き port）。外部ネットワークには出ない。
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-console-"));
const tokenFile = path.join(dir, "token");
const passwordFile = path.join(dir, "password");
writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
writeFileSync(passwordFile, "pw\n");

const human = (cursor: string, text = cursor, node = "cos"): ConsoleBlock => ({
  kind: "human",
  at: "2026-01-01T00:00:00Z",
  cursor,
  message_id: `h${cursor}`,
  node_id: node,
  text,
});
const reply = (cursor: string, text: string): ConsoleBlock => ({
  kind: "reply",
  at: "2026-01-01T00:00:01Z",
  cursor,
  message_id: `r${cursor}`,
  node_id: "cos",
  text,
  state: "done",
});
const progress = (): ConsoleBlock => ({
  kind: "progress",
  at: "2026-01-01T00:00:02Z",
  cursor: "p1",
  title: "作業",
  tier: "standard",
  progress: {
    count: 5,
    first: [],
    last: [],
    run_id: "R1",
    started_at: "2026-01-01T00:00:00Z",
    task_id: "T1",
    tool_count: 0,
    updated_at: "2026-01-01T00:00:03Z",
  },
});

const runEvents = (url: URL) => {
  const after = Number(url.searchParams.get("after_seq") ?? 0);
  const limit = Number(url.searchParams.get("limit") ?? 200);
  const all = Array.from({ length: 5 }, (_, i) => i + 1);
  const rest = all.filter((n) => n > after).slice(0, limit);
  return {
    has_more: rest.length > 0 && rest.at(-1) !== 5,
    items: rest.map((n) => ({
      id: n,
      seq: n,
      task_id: "T1",
      ts: "2026-01-01T00:00:00Z",
      event: { type: "worker_progress", kind: "status", msg: `line-${n}` },
    })),
  };
};

let initial: ConsoleBlock[] = [];
const daemon = createFakeDaemon({
  token: FIXTURE_TOKEN,
  fixtures: {
    "/api/v1/console": () => ({ items: initial, next_cursor: "n0" }),
    "/api/v1/tasks/T1/runs/R1/events": runEvents,
    "/api/v1/org": { items: [] },
  },
});
let server: http.Server;
let authed: http.Server;
let base: string;
let authedBase: string;

async function listen(app: http.RequestListener): Promise<[http.Server, string]> {
  const s = (await import("node:http")).createServer(app).listen(0, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    s.once("listening", resolve);
    s.once("error", reject);
  });
  const address = s.address();
  if (!address || typeof address === "string") throw new Error("no port");
  return [s, `http://127.0.0.1:${address.port}`];
}

test.beforeAll(async () => {
  const daemonUrl = await daemon.start();
  [server, base] = await listen(createApp({ daemonUrl, daemonTokenFile: tokenFile, log: () => {} }));
  [authed, authedBase] = await listen(
    createApp({ daemonUrl, daemonTokenFile: tokenFile, passwordFile, log: () => {} }),
  );
});
test.afterAll(async () => {
  for (const s of [server, authed]) {
    s.closeAllConnections();
    await new Promise<void>((resolve) => s.close(() => resolve()));
  }
  await daemon.close();
  rmSync(dir, { recursive: true, force: true });
});
test.beforeEach(() => {
  initial = [];
});

const waitFor = async (fn: () => boolean, ms = 5000) => {
  const end = Date.now() + ms;
  while (!fn() && Date.now() < end) await new Promise((r) => setTimeout(r, 20));
  expect(fn()).toBe(true);
};
const requests = (p: string) => daemon.requests.filter((r) => r.path === p);

test("parity: /console/stream 中継と再接続", async () => {
  test.setTimeout(30_000);
  expect((await fetch(`${authedBase}/console/stream?scope=all`, { redirect: "manual" })).status).toBe(401);

  const client = new QueryClient();
  const key = consoleQueryKey("node:cos", "0");
  const stream = openConsoleStream({
    scope: "node:cos",
    since: "s1",
    retryMs: 50,
    createSource: (url) => new NodeEventSource(`${base}${url}`) as unknown as EventSourceLike,
    onBlock: (block) => applyStreamBlock(client, key, block),
  });
  await waitFor(() => daemon.consoleClients === 1);
  expect(requests("/api/v1/console/stream").at(-1)?.query).toBe("?scope=node%3Acos&since=s1");
  expect(requests("/api/v1/console/stream").at(-1)?.authorization).toBe(`Bearer ${FIXTURE_TOKEN}`);

  daemon.sendConsoleBlock(human("c1"));
  daemon.sendConsoleBlock(human("c1")); // 重複
  daemon.sendConsoleBlock(reply("c2", "done"));
  await waitFor(() => client.getQueryData<ConsoleCache>(key)?.blocks.length === 2);

  daemon.dropConsoleClients();
  await waitFor(() => requests("/api/v1/console/stream").length >= 2 && daemon.consoleClients === 1);
  expect(requests("/api/v1/console/stream").at(-1)?.query).toBe("?scope=node%3Acos&since=c2"); // 続きから
  daemon.sendConsoleBlock(human("c3"));
  await waitFor(() => client.getQueryData<ConsoleCache>(key)?.blocks.length === 3);
  expect(client.getQueryData<ConsoleCache>(key)?.blocks.map((b) => b.cursor)).toEqual(["c1", "c2", "c3"]);
  stream.close();
});

test("parity: /console/new-conversation 開始", async () => {
  const res = await fetch(`${base}/api/console/new-conversation`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: "{}",
  });
  expect(res.status).toBe(204);
  expect(requests("/api/v1/console/new-conversation").at(-1)?.method).toBe("POST");
  const unauth = await fetch(`${authedBase}/api/console/new-conversation`, { method: "POST", body: "{}" });
  expect(unauth.status).toBe(401);
});

test("parity: runs/:runId/events 全行の取得", async ({ page }) => {
  initial = [progress()];
  await page.goto(`${base}/`);
  await page.getByRole("button", { name: /作業/ }).click();
  await page.getByRole("button", { name: /すべて見る/ }).click();
  for (const n of [1, 2, 3, 4, 5]) await expect(page.getByText(`line-${n}`)).toBeVisible();
  expect(requests("/api/v1/tasks/T1/runs/R1/events").length).toBeGreaterThanOrEqual(1);
});

test("parity: / Console の送信・返事・IME", async ({ page }) => {
  initial = [human("a1", "こんにちは")];
  await page.goto(`${base}/`);
  await expect(page.getByText("こんにちは")).toBeVisible();
  const box = page.getByRole("textbox", { name: "Console への入力" });
  const send = page.getByRole("button", { name: "送信" });

  // IME 変換中は送信しない
  await box.fill("変換中");
  await box.evaluate((el) => {
    el.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    const e = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    el.dispatchEvent(e);
  });
  expect(requests("/api/v1/console/instruct")).toHaveLength(0);
  await box.evaluate((el) => el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true })));

  // 二重に出ない（202 まで pending）
  daemon.setPostDelay(400);
  await box.fill("hello");
  await send.click();
  await expect(send).toBeDisabled();
  await send.click({ force: true }).catch(() => {});
  await expect(box).toHaveValue("");
  await expect.poll(() => requests("/api/v1/console/instruct").length).toBe(1);
  daemon.setPostDelay(0);
  expect(JSON.parse(requests("/api/v1/console/instruct")[0]?.body ?? "{}")).toEqual({ text: "hello" });

  // 返事が積み上がる
  await expect.poll(() => daemon.consoleClients).toBeGreaterThan(0);
  daemon.sendConsoleBlock(reply("a2", "了解です"));
  await expect(page.getByText("了解です")).toBeVisible();

  // 入力中の文は画面遷移で消えない。composer は下端に固定
  await box.fill("下書き");
  await page.getByRole("link", { name: /組織/ }).first().click();
  await page.goBack();
  await expect(page.getByRole("textbox", { name: "Console への入力" })).toHaveValue("下書き");
  const pos = await page.getByTestId("console-composer").evaluate((el) => getComputedStyle(el).position);
  expect(pos).toBe("fixed");
});

test("parity: /org/:id Console の送信と宛先", async ({ page }) => {
  await page.goto(`${base}/org/designer`);
  await expect(page.getByText("宛先: designer")).toBeVisible();
  await page.getByRole("textbox", { name: "Console への入力" }).fill("お願い");
  await page.getByRole("button", { name: "送信" }).click();
  await expect.poll(() => requests("/api/v1/console/instruct").length).toBeGreaterThan(0);
  expect(JSON.parse(requests("/api/v1/console/instruct").at(-1)?.body ?? "{}")).toEqual({
    text: "お願い",
    scope: "node:designer",
  });
  expect(requests("/api/v1/console/stream").at(-1)?.query).toContain("scope=node%3Adesigner");
});

test("parity: /tasks/:id/runs/:runId 360px で長い 1 行がページを広げない", async ({ page }) => {
  // run 画面の本文（stdout.jsonl）と Console の run events の両方に 2000 文字以上の 1 行を返す。
  // この test 専用の偽 daemon を立て、他の test の fixture には触れない。
  const long = "x".repeat(2400);
  const line = (text: string) =>
    JSON.stringify({ type: "assistant", message: { role: "assistant", content: [{ type: "text", text }] } });
  const toolResult = JSON.stringify({
    type: "user",
    message: { role: "user", content: [{ type: "tool_result", tool_use_id: "t1", content: long }] },
  });
  const own = createFakeDaemon({
    token: FIXTURE_TOKEN,
    fixtures: {
      "/api/v1/tasks/T1/runs/R1/events": () => ({
        has_more: false,
        items: [
          { id: 1, seq: 1, task_id: "T1", ts: "2026-01-01T00:00:00Z", event: { type: "worker_progress", msg: long } },
        ],
      }),
    },
    files: {
      "/api/v1/tasks/T1/runs/R1/stdout.jsonl": {
        body: `${line(long)}\n${toolResult}\n${long}\n`,
        type: "text/plain; charset=utf-8",
      },
    },
  });
  const [s, url] = await listen(createApp({ daemonUrl: await own.start(), daemonTokenFile: tokenFile, log: () => {} }));
  try {
    await page.setViewportSize({ width: 360, height: 740 });
    await page.goto(`${url}/tasks/T1/runs/R1`);
    await expect(page.getByRole("heading", { level: 1, name: "run ログ T1 / R1" })).toBeVisible();
    await expect(page.getByTestId("run-log-line-count")).toHaveAttribute("data-count", "3");
    const fits = () => page.evaluate(() => document.documentElement.scrollWidth);
    expect(await fits()).toBeLessThanOrEqual(360);
    // 折り返しを切っても、横 scroll は面の内側だけ。
    const wrapToggle = page.getByRole("button", { name: "長い行を折り返す" });
    await wrapToggle.click();
    await expect(wrapToggle).toHaveAttribute("aria-pressed", "false");
    for (const summary of await page.locator("[data-testid=run-log] summary").all()) await summary.click();
    expect(await fits()).toBeLessThanOrEqual(360);
    // 原文（LogSurface）でも同じ。
    await page.getByRole("button", { name: /原文/ }).click();
    await expect(page.getByRole("region", { name: "run ログ本文（原文）" })).toBeVisible();
    expect(await fits()).toBeLessThanOrEqual(360);
  } finally {
    s.closeAllConnections();
    await new Promise<void>((resolve) => s.close(() => resolve()));
    await own.close();
  }
});

test("parity: / Console 360px で長い 1 行がページを広げず、種類が文字で分かる", async ({ page }) => {
  const long = "y".repeat(2400);
  initial = [
    human("l1", long),
    reply("l2", `前置き\n\`\`\`\n${long}\n\`\`\``),
    {
      ...progress(),
      progress: { ...progress().progress, first: [{ seq: 1, at: "2026-01-01T00:00:00Z", text: long }] },
    },
  ];
  await page.setViewportSize({ width: 360, height: 740 });
  await page.goto(`${base}/`);
  const conversation = page.getByRole("list", { name: "Console の会話" });
  await expect(conversation.getByText("あなた", { exact: true })).toBeVisible();
  await expect(conversation.getByText("返事", { exact: true })).toBeVisible();
  await expect(conversation.getByText("作業中の run", { exact: true })).toBeVisible();
  // 返事の ``` は等幅の code 面（CodeBlock）で出す。
  await expect(page.getByRole("region", { name: "返事の code" })).toBeVisible();
  const fits = () => page.evaluate(() => document.documentElement.scrollWidth);
  expect(await fits()).toBeLessThanOrEqual(360);
  // progress を開くと先頭・末尾の行は LogSurface、全行も LogSurface。
  await page.getByRole("button", { name: /作業/ }).click();
  await expect(page.getByRole("region", { name: "run の先頭と末尾の行" })).toBeVisible();
  await page.getByRole("button", { name: /すべて見る/ }).click();
  await expect(page.getByRole("region", { name: "run の全行" })).toContainText("line-5");
  expect(await fits()).toBeLessThanOrEqual(360);
});

test("parity: / Console 追記の追従と『最新へ』", async ({ page }) => {
  initial = Array.from({ length: 30 }, (_, i) => human(`f${i}`, `発言 ${i}`));
  await page.goto(`${base}/`);
  await expect(page.getByText("発言 29")).toBeVisible();
  await expect.poll(() => daemon.consoleClients).toBeGreaterThan(0);
  const gap = () => page.evaluate(() => document.documentElement.scrollHeight - window.scrollY - window.innerHeight);
  // 開いた直後は末尾にいる。
  await expect.poll(gap).toBeLessThanOrEqual(24);
  // 上へ離れている間は追記で動かさず、「最新へ」を出す。
  await page.evaluate(() => window.scrollTo(0, 0));
  // scroll の event が届いてから追記する（2 frame 待つ）。
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  daemon.sendConsoleBlock(reply("f30", "追記 1"));
  const latest = page.getByRole("button", { name: "最新へ" });
  await expect(latest).toBeVisible();
  expect(await page.evaluate(() => window.scrollY)).toBe(0);
  await latest.click();
  await expect.poll(gap).toBeLessThanOrEqual(24);
  await expect(latest).toBeHidden();
  // 末尾にいれば追記に合わせて末尾へ送る。
  daemon.sendConsoleBlock(reply("f31", "追記 2\n".repeat(20)));
  await expect(page.getByText("追記 2").first()).toBeVisible();
  await expect.poll(gap).toBeLessThanOrEqual(24);
});
