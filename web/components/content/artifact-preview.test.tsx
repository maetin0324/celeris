import { mkdir } from "node:fs/promises";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { OVERLAY_FIXTURE_TIMEOUT, openOverlayFixture, seriousViolations } from "../ui/overlay-browser-test";
import { artifactKind } from "./artifact-kind";

describe("artifactKind", () => {
  it("拡張子で表示の種類を決め、表に無い形式は download だけにする", () => {
    expect(artifactKind("報告.md")).toBe("markdown");
    expect(artifactKind("a/B.MARKDOWN")).toBe("markdown");
    for (const name of ["run.log", "data.csv", "x.json", "main.rs", "out.jsonl", "README"])
      expect(artifactKind(name), name).toBe("text");
    for (const name of ["a.png", "b.JPG", "c.webp"]) expect(artifactKind(name), name).toBe("image");
    expect(artifactKind("fig.svg")).toBe("svg");
    expect(artifactKind("paper.pdf")).toBe("pdf");
    expect(artifactKind("page.html")).toBe("html");
    expect(artifactKind("page.htm")).toBe("html");
    for (const name of ["a.zip", "b.tar.gz", "noext", "x.exe"]) expect(artifactKind(name), name).toBe("other");
  });
});

describe("ArtifactPreview", () => {
  let fixture: Awaited<ReturnType<typeof openOverlayFixture>>;
  beforeAll(async () => {
    fixture = await openOverlayFixture("/components/content/fixtures/preview.html");
  }, OVERLAY_FIXTURE_TIMEOUT);
  afterAll(async () => {
    await fixture?.close();
  });

  it("取得失敗から再取得し、閉じて開き直した後も本文を回復できる", async () => {
    const { page } = fixture;
    const md = page.locator("[data-artifact-kind='markdown']");
    await page.setViewportSize({ width: 360, height: 800 });
    let fail = true;
    let requests = 0;
    await page.route("**/files/tasks/T1/artifacts/0", (route) => {
      requests += 1;
      return route.fulfill({ status: fail ? 503 : 200, body: fail ? "unavailable" : "取得した本文" });
    });
    await md.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    await page.getByRole("alert").waitFor();
    fail = false;
    await page.getByRole("button", { name: "再試行", exact: true }).click();
    await page.getByText("取得した本文", { exact: true }).waitFor();
    expect(await page.getByRole("alert").count()).toBe(0);
    expect(requests).toBe(2);
    await md.getByRole("button", { name: "本文を閉じる", exact: true }).click();
    fail = true;
    await md.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    await page.getByRole("alert").waitFor();
    await md.getByRole("button", { name: "本文を閉じる", exact: true }).click();
    fail = false;
    await md.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    await page.getByText("取得した本文", { exact: true }).waitFor();
    expect(await page.getByRole("alert").count()).toBe(0);
  });

  it("text は offset/length で分けて読み、UTF-8 の境目をまたいでも行番号付きで続きを足す", async () => {
    const { page } = fixture;
    await page.setViewportSize({ width: 360, height: 800 });
    // 16 byte ごと。「あ」(3 byte) が 16 byte 目の境目で割れる。
    const body = Buffer.from("line-one\nabcdefあいう\n3行目の長い行です長い行です長い行です\n");
    const offsets: string[] = [];
    await page.route(/\/files\/tasks\/T1\/artifacts\/2\?/, (route) => {
      const url = new URL(route.request().url());
      offsets.push(`${url.searchParams.get("offset")}+${url.searchParams.get("length")}`);
      const offset = Number(url.searchParams.get("offset"));
      const length = Number(url.searchParams.get("length"));
      return route.fulfill({
        status: 200,
        headers: { "content-type": "text/plain; charset=utf-8", "x-celeris-size": String(body.length) },
        body: body.subarray(offset, offset + length),
      });
    });
    const row = page.locator("[data-artifact-kind='text']");
    await row.getByRole("button", { name: "本文をここで見る", exact: true }).click();
    const text = row.getByRole("region", { name: "run.log の本文" });
    await text.getByText("line-one").waitFor();
    expect(offsets).toEqual(["0+16"]);
    const more = row.getByRole("button", { name: /続きを読む/ });
    const last = row.getByText(`3 行・${body.length} B`, { exact: true });
    // 続きの範囲を 1 つずつ読む。読み込み中はボタンが消えるので、ボタンか読み終わりの要約が出るのを待ってから進む。
    for (;;) {
      await more.or(last).first().waitFor();
      if ((await last.count()) > 0) break;
      await more.click();
    }
    expect(offsets.length).toBe(Math.ceil(body.length / 16));
    expect(await text.locator("tr").count()).toBe(3);
    expect(await text.locator("tr").nth(1).innerText()).toBe("2\tabcdefあいう");
    expect(await text.getAttribute("data-wrap")).toBe("true");
    await row.getByRole("button", { name: "折り返しを解除", exact: true }).click();
    expect(await text.getAttribute("data-wrap")).toBe("false");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await row.getByRole("button", { name: "本文を閉じる", exact: true }).click();
  }, 20_000);

  it("4幅で download の44px targetと本文の折り返しを保つ", async () => {
    const { page } = fixture;
    const directory = process.env.PREVIEW_SHOTS_DIR;
    if (directory) await mkdir(directory, { recursive: true });
    for (const width of [360, 390, 412, 1440]) {
      await page.setViewportSize({ width, height: 800 });
      const link = page
        .locator("[data-artifact-kind='other']")
        .getByRole("link", { name: "ダウンロード", exact: true });
      const bounds = await link.boundingBox();
      expect(bounds?.width).toBeGreaterThanOrEqual(44);
      expect(bounds?.height).toBeGreaterThanOrEqual(44);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      expect(await seriousViolations(page)).toEqual([]);
      if (directory) await page.screenshot({ path: join(directory, `preview-${width}.png`) });
    }
  });
});
