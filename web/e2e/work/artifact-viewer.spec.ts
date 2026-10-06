import { mkdir } from "node:fs/promises";
import path from "node:path";
import { crc32, deflateSync } from "node:zlib";
import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// 成果物のブラウザ内表示（ADR 2026-10-05-web-artifact-inline-view）。実の gateway（web/server/files.js）と偽 daemon で、
// markdown・text・画像・SVG・PDF・HTML（script 付きを含む）・未対応の形式を /tasks/T1?tab=artifacts で開く。
// 偽 daemon は実 daemon と同じく html・svg・pdf を application/octet-stream と filename で返す（api.md §3.8 の表に無い）。
// ARTIFACT_SHOTS_DIR を与えると 360/390/412/1440 の screenshot を残す。

test.describe.configure({ mode: "default" });

function png(width: number, height: number): Buffer {
  const chunk = (type: string, data: Buffer) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
    const crc = Buffer.alloc(4);
    crc.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, crc]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const rows = Buffer.alloc((width * 3 + 1) * height);
  for (let y = 0; y < height; y += 1)
    for (let x = 0; x < width; x += 1) {
      const at = y * (width * 3 + 1) + 1 + x * 3;
      rows.set([(x * 255) / width, (y * 255) / height, 160], at);
    }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", header),
    chunk("IDAT", deflateSync(rows)),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}

const pdf = `%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 100]>>endobj
trailer<</Root 1 0 R>>
%%EOF
`;
const longLog = Array.from(
  { length: 5000 },
  (_, i) => `${String(i + 1).padStart(5, "0")} 実行ログの行 ${"x".repeat(20)}`,
).join("\n");
const scriptHtml = `<!doctype html><html><head><title>報告</title></head><body>
<h1>HTML の報告</h1><p id="m">静的な本文</p>
<form action="/api/tasks/T1/cancel" method="post"><button id="send">送信</button></form>
<script>
document.getElementById("m").textContent = "script が走った";
try { parent.document.title = "pwned"; } catch (e) {}
try { top.__pwned = 1; } catch (e) {}
try { localStorage.setItem("pwned", "1"); } catch (e) {}
</script></body></html>`;
const scriptSvg = `<svg xmlns="http://www.w3.org/2000/svg" width="40" height="30"><rect width="40" height="30" fill="teal"/><script>parent.__pwned = 1</script></svg>`;

const names = [
  "報告.md",
  "run.log",
  "figure.png",
  "diagram.svg",
  "paper.pdf",
  "report.html",
  "attack.html",
  "bundle.zip",
] as const;
const bodies: Record<(typeof names)[number], string | Buffer> = {
  "報告.md": "# 報告の見出し\n\n本文の段落です。\n",
  "run.log": longLog,
  "figure.png": png(64, 48),
  "diagram.svg": scriptSvg,
  "paper.pdf": pdf,
  "report.html": "<!doctype html><h1>HTML の報告</h1><p style='color:teal'>inline style は効く</p>",
  "attack.html": scriptHtml,
  "bundle.zip": Buffer.from([0x50, 0x4b, 0x03, 0x04]),
};
const types: Partial<Record<(typeof names)[number], string>> = {
  "報告.md": "text/markdown; charset=utf-8",
  "run.log": "text/plain; charset=utf-8",
  "figure.png": "image/png",
};

async function open(page: Page, base: string) {
  await page.goto(`${base}/tasks/T1?tab=artifacts`);
  await expect(page.getByTestId("task-artifacts").locator("[data-artifact]")).toHaveCount(names.length);
}

function row(page: Page, idx: number) {
  return page.getByTestId("task-artifacts").locator(`[data-artifact='${idx}']`);
}

test.describe("成果物のブラウザ内表示", () => {
  let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;
  test.beforeAll(async () => {
    gateway = await startFixtureGateway({
      fixtures: {
        "/api/v1/tasks/T1/artifacts": {
          task_id: "T1",
          items: names.map((name, idx) => ({
            idx,
            run_id: "R1",
            ts: "2026-10-05T00:00:00Z",
            exists: true,
            forbidden: false,
            size: Buffer.byteLength(bodies[name]),
            sha256_matches: idx !== 1,
            artifact: { kind: "file", name, path: `reports/${name}`, sha256: `${idx}`.repeat(64) },
          })),
        },
      },
      files: Object.fromEntries(
        names.map((name, idx) => [
          `/api/v1/tasks/T1/artifacts/${idx}`,
          {
            body: bodies[name],
            type: types[name] ?? "application/octet-stream",
            disposition: `inline; filename="x"; filename*=UTF-8''${encodeURIComponent(name)}`,
          },
        ]),
      ),
    });
  });
  test.afterAll(async () => {
    await gateway?.close();
  });

  test("markdown・text（範囲取得と行番号）・未対応の形式", async ({ page }) => {
    await open(page, gateway.base);
    const md = row(page, 0);
    await md.getByRole("button", { name: "本文をここで見る" }).click();
    await expect(md.getByRole("heading", { name: "報告の見出し" })).toBeVisible();
    await expect(md.getByTestId("artifact-sha256")).toHaveAttribute("title", `sha256 ${"0".repeat(64)}`);

    const log = row(page, 1);
    await expect(log.getByText("記録後に変更あり")).toBeVisible();
    const offsets: string[] = [];
    page.on("request", (request) => {
      const url = new URL(request.url());
      if (url.pathname === "/files/tasks/T1/artifacts/1") offsets.push(url.search);
    });
    await log.getByRole("button", { name: "本文をここで見る" }).click();
    const text = log.getByRole("region", { name: "run.log の本文" });
    await expect(text.locator("tr").first()).toHaveText(/^1\s*00001 実行ログの行/);
    expect(offsets).toEqual(["?offset=0&length=131072"]);
    await expect(log.getByText(/128\.0 KB \/ \d+\.\d KB を表示中/)).toBeVisible();
    const before = await text.locator("tr").count();
    expect(before).toBeLessThan(5000);
    await log.getByRole("button", { name: /続きを読む/ }).click();
    await expect.poll(() => text.locator("tr").count()).toBe(5000);
    expect(offsets).toEqual(["?offset=0&length=131072", "?offset=131072&length=131072"]);
    await expect(text.locator("tr").last()).toHaveText(/^5000\s*05000 実行ログの行/);
    await expect(log.getByRole("button", { name: /続きを読む/ })).toHaveCount(0);
    await expect(text).toHaveAttribute("data-wrap", "true");
    await log.getByRole("button", { name: "折り返しを解除" }).click();
    await expect(text).toHaveAttribute("data-wrap", "false");

    const zip = row(page, 7);
    await expect(zip.getByRole("button", { name: "本文をここで見る" })).toHaveCount(0);
    await expect(zip.getByRole("link", { name: "新しいタブで開く" })).toHaveCount(0);
    await expect(zip.getByRole("link", { name: "ダウンロード" })).toHaveAttribute(
      "href",
      "/files/tasks/T1/artifacts/7?download=1",
    );
  });

  test("画像の拡大・縮小と SVG（script は走らない）", async ({ page }) => {
    await open(page, gateway.base);
    const image = row(page, 2);
    await image.getByRole("button", { name: "本文をここで見る" }).click();
    const img = image.getByRole("img", { name: "figure.png" });
    await expect.poll(() => img.evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(64);
    const zoom = image.getByTestId("artifact-image-zoom");
    await expect(zoom).toHaveText("全体を表示（64×48）");
    await image.getByRole("button", { name: "拡大" }).click();
    await expect(zoom).toHaveText("150%（64×48）");
    await expect.poll(() => img.evaluate((el) => el.getBoundingClientRect().width)).toBe(96);
    await image.getByRole("button", { name: "拡大" }).click();
    await expect(zoom).toHaveText("200%（64×48）");
    await image.getByRole("button", { name: "縮小" }).click();
    await image.getByRole("button", { name: "縮小" }).click();
    await image.getByRole("button", { name: "縮小" }).click();
    await expect(zoom).toHaveText("75%（64×48）");
    await expect.poll(() => img.evaluate((el) => el.getBoundingClientRect().width)).toBe(48);
    await image.getByRole("button", { name: "全体を表示" }).click();
    await expect(zoom).toHaveText("全体を表示（64×48）");

    const svg = row(page, 3);
    await svg.getByRole("button", { name: "本文をここで見る" }).click();
    const svgImg = svg.getByRole("img", { name: "diagram.svg" });
    await expect(svgImg).toHaveAttribute("src", "/files/tasks/T1/artifacts/3?view=1");
    await expect.poll(() => svgImg.evaluate((el: HTMLImageElement) => el.naturalWidth)).toBe(40);
    expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  });

  test("PDF は browser の viewer（object）で、応答は inline の application/pdf", async ({ page }) => {
    await open(page, gateway.base);
    const doc = row(page, 4);
    await doc.getByRole("button", { name: "本文をここで見る" }).click();
    const object = doc.locator("object[type='application/pdf']");
    await expect(object).toHaveAttribute("data", "/files/tasks/T1/artifacts/4?view=1");
    await expect(object).toHaveAttribute("aria-label", "paper.pdf（PDF）");
    const response = await page.request.get(`${gateway.base}/files/tasks/T1/artifacts/4?view=1`);
    expect(response.headers()["content-type"]).toBe("application/pdf");
    expect(response.headers()["content-disposition"]).toMatch(/^inline;/);
    expect(response.headers()["x-content-type-options"]).toBe("nosniff");
    expect(response.headers()["content-security-policy"]).toContain("frame-ancestors 'self'");
    expect((await response.body()).subarray(0, 5).toString()).toBe("%PDF-");
    await expect(doc.getByRole("link", { name: "新しいタブで開く", exact: true })).toHaveAttribute(
      "href",
      "/files/tasks/T1/artifacts/4?view=1",
    );
  });

  test("HTML は sandbox の iframe で表示し、script・form は同一 origin でも別 origin でも動かない", async ({
    page,
    context,
  }) => {
    await open(page, gateway.base);
    const safe = row(page, 5);
    await safe.getByRole("button", { name: "本文をここで見る" }).click();
    const safeFrame = safe.locator("iframe");
    await expect(safeFrame).toHaveAttribute("sandbox", "");
    await expect(safeFrame).toHaveAttribute("src", "/files/tasks/T1/artifacts/5?view=1");
    await expect(
      page.frameLocator("[data-artifact='5'] iframe").getByRole("heading", { name: "HTML の報告" }),
    ).toBeVisible();

    const attack = row(page, 6);
    await attack.getByRole("button", { name: "本文をここで見る" }).click();
    const frame = page.frameLocator("[data-artifact='6'] iframe");
    await expect(frame.locator("#m")).toHaveText("静的な本文");
    const title = await page.title();
    expect(title).not.toBe("pwned");
    expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
    expect(await page.evaluate(() => localStorage.getItem("pwned"))).toBeNull();
    // form（POST）も sandbox で止まる。押しても daemon に cancel は届かない。
    const before = gateway.daemon.requests.filter((r) => r.path === "/api/v1/tasks/T1/cancel").length;
    await frame.locator("#send").click();
    expect(gateway.daemon.requests.filter((r) => r.path === "/api/v1/tasks/T1/cancel").length).toBe(before);
    // iframe の document は opaque origin（親の document に触れない）。
    const child = page.frames().find((f) => f.url().endsWith("/files/tasks/T1/artifacts/6?view=1"));
    expect(await child?.evaluate(() => window.origin)).toBe("null");

    // 新しいタブで開いても、応答の CSP sandbox で script は走らず opaque origin。
    const tab = await context.newPage();
    const response = await tab.goto(`${gateway.base}/files/tasks/T1/artifacts/6?view=1`);
    expect(response?.headers()["content-type"]).toBe("text/html; charset=utf-8");
    expect(response?.headers()["content-security-policy"]).toMatch(/^sandbox; default-src 'none';/);
    expect(response?.headers()["content-security-policy"]).not.toMatch(/allow-scripts|allow-same-origin/);
    await expect(tab.locator("#m")).toHaveText("静的な本文");
    expect(await tab.evaluate(() => window.origin)).toBe("null");
    await tab.close();

    // view=1 の無い URL は従来どおり: 拡張子で HTML にせず octet-stream のまま、frame-ancestors 'none'（H8）。
    const raw = await page.request.get(`${gateway.base}/files/tasks/T1/artifacts/6?download=1`);
    expect(raw.headers()["content-type"]).toBe("application/octet-stream");
    expect(raw.headers()["content-security-policy"]).toBe("sandbox; default-src 'none'; frame-ancestors 'none'");
    expect(raw.headers()["x-frame-options"]).toBe("DENY");
  });

  test("4 幅で横に溢れず、操作は 44px 以上（screenshot）", async ({ page }) => {
    const directory = process.env.ARTIFACT_SHOTS_DIR;
    if (directory) await mkdir(directory, { recursive: true });
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      await open(page, gateway.base);
      for (const idx of [0, 1, 2, 5]) await row(page, idx).getByRole("button", { name: "本文をここで見る" }).click();
      await expect(row(page, 2).getByRole("img", { name: "figure.png" })).toBeVisible();
      await expect(row(page, 1).getByRole("region", { name: "run.log の本文" })).toBeVisible();
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth),
      ).toBe(0);
      for (const idx of [0, 1, 2, 5, 7]) {
        const targets = row(page, idx).locator("button:visible, a:visible");
        for (let i = 0; i < (await targets.count()); i += 1) {
          const box = await targets.nth(i).boundingBox();
          if (await targets.nth(i).getAttribute("data-testid")) continue;
          expect(box?.height, `${width} ${idx} ${i}`).toBeGreaterThanOrEqual(44);
          expect(box?.width, `${width} ${idx} ${i}`).toBeGreaterThanOrEqual(44);
        }
      }
      if (directory) {
        await page.screenshot({ path: path.join(directory, `task-artifacts-open-${width}.png`), fullPage: true });
        await row(page, 6).getByRole("button", { name: "本文をここで見る" }).click();
        await row(page, 4).getByRole("button", { name: "本文をここで見る" }).click();
        await row(page, 6).scrollIntoViewIfNeeded();
        await page.screenshot({ path: path.join(directory, `task-artifacts-html-pdf-${width}.png`), fullPage: true });
      }
    }
  });
});
