import { expect, type Locator, type Page, test, type WebSocketRoute } from "@playwright/test";
import { startBrowserGateway } from "../support/browser-gateway";

// Chromium 自身の canvas で 16x16 の単色 JPEG を作る（decode できる実 JPEG）。
async function solidJpeg(page: Page, fill: string): Promise<Buffer> {
  const base64 = await page.evaluate((color) => {
    const canvas = document.createElement("canvas");
    canvas.width = 16;
    canvas.height = 16;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2d context unavailable");
    ctx.fillStyle = color;
    ctx.fillRect(0, 0, 16, 16);
    return canvas.toDataURL("image/jpeg").split(",")[1];
  }, fill);
  const jpeg = Buffer.from(base64, "base64");
  // SOI と EOI が付いた完全な JPEG であることを先に確かめる。
  expect(jpeg.subarray(0, 2)).toEqual(Buffer.from([0xff, 0xd8]));
  expect(jpeg.subarray(-2)).toEqual(Buffer.from([0xff, 0xd9]));
  return jpeg;
}

// img を decode し直して、natural size と中央 1 px の色を読む（decode 失敗は evaluate が reject する）。
async function decodedFrame(image: Locator): Promise<{ width: number; rgb: number[] }> {
  return image.evaluate(async (img: HTMLImageElement) => {
    await img.decode();
    const canvas = document.createElement("canvas");
    canvas.width = img.naturalWidth;
    canvas.height = img.naturalHeight;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("2d context unavailable");
    ctx.drawImage(img, 0, 0);
    const [r, g, b] = ctx.getImageData(Math.floor(img.naturalWidth / 2), Math.floor(img.naturalHeight / 2), 1, 1).data;
    return { width: img.naturalWidth, rgb: [r, g, b] };
  });
}

test("owner sees a decoded launcher frame, a newer frame replaces it, and non-owner is refused", async ({
  page,
  browser,
}) => {
  const gateway = await startBrowserGateway({ backend: { credentialWait: false, framesAvailable: true } });
  try {
    await gateway.loginAsOwner(page);
    const red = await solidJpeg(page, "#ff0000");
    const blue = await solidJpeg(page, "#0000ff");
    // 2 枚目の送信に使うため、接続を捕まえておく（let の代入は callback 内で行うので箱に入れる）。
    const route: { socket?: WebSocketRoute } = {};
    await page.routeWebSocket("**/browser/live/T1/R1/frames", (socket) => {
      route.socket = socket;
      socket.send(red);
    });
    await page.goto(`${gateway.base}/browser/runs/T1/R1`);

    const image = page.getByTestId("browser-live-image");
    await expect(image).toHaveAttribute("src", /^blob:/);
    await expect.poll(async () => (await decodedFrame(image)).width).toBe(16);
    expect((await decodedFrame(image)).rgb[0]).toBeGreaterThan(200);
    const firstSrc = await image.getAttribute("src");

    expect(route.socket).toBeDefined();
    route.socket?.send(blue);
    await expect(image).not.toHaveAttribute("src", firstSrc ?? "");
    await expect.poll(async () => (await decodedFrame(image)).width).toBe(16);
    expect((await decodedFrame(image)).rgb[2]).toBeGreaterThan(200);

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
