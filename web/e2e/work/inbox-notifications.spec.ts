import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, inboxItemsFixture, noticesFixture } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

test.describe.configure({ mode: "default" });
const dir = makeTmpDir("celeris-work-inbox-");
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

test("多数の判断待ちは期限順で、一件の回答欄だけを開く", async ({ page }) => {
  const seed = inboxItemsFixture()[0];
  daemon.setInboxItems(
    Array.from({ length: 15 }, (_, index) => ({
      ...seed,
      id: `decision:work-${index}`,
      title: `判断 ${index}`,
      due_at: index === 14 ? "2026-10-05T01:00:00Z" : null,
    })),
  );
  await page.goto(`${gateway.base}/inbox`);
  const rows = page.locator("[data-inbox-item]");
  await expect(rows).toHaveCount(15);
  await expect(rows.first()).toHaveAttribute("data-inbox-item", "decision:work-14");
  await expect(page.getByRole("textbox", { name: /理由・note/ })).toHaveCount(0);

  await rows.first().getByRole("button", { name: "回答を開く" }).focus();
  await page.keyboard.press("Enter");
  await expect(rows.first().getByRole("button", { name: "回答を閉じる" })).toBeFocused();
  await expect(rows.first().getByRole("textbox", { name: /理由・note/ })).toBeVisible();
  await rows.nth(1).getByRole("button", { name: "回答を開く" }).click();
  await expect(rows.first().getByRole("textbox", { name: /理由・note/ })).toHaveCount(0);
  await expect(rows.nth(1).getByRole("textbox", { name: /理由・note/ })).toBeVisible();
});

test("Console は期限の近い判断を通知より先に示す", async ({ page }) => {
  const seed = inboxItemsFixture()[0];
  daemon.setInboxItems([{ ...seed, id: "decision:urgent", title: "期限のある判断", due_at: "2026-10-05T09:00:00Z" }]);
  await page.goto(`${gateway.base}/console`);
  const entries = page.getByRole("navigation", { name: "受信箱と通知" });
  await expect(entries.getByRole("list", { name: "期限の近い判断待ち" })).toContainText("期限のある判断");
  await expect(entries.getByRole("link", { name: /^受信箱/ })).toContainText("判断待ち 1 件");
});

test("通知の一括既読は未読があるとき枠付きで操作できる", async ({ page }) => {
  daemon.setNotices(noticesFixture());
  await page.goto(`${gateway.base}/notifications`);
  const button = page.getByRole("button", { name: "すべて既読にする" });
  await expect(button).toBeEnabled();
  await expect.poll(() => button.evaluate((element) => getComputedStyle(element).borderTopWidth)).toBe("1px");
});
