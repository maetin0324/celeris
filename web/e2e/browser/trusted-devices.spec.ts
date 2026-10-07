import { expect, request, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { startBrowserGateway } from "../support/browser-gateway";

// 信頼できる端末（ADR 2026-10-07-browser-trusted-devices）。CLI 承認で本人になった端末を画面から登録し、
// web の再起動（メモリの owner が消える）の後に読み込み直すと、challenge なしで本人に戻る。一覧から失効すると
// 状態が変わり、今使っている端末を失効させると本人のセッションも即座に終わる。端末の秘密は JS から見えない。

const DEVICE_COOKIE = "__celeris_web_device";

test.use({ bypassCSP: true }); // axe の注入だけに適用。配信する CSP は変更しない。

test("trusted_device: register, restart the web, resume without approval, then revoke", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    const backend = gateway.daemon.browser;
    if (!backend) throw new Error("browser backend is missing");
    const { csrf } = await gateway.loginAsOwner(page);
    // 別の端末を 1 台登録しておく（失効で一覧の状態が変わることを、本人のまま確かめる）。同じ login の
    // 別の cookie jar から登録し、この page の端末 cookie には触れない。
    const elsewhere = await request.newContext({ storageState: await page.context().storageState() });
    const other = await elsewhere.post(`${gateway.base}/browser/owner-session/device`, {
      headers: { Origin: gateway.base, "x-csrf-token": csrf },
      data: { name: "古い端末" },
    });
    await elsewhere.dispose();
    expect(other.status()).toBe(201);

    // 登録: CLI 承認の owner に「この端末を信頼する」form が出る。
    await page.goto(`${gateway.base}/browser/devices`);
    await expect(page.getByRole("heading", { level: 1, name: "信頼できる端末" })).toBeVisible();
    const form = page.getByTestId("browser-trust-device");
    await form.getByLabel("端末の名前").fill("手元のノート");
    await form.getByRole("button", { name: "この端末を信頼する" }).click();
    await expect(page.locator('[data-testid="browser-trusted-device"]', { hasText: "手元のノート" })).toBeVisible();
    await expect(page.getByTestId("browser-trust-device")).toHaveCount(0);
    expect(backend.records.devices.filter((r) => r.purpose === "device_register")).toHaveLength(2);
    // 端末の秘密は httpOnly cookie にあり、JS と画面には出ない。
    expect(await page.evaluate(() => document.cookie)).not.toContain(DEVICE_COOKIE);
    const cookie = (await page.context().cookies()).find((c) => c.name === DEVICE_COOKIE);
    expect(cookie?.httpOnly).toBe(true);
    expect(cookie?.sameSite).toBe("Strict");
    expect(cookie?.path).toBe("/browser/owner-session");
    const secret = cookie?.value.split(".")[1] ?? "";
    expect(secret.length).toBeGreaterThan(40);
    expect(await page.content()).not.toContain(secret);

    // web の再起動: メモリの owner は消えるが、読み込み直すと登録端末で自動に復帰する（challenge は出さない）。
    await gateway.restart();
    const verifiesBefore = backend.records.devices.filter((r) => r.purpose === "device_resume").length;
    await page.reload();
    await expect(page.getByRole("heading", { level: 1, name: "信頼できる端末" })).toBeVisible();
    const mine = page.locator('[data-testid="browser-trusted-device"]', { hasText: "手元のノート" });
    await expect(mine).toContainText("この端末");
    await expect(mine).toHaveAttribute("data-device-state", "active");
    await expect(mine).not.toContainText("未使用");
    await expect(page.getByTestId("browser-owner-request")).toHaveCount(0);
    expect(backend.records.devices.filter((r) => r.purpose === "device_resume").length).toBe(verifiesBefore + 1);
    // 回転: cookie の秘密は使うたびに替わる。
    const rotated = (await page.context().cookies()).find((c) => c.name === DEVICE_COOKIE);
    expect(rotated?.value.split(".")[1]).not.toBe(secret);

    expect(await seriousViolations(page)).toEqual([]);

    // 失効（ConfirmDialog）: 別の端末の状態が「失効済み」に変わり、本人のまま一覧を見られる。
    const old = page.locator('[data-testid="browser-trusted-device"]', { hasText: "古い端末" });
    await old.getByRole("button", { name: "古い端末 を失効" }).click();
    const dialog = page.getByRole("alertdialog").or(page.getByRole("dialog"));
    await expect(dialog).toContainText("古い端末");
    await dialog.getByRole("button", { name: "古い端末 を失効" }).click();
    await expect(old).toHaveAttribute("data-device-state", "revoked");
    await expect(old).toContainText("失効済み");
    await expect(old.getByRole("button", { name: "古い端末 を失効" })).toHaveCount(0);

    // 今の端末を失効させると、その端末から作った本人のセッションはすぐ終わり、challenge の案内に戻る。
    await mine.getByRole("button", { name: "手元のノート を失効" }).click();
    await dialog.getByRole("button", { name: "手元のノート を失効" }).click();
    await expect(page.getByTestId("browser-owner-request")).toBeVisible();
    expect(backend.devices.every((d) => d.revoked_reason === "owner")).toBe(true);
    // 失効した端末では、再起動の後も復帰しない。
    await gateway.restart();
    await page.reload();
    await expect(page.getByTestId("browser-owner-request")).toBeVisible();
  } finally {
    await gateway.close();
  }
});

test("trusted_device: a wrong device secret is rejected and the challenge notice explains it", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    const { csrf } = await gateway.loginAsOwner(page);
    const registered = await page.request.post(`${gateway.base}/browser/owner-session/device`, {
      headers: { Origin: gateway.base, "x-csrf-token": csrf },
      data: { name: "手元のノート" },
    });
    const { device } = (await registered.json()) as { device: { id: string } };
    await gateway.restart();
    const url = new URL(gateway.base);
    await page.context().addCookies([
      {
        name: DEVICE_COOKIE,
        value: `${device.id}.${"A".repeat(43)}`,
        domain: url.hostname,
        path: "/browser/owner-session",
        httpOnly: true,
        sameSite: "Strict",
      },
    ]);
    await page.goto(`${gateway.base}/browser`);
    await expect(page.getByTestId("browser-owner-request")).toBeVisible();
    await expect(page.getByTestId("browser-owner-resume-failed")).toContainText(
      "登録した端末として確認できませんでした",
    );
    // 誤った秘密の cookie は gateway が消す。
    expect((await page.context().cookies()).some((c) => c.name === DEVICE_COOKIE)).toBe(false);
  } finally {
    await gateway.close();
  }
});

test("trusted_device: the live view of an owner offers to trust this device", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    const offer = page.getByTestId("browser-trusted-device-offer");
    await expect(offer.getByRole("button", { name: "この端末を信頼する" })).toBeVisible();
    await offer.getByLabel("端末の名前").fill("ライブ用の端末");
    await offer.getByRole("button", { name: "この端末を信頼する" }).click();
    await expect(offer.getByRole("heading", { name: "信頼できる端末" })).toBeVisible();
    await offer.getByRole("link", { name: "登録した端末の一覧" }).click();
    await expect(page).toHaveURL(/\/browser\/devices$/);
    await expect(page.locator('[data-testid="browser-trusted-device"]', { hasText: "ライブ用の端末" })).toBeVisible();
  } finally {
    await gateway.close();
  }
});
