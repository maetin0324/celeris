import { expect, test } from "@playwright/test";
import { seriousViolations } from "../support/axe";
import { startFixtureGateway } from "../support/fixture-gateway";
import { screens } from "../support/screens";

// P5-03: screens.ts の全行（/login を含む）を偽 daemon の fixture で描いて検査する。
test.use({ bypassCSP: true }); // axe の注入だけに適用。配信する CSP は変更しない。
let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;
test.beforeAll(async () => {
  gateway = await startFixtureGateway();
});
test.afterAll(async () => {
  await gateway.close();
});

for (const screen of screens) {
  test(`S4 ${screen.path}: axe serious/critical, name, structure and focus`, async ({ page }) => {
    const heading = page.getByRole("heading", { level: 1, name: screen.heading });
    if (screen.path === "/login") {
      // /login は shell の外の画面なので主要 nav を持たない。
      await page.goto(`${gateway.base}${screen.fixture}`);
      await expect(heading).toBeVisible();
      await expect(page.getByRole("main")).toHaveCount(1);
    } else {
      // 現在地と同じ link は遷移しないので、"/" は別の画面から始める。
      await page.goto(screen.path === "/" ? `${gateway.base}/help` : gateway.base);
      const nav = page.getByRole("navigation", { name: "主要" });
      await expect(nav).toBeAttached();
      const link = nav.getByRole("link", { name: screen.heading, exact: true });
      const hasNavLink = (await link.count()) > 0;
      if (hasNavLink) await link.click();
      else await page.goto(`${gateway.base}${screen.fixture}`);
      if (hasNavLink) await expect(heading).toBeFocused();
      else await expect(heading).toBeVisible();
      await expect(page.getByRole("main")).toHaveCount(1);
      await expect(page.getByRole("navigation", { name: "主要" })).toHaveCount(1);
    }
    expect(await seriousViolations(page)).toEqual([]);
  });
}
