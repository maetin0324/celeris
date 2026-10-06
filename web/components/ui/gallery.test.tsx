import { mkdir } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { OVERLAY_FIXTURE_TIMEOUT, openOverlayFixture, seriousViolations } from "./overlay-browser-test";

// 部品と状態の gallery fixture。GALLERY_SHOT_DIR（絶対 path）があれば 3 状態 × 4 幅の full page PNG を書く。
const fixturePath = "/components/ui/fixtures/gallery.html";
const sections = [
  "StatusBadge",
  "Badge と Button",
  "Kbd",
  "Section と Panel",
  "DataList",
  "Table",
  "CodeBlock と LogSurface",
  "Icon",
  "状態表示",
  "素の border",
  "Notice と入力",
  "ScrollTabs",
  "ConfirmDialog と Drawer",
];
const states = [
  { name: "既定", query: "", role: undefined, dialog: undefined },
  { name: "confirm", query: "?open=confirm", role: "alertdialog", dialog: "タスクを削除" },
  { name: "drawer", query: "?open=drawer", role: "dialog", dialog: "タスクの詳細" },
] as const;
const widths = [360, 390, 412, 1440];
const shotDir = process.env.GALLERY_SHOT_DIR;

describe("gallery fixture", () => {
  let fixture: Awaited<ReturnType<typeof openOverlayFixture>>;
  beforeAll(async () => {
    fixture = await openOverlayFixture(fixturePath);
  }, OVERLAY_FIXTURE_TIMEOUT);
  afterAll(async () => {
    await fixture?.close();
  });

  it("各節の見出しと StatusBadge の『実行中』『未確認』が見える", async () => {
    const { page } = fixture;
    for (const name of sections) {
      expect(await page.getByRole("heading", { level: 2, name, exact: true }).isVisible()).toBe(true);
    }
    expect(await page.getByText("実行中", { exact: true }).first().isVisible()).toBe(true);
    const unknown = page.locator('[data-status="mystery_state"]');
    expect(await unknown.textContent()).toBe("未確認");
    for (const state of ["loading", "empty", "error", "stale", "disconnected", "permission-denied"]) {
      expect(await page.locator(`[data-fetch-state="${state}"]`).count()).toBe(1);
    }
  });

  it("素の border は --color-border（#D3DCE2）で描く", async () => {
    const color = await fixture.page
      .getByTestId("plain-border")
      .evaluate((element) => getComputedStyle(element).borderTopColor);
    expect(color).toBe("rgb(211, 220, 226)");
  });

  it("色を指定しない入力欄の枠は --color-input（白地で 3:1 以上）で描く", async () => {
    const color = await fixture.page
      .getByTestId("plain-input")
      .evaluate((element) => getComputedStyle(element).borderTopColor);
    expect(color).toBe("rgb(117, 133, 147)");
    // 白地（bg-white）に対する相対輝度のコントラスト比（WCAG 2.x の式）。
    const [r, g, b] = (color.match(/\d+/g) ?? []).map((channel) => {
      const c = Number(channel) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    const luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    expect((1 + 0.05) / (luminance + 0.05)).toBeGreaterThanOrEqual(3);
  });

  it("既定・confirm・drawer の 3 状態で axe の serious/critical が 0", async () => {
    const { page } = fixture;
    const origin = new URL(page.url()).origin;
    if (shotDir) {
      if (!isAbsolute(shotDir)) throw new Error("GALLERY_SHOT_DIR は絶対 path で渡す");
      await mkdir(shotDir, { recursive: true });
    }
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const state of states) {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.goto(`${origin}${fixturePath}${state.query}`);
      // dialog が開くと背後は aria-hidden になるので、h1 は role でなく要素で待つ。
      await page.locator("h1").waitFor();
      if (state.role) await page.getByRole(state.role, { name: state.dialog }).waitFor();
      expect(await seriousViolations(page), state.name).toEqual([]);
      if (!shotDir) continue;
      for (const width of widths) {
        await page.setViewportSize({ width, height: 900 });
        // 幅の変更後の layout が落ち着くまで 2 frame 待つ。
        await page.evaluate(
          () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
        );
        await page.screenshot({ path: join(shotDir, `gallery-${state.name}-${width}.png`), fullPage: true });
      }
    }
  }, 60_000);
});
