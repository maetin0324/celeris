import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createApp } from "../../server/app.js";
import { createFakeDaemon } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";

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

  test("parity: /releases 昇格と失敗表示 保留は成功と出さず、結果で成功になる", async ({ page }) => {
    await control({ mode: "succeed", pendingMs: 2500 });
    await page.goto(`${gateway.base}/releases`);
    await expect(page.getByRole("heading", { level: 1, name: "リリース" })).toBeVisible();
    await page.getByRole("button", { name: "bbbbbbbbbbbb を昇格" }).click();
    await expect(page.getByTestId("promote-pending")).toBeVisible();
    await expect(page.getByTestId("promote-succeeded")).toHaveCount(0);
    await expect(page.getByTestId("promote-succeeded")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByTestId("promote-pending")).toHaveCount(0);
    await expect(page.getByTestId("release-bbbbbbbbbbbb")).toContainText("current");
  });

  test("parity: /releases 昇格と失敗表示 失敗を表示する", async ({ page }) => {
    await control({ mode: "fail", pendingMs: 1000 });
    await page.goto(`${gateway.base}/releases`);
    await page.getByRole("button", { name: "aaaaaaaaaaaa を昇格" }).click();
    await expect(page.getByTestId("promote-pending")).toBeVisible();
    await expect(page.getByTestId("promote-failed")).toContainText("exit 1", { timeout: 15_000 });
    await expect(page.getByTestId("promote-succeeded")).toHaveCount(0);
  });

  test("parity: /releases 昇格と失敗表示 昇格中に gateway が入れ替わっても結果を出す", async ({ page }) => {
    test.setTimeout(60_000);
    await control({ mode: "succeed", pendingMs: 4000 });
    const first = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    await page.goto(`${first.base}/releases`);
    await page.getByRole("button", { name: "aaaaaaaaaaaa を昇格" }).click();
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
