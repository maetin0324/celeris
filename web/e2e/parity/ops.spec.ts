import { expect, test } from "@playwright/test";

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
    page.on("dialog", (dialog) => void dialog.accept());
    await page.goto(`${base}/providers`);
    await expect(page.getByRole("heading", { level: 1, name: "プロバイダ" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "claude-main" })).toBeVisible();
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
    await expect(page.getByRole("heading", { name: "codex-2" })).toHaveCount(0);
    expect(seen.filter((r) => r.path === "/api/v1/reload").length).toBeGreaterThanOrEqual(3);
  });
});
