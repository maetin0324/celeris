import { readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures, fixtureFor } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

const runsSchema = JSON.parse(
  readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../api/generated/schema.json"), "utf8"),
);

test.describe("P3-12", () => {
  const line = (text: string) =>
    JSON.stringify({ type: "assistant", message: { role: "assistant", content: [{ type: "text", text }] } });

  function runDetail(finished: boolean) {
    const value = fixtureFor(runsSchema.$defs.TaskDetail) as { task: Record<string, unknown>; runs: unknown[] };
    value.task = { ...value.task, id: "T1" };
    const run = fixtureFor(runsSchema.$defs.RunSummary) as Record<string, unknown>;
    value.runs = [
      { ...run, run_id: "R1", adapter: "claude-code", finished_at: finished ? "2026-09-30T00:00:00Z" : null },
    ];
    return value;
  }

  test("parity: /tasks/:id/runs/:runId 会話表示と追記", async ({ page }) => {
    const dir = makeTmpDir("celeris-web-run-log-");
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const stdoutPath = "/api/v1/tasks/T1/runs/R1/stdout";
    const lines = Array.from({ length: 40 }, (_, i) => line(`行 ${i + 1}`));
    lines.push("not json");
    let body = `${lines.join("\n")}\n`;
    let finished = false;
    const daemon = createFakeDaemon({
      token: FIXTURE_TOKEN,
      fixtures: { ...defaultFixtures, "/api/v1/tasks/T1": () => runDetail(finished) },
      files: { [stdoutPath]: { body: () => body, type: "text/plain; charset=utf-8" } },
    });
    const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
    const reads = () => daemon.requests.filter((request) => request.path === stdoutPath).length;
    try {
      await page.setViewportSize({ width: 390, height: 600 });
      await page.goto(`${gateway.base}/tasks/T1/runs/R1`);
      await expect(page.getByRole("heading", { level: 1, name: "run ログ T1 / R1" })).toBeVisible();
      // 表示の行数が fixture の行数（wc -l）と一致する。
      const count = page.getByTestId("run-log-line-count");
      await expect(count).toHaveAttribute("data-count", String(body.split("\n").length - 1));
      await expect(page.getByTestId("run-log")).toContainText("行 1");

      // 読んでいる位置を途中に置き、追記を追っても位置が動かない。
      await page.evaluate(() => window.scrollTo(0, 200));
      const before = await page.evaluate(() => window.scrollY);
      body += `${line("追記 1")}\n${line("追記 2")}\n`;
      await expect(count).toHaveAttribute("data-count", "43");
      await expect(page.getByTestId("run-log")).toContainText("追記 2");
      expect(await page.evaluate(() => window.scrollY)).toBe(before);

      // run が終わったら追い掛けをやめ、取り直さない。
      finished = true;
      daemon.sendEvent("task.event", {
        id: 1,
        seq: 1,
        task_id: "T1",
        ts: "2026-09-30T00:00:00Z",
        event: { type: "transitioned", from: "running", to: "done" },
      });
      await expect(page.getByTestId("run-log")).not.toContainText("実行中", { timeout: 10_000 });
      const settled = reads();
      await page.waitForTimeout(2500);
      expect(reads()).toBe(settled);
    } finally {
      await gateway.close();
      await daemon.close();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

test.describe("P3-11", () => {
  const longName = `${"very-long-directory-name-".repeat(6)}file.txt`;
  const longLine = `${"x".repeat(4000)}\n`;
  const tree = (url: URL) => {
    const p = url.searchParams.get("path") ?? "";
    if (p.includes("..")) return undefined;
    if (p === "src")
      return {
        repo: "code",
        path: "src",
        repos: [{ name: "code", kind: "git", dir: "/w/code" }],
        entries: [{ kind: "file", name: longName, path: `src/${longName}`, size: longLine.length }],
      };
    return {
      repo: "code",
      path: "",
      repos: [{ name: "code", kind: "git", dir: "/w/code" }],
      entries: [
        { kind: "dir", name: "src", path: "src" },
        { kind: "file", name: "README.md", path: "README.md", size: 12 },
        { kind: "file", name: "big.bin", path: "big.bin", size: 99_999_999 },
      ],
    };
  };
  const file = (url: URL) => {
    const p = url.searchParams.get("path") ?? "";
    if (p === "README.md")
      return { repo: "code", path: p, size: 12, binary: false, too_large: false, text: "# 見出し本文\n" };
    if (p === "big.bin") return { repo: "code", path: p, size: 99_999_999, binary: false, too_large: true };
    if (p.startsWith("src/"))
      return { repo: "code", path: p, size: longLine.length, binary: false, too_large: false, text: longLine };
    return undefined;
  };

  async function harness(
    fixtures: Record<string, unknown>,
    files: Record<string, { body: string; type?: string }> = {},
  ) {
    const dir = makeTmpDir("celeris-web-runs-files-");
    const tokenFile = path.join(dir, "token");
    writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
    const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, fixtures: { ...defaultFixtures, ...fixtures }, files });
    const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
    return {
      daemon,
      gateway,
      async close() {
        await gateway.close();
        await daemon.close();
        rmSync(dir, { recursive: true, force: true });
      },
    };
  }

  test("parity: /tasks/:id/files 木と本文・不正 path", async ({ page }) => {
    const h = await harness({ "/api/v1/tasks/T1/tree": tree, "/api/v1/tasks/T1/tree/file": file });
    try {
      await page.goto(`${h.gateway.base}/tasks/T1/files`);
      await expect(page.getByRole("heading", { level: 1, name: "作業ツリーと成果物 T1" })).toBeVisible();
      await page.locator("[data-entry='README.md']").click();
      await expect(page).toHaveURL(/file=README\.md/);
      await expect(page.getByTestId("files-body")).toContainText("見出し本文");

      // 大きい file は本文を読まない（daemon の too_large）。
      await page.locator("[data-entry='big.bin']").click();
      await expect(page.getByTestId("files-too-large")).toBeVisible();

      // ディレクトリへ移動し、長い path・長い行は枠の中に収まる。
      await page.locator("[data-entry='src']").click();
      await expect(page).toHaveURL(/path=src/);
      await page.locator(`[data-entry='src/${longName}']`).click();
      await expect(page.getByTestId("files-body")).toContainText("xxxx");
      await page.setViewportSize({ width: 375, height: 800 });
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth),
      ).toBe(0);

      // URL を直接開いても同じ状態。不正な path は枠の中の error。
      await page.goto(`${h.gateway.base}/tasks/T1/files?path=README.md&file=README.md`);
      await expect(page.getByTestId("files-body")).toContainText("見出し本文");
      await page.goto(`${h.gateway.base}/tasks/T1/files?path=..%2F..&file=..%2Fsecret`);
      await expect(page.getByRole("heading", { level: 1, name: "作業ツリーと成果物 T1" })).toBeVisible();
      await expect(page.getByTestId("files-error").first()).toBeVisible();
    } finally {
      await h.close();
    }
  });

  test("parity: 360px で深い階層・長い file 名の file 一覧・本文・成果物一覧が溢れない", async ({ page }) => {
    const deepDirName = `${"deeply-nested-directory-segment-".repeat(3)}end`;
    const deepDirPath = `src/${deepDirName}`;
    const deepFileName = `${"another-rather-long-file-name-segment-".repeat(3)}file.log`;
    const deepFilePath = `${deepDirPath}/${deepFileName}`;
    const deepLine = `${"z".repeat(5000)}\n`;
    const artifactName = `${"very-long-report-file-name-segment-".repeat(3)}summary.md`;
    const artifactPath = `ops/${"nested-artifact-directory-".repeat(3)}reports/${artifactName}`;
    const tree360 = (url: URL) => {
      const p = url.searchParams.get("path") ?? "";
      if (p.includes("..")) return undefined;
      if (p === "src")
        return {
          repo: "code",
          path: "src",
          repos: [{ name: "code", kind: "git", dir: "/w/code" }],
          entries: [{ kind: "dir", name: deepDirName, path: deepDirPath }],
        };
      if (p === deepDirPath)
        return {
          repo: "code",
          path: deepDirPath,
          repos: [{ name: "code", kind: "git", dir: "/w/code" }],
          entries: [{ kind: "file", name: deepFileName, path: deepFilePath, size: deepLine.length }],
        };
      return {
        repo: "code",
        path: "",
        repos: [{ name: "code", kind: "git", dir: "/w/code" }],
        entries: [{ kind: "dir", name: "src", path: "src" }],
      };
    };
    const file360 = (url: URL) => {
      const p = url.searchParams.get("path") ?? "";
      if (p === deepFilePath)
        return { repo: "code", path: p, size: deepLine.length, binary: false, too_large: false, text: deepLine };
      return undefined;
    };
    const h = await harness(
      {
        "/api/v1/tasks/T1/tree": tree360,
        "/api/v1/tasks/T1/tree/file": file360,
        "/api/v1/projects": { items: [{ id: "P1", request: "案件1", created_at: "2026-09-30T00:00:00Z" }] },
        "/api/v1/projects/P1": { project: { id: "P1" }, milestones: [], tasks: [{ id: "T1", title: "成果物の題" }] },
        "/api/v1/tasks/T1/artifacts": {
          items: [
            {
              idx: 0,
              run_id: "R1",
              ts: "2026-09-30T00:00:00Z",
              exists: true,
              forbidden: false,
              size: 10,
              artifact: { kind: "file", name: artifactName, path: artifactPath, sha256: "0" },
            },
          ],
        },
      },
      { "/api/v1/tasks/T1/artifacts/0": { body: "# 長い path の報告\n", type: "text/markdown" } },
    );
    const overflow = () =>
      page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    try {
      await page.setViewportSize({ width: 360, height: 800 });

      // file 一覧: 深い階層へたどり、長いディレクトリ名・長い file 名でも 360px で溢れない。省略した全文は title に。
      await page.goto(`${h.gateway.base}/tasks/T1/files`);
      await expect(page.getByRole("heading", { level: 1, name: "作業ツリーと成果物 T1" })).toBeVisible();
      await page.locator("[data-entry='src']").click();
      const dirEntry = page.locator(`[data-entry='${deepDirPath}']`);
      await expect(dirEntry).toHaveAttribute("title", deepDirPath);
      expect(await overflow()).toBe(0);
      await dirEntry.click();
      const fileEntry = page.locator(`[data-entry='${deepFilePath}']`);
      await expect(fileEntry).toHaveAttribute("title", deepFilePath);
      expect(await overflow()).toBe(0);

      // file 本文: 深い path の見出しと長い行が 360px で溢れない。全文は title に。
      await fileEntry.click();
      const body = page.getByTestId("files-body");
      await expect(body).toContainText("zzzz");
      await expect(body.locator("h2")).toHaveAttribute("title", deepFilePath);
      expect(await overflow()).toBe(0);

      // 成果物一覧: 長い path でも 360px で溢れない。全文は title に。
      await page.goto(`${h.gateway.base}/artifacts`);
      await page.getByLabel("案件").selectOption("P1");
      await page.getByRole("button", { name: "絞り込む" }).click();
      await expect(page).toHaveURL(/project=P1/);
      const list = page.getByTestId("artifacts-list");
      await expect(list.locator(`[data-artifact='T1/0'] [title='${artifactPath}']`)).toHaveAttribute(
        "title",
        artifactPath,
      );
      expect(await overflow()).toBe(0);
    } finally {
      await h.close();
    }
  });

  test("parity: /artifacts 絞り込みと開く", async ({ page }) => {
    const artifact = (idx: number, name: string) => ({
      idx,
      run_id: "R1",
      ts: "2026-09-30T00:00:00Z",
      exists: true,
      forbidden: false,
      size: 10,
      artifact: { kind: "file", name, path: name, sha256: "0" },
    });
    const h = await harness(
      {
        "/api/v1/projects": { items: [{ id: "P1", request: "案件1", created_at: "2026-09-30T00:00:00Z" }] },
        "/api/v1/projects/P1": { project: { id: "P1" }, milestones: [], tasks: [{ id: "T1", title: "成果物の題" }] },
        "/api/v1/tasks/T1/artifacts": { items: [artifact(0, "report.md"), artifact(1, "page.html")] },
      },
      {
        "/api/v1/tasks/T1/artifacts/0": { body: "# 報告の本文\n", type: "text/markdown" },
        "/api/v1/tasks/T1/artifacts/1": { body: "<script>window.__pwned=1</script>", type: "text/html" },
      },
    );
    try {
      await page.goto(`${h.gateway.base}/artifacts`);
      await expect(page.getByRole("heading", { level: 1, name: "成果物" })).toBeVisible();
      await page.getByLabel("案件").selectOption("P1");
      await page.getByRole("button", { name: "絞り込む" }).click();
      await expect(page).toHaveURL(/project=P1/);
      const list = page.getByTestId("artifacts-list");
      await expect(list).toContainText("成果物の題");
      await list.locator("[data-artifact='T1/0']").getByRole("button", { name: "本文をここで見る" }).click();
      await expect(list).toContainText("報告の本文");
      // H8: HTML は描画・実行せず download。
      const html = list.locator("[data-artifact='T1/1'] a[download]");
      await expect(html).toHaveAttribute("href", "/files/tasks/T1/artifacts/1?download=1");
      expect(await page.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
    } finally {
      await h.close();
    }
  });
});
