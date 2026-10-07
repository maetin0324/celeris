import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

// project ごとの identity 画面（ADR 2026-10-05-browser-department-web-live-view D2.4・D3）。一覧・登録・
// 失効・復元・削除が gateway を通って偽 browser backend に届き、登録した state JSON は画面に残らない。
// 本人でない session には一覧を出さず owner-session-notice を出す。

const SECRET = "identity-e2e-secret-cookie-value";

test("owner lists, registers, restores, revokes and deletes identities of a project", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    const backend = gateway.daemon.browser;
    if (!backend) throw new Error("browser backend is missing");
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/projects/P1/browser-identities`);
    await expect(page.getByRole("heading", { level: 1, name: "ブラウザの本人情報" })).toBeVisible();

    // 一覧: P1 の 2 件だけ（P2 の ID3 は出ない）。active にだけ失効・復元、全件に削除。
    const cards = page.getByTestId("browser-identity");
    await expect(cards).toHaveCount(2);
    const id1 = page.locator('[data-testid="browser-identity"][data-identity-id="ID1"]');
    const id2 = page.locator('[data-testid="browser-identity"][data-identity-id="ID2"]');
    await expect(id1).toContainText("https://billing.example.com");
    await expect(id1).toContainText("有効");
    await expect(id1).toContainText("2026-09-28 14:13:20Z");
    await expect(id2).toContainText("失効済み");
    await expect(id2.getByRole("button", { name: "ID2 を失効" })).toHaveCount(0);
    await expect(id2.getByRole("button", { name: "ID2 を復元" })).toHaveCount(0);
    await expect(id2.getByRole("button", { name: "ID2 を削除" })).toBeVisible();
    await expect(page.locator('[data-identity-id="ID3"]')).toHaveCount(0);

    // 登録: state JSON は伏せ字の入力で受け、送信後は空になり、画面のどこにも残らない。
    const form = page.getByTestId("browser-identity-register");
    await form.getByLabel("id", { exact: true }).fill("ID9");
    await form.getByLabel("サイト（origin）").fill("https://new.example.com");
    await form.getByLabel("確認した人").fill("human:owner");
    const state = form.getByLabel("state JSON");
    await state.fill(
      JSON.stringify({ entries: [{ origin: "https://new.example.com", kind: "cookie", name: "s", value: SECRET }] }),
    );
    expect(await state.evaluate((el) => getComputedStyle(el).getPropertyValue("-webkit-text-security"))).toBe("disc");
    await form.getByRole("button", { name: "登録する" }).click();
    await expect(form.getByRole("status")).toHaveText("登録しました。使う前に本人の復元操作が要ります。");
    await expect(state).toHaveValue("");
    await expect(page.locator('[data-identity-id="ID9"]')).toContainText("https://new.example.com");
    expect(await page.content()).not.toContain(SECRET);
    const created = backend.records.identities.find(
      (r) => r.method === "POST" && r.path === "/api/v1/browser/identities",
    );
    expect(created?.body).toMatchObject({
      identity_id: "ID9",
      project_id: "P1",
      origin: "https://new.example.com",
      demand_confirmed_by: "human:owner",
      ttl_secs: 604800,
      state: { entries: [{ value: SECRET }] },
    });

    // 復元: origin と project を付けて送る。
    await id1.getByRole("button", { name: "ID1 を復元" }).click();
    await expect(id1.getByRole("status")).toHaveText("復元しました。");
    expect(backend.records.identities.find((r) => r.path.endsWith("/ID1/restore"))).toMatchObject({
      method: "POST",
      path: "/api/v1/browser/identities/ID1/restore",
      body: { project_id: "P1", origin: "https://billing.example.com" },
    });

    // 失効: 状態が失効済みになり、失効・復元は消える。
    await id1.getByRole("button", { name: "ID1 を失効" }).click();
    await expect(id1).toHaveAttribute("data-identity-state", "revoked");
    await expect(id1).toContainText("失効済み");
    await expect(id1.getByRole("button", { name: "ID1 を失効" })).toHaveCount(0);
    expect(backend.records.identities.find((r) => r.path.endsWith("/ID1/revoke"))).toMatchObject({
      method: "POST",
      path: "/api/v1/browser/identities/ID1/revoke",
    });

    // 削除: ConfirmDialog で確かめてから送り、一覧から消える。
    await id2.getByRole("button", { name: "ID2 を削除" }).click();
    const dialog = page.getByRole("alertdialog").or(page.getByRole("dialog"));
    await expect(dialog).toContainText("https://old.example.com（ID2）");
    await dialog.getByRole("button", { name: "ID2 を削除" }).click();
    await expect(page.locator('[data-identity-id="ID2"]')).toHaveCount(0);
    expect(
      backend.records.identities.some((r) => r.method === "DELETE" && r.path === "/api/v1/browser/identities/ID2"),
    ).toBe(true);
  } finally {
    await gateway.close();
  }
});

test("a non-owner session sees the owner-session notice and no identities", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOther(page);
    await page.goto(`${gateway.base}/projects/P1/browser-identities`);
    await expect(page.getByTestId("browser-owner-request")).toBeVisible();
    await expect(page.getByTestId("browser-identity")).toHaveCount(0);
    await expect(page.getByTestId("browser-identity-register")).toHaveCount(0);
    expect(gateway.daemon.browser?.records.identities ?? []).toEqual([]);
  } finally {
    await gateway.close();
  }
});

test("the project detail links to its browser identities", async ({ page }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false } });
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/projects/P1`);
    await page.getByRole("link", { name: "ブラウザの identity" }).click();
    await expect(page).toHaveURL(/\/projects\/P1\/browser-identities$/);
    await expect(page.getByRole("heading", { level: 1, name: "ブラウザの本人情報" })).toBeVisible();
  } finally {
    await gateway.close();
  }
});
