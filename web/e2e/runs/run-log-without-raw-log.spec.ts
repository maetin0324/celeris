import { expect, test } from "@playwright/test";
import { startFixtureGateway } from "../support/fixture-gateway";

const timeout = { timeout: 20_000 };

test("生ログのない run は進捗と結果を表示し stdout を要求しない", async ({ page }) => {
  const gateway = await startFixtureGateway();
  const stdoutRequests: string[] = [];
  try {
    await page.route("**/api/tasks/T1", async (route) => {
      const response = await route.fetch();
      const detail = await response.json();
      detail.runs = detail.runs.map((run: { run_id: string; files?: object }) =>
        run.run_id === "R1" ? { ...run, files: { stdout: false, stderr: false, result: true } } : run,
      );
      await route.fulfill({ response, json: detail });
    });
    await page.route("**/api/tasks/T1/runs/R1/events*", (route) =>
      route.fulfill({
        json: {
          items: [
            {
              seq: 1,
              id: 1,
              task_id: "T1",
              ts: "2026-10-10T00:00:00Z",
              event: {
                type: "worker_progress",
                run_id: "R1",
                kind: "tool_result",
                tool: "browser.navigate",
                msg: "ページを確認しました",
                summary: "ログイン後の画面を確認",
              },
            },
          ],
          has_more: false,
        },
      }),
    );
    await page.route("**/api/tasks/T1/runs/R1/result", (route) =>
      route.fulfill({ json: { summary: "確認が完了しました", question: "次の操作を選んでください" } }),
    );
    await page.route("**/files/tasks/T1/runs/R1/stdout*", (route) => {
      stdoutRequests.push(route.request().url());
      return route.fulfill({ status: 500 });
    });
    await page.goto(`${gateway.base}/tasks/T1/runs/R1`);
    await expect(page.getByTestId("run-no-raw-log")).toContainText("機密保護のため生ログ", timeout);
    await expect(page.getByTestId("run-progress-events")).toContainText("ログイン後の画面を確認", timeout);
    await expect(page.getByRole("region", { name: "run の結果" })).toContainText("確認が完了しました", timeout);
    await expect(page.getByRole("region", { name: "run の結果" })).toContainText("次の操作を選んでください", timeout);
    expect(stdoutRequests).toEqual([]);
  } finally {
    await gateway.close();
  }
});
