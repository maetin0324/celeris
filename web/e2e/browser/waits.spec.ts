import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

for (const [decision, message, state] of [
  ["approve_once", "一回だけ承認済み", "approved"],
  ["deny", "拒否済み", "denied"],
] as const) {
  test(`decision ${decision} reaches the fixture and closes the wait`, async ({ page }) => {
    const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
    try {
      await gateway.loginAsOwner(page);
      await page.goto(`${gateway.base}/browser/runs/T1/R1`);
      const form = page.getByTestId("browser-decision-form");
      await expect(form).toBeVisible();
      await form.getByRole("button", { name: decision === "deny" ? "拒否" : "一回だけ承認" }).click();
      const card = page.getByTestId("browser-wait").filter({ hasText: "請求書フォームを送信する" });
      await expect(card).toHaveAttribute("data-wait-state", state);
      await expect(card).toContainText(message);
      const backend = gateway.daemon.browser;
      expect(backend?.records.waits.at(-1)).toMatchObject({
        task_id: "T1",
        wait_id: "W1",
        kind: "decision",
        body: { decision, expected_version: 1 },
      });
      const resolved = await page.request.get(`${gateway.base}/api/tasks/T1/browser/waits`);
      expect(resolved.status()).toBe(200);
      expect((await resolved.json()).items.find((wait: { wait_id: string }) => wait.wait_id === "W1").state).toBe(
        state,
      );
    } finally {
      await gateway.close();
    }
  });
}

test("credential reaches the fixture while the response and screen forget the password", async ({ page }) => {
  const gateway = await startBrowserGateway();
  const password = "browser-e2e-private-password";
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser/runs/T2/R2`);
    const form = page.getByTestId("browser-credential-form");
    await expect(form).toBeVisible();
    const input = form.getByLabel("パスワード");
    await form.getByLabel("ユーザー名").fill("browser-owner");
    await input.fill(password);
    const responsePromise = page.waitForResponse((response) => response.url().endsWith("/browser/waits/W2/credential"));
    await form.getByRole("button", { name: "登録する" }).click();
    const response = await responsePromise;
    expect(response.status()).toBe(200);
    expect(await response.text()).not.toContain(password);
    await expect(form).toHaveCount(0);
    const card = page.getByTestId("browser-wait").filter({ hasText: "社内ポータルにログインする" });
    await expect(card).toHaveAttribute("data-wait-state", "registered");
    await expect(card).toContainText("登録済み・使用は未承認");
    expect(await page.content()).not.toContain(password);
    expect(gateway.daemon.browser?.records.waits.at(-1)).toMatchObject({
      task_id: "T2",
      wait_id: "W2",
      kind: "credential",
      body: { username: "browser-owner", password, expected_version: 1 },
    });
  } finally {
    await gateway.close();
  }
});
