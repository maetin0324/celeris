import { expect, type Page, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

// 共通部品の a11y（qa-shared）: focus-visible・reduced motion・入力欄の枠の contrast。
// functional の project で走る（playwright.config.ts の NFR 一覧には入れない）。
let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;
test.beforeAll(async () => {
  gateway = await startFixtureGateway();
});
test.afterAll(async () => {
  await gateway.close();
});

const outlineOf = (page: Page) =>
  page.locator("#main h1").evaluate((el) => {
    const style = getComputedStyle(el);
    return { style: style.outlineStyle, width: Number.parseFloat(style.outlineWidth) };
  });

test("pointer で遷移した後の h1 には focus 枠が出ず、keyboard で遷移すると出る", async ({ page }) => {
  await page.goto(gateway.base);
  const nav = page.getByRole("navigation", { name: "主要" });
  await nav.getByRole("link", { name: "タスク", exact: true }).click();
  const inbox = page.getByRole("heading", { level: 1, name: "タスク" });
  await expect(inbox).toBeFocused();
  expect(await outlineOf(page)).toEqual({ style: "none", width: expect.any(Number) });

  // keyboard で link を辿って遷移する。focus は h1 へ移り（S4 は保つ）、focus-visible の枠が出る。
  // Tab で keyboard の操作に切り替え、link を辿ってから Enter で遷移する。
  await page.keyboard.press("Tab");
  const help = nav.getByRole("link", { name: "ヘルプ", exact: true });
  await help.focus();
  expect(await help.evaluate((el) => el.matches(":focus-visible"))).toBe(true);
  await page.keyboard.press("Enter");
  const notifications = page.getByRole("heading", { level: 1, name: "ヘルプ" });
  await expect(notifications).toBeFocused();
  const outline = await outlineOf(page);
  expect(outline.style).toBe("solid");
  expect(outline.width).toBeGreaterThanOrEqual(2);
});

test("skip link は shell の最初の focus 先で、押すと main へ focus が移る", async ({ page }) => {
  await page.goto(`${gateway.base}/help`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  const path = new URL(page.url()).pathname;
  // header の最初の link から Shift+Tab で戻ると skip link に着く（tab 順で header より前）。
  await page.getByRole("link", { name: "Celeris", exact: true }).focus();
  await page.keyboard.press("Shift+Tab");
  const skip = page.getByRole("link", { name: "本文へ移動" });
  await expect(skip).toBeFocused();
  await expect(skip).toBeInViewport();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("main")).toBeFocused();
  expect(new URL(page.url()).pathname).toBe(path);
  await expect(page.getByRole("heading", { level: 1, name: "ヘルプ" })).toBeVisible();
  // 押した後は画面の外へ戻り、見出しの上に重ならない。
  await expect(skip).not.toBeInViewport();
});

// computed style の秒数（"0.1s, 1e-05s" のような列）の最大値。
const maxSeconds = (value: string) =>
  Math.max(0, ...value.split(",").map((part) => Number.parseFloat(part) * (part.trim().endsWith("ms") ? 0.001 : 1)));

const motionOf = (page: Page) =>
  page.evaluate(() => {
    const els = [...document.querySelectorAll<HTMLElement>("#root *")];
    return {
      count: els.length,
      transitions: els.map((el) => getComputedStyle(el).transitionDuration),
      animations: els.map((el) => getComputedStyle(el).animationDuration),
      scroll: getComputedStyle(document.documentElement).scrollBehavior,
    };
  });

test("reducedMotion: 'reduce' では transition・animation の duration が 0 相当になる", async ({ page }) => {
  // 比べる基準: 既定では button 等に transition の duration がある。
  await page.goto(`${gateway.base}/tasks`);
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  const normal = await motionOf(page);
  expect(Math.max(...normal.transitions.map(maxSeconds))).toBeGreaterThan(0.05);

  await page.emulateMedia({ reducedMotion: "reduce" });
  const reduced = await motionOf(page);
  expect(reduced.count).toBeGreaterThan(0);
  expect(Math.max(...reduced.transitions.map(maxSeconds))).toBeLessThanOrEqual(0.001);
  expect(Math.max(...reduced.animations.map(maxSeconds))).toBeLessThanOrEqual(0.001);
  expect(reduced.scroll).toBe("auto");
});

// 入力欄の枠色と、その外側の背景色の contrast（WCAG 1.4.11 の 3:1）を computed style から出す。
const inputContrasts = (page: Page) =>
  page.evaluate(() => {
    const rgb = (value: string) => (value.match(/[\d.]+/g) ?? []).map(Number);
    const luminance = ([r, g, b]: number[]) => {
      const lin = (c: number) => {
        const v = c / 255;
        return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
      };
      return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    };
    const backgroundOf = (el: Element | null): number[] => {
      for (let node = el; node; node = node.parentElement) {
        const parts = rgb(getComputedStyle(node).backgroundColor);
        if (parts.length >= 3 && (parts.length < 4 || parts[3] === 1)) return parts.slice(0, 3);
      }
      return [255, 255, 255];
    };
    const fields = [
      ...document.querySelectorAll<HTMLElement>(
        "input:not([type=hidden]):not([type=checkbox]):not([type=radio]), textarea, select",
      ),
    ].filter((el) => el.getBoundingClientRect().width > 0);
    return fields.map((el) => {
      const border = rgb(getComputedStyle(el).borderTopColor).slice(0, 3);
      const back = backgroundOf(el.parentElement);
      const [hi, lo] = [luminance(border), luminance(back)].sort((a, b) => b - a);
      return { field: el.outerHTML.slice(0, 80), ratio: (hi + 0.05) / (lo + 0.05) };
    });
  });

for (const path of ["/login", "/knowledge", "/tasks/new"]) {
  test(`${path}: 入力欄の枠は背景に対して 3:1 以上`, async ({ page }) => {
    await page.goto(`${gateway.base}${path}`);
    await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
    await expect(page.locator("input:not([type=hidden]), textarea, select").first()).toBeVisible();
    const results = await inputContrasts(page);
    expect(results.length).toBeGreaterThan(0);
    for (const result of results) expect(result.ratio, result.field).toBeGreaterThanOrEqual(3);
  });
}
