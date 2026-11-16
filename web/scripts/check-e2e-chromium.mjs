// release gate の段 web-e2e-chromium（ADR 2026-11-16-release-gate-web-e2e D2）。
// e2e（playwright.config.ts の Desktop Chrome、headless）が使う chromium を 1 回起動して閉じる。
// browser は host の ~/.cache/ms-playwright（PLAYWRIGHT_BROWSERS_PATH があればそこ）にあるものだけを使い、
// download しない。無い・起動できないときは exit 1 で、探した path（Playwright の error）と入れ方を出す。
import { chromium } from "@playwright/test";

const INSTALL = "pnpm -C web exec playwright install chromium";

try {
  const browser = await chromium.launch();
  const version = browser.version();
  await browser.close();
  process.stdout.write(`e2e chromium: ${version} (${chromium.executablePath()})\n`);
} catch (error) {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(
    [
      "e2e chromium: Playwright の chromium を起動できません（gate は download しません）。",
      message,
      `release を作る user で \`${INSTALL}\` を実行し、@playwright/test の版に合う chromium を入れてください。`,
      "",
    ].join("\n"),
  );
  process.exit(1);
}
