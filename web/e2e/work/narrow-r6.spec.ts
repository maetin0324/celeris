/// <reference lib="dom" />
import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";
import { applyStateRoute, stateByKey } from "../support/states";

// fix-r6 narrow: after-r4 の目視で要修正・保留になった狭い幅の 7 点を固定する（rich fixture、screenshots と同じ data）。
// (1) /providers 360〜412: 一覧の右列（同時実行・前回の確認）が見えなかった → 行を積み「見出し: 値」で全部出す。
// (2) /tasks/T1 360: tab 列の「成果物」が右で切れた → 余白を詰め、収まらない幅は折り返す。
// (3) /graph 360〜412: 横に続く graph の手がかりが無かった → 注記と、見えていない側の端の影。
// (4) /tasks/T1/changes: 長い共通 prefix が毎行反復した → 共通の場所を 1 度だけ出し、各行はファイル名を先に。
// (5) /（1440・電話幅）: 会話枠の先頭 block の見出し行が上端で半分切れて見えた → 留めた宛先行の下にぼかしの帯。
// (6) stale の / 360: 会話枠が約 40px に縮んだ → 宛先行と送信欄を除いて 160px を保つ。
// (7) loading の /providers 360: 接続状態の語が長く header が 2 段に折れた → 語を短くし 1 段に保つ。

async function settle(page: Page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

for (const width of [360, 390, 412]) {
  test(`(1) /providers ${width}: 実行枠の一覧は枠に収まり、同時実行と前回の確認が見出し付きで見える`, async ({
    page,
  }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/providers`);
      const table = page.getByRole("region", { name: "実行枠の状態" });
      const row = table.locator("[data-provider='claude-main']");
      await expect(row).toBeVisible();
      const box = await table.evaluate((el) => ({
        scrollWidth: el.scrollWidth,
        clientWidth: el.clientWidth,
        right: el.getBoundingClientRect().right,
      }));
      expect(box.scrollWidth, "一覧の枠が横に溢れない").toBeLessThanOrEqual(box.clientWidth + 1);
      expect(box.right).toBeLessThanOrEqual(width);
      for (const [field, label] of [
        ["adapter", "道具:"],
        ["tiers", "受ける段:"],
        ["concurrency", "同時実行:"],
        ["last-check", "前回の確認:"],
      ] as const) {
        const cell = row.locator(`[data-field='${field}']`);
        await expect(cell).toBeInViewport();
        await expect(cell).toContainText(label);
        const right = await cell.evaluate((el) => el.getBoundingClientRect().right);
        expect(right, `${label} の右端`).toBeLessThanOrEqual(width);
      }
      await expect(row.locator("[data-field='concurrency']")).toContainText("上限 2");
      await expect(row.locator("[data-field='last-check']")).toContainText("まだ確認していません");
    } finally {
      await gateway.close();
    }
  });
}

test("(1) /providers 1280: 6 列の見出しが見え、積みの見出しは隠れる", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.goto(`${gateway.base}/providers`);
    const table = page.getByRole("region", { name: "実行枠の状態" });
    await expect(table.getByRole("columnheader", { name: "前回の確認" })).toBeVisible();
    await expect(table.locator("[data-provider='claude-main']").getByText("同時実行:")).toBeHidden();
  } finally {
    await gateway.close();
  }
});

test("(2) /tasks/T1 360: 5 つの tab が全部 viewport の中に全文で出る", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/tasks/T1`);
    const nav = page.getByRole("navigation", { name: "タスクの表示" });
    const tabs = nav.locator("[data-tab]");
    await expect(tabs).toHaveCount(5);
    const box = await nav.evaluate((el) => ({ scrollWidth: el.scrollWidth, clientWidth: el.clientWidth }));
    expect(box.scrollWidth).toBeLessThanOrEqual(box.clientWidth + 1);
    for (let i = 0; i < 5; i += 1) {
      const tab = tabs.nth(i);
      await expect(tab).toBeInViewport({ ratio: 1 });
      const right = await tab.evaluate((el) => el.getBoundingClientRect().right);
      expect(right).toBeLessThanOrEqual(360);
    }
    await expect(nav.getByRole("link", { name: "成果物" })).toBeInViewport({ ratio: 1 });
  } finally {
    await gateway.close();
  }
});

for (const width of [360, 412]) {
  test(`(3) /graph ${width}: 横に続くことの注記と端の影があり、右端まで動かすと影が左へ移る`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/graph`);
      const canvas = page.getByTestId("graph-canvas");
      await expect(canvas.locator("[data-graph-node='T2']")).toBeAttached();
      await expect(page.getByTestId("graph-scroll-hint")).toBeVisible();
      await expect(page.getByTestId("graph-scroll-hint")).toContainText("横に続きます");
      await expect(canvas).toHaveAttribute("aria-describedby", "graph-scroll-hint");
      await expect(page.getByTestId("graph-edge-end")).toBeVisible();
      await expect(page.getByTestId("graph-edge-start")).toHaveCount(0);
      await canvas.evaluate((el) => {
        el.scrollLeft = el.scrollWidth;
      });
      await expect(page.getByTestId("graph-edge-end")).toHaveCount(0);
      await expect(page.getByTestId("graph-edge-start")).toBeVisible();
    } finally {
      await gateway.close();
    }
  });
}

test("(3) /graph 1440: graph が枠に収まれば注記と影は出ない", async ({ page }) => {
  const gateway = await startFixtureGateway();
  try {
    await page.setViewportSize({ width: 1440, height: 800 });
    await page.goto(`${gateway.base}/graph`);
    const canvas = page.getByTestId("graph-canvas");
    await expect(canvas.locator("[data-graph-node='T2']")).toBeVisible();
    await settle(page);
    const fits = await canvas.evaluate((el) => el.scrollWidth <= el.clientWidth + 1);
    expect(fits).toBe(true);
    await expect(page.getByTestId("graph-scroll-hint")).toHaveCount(0);
    await expect(page.getByTestId("graph-edge-end")).toHaveCount(0);
  } finally {
    await gateway.close();
  }
});

for (const width of [360, 1440]) {
  test(`(4) /tasks/T1/changes ${width}: 共通の場所は 1 度だけ、各行はファイル名を先に出し全文 path を名前に持つ`, async ({
    page,
  }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}/tasks/T1/changes`);
      const files = page.getByTestId("changed-files").locator("[data-file]");
      await expect(files).toHaveCount(15);
      const common = page.getByTestId("changed-files-common");
      await expect(common).toHaveCount(1);
      const prefix = ((await common.locator("span").textContent()) ?? "").trim();
      expect(prefix).toMatch(/\/$/);
      expect(prefix.length).toBeGreaterThan(20);
      for (let i = 0; i < 15; i += 1) {
        const file = files.nth(i);
        const path = (await file.getAttribute("data-file")) ?? "";
        expect(path.startsWith(prefix)).toBe(true);
        const name = file.locator("[data-file-name]");
        await expect(name).toHaveText(path.split("/").at(-1) ?? "");
        await expect(file).toHaveAccessibleName(path);
        await expect(file).toHaveAttribute("title", path);
        // ファイル名は 1 行（共通 prefix の折り返しで縦に伸びない）。
        const metrics = await name.evaluate((el) => ({
          height: el.getBoundingClientRect().height,
          lineHeight: Number.parseFloat(getComputedStyle(el).lineHeight),
        }));
        expect(metrics.height).toBeLessThanOrEqual(metrics.lineHeight + 2);
      }
    } finally {
      await gateway.close();
    }
  });
}

for (const [width, height] of [
  [360, 800],
  [1440, 800],
] as const) {
  test(`(5) / ${width}: 会話枠に留めた宛先行の下に、切れ目をぼかす帯がある`, async ({ page }) => {
    const gateway = await startFixtureGateway();
    try {
      await page.setViewportSize({ width, height });
      await page.goto(`${gateway.base}/`);
      await expect(page.getByRole("list", { name: "Console の会話" }).getByRole("listitem").first()).toBeVisible();
      await settle(page);
      const fade = await page.locator("[data-home-console] [data-console-toolbar]").evaluate((el) => {
        const after = getComputedStyle(el, "::after");
        return {
          position: getComputedStyle(el).position,
          content: after.content,
          height: Number.parseFloat(after.height),
          image: after.backgroundImage,
          pointer: after.pointerEvents,
        };
      });
      expect(fade.position).toBe("sticky");
      expect(fade.content).not.toBe("none");
      expect(fade.height).toBeGreaterThanOrEqual(16);
      expect(fade.image).toContain("gradient");
      expect(fade.pointer).toBe("none");
    } finally {
      await gateway.close();
    }
  });
}

test("(6) stale の / 360: 会話枠は宛先行と送信欄を除いて 150px 以上の会話を見せる", async ({ page }) => {
  const stale = stateByKey("stale");
  const gateway = await startFixtureGateway(stale.daemon);
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await applyStateRoute(page, stale);
    await page.goto(`${gateway.base}/`);
    await expect(page.locator("[data-connection]")).toHaveAttribute("data-connection", "reconnecting", {
      timeout: 15_000,
    });
    await expect(page.getByRole("list", { name: "Console の会話" }).getByRole("listitem").first()).toBeAttached();
    await settle(page);
    // ページを末尾まで送った状態で、宛先行の下端から送信欄の上端までの会話の見える高さを測る。
    await page.evaluate(() => window.scrollTo({ top: document.documentElement.scrollHeight }));
    await settle(page);
    const visible = await page.evaluate(() => {
      const region = document.querySelector("[data-home-console]") as HTMLElement;
      const toolbar = region.querySelector("[data-console-toolbar]") as HTMLElement;
      const composer = region.querySelector('[data-testid="console-composer"]') as HTMLElement;
      const bottom = Math.min(region.getBoundingClientRect().bottom, composer.getBoundingClientRect().top);
      return bottom - toolbar.getBoundingClientRect().bottom;
    });
    expect(visible).toBeGreaterThanOrEqual(150);
    // 会話の最後の block は送信欄より上に見えている（余白だけが見える位置まで送らない）。
    const last = page.getByRole("list", { name: "Console の会話" }).getByRole("listitem").last();
    const lastBox = await last.boundingBox();
    const composerBox = await page.getByTestId("console-composer").boundingBox();
    if (!lastBox || !composerBox) throw new Error("no box");
    expect(lastBox.y + lastBox.height).toBeLessThanOrEqual(composerBox.y + 1);
    // 再接続を試すたびに接続状態は「確認中」を挟む。その間も黄帯は消えず、ページの高さが跳ねて scroll が先頭へ戻らない。
    const scrolled = await page.evaluate(() => window.scrollY);
    expect(scrolled).toBeGreaterThan(0);
    await page.evaluate(() => {
      const w = window as unknown as { __r6: { state: string | null; banner: boolean; y: number }[] };
      w.__r6 = [];
      const status = document.querySelector("[data-connection]") as HTMLElement;
      new MutationObserver(() => {
        w.__r6.push({
          state: status.getAttribute("data-connection"),
          banner: (document.querySelector("#main")?.textContent ?? "").includes("最新の状態は未確認です"),
          y: window.scrollY,
        });
      }).observe(status, { attributes: true, attributeFilter: ["data-connection"] });
    });
    // 「確認中」→「再接続中」の 1 巡を出来事として待つ（固定の時間では待たない）。
    await page.waitForFunction(
      () => {
        const log = (window as unknown as { __r6: { state: string | null }[] }).__r6;
        const i = log.findIndex((entry) => entry.state === "connecting");
        return i >= 0 && log.slice(i + 1).some((entry) => entry.state === "reconnecting");
      },
      undefined,
      { timeout: 20_000 },
    );
    const log = await page.evaluate(
      () => (window as unknown as { __r6: { state: string | null; banner: boolean; y: number }[] }).__r6,
    );
    for (const entry of log) {
      expect(entry.banner, `接続状態 ${entry.state} でも黄帯が出ている`).toBe(true);
      expect(entry.y, `接続状態 ${entry.state} でも scroll 位置を保つ`).toBe(scrolled);
    }
  } finally {
    await gateway.close();
  }
});

test("(7) loading の /providers 360: 接続を確認中でも header は 1 段（Celeris・接続状態・メニューが同じ行）", async ({
  page,
}) => {
  const loading = stateByKey("loading");
  const gateway = await startFixtureGateway(loading.daemon);
  try {
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/providers`);
    await expect(page.locator('[data-fetch-state="loading"]').first()).toBeVisible();
    const status = page.locator("[data-connection]");
    await expect(status).toHaveAttribute("data-connection", "connecting");
    await expect(status).toHaveText("接続状態: 確認中");
    const header = page.locator("header").first();
    const menu = header.getByRole("button", { name: "メニュー" });
    const [headerBox, statusBox, menuBox] = await Promise.all([
      header.boundingBox(),
      status.boundingBox(),
      menu.boundingBox(),
    ]);
    if (!headerBox || !statusBox || !menuBox) throw new Error("no box");
    // 1 段: メニューの button と接続状態の上下の範囲が重なり、header は button 1 つ分の高さ。
    expect(menuBox.y).toBeLessThan(statusBox.y + statusBox.height);
    expect(statusBox.y).toBeLessThan(menuBox.y + menuBox.height);
    expect(headerBox.height).toBeLessThanOrEqual(menuBox.height + 16);
    gateway.daemon.releaseHeld();
  } finally {
    await gateway.close();
  }
});
