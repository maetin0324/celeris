import http from "node:http";
import { expect, test } from "@playwright/test";
import { startGateway } from "../support/gateway";

// shell の parity（P2-02）。gateway は空き port の loopback で、daemon には接続しない。
let gateway: Awaited<ReturnType<typeof startGateway>>;

test.beforeAll(async () => {
  gateway = await startGateway();
});

test.afterAll(async () => {
  await gateway.close();
});

function rawStatus(route: string, host: string): Promise<number> {
  return new Promise((resolve, reject) => {
    const req = http.get({ hostname: "127.0.0.1", port: gateway.port, path: route, headers: { Host: host } }, (res) => {
      res.resume();
      resolve(res.statusCode ?? 0);
    });
    req.once("error", reject);
  });
}

test("parity: * 未定義パスの 404 と header", async ({ page }) => {
  // gateway: 未定義の path も Host・header の middleware を通り、HTML の 404 を返す。
  const response = await fetch(`${gateway.base}/no-such-page`);
  expect(response.status).toBe(404);
  expect(response.headers.get("content-type")).toContain("text/html");
  expect(response.headers.get("cache-control")).toBe("no-store");
  expect(response.headers.get("x-content-type-options")).toBe("nosniff");
  expect(response.headers.get("x-frame-options")).toBe("DENY");
  expect(response.headers.get("content-security-policy")).toContain("script-src 'self'");
  expect(await rawStatus("/no-such-page", "evil.example")).toBe(400);
  expect((await fetch(`${gateway.base}/tasks/T1/runs/R1`)).status).toBe(200);

  // client: shell の中の 404 で、ナビへ戻れる。
  await page.goto(`${gateway.base}/no-such-page`);
  const shell = page.locator("[data-shell]");
  await expect(shell.getByRole("heading", { level: 1, name: "ページが見つかりません" })).toBeVisible();
  await expect(shell.getByRole("navigation", { name: "主要" }).getByRole("link", { name: "タスク" })).toBeAttached();
  await shell.getByRole("link", { name: "ホームへ戻る" }).click();
  await expect(page).toHaveURL(`${gateway.base}/`);
  await expect(page.getByRole("heading", { level: 1, name: "ホーム" })).toBeFocused();
});
