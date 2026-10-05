import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TaskRoutingView } from "~/celeris/types";
import { TaskRoutingPanel } from "~/components/TaskRoutingPanel";
import routingEstimatorShadowFixture from "../fixtures/api/routing-estimator-shadow.json";

/**
 * Phase 5: estimator shadow の欄。primary（heuristic）の review・model・outcome とは別欄に、
 * estimator id/version・heuristic primary との差・timeout・prompt_required・dropped の理由を出す。
 * 本番切替の操作は出さない。`routing-estimator-shadow.json` は mock と共有しない（この unit 専用）。
 */
const view = routingEstimatorShadowFixture as unknown as TaskRoutingView;

describe("routing estimator shadow（heuristic primary との差）", () => {
  it("estimator shadow を primary・旧 shadow と別欄に出す", () => {
    const html = renderToStaticMarkup(<TaskRoutingPanel view={view} />);
    const review = html.indexOf('data-testid="task-routing-review"');
    const estimator = html.indexOf('data-testid="task-routing-estimator-shadow"');
    expect(review).toBeGreaterThan(-1);
    expect(estimator).toBeGreaterThan(review);
    // 本 fixture には estimator 以外の shadow がないので旧欄は出さない
    expect(html).not.toContain('data-testid="task-routing-shadow"');
    // primary の model は primary 欄に、estimator id は estimator 欄に（別欄）
    expect(html).toContain("primary-model");
    expect(html).toContain('data-testid="task-routing-estimator-id"');
    // estimator id の cell は estimator 欄（estimator-shadow 節）の内部に閉じる
    const estimatorSection = html.slice(html.indexOf('data-testid="task-routing-estimator-shadow"'));
    expect(estimatorSection).toContain("routellm/1");
    expect(estimatorSection.indexOf("routellm/1")).toBeLessThan(estimatorSection.indexOf("</section>"));
  });

  it("estimator id/version・状態・差・timeout・prompt_required・dropped の理由を出す", () => {
    const html = renderToStaticMarkup(<TaskRoutingPanel view={view} />);
    // id/version は `routellm/1`
    expect(html).toContain("routellm/1");
    // 状態（5 件の outcome 全部）
    expect(html).toContain("完了");
    expect(html).toContain("timeout");
    expect(html).toContain("見送り");
    expect(html).toContain("prompt 要");
    // heuristic 首位 / heuristic primary との差
    expect(html).toContain('heuristic 首位との差</dt><dd data-testid="task-routing-estimator-vs-heuristic">異なる');
    expect(html).toContain('heuristic primary との差</dt><dd data-testid="task-routing-estimator-vs-primary">異なる');
    // timeout・dropped・prompt_required の理由
    expect(html).toContain('task-routing-estimator-reason">timeout · timeout');
    expect(html).toContain("cap_exceeded");
    expect(html).toContain("（daemon の許可外）");
    // overhead
    expect(html).toContain("42 ms");
    // 本番切替の操作は出さない（切替・on といった操作語は欄に出さない）
    expect(html).not.toContain("本番切替");
    expect(html).not.toContain("shadow → opt-in");
  });

  it("estimator shadow の欠測は不明とし、旧 run（estimator 欄なし）には別欄を出さない", () => {
    const est = view.runs[0].routing_shadow?.[0].estimator;
    const oldEst: NonNullable<typeof est> = est
      ? {
          ...est,
          estimator_id: null,
          estimator_version: null,
          vs_heuristic: null,
          vs_primary: null,
          overhead_ms: null,
          dependencies: {},
        }
      : { dependencies: {}, outcome: "failed" };
    const oldView: TaskRoutingView = {
      ...view,
      estimator_shadow: undefined,
      runs: [
        {
          ...view.runs[0],
          routing_shadow: [
            { shadow_id: "x", primary_decision_id: "d", kind: "estimator", status: "failed", estimator: oldEst },
          ],
        },
      ],
    };
    const html = renderToStaticMarkup(<TaskRoutingPanel view={oldView} />);
    // 欄は出るが、値は不明（testid と値の間に class が入るため `class="…">不明` で確認）
    expect(html).toContain('data-testid="task-routing-estimator-shadow"');
    expect(html).toContain('task-routing-estimator-id" class="break-all">不明');
    expect(html).toContain('task-routing-estimator-vs-heuristic">不明');
    expect(html).toContain('task-routing-estimator-vs-primary">不明');
    // 要約が出ない
    expect(html).not.toContain('data-testid="task-routing-estimator-summary"');
  });

  it("estimator shadow の要約（coverage・差の件数）を出す", () => {
    const html = renderToStaticMarkup(<TaskRoutingPanel view={view} />);
    expect(html).toContain('data-testid="task-routing-estimator-summary"');
    expect(html).toContain("対象 4 件");
    expect(html).toContain("完了 1 件");
    expect(html).toContain("coverage 25%");
    expect(html).toContain("heuristic 首位と異なる 1 件");
    expect(html).toContain("heuristic primary と異なる 1 件");
    expect(html).toContain("平均 overhead 42 ms");
    expect(html).toContain("estimator routellm/1");
    // 要約が estimator 欄の直下（別欄の先頭）にある
    const section = html.indexOf('data-testid="task-routing-estimator-shadow"');
    const summary = html.indexOf('data-testid="task-routing-estimator-summary"');
    expect(summary).toBeGreaterThan(section);
  });

  it("estimator 欄が無くても primary・旧 shadow の表示は変わらない", () => {
    const plain: TaskRoutingView = {
      ...view,
      estimator_shadow: undefined,
      runs: [
        {
          task_id: "T",
          run_id: "R",
          lane: "standard",
          model: "primary-model",
          review: { passed: true },
          routing_shadow: [
            {
              shadow_id: "s1",
              primary_decision_id: "d",
              kind: "decision",
              status: "completed",
              candidate_model: "candidate-model",
              differs_from_primary: true,
            },
          ],
        },
      ],
    };
    const html = renderToStaticMarkup(<TaskRoutingPanel view={plain} />);
    // 旧 shadow 欄は出る（estimator を除いたものだけ）
    expect(html).toContain('data-testid="task-routing-shadow"');
    expect(html).toContain("判断のみ · 完了");
    // estimator 欄は出ない
    expect(html).not.toContain('data-testid="task-routing-estimator-shadow"');
    expect(html).not.toContain("estimator shadow");
  });
});
