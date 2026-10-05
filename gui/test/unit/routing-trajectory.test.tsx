import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { RequestRoutingAudit, RunRoutingAudit, TaskRoutingView } from "~/celeris/types";
import { TaskRoutingPanel } from "~/components/TaskRoutingPanel";
import { actualSourceLine, candidateStatusLabel } from "~/lib/routing-source-state";
import {
  escalationHistory,
  escalationLine,
  executedLaneNote,
  latestRoutingRun,
  outcomeLine,
  outcomeStateLabel,
} from "~/lib/task-routing";
import routingTrajectoryFixture from "../fixtures/api/routing-trajectory.json";

/**
 * 多目的 routing Phase 3（ADR 2026-10-04-multi-objective-model-routing §5・§6）: 軌跡（trajectory）の区別。
 * `routing_trajectory` fixture で固定するもの:
 * - 実行 lane（`run.lane`）と希望 lane（trace の `requested_lane`）
 * - 要求の実 source/model（出所 `from` 付き: proxy log → 試した source の最後 → proxy の決定）
 * - escalation（構造化監査 `escalation_audit` の理由・回数・新旧 lane）
 * - outcome の状態: `not_recorded`（未追記）/ `unreviewed`（未レビュー・中断。false とも 0 とも見ない）/ `judged`
 * - 監査不完全（`audit_incomplete`）は outcome の状態と別の欄（未レビューの run でも出うること）
 * fixture の最新 run（判定済み・escalation 済み）がパネルの中心になる。
 */

const view = routingTrajectoryFixture as unknown as TaskRoutingView;
const [cheapRun, standardRun] = view.runs;
const cheapRequest = (cheapRun.requests ?? [])[0] as RequestRoutingAudit;
const standardRequest = (standardRun.requests ?? [])[0] as RequestRoutingAudit;
const notRecordedRun: RunRoutingAudit = {
  task_id: "T1",
  run_id: "01RUNTRAJ0000000000000003",
  lane: "standard",
  review: null,
};

describe("outcome の状態（未追記 / 未レビュー / 判定済み）", () => {
  it("outcome_state の 3 値を別々の人の語で出す", () => {
    expect(outcomeStateLabel(cheapRun)).toBe("未レビュー（review・acceptance の判定待ち）");
    expect(outcomeStateLabel(standardRun)).toBe("判定済み");
    expect(outcomeStateLabel(notRecordedRun)).toBe("未追記（まだ outcome が無い）");
  });

  it("未レビュー（unreviewed）は false でも 0 でもない: 判定欄は null、reward も null", () => {
    const o = cheapRun.routing_outcome;
    expect(o).not.toBeNull();
    expect(o?.review_passed).toBeNull();
    expect(o?.acceptance_passed).toBeNull();
    expect(o?.reward).toBeNull();
    const line = outcomeLine(o);
    expect(line).toContain("未レビュー/中断");
    expect(line).toContain("reward 未判定");
    expect(line).not.toContain("不合格");
  });

  it("outcome 自体が未追記（routing_outcome が無い）は「未追記」。未レビューと見分けられる", () => {
    expect(notRecordedRun.routing_outcome).toBeUndefined();
    expect(outcomeLine(notRecordedRun.routing_outcome)).toBeNull();
    expect(outcomeStateLabel(notRecordedRun)).not.toBe(outcomeStateLabel(cheapRun));
    expect(outcomeStateLabel(notRecordedRun)).toContain("未追記");
  });

  it("判定済み（judged）は合格・reward・supersede を出す", () => {
    const o = standardRun.routing_outcome;
    expect(o).not.toBeNull();
    expect(o?.review_passed).toBe(true);
    expect(o?.acceptance_passed).toBe(true);
    expect(o?.reward).toBeCloseTo(0.982);
    expect(o?.supersedes).toBe("01OUTTRAJ0000000000000002a");
    expect(outcomeLine(o)).toContain("review 合格");
    expect(outcomeLine(o)).toContain("acceptance 合格");
    expect(outcomeLine(o)).toContain("reward 0.982");
    expect(outcomeLine(o)).toContain("置き換え");
  });
});

describe("軌跡 escalation（構造化監査）", () => {
  it("escalation_audit があれば新旧 lane・理由・品質失敗の回数を 1 行に", () => {
    expect(escalationLine(cheapRun)).toBeNull(); // escalation なし
    expect(escalationLine(standardRun)).toBe(
      "cheap → standard 2 consecutive quality failures at cheap (acceptance/review/test)（品質失敗 2 回で 1 段）",
    );
  });

  it("escalation の履歴は escalation か escalation_audit の付いた run だけ", () => {
    const history = escalationHistory(view);
    expect(history.map((e) => e.runId)).toEqual([standardRun.run_id]);
    expect(history[0].text).toContain("cheap → standard");
    expect(history[0].text).toContain("品質失敗 2 回");
  });

  it("構造化監査が無い旧 run は escalation の自由文字列をそのまま出す", () => {
    const legacy: RunRoutingAudit = {
      task_id: "T1",
      run_id: "R9",
      lane: "frontier",
      escalation: "retry after review_fail: standard -> frontier",
    };
    expect(escalationLine(legacy)).toBe("retry after review_fail: standard -> frontier");
  });
});

describe("実行 lane と希望 lane", () => {
  it("run.lane（実行）と trace.requested_lane（希望）が違えば注記。同じなら出さない", () => {
    expect(executedLaneNote(standardRun)).toBeNull(); // standard = standard
    expect(executedLaneNote({ ...cheapRun, lane: "standard" })).toBe("希望 cheap → 実行 standard");
    expect(executedLaneNote({ ...standardRun, lane: "frontier", requests: null })).toBeNull();
  });
});

describe("要求の実 source / model（出所付き）", () => {
  it("proxy log と結べた要求は from=proxy log。結べず試した source の最後なら from=request_attempts", () => {
    expect(cheapRequest.actual?.from).toBe("request_attempts");
    expect(actualSourceLine(cheapRequest.actual ?? { from: "none" })).toBe(
      "source openai-compatible:qwen/self / model qwen-local（試した source の最後）",
    );
    expect(standardRequest.actual?.from).toBe("proxy_log");
    expect(actualSourceLine(standardRequest.actual ?? { from: "none" })).toBe(
      "source codex-oauth/gpt-standard / model gpt-standard / account codex-a（proxy log）",
    );
  });

  it("試した順と最後の source へ落ちた原因（fallback_reason）を出す", () => {
    expect((standardRequest.attempts ?? []).map((a) => a.source_id)).toEqual([
      "codex-oauth/gpt-standard",
      "codex-oauth/gpt-standard",
    ]);
    expect((standardRequest.attempts ?? []).map((a) => a.account_id)).toEqual(["codex-b", "codex-a"]);
    expect(standardRequest.fallback_reason).toBe("rate_limit");
  });
});

describe("表示（SSR の HTML で確かめる）", () => {
  const html = renderToStaticMarkup(<TaskRoutingPanel view={view} />);

  it("実行 lane の欄は run.lane（最新 run = standard）", () => {
    expect(latestRoutingRun(view)?.run_id).toBe(standardRun.run_id);
    expect(html).toContain('data-testid="task-routing-lane"');
    expect(html).toContain("standard");
  });

  it("実 source 欄は出所付きで、要求ごとの監査にも試した順と fallback が出る", () => {
    expect(html).toContain('data-testid="task-routing-actual-sources"');
    expect(html).toContain("（proxy log）");
    expect(html).toContain('data-testid="routing-request-attempts"');
    expect(html).toContain("codex-oauth/gpt-standard → codex-oauth/gpt-standard");
    expect(html).toContain('data-testid="routing-request-fallback"');
    expect(html).toContain("最後の source へ落ちた原因: rate_limit");
    expect(html).toContain('data-testid="routing-request-actual"');
    expect(html).toContain("account codex-a（proxy log）");
    // provider 選択理由（trace の reasons）
    expect(html).toContain('data-testid="routing-request-reasons"');
    expect(html).toContain("quality");
  });

  it("escalation の理由は構造化監査から。候補の除外理由も見える", () => {
    expect(html).toContain('data-testid="task-routing-escalations"');
    expect(html).toContain("cheap → standard");
    expect(html).toContain("品質失敗 2 回");
    const cheapCandidate = cheapRequest.trace.candidates[1];
    expect(candidateStatusLabel(cheapCandidate)).toEqual({
      label: "品質が下限未満",
      excluded: true,
    });
  });

  it("outcome の状態を区別する: 最新 run（standard）は判定済み・reward・置き換え", () => {
    expect(html).toContain('data-testid="task-routing-outcome"');
    expect(html).toContain("判定済み");
    expect(html).toContain("reward 0.982");
    expect(html).toContain('data-testid="task-routing-outcome-supersede"');
  });

  it("監査不完全（audit_incomplete）は別の欄。run ごとに出る", () => {
    // 表示の中心は最新 run（standardRun, audit_incomplete=false）なので注記は出ない。
    expect(html).not.toContain('data-testid="task-routing-incomplete"');
    // 未レビューの run（cheapRun）は監査不完全でもある（別件事実）。両方を持つ view で確かめる。
    const mixed: TaskRoutingView = { ...view, runs: [cheapRun] };
    const mixedHtml = renderToStaticMarkup(<TaskRoutingPanel view={mixed} />);
    expect(mixedHtml).toContain('data-testid="task-routing-incomplete"');
    expect(mixedHtml).toContain("監査が不完全です（proxy log に対応する要求が無い）");
    expect(mixedHtml).toContain("未レビュー（review・acceptance の判定待ち）");
    expect(mixedHtml).toContain("reward 未判定");
  });
});
