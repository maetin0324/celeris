import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { createApp } from "../../server/app.js";

// /login の操作を固定する: label で欄を引ける・password 型・送信中表示・失敗時の alert と focus。
const PASSWORD = "e2e-login-password";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-login-"));
const passwordFile = path.join(dir, "password");
writeFileSync(passwordFile, `${PASSWORD}\n`);

let server: http.Server;
let base: string;

test.beforeAll(async () => {
  server = createApp({ passwordFile, log: () => {} }).listen(0, "127.0.0.1");
  await new Promise<void>((resolve, reject) => {
    server.once("listening", resolve);
    server.once("error", reject);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("gateway did not bind TCP");
  base = `http://127.0.0.1:${address.port}`;
});

test.afterAll(async () => {
  await new Promise<void>((resolve) => server.close(() => resolve()));
  rmSync(dir, { recursive: true, force: true });
});

test("login: label で欄を引け、password 型で値を表示しない", async ({ page }) => {
  await page.goto(`${base}/login`);
  await expect(page.getByRole("heading", { level: 1, name: "Celeris にログイン" })).toBeVisible();
  const input = page.getByLabel("パスワード");
  await expect(input).toHaveAttribute("type", "password");
  await expect(input).toHaveAttribute("autocomplete", "current-password");
  await expect(input).toBeFocused();
  await input.fill("secret-value");
  await expect(page.getByText("secret-value")).toHaveCount(0);
});

test("login: 送信中は button を無効にし文言を変える", async ({ page }) => {
  let release: () => void = () => {};
  const held = new Promise<void>((resolve) => {
    release = resolve;
  });
  // 応答を出来事で止め、送信中の表示を決定的に確かめる。
  await page.route(`${base}/login`, async (route) => {
    if (route.request().method() !== "POST") return route.continue();
    await held;
    await route.fulfill({ status: 401, contentType: "application/json", body: "{}" });
  });
  await page.goto(`${base}/login`);
  await page.getByLabel("パスワード").fill("wrong");
  await page.getByRole("button", { name: "ログイン" }).click();
  const busy = page.getByRole("button", { name: "ログイン中…" });
  await expect(busy).toBeDisabled();
  await expect(page.locator("form")).toHaveAttribute("aria-busy", "true");
  release();
  await expect(page.getByRole("button", { name: "ログイン", exact: true })).toBeEnabled();
});

test("login: 失敗時に alert が出て focus が欄に戻る", async ({ page }) => {
  await page.goto(`${base}/login`);
  const input = page.getByLabel("パスワード");
  await input.fill("wrong");
  await page.getByRole("button", { name: "ログイン" }).click();
  const alert = page.getByRole("alert");
  await expect(alert).toHaveText("パスワードが違います");
  await expect(input).toBeFocused();
  await expect(input).toHaveAttribute("aria-invalid", "true");
  await expect(input).toHaveAttribute("aria-describedby", (await alert.getAttribute("id")) ?? "missing");
  await expect(page.getByText(PASSWORD)).toHaveCount(0);
});

test("login: gateway に届かないときも理由を出し欄へ focus", async ({ page }) => {
  await page.route(`${base}/login`, (route) =>
    route.request().method() === "POST" ? route.abort("connectionrefused") : route.continue(),
  );
  await page.goto(`${base}/login`);
  const input = page.getByLabel("パスワード");
  await input.fill("anything");
  await page.getByRole("button", { name: "ログイン" }).click();
  await expect(page.getByRole("alert")).toContainText("gateway に接続できません");
  await expect(input).toBeFocused();
});
