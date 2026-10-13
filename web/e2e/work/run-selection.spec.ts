import { readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures, fixtureFor } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// run の選択（harness・source・model・account）の表示（ADR 2026-10-11 llm-source-harness-selection の「API と画面」）。
// R1 は解決済みの選択、R2 は旧 run（selection が legacy_unresolved）。
test.describe("run selection", () => {
  const schema = JSON.parse(
    readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
  );
  const line = JSON.stringify({
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text: "ok" }] },
  });

  const resolved = {
    status: "selected",
    harness_id: "claude-code",
    source_id: "claude-oauth",
    model_id: "claude-opus",
    account_id: "main",
    basis: null,
  };

  function taskDetail() {
    const value = fixtureFor(schema.$defs.TaskDetail) as { task: Record<string, unknown>; runs: unknown[] };
    value.task = { ...value.task, id: "T1" };
    const run = fixtureFor(schema.$defs.RunSummary) as Record<string, unknown>;
    const finished = "2026-09-30T00:00:00Z";
    value.runs = [
      { ...run, run_id: "R1", finished_at: finished, selection: resolved },
      { ...run, run_id: "R2", finished_at: finished, selection: { status: "legacy_unresolved" } },
    ];
    return value;
  }

  test("run 詳細と task の run 一覧に選択を出し、旧 run は記録なしと出す", async ({ page }) => {
    const dir = makeTmpDir("celeris-web-run-selection-");
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({
      token: FIXTURE_TOKEN,
      fixtures: { ...defaultFixtures, "/api/v1/tasks/T1": taskDetail() },
      files: {
        "/api/v1/tasks/T1/runs/R1/stdout": { body: () => `${line}\n`, type: "text/plain; charset=utf-8" },
        "/api/v1/tasks/T1/runs/R2/stdout": { body: () => `${line}\n`, type: "text/plain; charset=utf-8" },
      },
    });
    const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
    try {
      await page.goto(`${gateway.base}/tasks/T1/runs/R1`);
      const header = page.getByTestId("run-header");
      await expect(header).toContainText("claude-code");
      await expect(header).toContainText("claude-opus");
      await expect(header).toContainText("main");
      await expect(header).not.toContainText("記録なし（旧 run）");
      await header.getByRole("link", { name: "claude-oauth" }).click();
      await expect(page).toHaveURL(
        new RegExp(`${gateway.base.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}/sources/claude-oauth$`),
      );
      await expect(page.getByRole("heading", { level: 1, name: "claude-oauth" })).toBeVisible();

      await page.goto(`${gateway.base}/tasks/T1/runs/R2`);
      const legacy = page.getByTestId("run-header");
      await expect(legacy).toContainText("記録なし（旧 run）");
      await expect(legacy.getByRole("link")).toHaveCount(0);

      await page.goto(`${gateway.base}/tasks/T1`);
      await expect(page.getByText("claude-code / claude-oauth / claude-opus / 口座 main")).toBeVisible();
      await expect(page.getByText("記録なし（旧 run）").first()).toBeVisible();
    } finally {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
