import { expect, test } from "@playwright/test";
import { spaRoutePatterns } from "../../server/spa-routes.js";
import { seriousViolations } from "../support/axe";
import { startBrowserGateway } from "../support/browser-gateway";
import { startFixtureGateway } from "../support/fixture-gateway";
import { screens } from "../support/screens";

// P5-03 mobile・a11y の gate（X10）。全画面 × 幅 360 / 390 / 412 / 1440 で、axe の critical / serious が 0、
// ページ全体の横溢れが 0。44×44 と名前・構造は scripts/mobile-audit.mjs が同じ組で見る。
const WIDTHS = [360, 390, 412, 1440];
const fixtures = [...new Map(screens.map((screen) => [screen.fixture, screen])).values()];

test("parity-x: axe 全画面", () => {
  expect(screens.map((screen) => screen.path).sort()).toEqual([...spaRoutePatterns].sort());
  expect(WIDTHS).toEqual([360, 390, 412, 1440]);
});

test.use({ bypassCSP: true }); // axe の注入だけに適用。配信する CSP は変更しない。
let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;
test.beforeAll(async () => {
  gateway = await startFixtureGateway();
});
test.afterAll(async () => {
  await gateway.close();
});

for (const screen of fixtures) {
  test(`parity-x: axe ${screen.fixture} 4 幅で critical/serious 0・横溢れ 0`, async ({ page }) => {
    // /browser/settings の設定 form は本人（owner）の session にだけ出る（ADR 2026-10-08-browser-prod-enablement D3
    // 「web の扱い」。本人でなければ本人確認の案内だけ）。form（許可 origin 2 件）を 4 幅で検査するため、この画面だけは
    // owner socket を持つ browser gateway に本人として login して開く。
    const owner = screen.fixture === "/browser/settings" ? await startBrowserGateway() : null;
    try {
      if (owner) await owner.loginAsOwner(page);
      const base = owner?.base ?? gateway.base;
      for (const width of WIDTHS) {
        await page.setViewportSize({ width, height: 800 });
        await page.goto(`${base}${screen.fixture}`);
        await expect(page.getByRole("heading", { level: 1, name: screen.heading })).toBeVisible();
        if (owner) {
          await expect(page.getByRole("list", { name: "許可 origin の一覧" }).getByRole("listitem")).toHaveCount(2);
        }
        const overflow = await page.evaluate(
          () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
        );
        expect(overflow, `${screen.fixture} @${width}`).toBe(0);
        expect(await seriousViolations(page), `${screen.fixture} @${width}`).toEqual([]);
      }
    } finally {
      await owner?.close();
    }
  });
}
