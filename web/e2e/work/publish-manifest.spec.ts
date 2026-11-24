import { expect, type Page, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import {
  PUBLISH_DECISION_ID,
  PUBLISH_MANIFEST_ID,
  PUBLISH_MANIFEST_NAME,
  PUBLISH_MANIFEST_SHA,
} from "../support/fake-daemon.mjs";
import { startFixtureGateway } from "../support/fixture-gateway";

// publish manifest の画面（ADR 2026-11-23-project-observability-publish D3・D4）。実の gateway と偽 daemon の publishManifest の
// fixture（fake-daemon.mjs の createPublishFixture）で、人が案件の概要の「公開の準備度」から manifest を開き、
// claims → evidence → 再現条件 → limitations・未解決 → 人の署名 の順に読み、unsupported の claim・未署名・失効を強い警告で見て、
// 確認の dialog（manifest の sha256 と「Celeris は外に送らない。公開は人が行う。」）を経て既存の決定の回答で署名することを確かめる。
// 署名の状態は偽 daemon が版・file の変化・最後の回答から導いた値で、画面は持たない。外部の URL は開かない（属性だけを見る）。

const SECTIONS = ["claims", "evidence", "reproduction", "limitations", "signoff"];
const NOTICE = "Celeris は外に送らない。公開は人が行う。";

let gateway: Awaited<ReturnType<typeof startFixtureGateway>>;

test.beforeEach(async () => {
  gateway = await startFixtureGateway({ publishManifest: true });
});

test.afterEach(async () => {
  await gateway.close();
});

/** 偽 daemon の制御口（gateway に無いので FIXTURE_TOKEN 付きで daemon を直接呼ぶ）。 */
async function control(path: string, body?: Record<string, unknown>) {
  const response = await fetch(`${gateway.daemonUrl}/__fixture/publish/${path}`, {
    method: body ? "POST" : "GET",
    headers: { authorization: `Bearer ${FIXTURE_TOKEN}`, "content-type": "application/json" },
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!response.ok) throw new Error(`fixture control ${path} failed: ${response.status}`);
  return (await response.json()) as Record<string, unknown>;
}

type AnswerLog = { decision_id: string; principal: string; body: { option?: string }; status: number; code?: string };

async function answerLog() {
  return ((await control("answer-log")).answers ?? []) as AnswerLog[];
}

/** 画面から偽 daemon に届いた GET 以外の要求。 */
function mutations() {
  return gateway.daemon.requests.filter((r) => r.method !== "GET" && r.method !== "HEAD").map((r) => r.path);
}

async function openManifest(page: Page, version?: number) {
  await page.goto(
    `${gateway.base}/tasks/T1/publish/${PUBLISH_MANIFEST_ID}${version === undefined ? "" : `?version=${version}`}`,
  );
  const panel = page.getByTestId("publish-manifest");
  await expect(panel).toBeVisible();
  return panel;
}

test("概要の公開の準備度から開き、5 段の順に読み、unsupported の claim と未署名を強い警告で出し、公開の操作が無い", async ({
  page,
}) => {
  await page.goto(`${gateway.base}/projects/P1`);
  const readiness = page.getByTestId("project-overview-panel").locator("li[data-item='publish_readiness']");
  await expect(readiness.locator("h3 [data-mark]")).toHaveText("要判断");
  await readiness.locator("[data-link='manifest']").click();
  await expect(page).toHaveURL(new RegExp(`/tasks/T1/publish/${PUBLISH_MANIFEST_ID}\\?version=1$`));

  const panel = page.getByTestId("publish-manifest");
  await expect(panel).toBeVisible();
  await expect(panel).toContainText(PUBLISH_MANIFEST_NAME(1));
  await expect(panel).toContainText(PUBLISH_MANIFEST_SHA(1));
  const steps = panel.getByRole("list", { name: "manifest の 5 段" }).locator(":scope > li");
  expect(await steps.evaluateAll((rows) => rows.map((row) => row.getAttribute("data-section")))).toEqual(SECTIONS);
  await expect(steps.nth(0).getByRole("heading")).toHaveText("1. 主張（claims）");
  await expect(steps.nth(4).getByRole("heading")).toHaveText("5. 人の署名");

  // 強い警告は 5 段より前。unsupported の claim・未署名・公開を止める未解決の問いを文字で出す。
  const warnings = panel.getByTestId("manifest-warnings");
  expect(
    await panel.locator(":scope > *").evaluateAll((rows) => rows.map((row) => row.getAttribute("data-testid"))),
  ).toEqual([null, "manifest-warnings", null]);
  await expect(warnings.locator("li[data-warning='unsupported_claim']")).toHaveText(
    "強い警告: 根拠の無い主張（unsupported）: c2",
  );
  await expect(warnings.locator("li[data-warning='unsigned']")).toContainText("未署名。公開の前に人の署名が要る。");
  await expect(warnings.locator("li[data-warning='manifest_blocking_question']")).toContainText(
    "manifest に公開を止める未解決の問いがある",
  );
  await expect(panel.locator("li[data-claim='c2']")).toHaveAttribute("data-unsupported", "true");
  await expect(panel.locator("li[data-claim='c2']")).toContainText("根拠無し（unsupported）");
  await expect(panel.locator("li[data-claim='c1']")).toHaveAttribute("data-unsupported", "false");
  await expect(panel.locator("li[data-evidence='e4']")).toContainText("反証");
  await expect(panel.locator("li[data-reproduction='r1']")).toContainText("cargo run --release -p bench");
  await expect(panel.locator("li[data-question='q1']")).toContainText("公開を止める");
  await expect(panel.getByTestId("manifest-signoff")).toHaveAttribute("data-state", "unsigned");
  await expect(panel.getByTestId("manifest-no-publish")).toHaveText(NOTICE);

  // 公開の操作は画面に無い（署名と「署名しない」だけ）。
  await expect(page.getByRole("button", { name: /公開|publish/i })).toHaveCount(0);
  await expect(page.getByRole("link", { name: /公開する|publish/i })).toHaveCount(0);
  await expect(panel.getByRole("button")).toHaveText(["この版に署名する", "署名しない"]);
  expect(mutations()).toEqual([]);
});

test("evidence から commit・成果物・文献・chat・task へ移る", async ({ page }) => {
  let panel = await openManifest(page);
  await panel.locator("li[data-evidence='e1'] [data-link='git']").click();
  await expect(page).toHaveURL(/\/tasks\/T1\/changes$/);
  await expect(page.getByRole("heading", { name: "変更 T1" })).toBeVisible();

  panel = await openManifest(page);
  await panel.locator("li[data-evidence='e1'] [data-link='artifact']").click();
  await expect(page).toHaveURL(/\/tasks\/T1\?tab=artifacts$/);
  await expect(page.getByTestId("task-artifacts")).toBeVisible();

  panel = await openManifest(page);
  const paper = panel.locator("li[data-evidence='e3'] a[data-link='literature']");
  await expect(paper).toHaveAttribute("href", "https://example.org/papers/unifyfs");
  await expect(paper).toHaveAttribute("target", "_blank");
  await expect(paper).toHaveAttribute("rel", "noopener noreferrer");
  await expect(panel.locator("li[data-evidence='e3'] span[data-link='literature']")).toContainText("projects/demo.md");

  await panel.locator("li[data-evidence='e4'] [data-link='chat']").click();
  await expect(page).toHaveURL(/\/\?thread=chat-history$/);

  panel = await openManifest(page);
  await panel.locator("li[data-evidence='e2'] [data-link='task']").click();
  await expect(page).toHaveURL(/\/tasks\/T1$/);
});

test("署名: dialog が sha256 と『外に送らない』を示し、人の回答 1 件で署名済みになる", async ({ page }) => {
  const panel = await openManifest(page);
  await panel.getByTestId("manifest-sign").click();
  const dialog = page.getByRole("alertdialog");
  await expect(dialog).toContainText(`対象: ${PUBLISH_MANIFEST_NAME(1)}（sha256:${PUBLISH_MANIFEST_SHA(1)}）`);
  await expect(dialog).toContainText(NOTICE);
  await dialog.getByRole("button", { name: "この sha256 に署名する" }).click();
  await expect(dialog).toBeHidden();

  const signoff = panel.getByTestId("manifest-signoff");
  await expect(signoff).toHaveAttribute("data-state", "signed");
  await expect(signoff.locator("[data-slot='signoff-state']")).toHaveText("署名済み");
  await expect(signoff).toContainText("回答: human:web");
  await expect(panel.getByTestId("manifest-warnings").locator("li[data-warning='unsigned']")).toHaveCount(0);
  await expect(panel.getByTestId("manifest-sign")).toHaveCount(0);

  // 画面が送った変更の要求は既存の決定の回答 1 件だけ（公開・送信・起票の要求は無い）。
  expect(mutations()).toEqual([`/api/v1/decisions/${PUBLISH_DECISION_ID(1)}/answer`]);
  expect((await answerLog()).map((r) => [r.decision_id, r.principal, r.body.option, r.status])).toEqual([
    [PUBLISH_DECISION_ID(1), "human", "sign", 200],
  ]);

  await page.goto(`${gateway.base}/projects/P1`);
  await expect(
    page.getByTestId("project-overview-panel").locator("li[data-item='publish_readiness'] h3 [data-mark]"),
  ).toHaveText("確認済み");
});

test("失効: 署名の後に file が変わると stale、新しい版が出ると前の版は superseded で、署名し直しを求める", async ({
  page,
}) => {
  let panel = await openManifest(page);
  await panel.getByTestId("manifest-sign").click();
  await page.getByRole("alertdialog").getByRole("button", { name: "この sha256 に署名する" }).click();
  await expect(panel.getByTestId("manifest-signoff")).toHaveAttribute("data-state", "signed");

  await control("modify", { version: 1 });
  panel = await openManifest(page, 1);
  const signoff = panel.getByTestId("manifest-signoff");
  await expect(signoff).toHaveAttribute("data-state", "stale");
  await expect(signoff.locator("[data-slot='signoff-state']")).toHaveText("失効（署名の後に file が変わった）");
  const warnings = panel.getByTestId("manifest-warnings");
  await expect(warnings.locator("li[data-warning='signoff_stale']")).toContainText("署名は失効した");
  await expect(warnings.locator("li[data-warning='file_changed']")).toBeVisible();
  await expect(panel).toContainText(`今の file の sha256: ${"f".repeat(64)}`);
  await expect(panel.getByTestId("manifest-sign-unavailable")).toContainText("署名は失効しました");
  await expect(panel.getByTestId("manifest-sign")).toHaveCount(0);

  await page.goto(`${gateway.base}/projects/P1`);
  const overview = page.getByTestId("project-overview-panel");
  await expect(overview.locator("li[data-item='publish_readiness'] h3 [data-mark]")).toHaveText("矛盾・反証");
  await expect(
    overview.getByTestId("overview-strong-warnings").locator("li[data-warning='manifest_invalid']"),
  ).toBeVisible();

  await control("new-version", {});
  panel = await openManifest(page, 1);
  await expect(panel.getByTestId("manifest-signoff")).toHaveAttribute("data-state", "superseded");
  await expect(panel.getByTestId("manifest-warnings").locator("li[data-warning='signoff_superseded']")).toContainText(
    "より新しい版 v2",
  );
  await panel.getByTestId("manifest-versions").locator("[data-version='2']").click();
  await expect(page).toHaveURL(new RegExp(`/tasks/T1/publish/${PUBLISH_MANIFEST_ID}\\?version=2$`));
  await expect(page.getByTestId("publish-manifest").getByTestId("manifest-signoff")).toHaveAttribute(
    "data-state",
    "unsigned",
  );
  await expect(page.getByTestId("manifest-sign")).toBeVisible();
});

test("403: 人以外の主体の署名は daemon が publication_requires_human で拒み、画面は失敗を出して未署名のまま", async ({
  page,
}) => {
  await control("principal", { principal: "operator" });
  const panel = await openManifest(page);

  // (1) gateway の relay を通す既定の経路。gateway（web/server/relay.js）は daemon の 401・403 を 502 daemon_auth に変える。
  await panel.getByTestId("manifest-sign").click();
  const dialog = page.getByRole("alertdialog");
  await dialog.getByRole("button", { name: "この sha256 に署名する" }).click();
  await expect(dialog.getByRole("alert")).toContainText("daemon_auth");
  await dialog.getByRole("button", { name: "戻る" }).click();

  // (2) daemon の 403 の problem がそのまま届いたときの表示（gateway の置き換えを通さず、同じ要求を daemon へ送る）。
  await page.route("**/api/decisions/*/answer", async (route) => {
    const request = route.request();
    const upstream = await fetch(
      `${gateway.daemonUrl}${new URL(request.url()).pathname.replace(/^\/api\//, "/api/v1/")}`,
      {
        method: "POST",
        headers: { authorization: `Bearer ${FIXTURE_TOKEN}`, "content-type": "application/json" },
        body: request.postData() ?? "",
      },
    );
    await route.fulfill({
      status: upstream.status,
      headers: { "content-type": upstream.headers.get("content-type") ?? "application/json" },
      body: await upstream.text(),
    });
  });
  await panel.getByTestId("manifest-sign").click();
  await dialog.getByRole("button", { name: "この sha256 に署名する" }).click();
  await expect(dialog.getByRole("alert")).toHaveText("署名は人だけができます（publication_requires_human）。");
  await dialog.getByRole("button", { name: "戻る" }).click();
  await expect(panel.getByTestId("manifest-signoff")).toHaveAttribute("data-state", "unsigned");

  expect((await answerLog()).map((r) => [r.principal, r.body.option, r.status, r.code])).toEqual([
    ["operator", "sign", 403, "publication_requires_human"],
    ["operator", "sign", 403, "publication_requires_human"],
  ]);
  // 拒まれた回答は何も記録しない（読み直しても未署名）。
  const reread = await openManifest(page);
  await expect(reread.getByTestId("manifest-signoff")).toHaveAttribute("data-state", "unsigned");
});
