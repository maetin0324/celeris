import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import type http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { createApp } from "../../server/app.js";

// gateway の認証（P1-05 / P1-06）。gateway は空き port の loopback で、パスワードのファイルを与えて認証を有効にする。
const PASSWORD = "e2e-password";
const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-auth-"));
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

async function sessionCookie(): Promise<string> {
  const response = await fetch(`${base}/login`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify({ password: PASSWORD, next: "/" }),
  });
  expect(response.status).toBe(200);
  return (response.headers.get("set-cookie") ?? "").split(";")[0] ?? "";
}

test("parity: /logout cookie 消去と CSRF", async () => {
  const cookie = await sessionCookie();
  const before = (await (await fetch(`${base}/api/session`, { headers: { Cookie: cookie } })).json()) as {
    authenticated: boolean;
  };
  expect(before.authenticated).toBe(true);
  const foreign = await fetch(`${base}/logout`, {
    method: "POST",
    redirect: "manual",
    headers: { Cookie: cookie, Origin: "http://evil.example" },
  });
  expect(foreign.status).toBe(403);
  const logout = await fetch(`${base}/logout`, { method: "POST", redirect: "manual", headers: { Cookie: cookie } });
  expect(logout.status).toBe(303);
  expect(logout.headers.get("location")).toBe("/login");
  expect(logout.headers.get("set-cookie")).toMatch(/^__celeris_web_session=;.*Expires=Thu, 01 Jan 1970/);
});

async function login(page: import("@playwright/test").Page, password: string) {
  await page.getByLabel("パスワード").fill(password);
  await page.getByRole("button", { name: "ログイン" }).click();
}

test("parity: /login 成功・失敗・next・daemon 停止中", async ({ page }) => {
  // この gateway には daemon が居ない（偽 daemon も起こさない）状態で login が完結する。
  await page.goto(`${base}/login?next=${encodeURIComponent("/tasks?tab=runs")}`);
  await expect(page.getByLabel("パスワード")).toHaveAttribute("autocomplete", "current-password");
  const started = Date.now();
  await login(page, "wrong");
  await expect(page.getByRole("alert")).toHaveText("パスワードが違います");
  expect(Date.now() - started).toBeGreaterThanOrEqual(900);
  expect(await page.context().cookies()).toHaveLength(0);
  await login(page, PASSWORD);
  await page.waitForURL(`${base}/tasks?tab=runs`);
  const cookies = await page.context().cookies();
  expect(cookies.map((cookie) => cookie.name)).toEqual(["__celeris_web_session"]);
  // 外への next は "/" に落ちる。shell は 401 で /login へ移るので、先に画面を離れてから cookie を消す。
  await page.goto("about:blank");
  await page.context().clearCookies();
  await page.goto(`${base}/login?next=${encodeURIComponent("//evil.example/x")}`);
  await login(page, PASSWORD);
  await page.waitForURL(`${base}/`);
});

test("parity-x: auth cookie 属性・401・next・非 loopback", async ({ page }) => {
  for (const route of ["/api/tasks", "/files/x/y", "/events"])
    expect((await fetch(`${base}${route}`)).status).toBe(401);
  // 未認証の保護画面は /login?next= へ（daemon の health は見ない）。
  await page.goto(`${base}/tasks/01ABC?tab=runs`);
  await page.waitForURL(`${base}/login?next=${encodeURIComponent("/tasks/01ABC?tab=runs")}`);
  await login(page, PASSWORD);
  await page.waitForURL(`${base}/tasks/01ABC?tab=runs`);
  const [cookie] = await page.context().cookies();
  expect(cookie).toMatchObject({ name: "__celeris_web_session", httpOnly: true, sameSite: "Strict", secure: false });
  expect(cookie?.expires ?? 0).toBeGreaterThan(Date.now() / 1000 + 24 * 3600 - 120);
  // gui/ の cookie では入れない。shell の 401 遷移と競らないよう先に画面を離れる。
  await page.goto("about:blank");
  await page.context().clearCookies();
  await page.context().addCookies([{ name: "__celeris_gui_session", value: "e30%3D.abc", url: base }]);
  await page.goto(`${base}/`);
  await page.waitForURL(/\/login\?next=/);
  // 非 loopback の bind でパスワードのファイルが無ければ起動しない。
  expect(() => createApp({ bind: { host: "0.0.0.0", port: 7720 }, passwordFile: undefined })).toThrow(/PASSWORD_FILE/);
});

test("parity-x: auth キーボード表示でもボタンが見える", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 360 });
  await page.goto(`${base}/login`);
  await page.getByLabel("パスワード").focus();
  const box = await page.getByRole("button", { name: "ログイン" }).boundingBox();
  expect(box && box.y + box.height).toBeLessThanOrEqual(360);
});
