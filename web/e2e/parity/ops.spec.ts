import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createApp } from "../../server/app.js";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

// P4-16 /releases。
test.describe("P4-16 releases", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-releases-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  let daemonUrl: string;
  let gateway: Awaited<ReturnType<typeof startGateway>>;
  test.beforeAll(async () => {
    daemonUrl = await daemon.start();
    gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
  });
  test.afterAll(async () => {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  });
  const control = (body: { mode?: "succeed" | "fail"; pendingMs?: number }) =>
    fetch(`${daemonUrl}/__fake/releases`, {
      method: "POST",
      headers: { authorization: `Bearer ${FIXTURE_TOKEN}` },
      body: JSON.stringify(body),
    });

  // 昇格・巻き戻しは確認表示を挟む。一覧のボタンで開き、確認表示の同じ名前のボタンで確定する。
  const confirmPromote = async (page: Page, name: string) => {
    await page.getByRole("button", { name, exact: true }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("本番の daemon");
    await dialog.getByRole("button", { name, exact: true }).click();
  };

  test("parity: /releases 昇格と失敗表示 保留は成功と出さず、結果で成功になる", async ({ page }) => {
    await control({ mode: "succeed", pendingMs: 2500 });
    await page.goto(`${gateway.base}/releases`);
    await expect(page.getByRole("heading", { level: 1, name: "リリース" })).toBeVisible();
    await confirmPromote(page, "bbbbbbbbbbbb を昇格する");
    await expect(page.getByTestId("promote-pending")).toBeVisible();
    await expect(page.getByTestId("promote-succeeded")).toHaveCount(0);
    await expect(page.getByTestId("promote-succeeded")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("promote-pending")).toHaveCount(0);
    await expect(page.getByTestId("release-bbbbbbbbbbbb")).toContainText("current");
  });

  test("parity: /releases 昇格と失敗表示 失敗を表示する", async ({ page }) => {
    await control({ mode: "fail", pendingMs: 1000 });
    await page.goto(`${gateway.base}/releases`);
    await confirmPromote(page, "aaaaaaaaaaaa に巻き戻す");
    await expect(page.getByTestId("promote-pending")).toBeVisible();
    await expect(page.getByTestId("promote-failed")).toContainText("exit 1", { timeout: 15_000 });
    await expect(page.getByTestId("promote-succeeded")).toHaveCount(0);
  });

  test("parity: /releases 昇格と失敗表示 昇格中に gateway が入れ替わっても結果を出す", async ({ page }) => {
    test.setTimeout(60_000);
    await control({ mode: "succeed", pendingMs: 4000 });
    const first = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    await page.goto(`${first.base}/releases`);
    await confirmPromote(page, "aaaaaaaaaaaa に巻き戻す");
    await expect(page.getByTestId("promote-pending")).toBeVisible();
    await first.close();
    await expect(page.getByTestId("promote-pending")).toContainText("再接続", { timeout: 10_000 });
    await expect(page.getByTestId("promote-succeeded")).toHaveCount(0);
    // 同じ port で入れ替わった gateway を起こす。
    const server: http.Server = createApp({ log: () => {}, daemonUrl, daemonTokenFile: tokenFile }).listen(
      first.port,
      "127.0.0.1",
    );
    await new Promise<void>((resolve, reject) => {
      server.once("listening", resolve);
      server.once("error", reject);
    });
    try {
      await expect(page.getByTestId("promote-succeeded")).toBeVisible({ timeout: 30_000 });
    } finally {
      await new Promise<void>((resolve) => {
        server.close(() => resolve());
        server.closeAllConnections();
      });
    }
  });
});

// P4-12: 自己完結の describe（import は動的。/releases の describe と import を共有しない）。
test.describe("P4-12 daemon/providers", () => {
  type Stop = () => Promise<void>;
  let base = "";
  let stop: Stop = async () => {};
  let seen: Array<{ path: string; method?: string }> = [];
  test.beforeAll(async () => {
    const fs = await import("node:fs");
    const os = await import("node:os");
    const nodePath = await import("node:path");
    const { FIXTURE_TOKEN } = await import("../../scripts/check-secrets.mjs");
    const { createFakeDaemon } = await import("../support/fake-daemon.mjs");
    const { startGateway } = await import("../support/gateway");
    const dir = fs.mkdtempSync(nodePath.join(os.tmpdir(), "celeris-ops-dp-"));
    const tokenFile = nodePath.join(dir, "token");
    fs.writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
    const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
    base = gateway.base;
    seen = daemon.requests;
    stop = async () => {
      await gateway.close();
      await daemon.close();
      fs.rmSync(dir, { recursive: true, force: true });
    };
  });
  test.afterAll(async () => {
    await stop();
  });

  test("parity: /daemon 状態・replay", async ({ page }) => {
    await page.goto(`${base}/daemon`);
    await expect(page.getByRole("heading", { level: 1, name: "daemon" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "状態" })).toBeVisible();
    await page.getByRole("button", { name: "replay を実行" }).click();
    await expect(page.getByText("0 mismatches across 3 tasks")).toBeVisible();
    expect(seen.some((r) => r.path === "/api/v1/replay" && r.method === "POST")).toBe(true);
  });

  test("parity: /providers 追加・変更・削除・確認", async ({ page }) => {
    await page.goto(`${base}/providers`);
    await expect(page.getByRole("heading", { level: 1, name: "プロバイダ" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "claude-main" })).toBeVisible();
    const qwenModel = page.getByRole("listitem", { name: "model qwen3-coder" });
    await expect(qwenModel.getByText("不明（価格の情報なし。0 円ではありません）")).toBeVisible();
    await expect(page.getByRole("listitem", { name: "deployment openai-compatible:qwen/qwen3-coder" })).toBeVisible();
    await page.getByLabel("新規 id").fill("codex-2");
    await page.getByLabel("adapter").selectOption("codex");
    await page.getByRole("button", { name: "追加" }).click();
    await expect(page.getByRole("heading", { name: "codex-2" })).toBeVisible();
    const card = page.getByRole("listitem", { name: "プロバイダ codex-2" });
    await card.getByLabel("concurrency").fill("4");
    await card.getByRole("button", { name: "変更を保存" }).click();
    await expect(card.getByText(/concurrency 4/)).toBeVisible();
    await card.getByRole("button", { name: "接続を確認" }).click();
    await expect(card.getByText("確認結果: ok")).toBeVisible();
    await card.getByRole("button", { name: "削除" }).click();
    await page
      .getByRole("alertdialog", { name: "実行枠を削除しますか" })
      .getByRole("button", { name: "codex-2 を削除" })
      .click();
    await expect(page.getByRole("heading", { name: "codex-2" })).toHaveCount(0);
    await expect.poll(() => seen.filter((r) => r.path === "/api/v1/reload").length).toBeGreaterThanOrEqual(3);
  });
});

// P4-13..15: accounts / secrets / mcp / clusters（自己完結の describe）。
test.describe("P4-13..15 accounts/clusters", () => {
  type Stop = () => Promise<void>;
  let base = "";
  let stop: Stop = async () => {};
  let seen: Array<{ path: string; method?: string; body?: string }> = [];
  test.beforeAll(async () => {
    const fs = await import("node:fs");
    const os = await import("node:os");
    const nodePath = await import("node:path");
    const { FIXTURE_TOKEN } = await import("../../scripts/check-secrets.mjs");
    const { createFakeDaemon } = await import("../support/fake-daemon.mjs");
    const { startGateway } = await import("../support/gateway");
    const dir = fs.mkdtempSync(nodePath.join(os.tmpdir(), "celeris-ops-ac-"));
    const tokenFile = nodePath.join(dir, "token");
    fs.writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
    const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
    base = gateway.base;
    seen = daemon.requests;
    stop = async () => {
      await gateway.close();
      await daemon.close();
      fs.rmSync(dir, { recursive: true, force: true });
    };
  });
  test.afterAll(async () => {
    await stop();
  });

  test("parity: /accounts 追加・ログイン・secret・削除", async ({ page }) => {
    await page.goto(`${base}/accounts`);
    await expect(page.getByRole("heading", { level: 1, name: "アカウント" })).toBeVisible();
    await page.getByLabel("アカウント id").fill("sub");
    await page.getByRole("button", { name: "アカウントを追加" }).click();
    const card = page.getByRole("listitem", { name: "アカウント sub" });
    await expect(card).toBeVisible();
    await card.getByRole("button", { name: "確認" }).click();
    await expect(card.getByText("確認結果: ok")).toBeVisible();
    await card.getByRole("button", { name: "ログイン開始" }).click();
    await expect(card.getByText("user code: ABCD-1234")).toBeVisible();
    // 取り直しでコード入力欄が消えない（サーバの login_pending と手元の state の両方）。
    await page.evaluate(() => window.dispatchEvent(new Event("focus")));
    await page.reload();
    const again = page.getByRole("listitem", { name: "アカウント sub" });
    await expect(again.getByRole("status").filter({ hasText: "ログイン待ち" })).toBeVisible();
    await again.getByLabel("認証コード").fill("code-xyz");
    await again.getByRole("button", { name: "コードを送る" }).click();
    await expect(again.getByText("ログイン済み")).toBeVisible();
    // secret: 値は password 入力で、送信後に空になり、URL・storage に残らない。
    const secretInput = page.getByLabel("secret 値");
    await expect(secretInput).toHaveAttribute("type", "password");
    await expect(secretInput).toHaveAttribute("autocomplete", "new-password");
    await page.getByLabel("secret id").fill("OPENAI_KEY");
    await secretInput.fill("s3cr3t-value-123");
    await page.getByRole("button", { name: "secret を保存" }).click();
    await expect(page.getByRole("listitem", { name: "secret OPENAI_KEY" })).toBeVisible();
    await expect(secretInput).toHaveValue("");
    expect(page.url()).not.toContain("s3cr3t");
    const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }]));
    expect(stored).not.toContain("s3cr3t");
    await expect(page.getByText("s3cr3t-value-123")).toHaveCount(0);
    await expect(page.getByText("claude-oauth")).toBeVisible();
    await page
      .getByRole("listitem", { name: "secret OPENAI_KEY" })
      .getByRole("button", { name: "secret を削除" })
      .click();
    await page
      .getByRole("alertdialog", { name: "secret を削除しますか" })
      .getByRole("button", { name: "OPENAI_KEY を削除" })
      .click();
    await expect(page.getByRole("listitem", { name: "secret OPENAI_KEY" })).toHaveCount(0);
    await card.getByRole("button", { name: "削除" }).click();
    await page
      .getByRole("alertdialog", { name: "アカウントを削除しますか" })
      .getByRole("button", { name: "sub を削除" })
      .click();
    await expect(page.getByRole("listitem", { name: "アカウント sub" })).toHaveCount(0);
    expect(seen.some((r) => r.path.startsWith("/api/v1/accounts/sub/login/code") && r.method === "POST")).toBe(true);
  });

  test("parity: mcp/clients/:id/calls 取得", async ({ page }) => {
    await page.goto(`${base}/accounts`);
    const card = page.getByRole("listitem", { name: "MCP クライアント editor" });
    await expect(card).toBeVisible();
    const before = seen.filter((r) => r.path.includes("/mcp/clients/c1/calls")).length;
    expect(before).toBe(0);
    await card.getByText("editor（有効）").click();
    await expect(card.getByText(/tasks_list ok/)).toBeVisible();
    expect(seen.some((r) => r.path === "/api/v1/mcp/clients/c1/calls" && r.method === "GET")).toBe(true);
  });

  test("parity: /clusters 接続・作業ディレクトリ", async ({ page }) => {
    await page.goto(`${base}/clusters`);
    await expect(page.getByRole("heading", { level: 1, name: "クラスタ" })).toBeVisible();
    const card = page.getByRole("listitem", { name: "クラスタ pegasus" });
    await card.getByRole("button", { name: "接続" }).click();
    await expect(card.getByText("コード待ち: Verification code:")).toBeVisible();
    await page.reload();
    const again = page.getByRole("listitem", { name: "クラスタ pegasus" });
    await expect(again.getByText("コード待ち")).toBeVisible();
    await again.getByLabel("接続コード", { exact: true }).fill("123456");
    await again.getByRole("button", { name: "コードを送る" }).click();
    await expect(again.getByText("接続中")).toBeVisible();
    await again.getByLabel("作業ディレクトリ").fill("/work/me");
    await again.getByRole("button", { name: "作業ディレクトリを保存" }).click();
    await expect(again.getByText("/work/me（db）")).toBeVisible();
    await again.getByRole("button", { name: "上書きを消す" }).click();
    await expect(again.getByText("/work/me（db）")).toHaveCount(0);
    expect(seen.some((r) => r.path === "/api/v1/clusters/pegasus/settings" && r.method === "PUT")).toBe(true);
  });
});
