import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { configureApiClient } from "../../api/client";
import {
  type AssignmentList,
  assignmentPath,
  assignmentsPreviewPath,
  deleteAssignment,
  previewAssignment,
  putAssignment,
  type RoleSlotView,
} from "../../api/model-assignments";
import {
  availabilityLabel,
  groupSlots,
  hasOpencodeGoProvider,
  llmSourceName,
  OPENCODE_GO_PROVIDER_BODY,
  poolRemaining,
  quotaLabel,
  RoleAssignmentsView,
} from "./model-assignments";
import type { LlmModel } from "./models-catalog";

const slot = (over: Partial<RoleSlotView>): RoleSlotView => ({
  source: "claude-oauth",
  tier: "frontier",
  model_id: null,
  origin: null,
  excluded_reason: null,
  providers: [],
  proxy: false,
  available: null,
  last_seen: null,
  ...over,
});

const data: AssignmentList = {
  items: [],
  effective: [
    slot({ tier: "cheap", model_id: "haiku", origin: "assignment", excluded_reason: "catalog:unavailable" }),
    slot({
      tier: "frontier",
      model_id: "opus",
      origin: "assignment",
      available: true,
      last_seen: "2026-10-06T00:00:00Z",
    }),
    slot({ tier: "standard", model_id: "sonnet", origin: "config", available: false }),
    slot({ source: "opencode-go", tier: "frontier" }),
    slot({ source: "opencode-go", tier: "standard" }),
    slot({ source: "opencode-go", tier: "cheap" }),
  ],
};

const model = (over: Partial<LlmModel>): LlmModel => ({
  source: "claude-oauth",
  model_id: "opus",
  display_name: null,
  available: true,
  first_seen: "2026-10-01T00:00:00Z",
  last_seen: "2026-10-06T00:00:00Z",
  capabilities: {},
  override: null,
  routing: { tiers: [], deployments: [] },
  ...over,
});

const accounts = [
  { adapter: "claude-code", usage: { five_hour: { utilization: 0.2 }, seven_day: { utilization: 0.4 } } },
  { adapter: "claude-code", usage: { five_hour: { utilization: 0.9 }, seven_day: null } },
  { adapter: "opencode-go", usage: null },
];

describe("helpers", () => {
  it("役割は frontier / standard / cheap の順に並ぶ", () => {
    const groups = groupSlots(data.effective);
    expect(groups.map((g) => g.source)).toEqual(["claude-oauth", "opencode-go"]);
    expect(groups[0]?.slots.map((s) => s.tier)).toEqual(["frontier", "standard", "cheap"]);
  });

  it("枠は pool の最小残量。取れない source は不明", () => {
    expect(poolRemaining(accounts, "claude-oauth")).toBeCloseTo(0.1);
    expect(poolRemaining(accounts, "opencode-go")).toBeNull();
    expect(poolRemaining(accounts, "codex-oauth")).toBeNull();
    expect(poolRemaining(null, "claude-oauth")).toBeNull();
    expect(quotaLabel(0.1)).toBe("残り 10%");
    expect(quotaLabel(null)).toBe("不明");
  });

  it("availability の語", () => {
    expect(availabilityLabel(true).label).toBe("利用可");
    expect(availabilityLabel(false).label).toBe("消失");
    expect(availabilityLabel(null).label).toBe("不明");
  });

  it("llm_source は文字列でも object でも opencode_go を見つける", () => {
    expect(llmSourceName({ origin: "explicit", source: "opencode_go" })).toBe("opencode_go");
    expect(hasOpencodeGoProvider([{ llm_source: "opencode_go" }])).toBe(true);
    expect(hasOpencodeGoProvider([{ llm_source: { origin: "explicit", source: "opencode_go" } }])).toBe(true);
    expect(hasOpencodeGoProvider([{ llm_source: "claude_oauth" }, {}])).toBe(false);
  });
});

describe("RoleAssignmentsView", () => {
  const render = (providers: { llm_source?: unknown }[] | null) =>
    renderToStaticMarkup(
      <QueryClientProvider client={new QueryClient()}>
        <RoleAssignmentsView
          data={data}
          models={[model({}), model({ model_id: "gone", available: false })]}
          accounts={accounts}
          providers={providers}
        />
      </QueryClientProvider>,
    );

  it("全モデルの役割トグルと検索・絞り込み・役割別表示を出す", () => {
    const out = render([]);
    expect(out).toContain("モデルごとの役割");
    expect(out).toContain('aria-label="claude-oauth opus frontier"');
    expect(out).toContain('aria-label="claude-oauth opus standard"');
    expect(out).toContain('aria-label="claude-oauth opus cheap"');
    expect(out).toContain("供給元で絞り込み");
    expect(out).toContain("状態で絞り込み");
    expect(out).toContain("役割ごとの表示");
    expect(out).toContain("消失");
  });

  it("opencode go の provider が無いときだけ追加ボタンを出す", () => {
    expect(render([])).toContain("opencode go を使う（provider を追加）");
    expect(render([{ llm_source: "opencode_go" }])).not.toContain("opencode go を使う");
    expect(render(null)).not.toContain("opencode go を使う");
  });
});

describe("client", () => {
  type Call = { path: string; method: string; body: unknown };
  const calls: Call[] = [];
  const respond = (status: number, body?: unknown) => {
    configureApiClient({
      fetcher: async (input, init) => {
        calls.push({
          path: String(input),
          method: init?.method ?? "GET",
          body: typeof init?.body === "string" ? JSON.parse(init.body) : undefined,
        });
        return new Response(body === undefined ? null : JSON.stringify(body), { status });
      },
    });
  };
  afterEach(() => {
    calls.length = 0;
    configureApiClient({ fetcher: (input, init) => fetch(input, init) });
  });

  it("PUT は source と model 以外の / を path に入れ、本文は { model_id, note? }", async () => {
    respond(200, { item: {}, impact: { changes: [] } });
    await putAssignment("openai-compatible:qwen", "cheap", { model_id: "org/qwen3", note: "n" });
    expect(calls[0]).toEqual({
      path: "/api/llm/models/assignments/openai-compatible%3Aqwen/cheap",
      method: "PUT",
      body: { model_id: "org/qwen3", note: "n" },
    });
    expect(assignmentPath("a/b", "cheap")).toBe("/api/llm/models/assignments/a%2Fb/cheap");
  });

  it("DELETE は 204 を受ける", async () => {
    respond(204);
    await deleteAssignment("claude-oauth", "frontier");
    expect(calls[0]).toMatchObject({ method: "DELETE", path: "/api/llm/models/assignments/claude-oauth/frontier" });
  });

  it("preview は POST で model_id に null も送れる", async () => {
    respond(200, { impact: { changes: [] } });
    await previewAssignment({ source: "claude-oauth", tier: "cheap", model_id: null });
    expect(calls[0]).toEqual({
      path: assignmentsPreviewPath,
      method: "POST",
      body: { source: "claude-oauth", tier: "cheap", model_id: null },
    });
  });

  it("opencode go の provider の本文は ADR D3 の形", () => {
    expect(OPENCODE_GO_PROVIDER_BODY).toEqual({
      id: "opencode-go",
      adapter: "acp",
      llm_source: "opencode_go",
      account_pool: "opencode-go",
      tiers: ["frontier", "standard", "cheap"],
      concurrency: 1,
    });
  });
});

describe("role membership ordering", () => {
  it("同じ source の複数候補を保持し、並べ替えは他の役割を変えない", async () => {
    const { roleMembers, moveMember } = await import("./model-role-editor");
    const multiple: AssignmentList = {
      items: [],
      effective: [
        slot({ source: "opencode-go", tier: "standard", model_id: "a", priority: 5 }),
        slot({ source: "opencode-go", tier: "standard", model_id: "b", priority: 1 }),
        slot({ source: "opencode-go", tier: "frontier", model_id: "a", priority: 0 }),
      ],
    };
    const ordered = roleMembers(multiple, "standard");
    expect(ordered.map((m) => m.model_id)).toEqual(["b", "a"]);
    expect(moveMember(ordered, 1, -1)).toEqual([
      { source: "opencode-go", model_id: "a", priority: 0 },
      { source: "opencode-go", model_id: "b", priority: 1 },
    ]);
    expect(roleMembers(multiple, "frontier").map((m) => m.model_id)).toEqual(["a"]);
  });
});
