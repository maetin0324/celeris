import { expect, type Page, test } from "@playwright/test";
import type { ClusterView, DaemonSnapshot, DaemonView, ReleaseItem, Releases } from "../../api/generated/types";
import { startFixtureGateway } from "../support/fixture-gateway";

// beforeAll の一時 dir・server をこの file の試験で共有するので、file 内は 1 worker で順に流す（2026-10-04 の並列化と同じ扱い）。
test.describe.configure({ mode: "default" });

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
    const status = page.getByRole("list", { name: "クラスタの状態" });
    await expect(status.getByText("切断: keepalive timeout", { exact: false })).toBeVisible();
    await expect(status.getByText("失敗理由:")).toHaveCount(2);
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
    await expect(list.getByText("最終取得")).toBeVisible();
    await expect(list.getByText("10 秒ごとに自動で取り直します", { exact: false })).toBeVisible();
    await expect(list.getByText(/^[0-9a-f]{12}（active）$/)).toBeVisible();
    await expect(list.getByText("celeris-host (pid 4242)")).toBeVisible();
  });

  test("最後の tick が古いと「応答なし」と理由を出す", async ({ page }) => {
    await routeDaemon(page, 5 * 60_000);
    await page.goto(`${base}/daemon`);
    const live = page.getByTestId("daemon-liveness");
    await expect(live).toContainText("応答なし");
    await expect(live).toContainText("最後の動作確認から 1 分以上たっています");
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

const release = (over: Partial<ReleaseItem>): ReleaseItem => ({
  sha12: "000000000000",
  gate_ok: true,
  is_current: false,
  is_previous: false,
  promoting: false,
  built_at: "2026-10-03T00:00:00Z",
  ref: "main",
  ...over,
});

const releases: Releases = {
  current: "cccccccccccc",
  previous: "dddddddddddd",
  running: { release: "cccccccccccc", role: "active", instance_id: "I1" },
  instances: [],
  items: [
    release({ sha12: "eeeeeeeeeeee", ref: "refs/heads/a-very-long-branch-name-for-the-release-under-test" }),
    release({ sha12: "cccccccccccc", is_current: true, promoted_at: "2026-10-02T00:00:00Z" }),
    release({
      sha12: "dddddddddddd",
      is_previous: true,
      promoted_at: "2026-10-01T00:00:00Z",
      promote_failed: { failed_at: "2026-10-01T12:00:00Z", error: "promote.sh が exit 1 で終わりました" },
    }),
  ],
};

async function routeReleases(page: Page) {
  await page.route("**/api/releases", (route) =>
    route.request().method() === "GET" ? route.fulfill({ json: releases }) : route.fallback(),
  );
}

test.describe("releases", () => {
  test("昇格の確認表示に対象版・現在版・影響が出て、確定ボタンに動詞と対象が入る", async ({ page }) => {
    await routeReleases(page);
    await page.goto(`${base}/releases`);
    await expect(page.getByRole("heading", { level: 1, name: "リリース" })).toBeVisible();
    const table = page.getByRole("region", { name: "リリースの一覧" });
    await expect(table.getByTestId("release-cccccccccccc")).toContainText("稼働中の版です");
    await expect(table.getByTestId("release-dddddddddddd")).toContainText("promote.sh が exit 1 で終わりました");
    await expect(table.getByRole("button", { name: "cccccccccccc を昇格する" })).toBeDisabled();

    await table.getByRole("button", { name: "eeeeeeeeeeee を昇格する" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("対象版 eeeeeeeeeeee");
    await expect(dialog).toContainText("現在版 cccccccccccc");
    await expect(dialog).toContainText("影響: 本番の daemon が eeeeeeeeeeee に引き継がれ");
    await expect(dialog.getByRole("button", { name: "eeeeeeeeeeee を昇格する", exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: "戻る" }).click();
    await expect(dialog).toHaveCount(0);

    await table.getByRole("button", { name: "dddddddddddd に巻き戻す" }).click();
    await expect(dialog).toContainText("前の版 dddddddddddd に巻き戻しますか");
    await expect(dialog).toContainText("現在版 cccccccccccc");
    await expect(dialog.getByRole("button", { name: "dddddddddddd に巻き戻す", exact: true })).toBeVisible();
  });

  test("昇格の結果を StatusBadge と文字で出す（進行中→失敗）", async ({ page }) => {
    let promoted = false;
    await page.route("**/api/releases", (route) => {
      if (!promoted) return route.fulfill({ json: releases });
      const failed: Releases = {
        ...releases,
        items: releases.items.map((item) =>
          item.sha12 === "eeeeeeeeeeee"
            ? { ...item, promote_failed: { failed_at: "2999-01-01T00:00:00Z", error: "gate が落ちました" } }
            : item,
        ),
      };
      return route.fulfill({ json: failed });
    });
    await page.route("**/api/releases/*/promote", (route) => {
      promoted = true;
      return route.fulfill({
        status: 202,
        json: { sha12: "eeeeeeeeeeee", log: "l", started_at: "2026-10-04T00:00:00Z", script_from: "current" },
      });
    });
    await page.goto(`${base}/releases`);
    await page.getByRole("button", { name: "eeeeeeeeeeee を昇格する" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "eeeeeeeeeeee を昇格する", exact: true }).click();
    const failed = page.getByTestId("promote-failed");
    await expect(failed).toContainText("失敗", { timeout: 15_000 });
    await expect(failed).toContainText("eeeeeeeeeeee の昇格に失敗しました: gate が落ちました");
  });

  test("403 で昇格・巻き戻しを無効にし、理由を出す", async ({ page }) => {
    await routeReleases(page);
    await page.route("**/api/releases/*/promote", (route) =>
      route.fulfill({ status: 403, json: { error: "release の昇格は許可されていません" } }),
    );
    await page.goto(`${base}/releases`);
    await page.getByRole("button", { name: "eeeeeeeeeeee を昇格する" }).click();
    await page.getByRole("alertdialog").getByRole("button", { name: "eeeeeeeeeeee を昇格する", exact: true }).click();
    const alert = page.getByTestId("releases-denied");
    await expect(alert).toContainText("権限がありません（403）");
    await expect(alert).toContainText("release の昇格は許可されていません");
    await expect(page.getByRole("button", { name: "eeeeeeeeeeee を昇格する" })).toBeDisabled();
    await expect(page.getByRole("button", { name: "dddddddddddd に巻き戻す" })).toBeDisabled();
  });

  test("360px で横に溢れず、確認表示も幅に収まる", async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 800 });
    await routeReleases(page);
    await page.goto(`${base}/releases`);
    const release = page.getByTestId("mobile-release-eeeeeeeeeeee");
    await expect(release).toBeVisible();
    await expect(release.getByText("問題・直近の失敗")).toBeVisible();
    expect(await overflow(page)).toBe(0);
    await page.getByRole("button", { name: "eeeeeeeeeeee を昇格する" }).click();
    const dialog = page.getByRole("alertdialog");
    await expect(dialog).toContainText("現在版 cccccccccccc");
    const box = await dialog.boundingBox();
    expect(box && box.x >= 0 && box.x + box.width <= 360).toBe(true);
    await expect(dialog.getByRole("button", { name: "eeeeeeeeeeee を昇格する", exact: true })).toBeInViewport();
    expect(await overflow(page)).toBe(0);
  });
});
