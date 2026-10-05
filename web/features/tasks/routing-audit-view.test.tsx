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

// 候補 1 件の <li> の中身だけを取り出す（別の候補の文言と混ざらないように）。
function candidate(out: string, deploymentId: string): string {
  const start = out.indexOf(`aria-label="候補 ${deploymentId}"`);
  expect(start).toBeGreaterThanOrEqual(0);
  return out.slice(start, out.indexOf("</li>", start));
}

describe("task の routing 監査（ADR 2026-10-04-multi-objective-model-routing §8 Phase 2）", () => {
  it("fixture は TaskRoutingView の生成型に合う", () => {
    expect(validateFixture(routingAuditFixture, schema.$defs.TaskRoutingView)).toEqual([]);
  });

  it("監査が不完全なことを先に言い、結べない要求を理由と一緒に出す", () => {
    const out = html(routingAuditFixture);
    expect(out).toContain("監査が不完全です（request_log_missing）");
    expect(out).toContain("結べていません: request_log_missing");
    expect(out).toContain("要求 Q1（決定 D1）");
  });

  it("候補の選択・除外理由・score 内訳を出し、選ばれた候補と除外を区別する", () => {
    const out = html(routingAuditFixture);
    expect(candidate(out, "openai-compatible:qwen/qwen3-coder")).toContain("選択");
    expect(candidate(out, "claude-oauth/claude-opus")).toContain("残量が枯渇している");
    expect(candidate(out, "openai-compatible:qwen/qwen3-coder")).toContain("score 0.58: 品質 0.7×1");
    expect(candidate(out, "openai-compatible:qwen/qwen3-coder")).toContain("（不明: shadow）");
  });

  it("最終の source・model・account を trace から出す（欠測は不明）", () => {
    const out = html(routingAuditFixture);
    expect(out).toContain("最終: source openai-compatible:qwen / model Qwen/Qwen3-Coder / 口座 不明");
  });

  it("費用は請求と機会費用を分け、欠測の請求を 0 円と書かない", () => {
    const out = html(routingAuditFixture);
    const claude = candidate(out, "claude-oauth/claude-opus");
    expect(claude).toContain('請求（実料金）</dt><dd class="break-words">不明</dd>');
    expect(claude).toContain('枠の機会費用（subscription の残量）</dt><dd class="break-words">$0.5</dd>');
    const qwen = candidate(out, "openai-compatible:qwen/qwen3-coder");
    expect(qwen).toContain('自前計算の機会費用（GPU・待ち）</dt><dd class="break-words">$0.01</dd>');
  });

  it("要求の記録が無い旧 run は、欄を不明として旧 run と書く", () => {
    const out = html({
      task_id: "T1",
      runs: [{ run_id: "R0", task_id: "T1", requests: null }],
      unbound_requests: [],
    });
    expect(out).toContain("要求単位の記録がない旧 run です");
  });
});

describe("task の routing 軌跡（ADR 2026-10-04-multi-objective-model-routing §5・§6・§10 Phase 3）", () => {
  it("fixture は TaskRoutingView の生成型に合う", () => {
    expect(validateFixture(routingTrajectoryFixture, schema.$defs.TaskRoutingView)).toEqual([]);
  });

  it("実行 lane・選定理由・実際の source を出す", () => {
    const out = html(routingTrajectoryFixture);
    expect(out).toContain("実行 lane standard");
    expect(out).toContain("選定理由: trajectory_escalation");
    expect(out).toContain("source claude-oauth / model claude-sonnet / 口座 main（出所: proxy_log）");
  });

  it("escalation の理由を requested→selected と連続失敗回数つきで出す", () => {
    const out = html(routingTrajectoryFixture);
    expect(out).toContain("escalation: cheap（直前 cheap） → standard / 理由 repeated_review_failures（連続失敗 2 回");
  });

  it("outcome_state で unreviewed・not_recorded・audit_incomplete を区別する（false や 0 に丸めない）", () => {
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
    expect(out).toContain("実行枠: 供給元 claude-oauth / model claude-sonnet / 口座 main");
    const section = out.indexOf('aria-label="shadow 監査"');
    expect(section).toBeGreaterThan(out.indexOf("実行枠: 供給元 claude-oauth"));
    expect(out.match(/aria-label="shadow 監査"/g)).toHaveLength(1);
  });

  it("decision と execution、completed/failed/dropped と理由、候補との差を出す", () => {
    const out = html(routingShadowFixture);
    const s1 = shadow(out, "S1");
    expect(s1).toContain("判断のみ（decision、追加呼出しなし） / 完了");
    expect(s1).toContain("候補: source openai-compatible:qwen / model Qwen/Qwen3-Coder / primary との差 あり");
    expect(s1).not.toContain("上限消費");
    expect(shadow(out, "S2")).toContain("primary との差 なし（同じ）");
    expect(shadow(out, "S3")).toContain("実行（execution、候補で生成） / 失敗 / 理由 timeout");
    expect(shadow(out, "S4")).toContain("破棄 / 理由 cap_exceeded");
  });

  it("上限の予約と消費を出し、欠測は不明と書く（0 に丸めない）", () => {
    const out = html(routingShadowFixture);
    expect(shadow(out, "S2")).toContain(
      "上限消費（2026-10-05 UTC、charged）: 予約 1500 token・$0.05 / 消費 1000 token・$0.03",
    );
    expect(shadow(out, "S3")).toContain("token: 入力 800 / 出力 不明");
    const s4 = shadow(out, "S4");
    expect(s4).toContain("候補: source 不明 / model 不明 / primary との差 不明");
    expect(s4).toContain("上限消費: 不明（予約の記録なし）");
    expect(s4).not.toContain("0 token");
  });

  it("shadow の記録が無い run には shadow の節を出さない", () => {
    const out = html({ ...routingShadowFixture, runs: routingShadowFixture.runs.slice(1) });
    expect(out).not.toContain("shadow 監査");
  });
});
