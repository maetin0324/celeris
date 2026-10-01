import { expect, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { startFixtureGateway } from "../support/fixture-gateway";
import { screens } from "../support/screens";

// P5-03 mobile・a11y の gate（X10）。全画面 × 幅 360 / 390 / 412 / 1440 で、axe の critical / serious が 0、
// ページ全体の横溢れが 0。44×44 と名前・構造は scripts/mobile-audit.mjs が同じ組で見る。
const WIDTHS = [360, 390, 412, 1440];
const fixtures = [...new Map(screens.map((screen) => [screen.fixture, screen])).values()];

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
    for (const width of WIDTHS) {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(`${gateway.base}${screen.fixture}`);
      await expect(page.getByRole("heading", { level: 1, name: screen.heading })).toBeVisible();
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, `${screen.fixture} @${width}`).toBe(0);
      expect(await seriousViolations(page), `${screen.fixture} @${width}`).toEqual([]);
    }
  });
}
