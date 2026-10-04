import { expect, type Page, test } from "@playwright/test";
import type { ClusterView, DaemonSnapshot, DaemonView } from "../../api/generated/types";
import { startFixtureGateway } from "../support/fixture-gateway";

// 実行系 ops（/clusters・/daemon）の状態表。状態と失敗理由が文字で読めること、403 で操作が無効になり理由が出ること、
// 360px で横に溢れないことを確かめる。fixture に無い状態は page.route で応答を差し替える。
let base = "";
let close: () => Promise<void> = async () => {};
test.beforeAll(async () => {
  const gateway = await startFixtureGateway();
  base = gateway.base;
  close = gateway.close;
});
test.afterAll(async () => {
  await close();
});

const overflow = (page: Page) =>
  page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);

const cluster = (over: Partial<ClusterView>): ClusterView => ({
  id: "c",
  host: "c.example",
  concurrency: 2,
  delete_on_push: false,
  env_keys: [],
  has_setup: false,
  rsync_excludes: [],
  sync: "rsync",
  ...over,
});

const clusters: ClusterView[] = [
  cluster({ id: "pegasus", host: "pegasus.example", auth: "totp", connected: true, in_use: 1 }),
  cluster({
    id: "sirius-with-a-very-long-cluster-identifier",
    host: "sirius.login.example.very-long-host-name.example.org",
    connected: false,
    stats: {
      last_24h: { last_lost_at: "2026-10-04T00:00:00Z", last_lost_cause: "keepalive timeout", losses: 2 },
    },
    tunnel_forwards: [{ listen: "127.0.0.1:18000", target: "bnode150:18000", last_error: "connection refused" }],
  }),
];

async function routeClusters(page: Page) {
  await page.route("**/api/clusters", (route) =>
    route.request().method() === "GET" ? route.fulfill({ json: { items: clusters } }) : route.fallback(),
  );
}

function snapshot(tickAgoMs: number, now: number): DaemonSnapshot {
  return {
    hostname: "celeris-host",
    pid: 4242,
    instance_id: "I1",
    started_at: new Date(now - 3_600_000).toISOString(),
    last_tick_at: new Date(now - tickAgoMs).toISOString(),
    tick_ms: 12,
    ticks: 300,
    in_flight: [],
    awaiting_human: [],
    unroutable: [],
    cooldowns: [],
    providers: [],
  };
}

async function routeDaemon(page: Page, tickAgoMs: number) {
  await page.route("**/api/daemon", (route) => {
    const now = Date.now();
    const view: DaemonView = { now: new Date(now).toISOString(), snapshot: snapshot(tickAgoMs, now) };
    return route.fulfill({ json: view });
  });
}

test.describe("clusters", () => {
  test("接続状態・最終確認・失敗理由を表の文字で読める", async ({ page }) => {
    await routeClusters(page);
    await page.goto(`${base}/clusters`);
    await expect(page.getByRole("heading", { level: 1, name: "クラスタ" })).toBeVisible();
    const table = page.getByRole("region", { name: "クラスタの状態" });
    await expect(table.getByRole("columnheader", { name: "最終確認" })).toBeVisible();
    const ok = table.getByTestId("cluster-row-pegasus");
    await expect(ok).toContainText("接続中");
    await expect(ok).toContainText("なし");
    const ng = table.getByTestId("cluster-row-sirius-with-a-very-long-cluster-identifier");
    await expect(ng).toContainText("未接続");
    await expect(ng).toContainText("切断: keepalive timeout");
    await expect(ng).toContainText("転送 127.0.0.1:18000: connection refused");
  });

  test("403 で操作を無効にし、理由を文字で出す", async ({ page }) => {
    await routeClusters(page);
    await page.route("**/api/clusters/*/connect", (route) =>
      route.fulfill({ status: 403, json: { detail: "cluster の操作は許可されていません" } }),
    );
    await page.goto(`${base}/clusters`);
    const card = page.getByRole("listitem", { name: "クラスタ pegasus" });
    await card.getByRole("button", { name: "接続" }).click();
    const alert = page.getByTestId("clusters-denied");
    await expect(alert).toContainText("権限がありません（403）");
    await expect(alert).toContainText("cluster の操作は許可されていません");
    await expect(card.getByRole("button", { name: "接続" })).toBeDisabled();
    await expect(card.getByRole("button", { name: "上書きを消す" })).toBeDisabled();
    const other = page.getByRole("listitem", { name: "クラスタ sirius-with-a-very-long-cluster-identifier" });
    await expect(other.getByRole("button", { name: "接続" })).toBeDisabled();
  });

  test("360px で横に溢れない", async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 800 });
    await routeClusters(page);
    await page.goto(`${base}/clusters`);
    await expect(page.getByText("切断: keepalive timeout")).toBeVisible();
    expect(await overflow(page)).toBe(0);
  });
});

test.describe("daemon", () => {
  test("稼働状態・版・最終 poll を文字で読める", async ({ page }) => {
    await routeDaemon(page, 5_000);
    await page.goto(`${base}/daemon`);
    await expect(page.getByRole("heading", { level: 1, name: "daemon" })).toBeVisible();
    await expect(page.getByTestId("daemon-liveness")).toContainText("稼働中");
    const list = page.getByRole("region", { name: "状態" });
    await expect(list.getByText("最終 poll")).toBeVisible();
    await expect(list.getByText("10 秒ごとに自動で取り直します", { exact: false })).toBeVisible();
    await expect(list.getByText(/^[0-9a-f]{12}（active）$/)).toBeVisible();
    await expect(list.getByText("celeris-host (pid 4242)")).toBeVisible();
  });

  test("最後の tick が古いと「応答なし」と理由を出す", async ({ page }) => {
    await routeDaemon(page, 5 * 60_000);
    await page.goto(`${base}/daemon`);
    const live = page.getByTestId("daemon-liveness");
    await expect(live).toContainText("応答なし");
    await expect(live).toContainText("最後の tick から 1 分以上たっています");
  });

  test("403 で replay と版の読み取りが止まり、理由を出す", async ({ page }) => {
    await routeDaemon(page, 5_000);
    await page.route("**/api/releases", (route) => route.fulfill({ status: 403, json: { error: "forbidden" } }));
    await page.route("**/api/replay", (route) => route.fulfill({ status: 403, json: { error: "forbidden" } }));
    await page.goto(`${base}/daemon`);
    await expect(page.getByText("権限がなく読めません（403）")).toBeVisible();
    const button = page.getByRole("button", { name: "replay を実行" });
    await button.click();
    await expect(page.getByTestId("replay-error")).toContainText("権限がありません（403）");
    await expect(button).toBeDisabled();
  });

  test("360px で横に溢れない", async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 800 });
    await routeDaemon(page, 5_000);
    await page.goto(`${base}/daemon`);
    await expect(page.getByTestId("daemon-liveness")).toBeVisible();
    expect(await overflow(page)).toBe(0);
  });
});
