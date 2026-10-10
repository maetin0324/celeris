import { expect, test } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

test("owner receives the latest launcher frame and non-owner is refused", async ({ page, browser }) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false, framesAvailable: true } });
  try {
    await gateway.loginAsOwner(page);
    await page.routeWebSocket("**/browser/live/T1/R1/frames", (socket) => {
      socket.send(Buffer.from([0xff, 0xd8, 0xff, 0xd9]));
    });
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);
    await expect(page.getByTestId("browser-live-image")).toHaveAttribute("src", /^blob:/);
    await expect(page.getByText("読み取り専用の映像です。入力や操作はできません。")).toBeVisible();
    await expect(page.getByRole("button", { name: "引き継ぐ（60 秒）" })).toHaveCount(0);

    const guestContext = await browser.newContext();
    const guest = await guestContext.newPage();
    await guest.goto(`${gateway.base}/browser/runs/T1/R1`);
    await expect(guest.getByRole("button", { name: "ログイン" })).toBeVisible();
    await expect(guest.getByTestId("browser-live-image")).toHaveCount(0);
    await guest.close();
    await guestContext.close();
  } finally {
    await gateway.close();
  }
});
