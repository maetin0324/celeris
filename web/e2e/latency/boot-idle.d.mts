import type { Page } from "@playwright/test";

/** nav と main h1 の後、2 frame + requestIdleCallback の 1 巡に long task が無くなるまで待つ。戻り値は巡の数。 */
export function waitForBootIdle(page: Page, timeoutMs?: number): Promise<number>;
