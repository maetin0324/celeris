import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { CandidateTrace, LlmSourceStateView, RequestRoutingAudit, TaskRoutingView } from "~/celeris/types";
import { RequestAudit, SourceDeployments } from "~/components/RoutingSourceState";
import {
  auditIncompleteNote,
  candidateStatusLabel,
  costRows,
  finalSelection,
  freshnessLabel,
  incompleteReasonLabel,
  resetLabel,
  scoreRows,
  unknownFieldLabels,
  usdLabel,
} from "~/lib/routing-source-state";
import routingFixture from "../fixtures/api/routing-source-state.json";

/**
 * 多目的 routing の表示（Phase 2）。`test/fixtures/api/routing-source-state.json` は mock（`scripts/lib/celeris-fixture.mjs`）
 * と共有する。未知は「不明」（0 円・満タンに見せない）、請求と機会費用は別の欄、除外理由・最終の source・
 * 監査の不完全さが見えること、時刻は注入して決定的に確かめる。
 */

const fixture = routingFixture as unknown as {
  llm_sources: { sources: { id: string; deployments: LlmSourceStateView[] }[] };
  task_routing: TaskRoutingView;
};

const codex = fixture.llm_sources.sources[0].deployments[0];
const qwen = fixture.llm_sources.sources[1].deployments[0];
const runWithRequests = fixture.task_routing.runs[0];
const request = (runWithRequests.requests ?? [])[0] as RequestRoutingAudit;
const candidates = request.trace.candidates;
const NOW_MS = Date.parse("2026-10-05T12:00:00Z");

describe("請求と機会費用（cost rows）", () => {
  it("未知の請求・機会費用・実効費用は「不明」。0 円に見せない", () => {
    const rows = Object.fromEntries(costRows(qwen.cost).map((r) => [r.key, r.value]));
    expect(rows.cash).toBe("不明");
    expect(rows.shadow).toBe("不明");
    expect(rows.effective).toBe("不明");
    expect(rows.resource).toBe("$0.12");
    expect(usdLabel(null)).toBe("不明");
    expect(usdLabel(undefined)).toBe("不明");
    expect(usdLabel(Number.NaN)).toBe("不明");
  });

  it("請求（cash）と機会費用（shadow・resource）は別の行で、機会費用の行だけ opportunity", () => {
    const rows = costRows(codex.cost);
    expect(rows.map((r) => r.key)).toEqual(["cash", "shadow", "resource", "effective"]);
    const byKey = Object.fromEntries(rows.map((r) => [r.key, r]));
    expect(byKey.cash.opportunity).toBe(false);
    expect(byKey.cash.value).toBe("$0.00"); // 既知の 0（subscription の API 請求は 0）
    expect(byKey.shadow.opportunity).toBe(true);
    expect(byKey.shadow.value).toBe("$0.31");
    expect(byKey.resource.value).toBe("不明");
    expect(byKey.effective.value).toBe("不明"); // 未知の成分があれば合計も不明
  });

  it("費用の見積もりが無い（cost が null）と全部「不明」", () => {
    expect(costRows(null).every((r) => r.value === "不明")).toBe(true);
  });
});

describe("鮮度・残量・reset（時刻は注入）", () => {
  it("未観測は「未観測」と古い扱い。観測済みで期限切れは「古い」", () => {
    expect(freshnessLabel(qwen.freshness)).toEqual({ text: "未観測（判定に使わない）", stale: true });
    const stale = freshnessLabel({
      observed_at: "2026-10-05T11:00:00Z",
      age_secs: 3600,
      expires_at: null,
      stale: true,
    });
    expect(stale.stale).toBe(true);
    expect(stale.text).toContain("古い");
    expect(freshnessLabel(codex.freshness).stale).toBe(false);
  });

  it("reset までの残り。未知は「不明」、過ぎていれば過ぎたと出す", () => {
    expect(resetLabel(null, NOW_MS)).toBe("不明");
    expect(resetLabel("not-a-time", NOW_MS)).toBe("不明");
    expect(resetLabel("2026-10-05T14:00:00Z", NOW_MS)).toMatch(/^あと .+/);
    expect(resetLabel("2026-10-05T14:00:00Z", NOW_MS)).not.toBe(resetLabel("2026-10-05T13:00:00Z", NOW_MS));
    expect(resetLabel("2026-10-05T11:00:00Z", NOW_MS)).toBe("reset 時刻を過ぎています");
  });

  it("未知の欄の名前を人の語にする", () => {
    expect(unknownFieldLabels(qwen.unknown)).toContain("請求");
    expect(unknownFieldLabels(["something_new"])).toEqual(["something_new"]);
  });
});

describe("候補の除外理由と score 内訳", () => {
  it("除外理由（excluded_reason）を人の語に。採用候補は「採用候補」", () => {
    expect(candidateStatusLabel(candidates[0])).toEqual({ label: "採用候補", excluded: false });
    expect(candidateStatusLabel(candidates[1])).toEqual({ label: "残量切れ", excluded: true });
    expect(candidateStatusLabel(candidates[2])).toEqual({ label: "cooldown 中", excluded: true });
  });

  it("旧 event（excluded_reason が無い）は生のコードをそのまま出す", () => {
    const legacy: CandidateTrace = { ...candidates[0], excluded_reason: null, excluded_reasons: ["budget_cap"] };
    expect(candidateStatusLabel(legacy)).toEqual({ label: "budget_cap", excluded: true });
  });

  it("score の内訳は Q・C・L・P の項と未知の注記（1 として計算）", () => {
    const rows = scoreRows(candidates[0].score_breakdown);
    expect(rows.map((r) => r.key)).toEqual(["q", "c", "l", "p"]);
    expect(rows.find((r) => r.key === "c")?.unknownNote).toBe("不明（1 として計算）");
    expect(rows.find((r) => r.key === "q")?.unknownNote).toBeNull();
    expect(scoreRows(null)).toEqual([]);
  });
});

describe("最終の選択と監査の不完全さ", () => {
  it("最終の source・model・account は proxy log の実際の行き先を優先する", () => {
    expect(finalSelection(request)).toEqual({
      source: "codex-oauth/gpt-standard",
      model: "gpt-standard",
      account: "codex-a",
    });
    const noLog = { ...request, log: null };
    expect(finalSelection(noLog).account).toBe("codex-a"); // trace の値
    const empty = {
      ...request,
      log: null,
      trace: { ...request.trace, source_id: null, model: null, account_id: null },
    };
    expect(finalSelection(empty)).toEqual({ source: "不明", model: "不明", account: "不明" });
  });

  it("監査が不完全なら理由を並べて出し、完全なら何も出さない", () => {
    expect(auditIncompleteNote(runWithRequests.audit_incomplete, runWithRequests.incomplete_reasons)).toBe(
      "監査が不完全です（要求 ID が無い・proxy log に対応する要求が無い）",
    );
    expect(auditIncompleteNote(false, [])).toBeNull();
    expect(auditIncompleteNote(null, null)).toBeNull();
    expect(incompleteReasonLabel("request_log_mismatch")).toBe("proxy log と決定が食い違う");
  });
});

describe("表示（SSR の HTML で確かめる）", () => {
  it("deployment の表示は未知を「不明」と出し、請求と機会費用の欄を分ける", () => {
    const html = renderToStaticMarkup(<SourceDeployments deployments={[qwen]} nowMs={NOW_MS} />);
    expect(html).toContain('data-testid="routing-freshness"');
    expect(html).toContain("未観測");
    expect(html).toContain('data-cost-key="cash"');
    expect(html).toContain('data-cost-key="shadow"');
    expect(html).toContain('data-testid="routing-cost-cash"');
    expect(html).not.toMatch(/routing-cost-cash"[^>]*>\$0\.00/);
    expect(html).toContain("不明: ");
  });

  it("要求の監査は最終の source と候補の除外理由を出す。本文色に --success を使わない", () => {
    const html = renderToStaticMarkup(<RequestAudit request={request} />);
    expect(html).toContain("source codex-oauth/gpt-standard");
    expect(html).toContain("残量切れ");
    expect(html).toContain("cooldown 中");
    expect(html).toContain("proxy log に対応する要求が無い");
    expect(html).not.toContain("text-success");
    expect(html).toContain("text-danger-soft-fg");
  });
});
