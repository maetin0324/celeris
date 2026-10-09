import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { startBrowserGateway } from "../support/browser-gateway";

const path = "/browser/settings";
const screenshotDir = process.env.BROWSER_SETTINGS_SCREENSHOT_DIR;
test.use({ bypassCSP: true });

async function captureWidths(page: import("@playwright/test").Page, stage: string) {
  if (!screenshotDir) return;
  mkdirSync(screenshotDir, { recursive: true });
  for (const width of [360, 390, 412, 1440]) {
    await page.setViewportSize({ width, height: 800 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth), `${stage} @${width}`).toBeLessThanOrEqual(
      width,
    );
    await page.screenshot({ path: join(screenshotDir, `settings-${stage}-${width}.png`), fullPage: true });
  }
}

test("loopback の初期値を表示し、確認して設定を保存する", async ({ page }) => {
  const gateway = await startBrowserGateway();
  try {
    await page.route("**/api/browser/site-policies", (route) =>
      route.fulfill({
        json: {
          items: [
            {
              policy_id: "billing-policy",
              exact_origin: "https://billing.example.com",
              login_url: "https://billing.example.com/login",
              password_selector: "#password",
              submit_selector: null,
              source: "api",
              created_at: "2026-10-08T00:00:00Z",
              updated_at: "2026-10-08T00:00:00Z",
            },
          ],
        },
      }),
    );
    await page.route("**/api/browser/readiness", (route) =>
      route.fulfill({ json: { items: [{ status: "OK", check: "ledger", detail: "適合台帳を確認済み" }] } }),
    );
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}${path}`);
    await expect(page.getByRole("heading", { level: 1, name: "ブラウザ実行課の設定" })).toBeVisible();
    const origins = page.getByRole("list", { name: "許可 origin の一覧" });
    await expect(origins.getByRole("listitem")).toHaveCount(2);
    await expect(origins).toContainText("http://localhost:3000");
    await expect(origins).toContainText("http://127.0.0.1:3000");
    await page.setViewportSize({ width: 360, height: 800 });
    expect(await seriousViolations(page)).toEqual([]);
    for (const button of await page.getByRole("button").all()) {
      if (!(await button.isVisible())) continue;
      const box = await button.boundingBox();
      expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
      expect(box?.width ?? 0).toBeGreaterThanOrEqual(44);
    }
    await captureWidths(page, "before");
    await page.getByLabel("追加する origin").fill("https://billing.example.com");
    await page.getByRole("button", { name: "追加", exact: true }).click();
    await page.getByRole("button", { name: "http://127.0.0.1:3000 を削除" }).click();
    await page.getByLabel("使える harness（カンマ区切り）").fill("codex, claude-code");
    await page.getByLabel("既定の harness").fill("codex");
    await page.getByLabel("最大試行回数").fill("3");
    // ADR 2026-10-08: 既定はどちらも承認なし。download だけ承認対象に戻す。
    const approvals = page.getByRole("list", { name: "毎回の承認が要る操作" });
    await expect(approvals.getByRole("checkbox")).toHaveCount(2);
    await expect(approvals.getByLabel(/^click/)).not.toBeChecked();
    await expect(approvals.getByLabel(/^download/)).not.toBeChecked();
    await approvals.getByLabel(/^download/).check();
    await page.getByLabel("credential の使用を許可（credential_use）").check();
    await page.getByLabel("billing-policy", { exact: true }).check();
    await page.getByLabel("policy と identity の対応（policy ID=identity ID）").fill("billing-policy=ID1");
    await page.getByRole("button", { name: "変更内容を確認して保存" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("許可 origin の縮小は既存 task の次の run");
    expect(gateway.daemon.browserSettingsEvents).toHaveLength(0);
    await dialog.getByRole("button", { name: "設定を保存" }).click();
    await expect(page.getByRole("status").filter({ hasText: "設定を保存しました" })).toBeVisible();
    await expect(origins).toContainText("https://billing.example.com");
    await expect(origins).not.toContainText("127.0.0.1");
    await expect(page.getByText("変更者: admin")).toBeVisible();
    await expect(approvals.getByLabel(/^download/)).toBeChecked();
    await captureWidths(page, "after");
    const event = gateway.daemon.browserSettingsEvents[0];
    expect(event?.actor).toBe("admin");
    expect(event?.ts).toBe("2026-10-06T01:23:45Z");
    expect(event?.after).toMatchObject({
      browser: {
        allowed_domains: ["http://localhost:3000", "https://billing.example.com"],
        approval_actions: ["download"],
        credential_policy_ids: ["billing-policy"],
        credential_identity_ids: { "billing-policy": "ID1" },
      },
      harnesses: { allowed: ["codex", "claude-code"], default: "codex" },
      budget: { max_attempts: 3, max_lane: "standard" },
    });
  } finally {
    await gateway.close();
  }
});

test("不正 scheme・全体 wildcard・public suffix wildcard は項目別に拒否する", async ({ page }) => {
  const gateway = await startBrowserGateway();
  try {
    await page.route("**/api/browser/site-policies", (route) =>
      route.fulfill({
        json: {
          items: [
            {
              policy_id: "billing-policy",
              exact_origin: "https://billing.example.com",
              login_url: "https://billing.example.com/login",
              password_selector: "#password",
              submit_selector: null,
              source: "api",
              created_at: "2026-10-08T00:00:00Z",
              updated_at: "2026-10-08T00:00:00Z",
            },
          ],
        },
      }),
    );
    await page.route("**/api/browser/readiness", (route) =>
      route.fulfill({ json: { items: [{ status: "OK", check: "ledger", detail: "適合台帳を確認済み" }] } }),
    );
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}${path}`);
    for (const value of ["ftp://example.com", "http://example.com", "*", "https://*.com", "https://*.co.uk"]) {
      await page.getByLabel("追加する origin").fill(value);
      await page.getByRole("button", { name: "追加", exact: true }).click();
      await page.getByRole("button", { name: "変更内容を確認して保存" }).click();
      await page.getByRole("alertdialog").getByRole("button", { name: "設定を保存" }).click();
      await expect(page.getByText("expected valid browser origins (scheme or wildcard)")).toBeVisible();
      expect(gateway.daemon.browserSettingsEvents).toHaveLength(0);
      await page.getByRole("alertdialog").getByRole("button", { name: "戻る" }).click();
      await page.getByRole("button", { name: `${value} を削除` }).click();
    }
  } finally {
    await gateway.close();
  }
});

test("site policy を追加・編集・削除し、credential grant を選択して保存する", async ({ page }) => {
  const gateway = await startBrowserGateway();
  let items: Array<Record<string, unknown>> = [];
  const writes: Array<{ method: string; body: Record<string, unknown> }> = [];
  let gets = 0;
  try {
    await page.route("**/api/browser/site-policies", (route) => {
      gets++;
      return route.fulfill({ json: { items } });
    });
    await page.route("**/browser/site-policies/*", async (route) => {
      const request = route.request();
      const body = request.postDataJSON();
      writes.push({ method: request.method(), body });
      if (request.method() === "DELETE") {
        items = [];
        return route.fulfill({ json: {} });
      }
      const { csrf: _csrf, ...fields } = body;
      items = [
        {
          ...fields,
          policy_id: "courses.login",
          source: "api",
          created_at: "2026-10-08T00:00:00Z",
          updated_at: "2026-10-08T00:00:00Z",
        },
      ];
      return route.fulfill({ json: { created: writes.length === 1, policy: items[0] } });
    });
    await page.route("**/api/browser/readiness", (route) =>
      route.fulfill({ json: { items: [{ status: "OK", check: "ledger", detail: "適合台帳を確認済み" }] } }),
    );
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}${path}`);
    await expect(page.getByText("ログイン先はまだ登録されていません。")).toBeVisible();
    await page.getByRole("button", { name: "ログイン先を追加" }).click();
    const form = page.getByRole("form", { name: "ログイン先の追加" });
    await form.getByLabel("policy ID", { exact: true }).fill("courses.login");
    await form.getByLabel("正確な origin（exact_origin）").fill("https://courses.example.com");
    await form.getByLabel("ログイン URL（login_url）").fill("https://courses.example.com/login");
    await form.getByLabel("password selector", { exact: true }).fill("#password");
    await form.getByRole("button", { name: "ログイン先を保存" }).click();
    await expect(page.getByRole("status").filter({ hasText: "courses.login を保存しました" })).toBeVisible();
    expect(writes[0]).toMatchObject({
      method: "PUT",
      body: {
        exact_origin: "https://courses.example.com",
        login_url: "https://courses.example.com/login",
        password_selector: "#password",
        submit_selector: null,
        csrf: expect.any(String),
      },
    });
    await page.getByRole("button", { name: "courses.login を編集" }).click();
    const editing = page.getByRole("form", { name: "ログイン先の編集" });
    await expect(editing.getByLabel("policy ID", { exact: true })).toHaveAttribute("readonly", "");
    await editing.getByLabel("ログイン URL（login_url）").fill("https://courses.example.com/new-login");
    await editing.getByLabel("submit selector（任意）").fill("#submit");
    // ADR 2026-10-09 credential username / post-login: username 欄とログイン後の読み取りの opt-in。
    await editing.getByLabel("username selector（任意。password 欄と同じ頁）").fill("#user");
    await editing.getByLabel("ログインした後、下の origin の頁を agent に読み取らせる").check();
    await editing
      .getByLabel("読み取り先 origin（1 行に 1 つ。ログイン先の origin は入れられません）")
      .fill("https://lms.example.com");
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 800 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width);
      expect(await seriousViolations(page)).toEqual([]);
      for (const target of await page
        .locator("button, input:not([type=checkbox]), textarea, select, label:has(input[type=checkbox])")
        .all()) {
        if (!(await target.isVisible())) continue;
        const box = await target.boundingBox();
        expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
        expect(box?.width ?? 0).toBeGreaterThanOrEqual(44);
      }
    }
    await captureWidths(page, "site-policy-edit");
    // 頁の内容が LLM に渡ることを確認するまで送らない。
    await editing.getByRole("button", { name: "ログイン先を保存" }).click();
    await expect(editing.getByRole("alert")).toContainText("LLM");
    expect(writes).toHaveLength(1);
    await editing.getByLabel(/LLM\s*とその提供元に渡ることを確認しました/).check();
    await editing.getByRole("button", { name: "ログイン先を保存" }).click();
    await expect(page.getByRole("list", { name: "ログイン先の一覧" })).toContainText("new-login");
    expect(writes[1]?.body).toMatchObject({
      login_url: "https://courses.example.com/new-login",
      submit_selector: "#submit",
      username_selector: "#user",
      post_login: { read_origins: ["https://lms.example.com"], actions: ["snapshot", "extract"] },
    });
    await page.getByLabel("credential の使用を許可（credential_use）").check();
    await page.getByLabel("courses.login", { exact: true }).check();
    await page.getByRole("button", { name: "変更内容を確認して保存" }).click();
    const grant = page.waitForRequest(
      (request) => request.url().endsWith("/browser/settings") && request.method() === "PATCH",
    );
    await page.getByRole("alertdialog").getByRole("button", { name: "設定を保存" }).click();
    expect((await grant).postDataJSON()).toMatchObject({
      credential_use: true,
      credential_policy_ids: ["courses.login"],
    });
    await expect(page.getByRole("status").filter({ hasText: "設定を保存しました" })).toBeVisible();
    // fixture は削除の参照制約を検査しないので、先に grant から外す。
    await page.getByLabel("courses.login", { exact: true }).uncheck();
    await page.getByLabel("credential の使用を許可（credential_use）").uncheck();
    await page.getByRole("button", { name: "変更内容を確認して保存" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "設定を保存" }).click();
    await expect(page.getByRole("alertdialog")).toHaveCount(0);
    await page.getByRole("button", { name: "courses.login を削除" }).click();
    expect(writes).toHaveLength(2);
    await page.getByRole("alertdialog").getByRole("button", { name: "courses.login を削除" }).click();
    await expect(page.getByText("ログイン先はまだ登録されていません。")).toBeVisible();
    expect(writes[2]).toMatchObject({ method: "DELETE", body: { csrf: expect.any(String) } });
    expect(gets).toBeGreaterThanOrEqual(4);
  } finally {
    await gateway.close();
  }
});
