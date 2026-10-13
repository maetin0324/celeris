import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import type { TaskRoutingView } from "../../api/generated/types";
import {
  routingAuditFixture,
  routingShadowFixture,
  routingTrajectoryFixture,
  validateFixture,
} from "../../e2e/support/fake-daemon.mjs";
import { RoutingAuditView } from "./routing-audit-view";

const html = (data: TaskRoutingView) => renderToStaticMarkup(<RoutingAuditView data={data} />);

describe("task の routing 監査（ADR 2026-10-04-multi-objective-model-routing §8 Phase 2）", () => {
  it("fixture は TaskRoutingView の生成型に合う", () => {
    expect(validateFixture(routingAuditFixture, schema.$defs.TaskRoutingView)).toEqual([]);
  });

  it("どの run にも結べない要求を理由なしの一覧で出す", () => {
    const out = html(routingAuditFixture);
    expect(out).toContain("どの run にも結べない要求");
    expect(out).toContain("要求 不明（決定 D2）");
  });

  it("実行枠を harness・source・model・口座の順に出す（欠測は不明）", () => {
    const out = html(routingAuditFixture);
    expect(out).toContain("実行枠: harness 不明 / source openai-compatible:qwen / model qwen3-coder / 口座 不明");
    const withHarness = html({
      ...routingAuditFixture,
      runs: routingAuditFixture.runs.map((run) => ({ ...run, harness: "opencode" })),
    });
    expect(withHarness).toContain("実行枠: harness opencode / source openai-compatible:qwen / model qwen3-coder");
  });

  it("実行 lane・組織・規則と実測の請求を出し、欠測の請求は不明と書く", () => {
    const out = html(routingAuditFixture);
    expect(out).toContain("R1: 実行 lane cheap / 組織 software-engineering（cheap-local-first）");
    expect(out).toContain("実測の請求: $0.02");
    const none = html({
      ...routingAuditFixture,
      runs: routingAuditFixture.runs.map((run) => ({ ...run, cost_usd: null })),
    });
    expect(none).toContain("実測の請求: 不明");
    expect(none).not.toContain("実測の請求: $0");
  });
});

describe("task の routing 軌跡（ADR 2026-10-04-multi-objective-model-routing §5・§6・§10 Phase 3）", () => {
  it("fixture は TaskRoutingView の生成型に合う", () => {
    expect(validateFixture(routingTrajectoryFixture, schema.$defs.TaskRoutingView)).toEqual([]);
  });

  it("実行 lane・選定理由を出す", () => {
    const out = html(routingTrajectoryFixture);
    expect(out).toContain("実行 lane standard");
    expect(out).toContain("選定理由: trajectory_escalation");
    expect(out).toContain("実行枠: harness claude-code / source claude-oauth / model claude-sonnet / 口座 main");
  });

  it("escalation の理由を requested→selected と連続失敗回数つきで出す", () => {
    const out = html(routingTrajectoryFixture);
    expect(out).toContain("escalation: cheap（直前 cheap） → standard / 理由 repeated_review_failures（連続失敗 2 回");
  });

  it("outcome_state で unreviewed・not_recorded を区別する（false や 0 に丸めない）", () => {
    const out = html(routingTrajectoryFixture);
    expect(out).toContain("outcome: 未レビュー（合否は null のまま）");
    expect(out).toContain("outcome: 未記録");
    expect(out).not.toContain("outcome: 判定済み");
  });
});

// shadow 1 件の <li> の中身だけを取り出す。
function shadow(out: string, shadowId: string): string {
  const start = out.indexOf(`aria-label="shadow ${shadowId}"`);
  expect(start).toBeGreaterThanOrEqual(0);
  return out.slice(start, out.indexOf("</li>", start));
}

describe("task の routing shadow 監査（ADR 2026-10-04-multi-objective-model-routing §7.1・§10 Phase 4）", () => {
  it("fixture routing_shadow は TaskRoutingView の生成型に合う", () => {
    expect(validateFixture(routingShadowFixture, schema.$defs.TaskRoutingView)).toEqual([]);
  });

  it("shadow は primary とは別の節に出し、primary の実行枠は primary の値のまま", () => {
    const out = html(routingShadowFixture);
    expect(out).toContain("実行枠: harness claude-code / source claude-oauth / model claude-sonnet / 口座 main");
    const section = out.indexOf('aria-label="shadow 監査"');
    expect(section).toBeGreaterThan(out.indexOf("実行枠: harness claude-code / source claude-oauth"));
    expect(out.match(/aria-label="shadow 監査"/g)).toHaveLength(1);
  });

  it("decision と execution、completed/failed/dropped と理由、候補との差を出す", () => {
    const out = html(routingShadowFixture);
    const s1 = shadow(out, "S1");
    expect(s1).toContain("判断のみ（decision、追加呼出しなし） / 完了");
    expect(s1).toContain("候補: source openai-compatible:qwen / model Qwen/Qwen3-Coder / primary との差 あり");
    expect(shadow(out, "S2")).toContain("primary との差 なし（同じ）");
    expect(shadow(out, "S3")).toContain("実行（execution、候補で生成） / 失敗 / 理由 timeout");
    expect(shadow(out, "S4")).toContain("破棄 / 理由 cap_exceeded");
  });

  it("token の欠測は不明と書き、候補の欠測も不明と書く（0 に丸めない）", () => {
    const out = html(routingShadowFixture);
    expect(shadow(out, "S3")).toContain("token: 入力 800 / 出力 不明");
    const s4 = shadow(out, "S4");
    expect(s4).toContain("候補: source 不明 / model 不明 / primary との差 不明");
    expect(s4).not.toContain("0 token");
  });

  it("shadow の記録が無い run には shadow の節を出さない", () => {
    const out = html({ ...routingShadowFixture, runs: routingShadowFixture.runs.slice(1) });
    expect(out).not.toContain("shadow 監査");
  });

  it("上限の予約・消費の欄は出さない（予約の API は無い）", () => {
    const out = html(routingShadowFixture);
    expect(out).not.toContain("上限消費");
    expect(out).not.toContain("予約");
  });
});
