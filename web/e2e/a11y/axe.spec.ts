import { createRequire } from "node:module";
import { expect, test } from "@playwright/test";
import { startGateway } from "../support/gateway";
import { v3Screens } from "../support/screens";

const axePath = createRequire(import.meta.url).resolve("axe-core/axe.min.js");
test.use({ bypassCSP: true }); // axe の注入だけに適用。配信する CSP は変更しない。
for (const screen of v3Screens()) {
  test(`S4 ${screen.path}: axe serious/critical, name, structure and focus`, async ({ page }) => {
    const gateway = await startGateway();
    try {
      // 現在地と同じ link は遷移しないので、"/" は別の画面から始める。
      await page.goto(screen.path === "/" ? `${gateway.base}/help` : gateway.base);
      const nav = page.getByRole("navigation", { name: "主要" });
      await expect(nav).toBeAttached();
      const link = nav.getByRole("link", { name: screen.heading });
      const hasNavLink = (await link.count()) > 0;
      if (hasNavLink) await link.click();
      else await page.goto(`${gateway.base}${screen.fixture}`);
      const heading = page.getByRole("heading", { level: 1, name: screen.heading });
      if (hasNavLink) await expect(heading).toBeFocused();
      else await expect(heading).toBeVisible();
      await expect(page.getByRole("main")).toHaveCount(1);
      await expect(page.getByRole("navigation", { name: "主要" })).toHaveCount(1);
      await page.addScriptTag({ path: axePath });
      const violations = await page.evaluate(async () => {
        const axe = (
          window as unknown as Window & {
            axe: { run: () => Promise<{ violations: Array<{ id: string; impact: string }> }> };
          }
        ).axe;
        return (await axe.run()).violations.filter((item) => item.impact === "critical" || item.impact === "serious");
      });
      expect(violations).toEqual([]);
    } finally {
      await gateway.close();
    }
  });
}
