import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createMemoryRouter, RouterProvider } from "react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { applyRetry } from "~/celeris/actions.server";
import { CelerisClient } from "~/celeris/client.server";
import { decomposeTask } from "~/celeris/tasks-admin.server";
import type { DecomposeResult, ExecutionGateDecision, ExecutionView, Task } from "~/celeris/types";
import { ExecutionModeControl } from "~/components/ExecutionModeControl";
import { executionModeControls, gateDecisionLine } from "~/lib/execution-mode";
import { task as taskFixture } from "../mock-celeris/fixtures";
import { type MockCeleris, sendJson, sendProblem, startMockCeleris } from "../mock-celeris/server";

/**
 * celeris ADR-0072「Phase F6 実装時の決定」: 起票済みのタスクを後から分解の経路に入れる（タスク詳細の
 * 「実行の形」）。表示の判定（どのボタンを出すか）と、フォーム → `POST /tasks/{id}/execution/decompose` /
 * `POST /tasks/{id}/retry {execution}` の写し。
 */

const ID = "01BOARDTASK00000000000001";

const shadowHint: ExecutionGateDecision = {
  mode: "atomic",
  source: "hint",
  score: 4,
  threshold: 5,
  rule_id: "atomic/score",
  policy_version: "exec-gate/1",
  shadow: true,
};

function routed(over: Partial<Task> = {}, decision: ExecutionGateDecision | null = shadowHint): Task {
  return taskFixture({
    id: ID,
    kind: "execute",
    status: "ready",
    routing: {
      execution_hint: { mode: "compound", explicit: false },
      ...(decision ? { execution: decision } : {}),
    },
    ...over,
  });
}

function execution(over: Partial<ExecutionView> = {}): ExecutionView {
  return {
    gate: shadowHint,
    metrics: {
      continuations: 0,
      repairs: 0,
      replans: 0,
      runs: 0,
      work_units_done: 0,
      work_units_total: 0,
    } as unknown as ExecutionView["metrics"],
    ...over,
  };
}

describe("gateDecisionLine", () => {
  it("判定の出どころ（CoS のヒント + 規則表）と shadow を 1 行で出す", () => {
    expect(gateDecisionLine(shadowHint)).toBe("atomic（CoS のヒント + 規則表、shadow: 記録だけ）— atomic/score");
    expect(
      gateDecisionLine({ ...shadowHint, mode: "compound", source: "human", shadow: false, rule_id: "human/explicit" }),
    ).toBe("compound（人の明示）— human/explicit");
    expect(gateDecisionLine(null)).toBeNull();
  });
});

describe("executionModeControls", () => {
  it("ready の atomic 判定なら compound への切り替えと atomic の固定を出す", () => {
    const c = executionModeControls(routed(), false);
    expect(c.actions).toEqual(["compound", "atomic"]);
    expect(c.regatePending).toBe(false);
  });

  it("人が compound を明示した直後（判定がまだ無い）は atomic に戻すだけを出し、再判定待ちを示す", () => {
    const t = routed({ routing: { execution_hint: { mode: "compound", explicit: true } } }, null);
    const c = executionModeControls(t, false);
    expect(c.actions).toEqual(["atomic"]);
    expect(c.regatePending).toBe(true);
  });

  it("計画があれば replan の依頼だけ（atomic には戻せない）", () => {
    const c = executionModeControls(routed(), true);
    expect(c.actions).toEqual(["replan"]);
    expect(c.note).toContain("atomic には戻せません");
  });

  it("failed / cancelled は「計画を作らせてやり直す」、running は操作を出さず理由を出す", () => {
    expect(executionModeControls(routed({ status: "failed" }), false).actions).toEqual(["retry_compound"]);
    expect(executionModeControls(routed({ status: "cancelled" }), false).actions).toEqual(["retry_compound"]);
    const running = executionModeControls(routed({ status: "running" }), false);
    expect(running.actions).toEqual([]);
    expect(running.note).toContain("走っている run は止めません");
    expect(executionModeControls(routed({ status: "done" }), false).actions).toEqual([]);
  });

  it("gate の対象外（routing の無い旧タスク・approval）は何も出さない", () => {
    expect(executionModeControls(taskFixture({ kind: "execute", routing: undefined }), false).actions).toEqual([]);
    expect(executionModeControls(routed({ kind: "approval" }), false).actions).toEqual([]);
  });
});

function render(t: Task, e: ExecutionView | null): string {
  const router = createMemoryRouter(
    [{ path: "/", element: createElement(ExecutionModeControl, { task: t, execution: e }) }],
    { initialEntries: ["/"] },
  );
  return renderToStaticMarkup(createElement(RouterProvider, { router }));
}

describe("ExecutionModeControl", () => {
  it("判定の出どころとボタン（compound / atomic）を出し、decompose の intent で送る", () => {
    const html = render(routed(), execution());
    expect(html).toContain('data-testid="execution-mode-decision"');
    expect(html).toContain("CoS のヒント + 規則表");
    expect(html).toContain("CoS のヒント: compound（+2 点）");
    expect(html).toContain('name="intent" value="execution_decompose"');
    expect(html).toContain('data-testid="execution-mode-compound"');
    expect(html).toContain("計画を作らせる（compound に切り替え）");
    expect(html).toContain('data-testid="execution-mode-atomic"');
    expect(html).not.toContain('data-testid="execution-mode-retry_compound"');
  });

  it("failed のタスクには retry + execution=compound のフォームを出す", () => {
    const html = render(routed({ status: "failed" }), execution());
    expect(html).toContain('data-testid="execution-mode-retry-form"');
    expect(html).toContain('name="execution" value="compound"');
    expect(html).toContain('name="intent" value="retry"');
    expect(html).not.toContain('data-testid="execution-mode-form"');
  });

  it("gate の対象外のタスクでは節ごと出さない", () => {
    expect(render(taskFixture({ kind: "approval", routing: undefined }), null)).toBe("");
  });
});

let mock: MockCeleris;
let client: CelerisClient;

beforeEach(async () => {
  mock = await startMockCeleris();
  client = new CelerisClient({ baseUrl: mock.baseUrl });
});

afterEach(async () => {
  await mock.close();
});

function form(entries: Record<string, string>): FormData {
  const f = new FormData();
  for (const [k, v] of Object.entries(entries)) f.append(k, v);
  return f;
}

describe("decomposeTask（POST /tasks/{id}/execution/decompose）", () => {
  it("mode と note をそのまま写し、結果を返す（空の note は送らない）", async () => {
    const result: DecomposeResult = { mode: "compound", replan: false, task: routed() };
    const bodies: unknown[] = [];
    mock.on("POST", `/api/v1/tasks/${ID}/execution/decompose`, (_req, res, body) => {
      bodies.push(JSON.parse(body));
      sendJson(res, 200, result);
    });
    const ok = await decomposeTask(
      client,
      ID,
      form({ intent: "execution_decompose", mode: "compound", note: "分けて" }),
    );
    expect(ok).toEqual({ ok: true, op: "execution_decompose", taskId: ID, result });
    await decomposeTask(client, ID, form({ mode: "atomic", note: "  " }));
    expect(bodies).toEqual([{ mode: "compound", note: "分けて" }, { mode: "atomic" }]);
  });

  it("celeris の 409（走っている run）をそのまま返し、知らない mode は送らない", async () => {
    mock.on("POST", `/api/v1/tasks/${ID}/execution/decompose`, (_req, res) => {
      sendProblem(res, {
        status: 409,
        code: "invalid_transition",
        detail: "cannot be re-gated while a run is in flight",
      });
    });
    const conflict = await decomposeTask(client, ID, form({ mode: "compound" }));
    expect(conflict.ok).toBe(false);
    if (conflict.ok) throw new Error("expected failure");
    expect(conflict.error.status).toBe(409);
    expect(conflict.error.detail).toContain("in flight");

    const bad = await decomposeTask(client, ID, form({ mode: "split" }));
    expect(bad.ok).toBe(false);
    if (bad.ok) throw new Error("expected failure");
    expect(bad.error.status).toBe(400);
  });
});

describe("applyRetry の execution（計画を作らせてやり直す）", () => {
  it("execution=compound を本文に足す", async () => {
    mock.on("POST", `/api/v1/tasks/${ID}/retry`, (_req, res, body) => {
      expect(JSON.parse(body)).toEqual({ accept: true, execution: "compound" });
      sendJson(res, 201, { task_id: "T2", rewired: [] });
    });
    const outcome = await applyRetry(client, ID, form({ intent: "retry", execution: "compound" }));
    expect(outcome).toEqual({ ok: true, taskId: ID, result: { task_id: "T2", rewired: [] } });
  });
});
