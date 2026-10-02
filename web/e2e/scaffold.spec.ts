import { expect, test } from "@playwright/test";

test("/login の枠が表示される", async ({ page }) => {
  await page.goto("/login");
  await expect(page.getByRole("heading", { name: "Celeris にログイン" })).toBeVisible();
});
