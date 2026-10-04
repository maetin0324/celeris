import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import http from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { screens } from "../support/screens";

// file 単位の共有状態（module で作る一時 dir・beforeAll の server）に依存するので、fullyParallel でも
// この file の試験は 1 worker で順に流す（file どうしは並列）。
test.describe.configure({ mode: "default" });

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

test("parity-x: daemon 停止中のバナーと復旧", async ({ page }) => {
  test.setTimeout(90_000);
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-shell-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  let daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const daemonUrl = await daemon.start();
  const daemonPort = Number(new URL(daemonUrl).port);
  const local = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
  try {
    await page.goto(`${local.base}/`);
    const shell = page.locator("[data-shell]");
    const banner = page.locator("[data-celeris-down]");
    await expect(shell.getByRole("heading", { level: 1, name: "ホーム" })).toBeVisible();
    await expect.poll(() => daemon.requests.some((r) => r.path === "/api/v1/health")).toBe(true);
    await expect(banner).toHaveCount(0);

    // 停止: バナーが出て、shell・ナビは使える。
    await daemon.close();
    await expect(banner).toBeVisible({ timeout: 20_000 });
    await expect(page.getByRole("alert").filter({ hasText: "celeris に接続できません" })).toBeVisible();
    await shell.getByRole("navigation", { name: "主要" }).getByRole("link", { name: "タスク" }).click();
    await expect(page).toHaveURL(`${local.base}/tasks`);
    await expect(shell.getByRole("heading", { level: 1 })).toBeVisible();
    // login も daemon 無しで開ける。
    const loginPage = await page.context().newPage();
    await loginPage.goto(`${local.base}/login`);
    await expect(loginPage.getByLabel("パスワード")).toBeVisible();
    await loginPage.close();

    // 復旧: バナーが消え、表示中の key（inbox・daemon）を取り直す。
    daemon = createFakeDaemon({ token: FIXTURE_TOKEN, port: daemonPort });
    await daemon.start();
    await expect(banner).toHaveCount(0, { timeout: 20_000 });
    await expect
      .poll(() => daemon.requests.filter((r) => r.path === "/api/v1/inbox" || r.path === "/api/v1/daemon").length, {
        timeout: 5_000,
      })
      .toBeGreaterThanOrEqual(2);
  } finally {
    await local.close();
    await daemon.close().catch(() => {});
    rmSync(dir, { recursive: true, force: true });
  }
});

// shell の時刻表示（P2-06・X7）。ブラウザの時刻帯で絶対時刻を出し、相対時刻は hello.now のずれで補正する。
async function withDaemon<T>(run: (ctx: { base: string; daemon: ReturnType<typeof createFakeDaemon> }) => Promise<T>) {
  const dir = mkdtempSync(path.join(tmpdir(), "celeris-web-e2e-shell-"));
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN });
  const daemonUrl = await daemon.start();
  const local = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
  try {
    return await run({ base: local.base, daemon });
  } finally {
    await local.close();
    await daemon.close().catch(() => {});
    rmSync(dir, { recursive: true, force: true });
  }
}

for (const timeZone of ["Asia/Tokyo", "America/New_York"]) {
  test.describe(`timezone ${timeZone}`, () => {
    test.use({ timezoneId: timeZone });
    test("parity-x: timezone 表示と相対時刻", async ({ page }) => {
      test.setTimeout(60_000);
      await withDaemon(async ({ base, daemon }) => {
        await page.goto(`${base}/`);
        await expect(page.getByRole("heading", { level: 1, name: "ホーム" })).toBeVisible();
        await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
        // server の時計が 1 時間進んでいる。補正しなければ「1 時間後」になる。
        const helloDate = new Date(Math.floor(Date.now() / 1000) * 1000 + 3_600_000);
        daemon.sendEvent("hello", { cursor: 0, now: helloDate.toISOString(), daemon: null });
        const block = page.locator("[data-server-time]");
        await expect(block).toBeAttached();
        const expected = new Intl.DateTimeFormat("ja-JP", {
          dateStyle: "medium",
          timeStyle: "medium",
          timeZone,
        }).format(helloDate);
        await expect(block.locator("time[data-absolute]")).toHaveText(expected);
        await expect(block.locator("time[data-absolute]")).toHaveAttribute("datetime", helloDate.toISOString());
        const relative = await block.locator("[data-relative]").textContent();
        expect(relative).toMatch(/^(今|\d+ 秒前)$/);
      });
    });
  });
}

test.describe("P5-02 全画面 storage gate", () => {
  test("parity-x: storage に機密が無い（全画面）", async ({ page }) => {
    test.setTimeout(180_000);
    expect(screens.length).toBeGreaterThanOrEqual(30);
    await withDaemon(async ({ base, daemon }) => {
      await page.goto(`${base}/`);
      await expect.poll(() => daemon.streamClients).toBeGreaterThan(0);
      daemon.sendEvent("hello", { cursor: 0, now: new Date().toISOString(), daemon: null });
      for (const screen of screens) {
        await page.goto(`${base}${screen.fixture}`);
        await expect(page.getByRole("heading", { level: 1, name: screen.heading }), screen.path).toBeVisible();
        const snapshot = await page.evaluate(async () => {
          const dump = (s: Storage) => Object.fromEntries(Object.keys(s).map((k) => [k, s.getItem(k)]));
          return {
            local: dump(localStorage),
            session: dump(sessionStorage),
            idb: ((await indexedDB.databases?.()) ?? []).map((d) => d.name),
            caches: await caches.keys(),
            workers: (await navigator.serviceWorker.getRegistrations()).length,
          };
        });
        // localStorage: 表示の好みの key だけ（何も設定していなければ空）。
        for (const [key, value] of Object.entries(snapshot.local)) {
          expect(key, screen.path).toBe("celeris.web.display");
          const parsed = JSON.parse(value ?? "{}") as Record<string, unknown>;
          expect(Object.keys(parsed).every((k) => k === "timeZone" || k === "theme")).toBe(true);
        }
        // sessionStorage: 使わない（scroll 位置は memory）。
        expect(snapshot.session, screen.path).toEqual({});
        expect(snapshot.idb, screen.path).toEqual([]);
        expect(snapshot.caches, screen.path).toEqual([]);
        expect(snapshot.workers, screen.path).toBe(0);
        // token と API 応答の本文がどの storage にも無い。
        const all = JSON.stringify([snapshot.local, snapshot.session]);
        expect(all, screen.path).not.toContain(FIXTURE_TOKEN);
        for (const fixture of Object.values(defaultFixtures)) {
          const body = JSON.stringify(fixture);
          if (body.length > 20) expect(all, screen.path).not.toContain(body);
        }
      }
      await expect.poll(() => daemon.requests.some((r) => r.path === "/api/v1/inbox")).toBe(true);
    });
  });
});
