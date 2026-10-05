import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, type Locator, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

// beforeAll の一時 dir・server をこの file の試験で共有するので、file 内は 1 worker で順に流す（2026-10-04 の並列化と同じ扱い）。
test.describe.configure({ mode: "default" });

// 設定系 ops（/providers・/accounts の secret・MCP）: 状態を文字で読めること、secret の値が DOM に出ないこと、
// form の label・項目の error・失敗時の focus、403 で操作を止めて理由を出すこと。
// 403 は fixture に経路が無いので、browser の要求を page.route で 403 にする（fixture は変えない）。

const SECRET_VALUE = "ops-config-s3cr3t-9876";

/** 入力欄の aria-describedby が error 文を指し、aria-invalid で、focus されていること。 */
async function expectFieldError(page: Page, input: Locator, text: string | RegExp) {
  await expect(input).toHaveAttribute("aria-invalid", "true");
  await expect(input).toBeFocused();
  const describedBy = await input.getAttribute("aria-describedby");
  expect(describedBy).toBeTruthy();
  await expect(page.locator(`[id="${describedBy}"]`)).toContainText(text);
}

test.describe("ops-config", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-ops-config-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  let gateway: Awaited<ReturnType<typeof startGateway>>;
  test.beforeAll(async () => {
    gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  });
  test.afterAll(async () => {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  });

  test("providers: 状態の表と card が状態を文字で示し、確認で変わる", async ({ page }) => {
    await page.goto(`${gateway.base}/providers`);
    await expect(page.getByRole("heading", { level: 1, name: "プロバイダ" })).toBeVisible();
    const table = page.getByRole("region", { name: "実行枠の状態" });
    const row = table.getByRole("row").filter({ hasText: "claude-main" });
    await expect(row.locator("[data-slot=badge]")).toHaveText("未確認");
    const card = page.getByRole("listitem", { name: "プロバイダ claude-main" });
    await expect(card.locator("[data-slot=badge]").first()).toHaveText("未確認");
    await card.getByRole("button", { name: "接続を確認" }).click();
    await expect(card.getByText("確認結果: ok")).toBeVisible();
    await expect(card.locator("[data-slot=badge]").first()).toHaveText("接続確認済み");
    await expect(row.locator("[data-slot=badge]")).toHaveText("接続確認済み");
  });

  test("providers: 追加 form は可視 label・項目の error・最初の error へ focus", async ({ page }) => {
    await page.goto(`${gateway.base}/providers`);
    const form = page.getByRole("form", { name: "プロバイダを追加" });
    const id = form.getByLabel("新規 id");
    const concurrency = form.getByLabel("新規の同時実行数");
    // 空の id と不正な concurrency: 先に並ぶ id へ focus。
    await concurrency.fill("-2");
    await form.getByRole("button", { name: "追加" }).click();
    await expectFieldError(page, id, "id を入力してください");
    await expect(concurrency).toHaveAttribute("aria-invalid", "true");
    // id を直すと concurrency の error へ focus が移る。
    await id.fill("codex-x");
    await form.getByRole("button", { name: "追加" }).click();
    await expectFieldError(page, concurrency, "0 以上の整数");
    // サーバの 422（重複）は id 欄に結び付けて focus を戻す。
    await concurrency.fill("");
    await id.fill("claude-main");
    await form.getByRole("button", { name: "追加" }).click();
    await expectFieldError(page, id, "id が不正か重複しています");
  });

  test("providers: 403 で変更操作を止め、理由を出す", async ({ page }) => {
    await page.route("**/api/providers/claude-main/check", (route) =>
      route.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: "forbidden" }) }),
    );
    await page.goto(`${gateway.base}/providers`);
    const card = page.getByRole("listitem", { name: "プロバイダ claude-main" });
    await card.getByRole("button", { name: "接続を確認" }).click();
    const notice = page.getByRole("alert").filter({ hasText: "この操作を行う権限がありません" });
    await expect(notice).toBeVisible();
    const noticeId = await notice.getAttribute("id");
    for (const name of ["変更を保存", "接続を確認", "削除"]) {
      const button = card.getByRole("button", { name });
      await expect(button).toBeDisabled();
      await expect(button).toHaveAttribute("aria-describedby", noticeId ?? "");
    }
    await expect(page.getByRole("button", { name: "追加" })).toBeDisabled();
  });

  test("secret: 値は DOM に出ず、置き換えは ConfirmDialog で影響を示し、削除の確認に影響を書く", async ({ page }) => {
    await page.goto(`${gateway.base}/accounts`);
    const form = page.getByRole("form", { name: "secret を設定" });
    // 空送信: 先に並ぶ id へ focus、値の欄にも error。
    await form.getByRole("button", { name: "secret を保存" }).click();
    await expectFieldError(page, form.getByLabel("secret id"), "secret id を入力してください");
    await expect(form.getByLabel("secret 値")).toHaveAttribute("aria-invalid", "true");
    await form.getByLabel("secret id").fill("CFG_KEY");
    await form.getByLabel("secret 値").fill(SECRET_VALUE);
    await form.getByRole("button", { name: "secret を保存" }).click();
    const item = page.getByRole("listitem", { name: "secret CFG_KEY" });
    await expect(item.locator("[data-slot=badge]")).toHaveText("保存あり");
    await expect(item.getByText("表示しません")).toBeVisible();
    await expect(item.getByText("更新日時")).toBeVisible();
    expect(await page.content()).not.toContain(SECRET_VALUE);
    // 既にある id は追加 form で上書きしない（置き換えへ案内）。
    await form.getByLabel("secret id").fill("CFG_KEY");
    await form.getByLabel("secret 値").fill("other");
    await form.getByRole("button", { name: "secret を保存" }).click();
    await expectFieldError(page, form.getByLabel("secret id"), "既にあります");
    await form.getByLabel("secret 値").fill("");
    // 置き換え: 空なら dialog を開かず項目の error、値を入れると ConfirmDialog で影響を示す。
    await item.getByRole("button", { name: "置き換え", exact: true }).click();
    const replaceInput = item.getByLabel("CFG_KEY の新しい値");
    await expect(replaceInput).toHaveAttribute("type", "password");
    await item.getByRole("button", { name: "値を置き換える" }).click();
    await expectFieldError(page, replaceInput, "新しい値を入力してください");
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await replaceInput.fill(`${SECRET_VALUE}-2`);
    await item.getByRole("button", { name: "値を置き換える" }).click();
    const dialog = page.getByRole("alertdialog", { name: "secret の値を置き換えますか" });
    await expect(dialog).toContainText("対象: secret CFG_KEY");
    await expect(dialog).toContainText("次の run から新しい値が使われます");
    await expect(dialog).toContainText("前の値には戻せません");
    const before = daemon.requests.filter((r) => r.path === "/api/v1/secrets/CFG_KEY" && r.method === "PUT").length;
    await dialog.getByRole("button", { name: "CFG_KEY を置き換える" }).click();
    await expect(dialog).toHaveCount(0);
    await expect
      .poll(() => daemon.requests.filter((r) => r.path === "/api/v1/secrets/CFG_KEY" && r.method === "PUT").length)
      .toBe(before + 1);
    expect(await page.content()).not.toContain(SECRET_VALUE);
    // 削除: 確認文に影響と戻せないことを書く。
    await item.getByRole("button", { name: "secret を削除" }).click();
    const deleteDialog = page.getByRole("alertdialog", { name: "secret を削除しますか" });
    await expect(deleteDialog).toContainText("影響する設定");
    await expect(deleteDialog).toContainText("元に戻せません");
    await deleteDialog.getByRole("button", { name: "CFG_KEY を削除" }).click();
    await expect(page.getByRole("listitem", { name: "secret CFG_KEY" })).toHaveCount(0);
  });

  test("secret: 403 で保存・置き換え・削除を止め、理由を出す", async ({ page }) => {
    await page.route("**/api/secrets/DENIED_KEY", (route) =>
      route.fulfill({ status: 403, contentType: "application/json", body: JSON.stringify({ error: "forbidden" }) }),
    );
    await page.goto(`${gateway.base}/accounts`);
    const form = page.getByRole("form", { name: "secret を設定" });
    await form.getByLabel("secret id").fill("DENIED_KEY");
    await form.getByLabel("secret 値").fill("x");
    await form.getByRole("button", { name: "secret を保存" }).click();
    const notice = page.getByRole("alert").filter({ hasText: "secret の保存・置き換え・削除は止めています" });
    await expect(notice).toBeVisible();
    const button = form.getByRole("button", { name: "secret を保存" });
    await expect(button).toBeDisabled();
    await expect(button).toHaveAttribute("aria-describedby", (await notice.getAttribute("id")) ?? "");
  });

  test("MCP: client の状態を文字の badge で示す", async ({ page }) => {
    await page.goto(`${gateway.base}/accounts`);
    const card = page.getByRole("listitem", { name: "MCP クライアント editor" });
    await expect(card.locator("[data-slot=badge]").first()).toHaveText("有効");
    await card.getByText("editor（有効）").click();
    await expect(card.getByText("最終利用")).toBeVisible();
    await expect(card.getByText(/tasks_list ok/)).toBeVisible();
    await expect(card.locator("[data-slot=badge]").filter({ hasText: "成功" })).toBeVisible();
  });
});
