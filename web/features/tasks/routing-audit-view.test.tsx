import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import schema from "../../api/generated/schema.json";
import type { TaskRoutingView } from "../../api/generated/types";
import { routingAuditFixture, validateFixture } from "../../e2e/support/fake-daemon.mjs";
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
