import { createRequire } from "node:module";
import { expect, test } from "@playwright/test";
import { startGateway } from "../support/gateway";
import { screens } from "../support/screens";

const axePath = createRequire(import.meta.url).resolve("axe-core/axe.min.js");
test.use({ bypassCSP: true }); // axe の注入だけに適用。配信する CSP は変更しない。
for (const screen of screens.filter((item) => item.path === "/tasks" || item.path === "/inbox")) {
  test(`S4 ${screen.path}: axe serious/critical, name, structure and focus`, async ({ page }) => {
    const gateway = await startGateway();
    try {
      await page.goto(`${gateway.base}/`);
      await page.getByRole("navigation", { name: "主要" }).getByRole("link", { name: screen.heading }).click();
      const heading = page.getByRole("heading", { level: 1, name: screen.heading });
      await expect(heading).toBeFocused();
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
