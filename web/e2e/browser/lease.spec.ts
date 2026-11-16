import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

// ADR 2026-10-05-browser-department-web-live-view D3.3: run 画面の control bar で lease を取り、延ばし、
// エージェントに返す。画面を離れると release beacon が fixture の disconnect に届き、paused に落ちる。
// lease の時計は偽 backend の clock の差し替えと期限の書き換えで進める（実時間を待たない）。
//
// control bar（引き継ぎ・lease）が出るのは Live View の映像が無い run だけ（browser-run-screen.tsx）。
// `[browser] runtime = "daemon"` で gateway に Live View の中継が無い（`liveUpstream: false` → relay_unavailable）run、
// 実行中でない run、旧い launcher protocol の run がこれに当たる。daemon の takeover の口
// （crates/task-api/src/browser_control.rs）も現行。Live View の映像がある run（本番の `runtime = "launcher"`）は
// 読み取りだけの帯を出し、入力・引き継ぎを出さない（ADR 2026-10-10-browser-launcher-live-view-frames D4。最後の試験）。

const KEY = "T1/R1/S1";

test("owner takes over, renews, resumes, and leaving the screen releases the lease", async ({ page }) => {
  const gateway = await startBrowserGateway({ liveUpstream: false, backend: { credentialWait: false } });
  const backend = gateway.daemon.browser;
  if (!backend) throw new Error("browser backend is not enabled");
  let offset = 0;
  backend.clock.now = () => Date.now() + offset;
  const control = () => {
    const state = backend.control.get(KEY);
    if (!state) throw new Error("control state missing");
    return state;
  };
  try {
    await gateway.loginAsOwner(page);
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    const bar = page.getByTestId("browser-control-bar");
    const phase = page.getByTestId("browser-control-phase");
    await expect(phase).toHaveText("監視のみ — エージェントが操作中");
    await expect(bar).toHaveAttribute("data-operating", "false");
    await expect(page.getByRole("status").filter({ hasText: "監視のみ" })).toHaveAttribute("aria-live", "polite");

    // 映像の中継が無いので、Live View の枠はイベントでの監視を案内する（映像の接続も iframe も作らない）。
    await expect(page.getByTestId("browser-live-unavailable")).toHaveAttribute("data-reason", "relay_unavailable");
    await expect(page.locator("iframe")).toHaveCount(0);
    await expect(page.getByTestId("browser-live-frame")).toHaveCount(0);

    await page.getByRole("button", { name: "一時停止" }).click();
    await expect(phase).toHaveText("一時停止中 — 引き継げます");

    await page.getByRole("button", { name: "引き継ぐ（60 秒）" }).click();
    await expect(phase).toContainText("あなたが操作中 — 残り");
    await expect(bar).toHaveAttribute("data-operating", "true");
    const firstExpiry = control().lease_expires_at ?? 0;

    // gateway の assertion は 20 秒で切れるので、時計は 10 秒だけ進める。
    offset += 10_000;
    await page.getByRole("button", { name: /^延長（60 秒）/ }).click();
    await expect.poll(() => control().lease_expires_at ?? 0).toBeGreaterThanOrEqual(firstExpiry + 10);
    await expect(phase).toContainText("あなたが操作中 — 残り");

    // 返すには 2 つの確認が要る。
    const resume = page.getByRole("button", { name: "エージェントに返す" });
    await expect(resume).toBeDisabled();
    await page.getByLabel("画面の最新の状態（fresh snapshot）を確かめた").check();
    await page.getByLabel("開いているサイトが許可された origin であることを確かめた").check();
    await resume.click();
    await expect(phase).toHaveText("監視のみ — エージェントが操作中");
    await expect(bar).toHaveAttribute("data-operating", "false");
    const resumed = backend.records.control.at(-1)?.body.command as Record<string, unknown>;
    expect(resumed).toMatchObject({ kind: "resume", fresh_snapshot: true, policy_origin_ok: true });

    // 取り直してから画面を離れる。route 遷移の unmount で release beacon が飛ぶ。
    await page.getByRole("button", { name: "一時停止" }).click();
    await expect(phase).toHaveText("一時停止中 — 引き継げます");
    await page.getByRole("button", { name: "引き継ぐ（60 秒）" }).click();
    await expect(phase).toContainText("あなたが操作中");
    const before = backend.records.disconnect.length;
    await page.getByRole("link", { name: "ブラウザ" }).first().click();
    await expect(page).toHaveURL(/\/browser\/?$/);
    await expect.poll(() => backend.records.disconnect.length).toBe(before + 1);
    expect(backend.records.disconnect.at(-1)?.key).toBe(KEY);
    expect(control().phase).toBe("paused");
    expect(control().lease_holder).toBeNull();
  } finally {
    await gateway.close();
  }
});

test("pagehide releases the lease and an expired lease shows paused", async ({ page }) => {
  const gateway = await startBrowserGateway({ liveUpstream: false, backend: { credentialWait: false } });
  const backend = gateway.daemon.browser;
  if (!backend) throw new Error("browser backend is not enabled");
  try {
    await gateway.loginAsOwner(page);
    backend.setControl("T1", "R1", "S1", { phase: "paused" });
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    const phase = page.getByTestId("browser-control-phase");
    await page.getByRole("button", { name: "引き継ぐ（60 秒）" }).click();
    await expect(phase).toContainText("あなたが操作中");

    // 第 3 層: lease の期限を過ぎると paused に落ち、期限切れを明示する（agent には戻らない）。
    backend.setControl("T1", "R1", "S1", { lease_expires_at: Math.floor(Date.now() / 1000) - 1 });
    await expect(phase).toHaveText("一時停止中 — 引き継げます");
    await expect(page.getByTestId("browser-control-expired")).toBeVisible();

    // 第 2 層（pagehide）: 取り直してから別の document へ移る。
    await page.getByRole("button", { name: "引き継ぐ（60 秒）" }).click();
    await expect(phase).toContainText("あなたが操作中");
    const before = backend.records.disconnect.length;
    await page.goto(`${gateway.base}/browser`);
    await expect.poll(() => backend.records.disconnect.length).toBe(before + 1);
    expect(backend.control.get("T1/R1/S1")?.phase).toBe("paused");
  } finally {
    await gateway.close();
  }
});

test("a run with Live View frames shows the read-only band and no control bar or takeover", async ({ page }) => {
  // 本番の `runtime = "launcher"`: launcher が映像の frame を出し（framesAvailable）、dashboard の中継は無い。
  const gateway = await startBrowserGateway({
    liveUpstream: false,
    backend: { credentialWait: false, framesAvailable: true },
  });
  try {
    await gateway.loginAsOwner(page);
    const frames: string[] = [];
    await page.routeWebSocket("**/browser/live/T1/R1/frames", (socket) => {
      frames.push(socket.url());
    });
    await page.setViewportSize({ width: 360, height: 800 });
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    const band = page.getByTestId("browser-control-bar");
    await expect(band).toContainText("読み取り専用");
    await expect(band).toContainText("launcher run では入力・引き継ぎ操作を利用できません。");
    await expect(page.getByTestId("browser-live-frame")).toBeVisible();
    // 映像は gateway の同一 origin の WebSocket から読む（iframe は使わない）。
    await expect.poll(() => frames.map((url) => new URL(url).pathname)).toEqual(["/browser/live/T1/R1/frames"]);
    await expect(page.locator("iframe")).toHaveCount(0);
    // control bar の状態・lease の操作は出さない。
    await expect(page.getByTestId("browser-control-phase")).toHaveCount(0);
    for (const name of ["一時停止", "引き継ぐ（60 秒）", "エージェントに返す"])
      await expect(page.getByRole("button", { name })).toHaveCount(0);
    // 狭い幅でも帯 → Live View → 待ち → イベントの順に並び、横に溢れない。
    const tops = await Promise.all(
      ["browser-control-bar", "browser-live-view", "browser-run-waits", "browser-live-events"].map(async (id) => {
        const box = await page.getByTestId(id).boundingBox();
        if (!box) throw new Error(`${id} has no box`);
        return box.y;
      }),
    );
    expect(tops).toEqual([...tops].sort((a, b) => a - b));
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(360);
  } finally {
    await gateway.close();
  }
});
