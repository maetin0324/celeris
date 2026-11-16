import { rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { expect, type Locator, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { recordLatency } from "../support/latency-results";
import { v3Screens } from "../support/screens";
import { makeTmpDir } from "../support/tmp-dir";
import { waitForBootIdle } from "./boot-idle.mjs";
import { inAppRoutes } from "./in-app-routes";

// click の actionability（visible・stable・hit test）は計測区間の前に trial で確かめ、区間では
// force の click（mouse の move・down・up）だけを打つ。負荷下では actionability の rAF 待ちと往復が
// click 1 回に 150〜300ms かかり、click イベントの前に予算を使い切っていた（ADR-0081 付記）。
async function armClick(link: Locator): Promise<() => Promise<void>> {
  await link.click({ trial: true });
  return () => link.click({ force: true });
}

// 画面データ = 画面の枠（[data-screen]）の子のうち、見出し（h1）と page header（パンくず・見出し・説明・主操作）を
// 除いた本文。h1 の兄弟は本文ではない: header の slot を持つ画面では h1 は header の中にあり、兄弟は説明文か
// （説明あり）、無い（パンくず・主操作だけ。/knowledge・/knowledge/inbox・/tasks/$id/runs/$runId・/cron/$id）。
function screenBody(heading: Locator): Locator {
  return heading
    .locator("xpath=ancestor::*[@data-screen][1]")
    .locator(':scope > :not(h1):not([data-slot="page-header"])')
    .first();
}

// ホーム（/）は統括 CoS を開き、URL を /?thread=<統括 CoS の thread> に置き換える
// （ADR 2026-10-28-single-human-window-general-cos D5）。偽 daemon の統括 CoS は chat-main。
// 現在地と同じ link は遷移しないので、ホームへの遷移は別の画面（/help）から始める（a11y/axe.spec.ts と同じ）。
const landingOf = (fixture: string) => (fixture === "/" ? "/?thread=chat-main" : fixture);
const startOf = (fixture: string) => (fixture === "/" ? "/help" : "/");

// baseline と同じくクリックを起点に URL、見出し、画面データ（現 Phase は準備中の枠）を別々に測る。
for (const screen of v3Screens()) {
  test(`S1 ${screen.path}: JSON 0/5/10s でも shell 遷移は独立`, async ({ page }) => {
    test.setTimeout(120_000);
    const dir = makeTmpDir("celeris-v3-latency-");
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const entry = inAppRoutes[screen.fixture];
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, fixtures: { ...defaultFixtures, ...entry?.fixtures } });
    const daemonUrl = await daemon.start();
    const gateway = await startGateway({ daemonUrl, daemonTokenFile: tokenFile });
    try {
      const measures: Array<{ delay: number; url: number; heading: number; data: number }> = [];
      // 初回だけの chunk 読み込み等は共用 host の負荷で大きく揺れるため、各画面で
      // 同じ遷移を計測外に 1 回実行する。cold 値は gate に使わず、退行確認用に記録する。
      daemon.setDelay(0);
      let warmupNavigate: () => Promise<void>;
      if (entry) {
        await page.goto(`${gateway.base}${entry.parent}`);
        const link = page.locator(`main a[href="${screen.fixture}"]`).first();
        await expect(link).toBeVisible();
        await waitForBootIdle(page);
        warmupNavigate = await armClick(link);
      } else {
        await page.goto(`${gateway.base}${startOf(screen.fixture)}`);
        await waitForBootIdle(page);
        const link = page
          .getByRole("navigation", { name: "主要", exact: true })
          .getByRole("link", { name: screen.heading });
        if (await link.count()) warmupNavigate = await armClick(link);
        else
          warmupNavigate = () =>
            page.evaluate((to) => {
              history.pushState({}, "", to);
              dispatchEvent(new PopStateEvent("popstate", { state: history.state }));
            }, screen.fixture);
      }
      const coldStart = performance.now();
      await warmupNavigate();
      await expect(page).toHaveURL(`${gateway.base}${landingOf(screen.fixture)}`);
      const coldUrl = performance.now() - coldStart;
      const coldHeadingNode = page.getByRole("heading", { level: 1, name: screen.heading });
      await expect(coldHeadingNode).toBeVisible();
      const coldHeading = performance.now() - coldStart;
      await expect(screenBody(coldHeadingNode)).toBeVisible();
      const coldPath = { url: coldUrl, heading: coldHeading, data: performance.now() - coldStart };
      for (const delay of [0, 5000, 10000]) {
        // 計測区間は click（またはアプリ内の popstate）から。文書の読み込み・起動は区間の外で済ませる
        // （ADR-0081 付記、人の決定 b。nav にリンクの無い経路は in-app-routes.ts の親画面から）。
        let navigate: () => Promise<void>;
        if (entry) {
          // 親画面のリンクは data で描かれるので、親は遅延 0 で開き、リンクが出てから遅延を戻す。
          daemon.setDelay(0);
          await page.goto(`${gateway.base}${entry.parent}`);
          const link = page.locator(`main a[href="${screen.fixture}"]`).first();
          await expect(link).toBeVisible();
          await waitForBootIdle(page);
          daemon.setDelay(delay);
          navigate = await armClick(link);
        } else {
          daemon.setDelay(delay);
          await page.goto(`${gateway.base}${startOf(screen.fixture)}`);
          // goto 直後の起動の long task を計測に含めない（ADR-0081 付記、人の決定 b）。
          await waitForBootIdle(page);
          const link = page
            .getByRole("navigation", { name: "主要", exact: true })
            .getByRole("link", { name: screen.heading });
          if (await link.count()) navigate = await armClick(link);
          else
            navigate = () =>
              page.evaluate((to) => {
                history.pushState({}, "", to);
                dispatchEvent(new PopStateEvent("popstate", { state: history.state }));
              }, screen.fixture);
        }
        const start = performance.now();
        await navigate();
        await expect(page).toHaveURL(`${gateway.base}${landingOf(screen.fixture)}`);
        const url = performance.now() - start;
        const headingNode = page.getByRole("heading", { level: 1, name: screen.heading });
        await expect(headingNode).toBeVisible();
        const heading = performance.now() - start;
        await expect(screenBody(headingNode)).toBeVisible();
        const data = performance.now() - start;
        measures.push({ delay, url, heading, data });
        expect(url, `${screen.path} URL @${delay}`).toBeLessThanOrEqual(300);
        expect(heading, `${screen.path} heading @${delay}`).toBeLessThanOrEqual(300);
      }
      process.stdout.write(`S1 ${screen.path} ${JSON.stringify({ coldPath, measures })}\n`);
      recordLatency({ kind: "S1", path: screen.path, fixture: screen.fixture, coldPath, measures });
      expect(Math.abs(measures[2].url - measures[0].url), "URL 10s-0s").toBeLessThanOrEqual(100);
      expect(Math.abs(measures[2].heading - measures[0].heading), "heading 10s-0s").toBeLessThanOrEqual(100);
    } finally {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
}
