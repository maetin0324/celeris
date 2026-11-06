import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { expect, test } from "@playwright/test";
import { FIXTURE_TOKEN } from "../../scripts/check-secrets.mjs";
import { createFakeDaemon, defaultFixtures } from "../support/fake-daemon.mjs";
import { startGateway } from "../support/gateway";
import { makeTmpDir } from "../support/tmp-dir";

// ADR 2026-11-04-project-controller A9: 案件の詳細の「自動運営」の画面（長期方針・途中目標・関連 root・今後の作業・
// 待機と人の要件・過去の判断と根拠・mode の切り替え）。偽 daemon の fixture はこの file の中にまとめる。
// LONG_HORIZON_PILOT_SCREEN_DIR があれば PC 幅とスマホ幅の画面を png で残す（進捗の証拠）。

const now = "2026-11-04T09:00:00Z";
const project = {
  id: "P1",
  title: "pilot:C08-1",
  request: "長期に計測を続ける。\n\n```celeris-outcomes\n[]\n```",
  status: "active",
  created_at: now,
  updated_at: now,
  workspace: { kind: "local", path: "/work/pilot" },
};
const root = (id: string, title: string, status: string) => ({
  id,
  title,
  status,
  depends_on: [],
  conversation: false,
  is_root_task: true,
});
const detail = {
  project,
  milestones: [],
  root_totals: { root_tasks: 2, by_status: { done: 1, running: 1 }, totals: {} },
  tasks: [root("R1", "pilot:C08-1: 調査と実装", "done"), root("R2", "pilot:C08-1: 実験セル A", "running")],
  brief: {
    project_id: "P1",
    objective: "計測を再現できるようにして、表を出し続ける",
    state: "root 1 件が完了。次は実験セル A。",
    open_issues: [],
    next_actions: ["実験セル A の root を終える", "表 2 を出す"],
    waiting_on: [{ kind: "human", note: "測定用 account の login", target: null }],
    evidence_refs: ["task:R1"],
    updated_at: now,
    updated_by: "cos:TH1",
  },
};
const decision = (id: string, rule: string, effect: string, ref: string | null) => ({
  id,
  project_id: "P1",
  based_on_revision: 3,
  human_epoch: 0,
  snapshot_sha256: "s",
  request_sha256: "r",
  generation_id: null,
  mode: "enabled",
  proposal_json: JSON.stringify({ explanation: { rule_id: rule } }),
  validation: rule,
  effect_json: "{}",
  effect_state: effect,
  effect_ref: ref,
  progress_epoch: 1,
  created_at: now,
  applied_at: now,
});
const controller = {
  state: {
    project_id: "P1",
    mode: "enabled",
    phase: "active",
    revision: 4,
    event_cursor: 0,
    next_wake_at: "2026-11-04T15:00:00Z",
    retry_count: 0,
    human_epoch: 0,
    progress_epoch: 1,
    updated_at: now,
    updated_by: "human:web",
  },
  pending_wakes: [],
  last_decision: null,
  budget_today: {
    day: "2026-11-04",
    cos_inputs: 0,
    roots: 1,
    tokens: 900,
    usd: 1.5,
    usd_unknown_runs: 0,
    price_table_version: "2026-09-approx-1",
    chat_starts: 0,
  },
  summary: {
    outcomes: [
      { id: "o1", title: "表 1", source: "request_block", recorded: true, evidence_root_ids: ["R1"], reached: true },
      { id: "o2", title: "表 2", source: "request_block", recorded: false, evidence_root_ids: [], reached: false },
    ],
    outcomes_invalid: false,
    waiting: null,
    authority: { launch_root_allowed: false, launch_root_reason: "no_grant", grants: [] },
    next_wake: { at: "2026-11-04T15:00:00Z", backoff_until: null, pending_kinds: [] },
    budget_limits: {
      max_cos_inputs_per_day: 10,
      max_roots_per_day: 5,
      max_no_progress_decisions: 5,
      max_same_failure_repeats: 3,
      max_invalid_decisions: 3,
      max_usd_per_day: 100,
      ttl_secs: 259200,
    },
    trace: null,
  },
};
const fixtures = {
  "/api/v1/projects/P1": detail,
  "/api/v1/projects/P1/controller": controller,
  "/api/v1/projects/P1/controller/decisions": {
    items: [decision("D2", "pc/outcome-open", "applied", "R2"), decision("D1", "pc/milestone-reached", "none", null)],
  },
  "/api/v1/projects/P1/authority": {
    grants: [],
    proposals: [],
    audit: [],
  },
  "/api/v1/tasks/R1/artifacts": { items: [] },
  "/api/v1/tasks/R2/artifacts": { items: [] },
};

type Sent = { method: string; path: string; body: unknown };

test("parity: /projects/:id 自動運営の画面と mode の切り替え", async ({ page }) => {
  const dir = makeTmpDir("celeris-web-controller-");
  const tokenFile = path.join(dir, "token");
  writeFileSync(tokenFile, `${FIXTURE_TOKEN}\n`);
  const daemon = createFakeDaemon({ token: FIXTURE_TOKEN, fixtures: { ...defaultFixtures, ...fixtures } });
  const gateway = await startGateway({ daemonUrl: await daemon.start(), daemonTokenFile: tokenFile });
  const sent: Sent[] = [];
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    if (request.method() === "GET") return route.fallback();
    sent.push({ method: request.method(), path: new URL(request.url()).pathname, body: request.postDataJSON() });
    await route.fulfill({ status: 200, contentType: "application/json", body: "{}" });
  });
  const shots = process.env.LONG_HORIZON_PILOT_SCREEN_DIR;
  try {
    await page.setViewportSize({ width: 1280, height: 900 });
    await page.goto(`${gateway.base}/projects/P1`);
    const panel = page.getByTestId("project-controller");
    await expect(panel).toBeVisible();
    await expect(page.getByTestId("controller-mode")).toHaveAttribute("data-mode", "enabled");
    await expect(page.getByTestId("controller-policy")).toContainText("計測を再現できるようにして、表を出し続ける");
    await expect(page.getByTestId("controller-roots")).toContainText("実験セル A");
    await expect(page.getByTestId("controller-outcomes")).toContainText("達成");
    await expect(page.getByTestId("controller-outcomes")).toContainText("未達");
    await expect(page.getByTestId("controller-authority-scope")).toContainText("許可なし（no_grant）");
    await expect(page.getByTestId("controller-next")).toContainText("表 2 を出す");
    await expect(page.getByTestId("controller-next-wake")).toContainText("2026-11-04T15:00:00Z");
    await expect(page.getByTestId("controller-waiting")).toContainText("測定用 account の login");
    await expect(page.getByTestId("controller-waiting")).toContainText("人の確認（human check）と本番の操作");
    await expect(page.getByTestId("controller-decisions")).toContainText("pc/outcome-open");
    await expect(page.getByTestId("controller-decisions")).toContainText("効果の参照: R2");
    await expect(page.getByTestId("controller-evidence")).toContainText("task:R1");
    await expect(page.getByTestId("project-roadmap")).toBeVisible();
    await expect(page.getByTestId("project-authority")).toBeVisible();
    if (shots) {
      mkdirSync(shots, { recursive: true });
      await page.getByTestId("project-controller-section").screenshot({ path: path.join(shots, "controller-pc.png") });
    }
    // 理由が無ければ切り替えられない。止める（off）は確認なしで送る。
    await expect(page.getByTestId("controller-mode-off")).toBeDisabled();
    await page.getByTestId("controller-mode-reason").fill("従来の CoS に戻す");
    await page.getByTestId("controller-mode-off").click();
    await expect
      .poll(() => sent.at(-1))
      .toMatchObject({
        method: "PUT",
        path: "/api/projects/P1/controller",
        body: { mode: "off", expected_revision: 4, reason: "従来の CoS に戻す" },
      });
    // スマホ幅でも横に溢れない。
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(panel).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    if (shots) {
      await page
        .getByTestId("project-controller-section")
        .screenshot({ path: path.join(shots, "controller-phone.png") });
    }
  } finally {
    await gateway.close();
    await daemon.close();
    rmSync(dir, { recursive: true, force: true });
  }
});
