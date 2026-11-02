import { describe, expect, it } from "vitest";
import "../main";
import {
  type CosDigestInput,
  type CosTriageDecision,
  type CosTriageItemReport,
  cosDigest,
  decide,
  parseInput,
  rustTrim,
} from "./cos_digest";

const ctx = { token: "01M6EVP9GT07K2G01EMW0E6HNC" };

function item(over: Partial<CosTriageItemReport> = {}): CosTriageItemReport {
  return {
    item_id: "item-0",
    thread_id: "thread-1",
    source_kind: "decision",
    source_key: "decision:01TASK",
    source_revision: "1",
    state: "answered",
    summary: "件 0",
    detail: "",
    reason: "理由 0",
    operation_id: null,
    run_id: "run-1",
    created_at: "2026-11-02T00:00:00Z",
    route: null,
    decision: null,
    ...over,
  };
}

function input(over: Partial<CosDigestInput> = {}): CosDigestInput {
  return { thread: "thread-1", run_id: "run-1", run_state: "completed", reason: null, items: [item()], ...over };
}

const packet = (over: Partial<CosTriageDecision> = {}): CosTriageDecision => ({
  summary: "どちらで進めるか",
  background: null,
  on_answer: null,
  options: [
    { key: "a", label: "A 案", detail: null },
    { key: "b", label: "B 案", detail: null },
  ],
  recommended: "a",
  recommendation_reason: null,
  web_path: "/tasks/01TASK/decisions",
  ...over,
});

function post(i: CosDigestInput) {
  const first = decide(i).commands[0];
  if (first?.kind !== "post_digest") throw new Error("no post_digest");
  return first;
}

describe("cos_digest run_state", () => {
  it("completed: one post_digest, no requeue, plain header", () => {
    const out = decide(input());
    expect(out.commands.map((c) => c.kind)).toEqual(["post_digest"]);
    const p = post(input());
    expect(p.idempotency_key).toBe("cos-triage-digest:run-1");
    expect(p.text.startsWith("受信箱の一次対応の結果（1 件)\n")).toBe(true);
    expect(p.text.endsWith("\n(run run-1)\n")).toBe(true);
  });

  it("failed: header carries the reason, no requeue", () => {
    const i = input({ run_state: "failed", reason: "timeout" });
    expect(decide(i).commands.map((c) => c.kind)).toEqual(["post_digest"]);
    expect(post(i).text.startsWith("受信箱の一次対応の結果（1 件。run は失敗: timeout)\n")).toBe(true);
  });

  it("failed without a reason (null or blank) leaves the reason out", () => {
    for (const reason of [null, "", " \u0085 "]) {
      expect(post(input({ run_state: "failed", reason })).text.split("\n")[0]).toBe("受信箱の一次対応の結果（1 件。run は失敗)");
    }
  });

  it.each(["stopped", "interrupted"])("%s: requeue_run after post_digest, running items go back to pending", (state) => {
    const i = input({ run_state: state, items: [item({ state: "running" })] });
    const out = decide(i);
    expect(out.commands.map((c) => c.kind)).toEqual(["post_digest", "requeue_run"]);
    expect(out.commands[1]).toEqual({ kind: "requeue_run", run_id: "run-1" });
    const p = post(i);
    expect(p.text).toContain(`run は ${state} で終わった)`);
    expect(p.text).toContain("- 判断: 中断されたので次の run で扱う\n");
    expect(p.cards[0]?.state).toBe("pending");
  });

  it("an unknown run_state is reported as is and does not requeue (same as Rust)", () => {
    const i = input({ run_state: "cancelled", items: [item({ state: "running" })] });
    expect(decide(i).commands.map((c) => c.kind)).toEqual(["post_digest"]);
    expect(post(i).text).toContain("run は cancelled で終わった)");
    expect(post(i).cards[0]?.state).toBe("running");
  });

  it("an empty run still posts one digest", () => {
    expect(post(input({ items: [] })).text).toBe("受信箱の一次対応の結果（0 件)\n\n(run run-1)\n");
  });
});

describe("cos_digest item state", () => {
  const cases: [string, string, string][] = [
    ["answered", "代わりに答えた", "answered"],
    ["observed", "見ただけ（判断は不要）", "observed"],
    ["escalated", "人に回した", "escalated"],
    ["fallback", "未処理のまま人へ直接通知した（CoS 不在の退避）", "pending"],
    ["resolved", "元の待ちが閉じていた（何もしていない）", "resolved"],
    ["pending", "中断されたので次の run で扱う", "pending"],
    ["running", "未処理（人へ直接通知する）", "running"],
    ["handled", "状態不明", "handled"],
  ];
  it.each(cases)("%s → judgment and card state", (state, line, cardState) => {
    const p = post(input({ items: [item({ state })] }));
    expect(p.text).toContain(`- 判断: ${line}\n`);
    expect(p.cards[0]?.state).toBe(cardState);
  });

  it("maps source kinds to labels and card kinds", () => {
    const kinds = ["decision", "question", "authorization", "acceptance_check", "plan_gate", "phase_gate", "notice", "task"];
    const p = post(input({ items: kinds.map((k, n) => item({ source_kind: k, source_key: `${k}:${n}` })) }));
    expect(p.cards.map((c) => c.kind)).toEqual(["decision", "question", "approval", "approval", "plan_gate", "plan_gate", "notice", "task"]);
    expect(p.text).toContain("（受け入れ検査）");
    expect(p.text).toContain("（項目）");
  });
});

describe("cos_digest reason and title", () => {
  it.each([null, "", "   "])("no reason (%j) → 理由の記録なし", (reason) => {
    const p = post(input({ items: [item({ reason })] }));
    expect(p.text).toContain("- 理由: 理由の記録なし\n");
    expect(p.cards[0]?.reason).toBe(reason);
  });

  it("the reason line is trimmed like Rust str::trim; the card keeps the raw reason", () => {
    const p = post(input({ items: [item({ reason: "　 理由 \n" })] }));
    expect(p.text).toContain("- 理由: 理由\n");
    expect(p.cards[0]?.reason).toBe("　 理由 \n");
  });

  it("rustTrim keeps U+FEFF and strips U+0085 (unlike String.prototype.trim)", () => {
    expect(rustTrim("﻿a\u0085")).toBe("﻿a");
  });

  it("a blank summary falls back to source_kind:source_key", () => {
    const p = post(input({ items: [item({ summary: "  " })] }));
    expect(p.cards[0]?.title).toBe("decision:decision:01TASK");
    expect(p.text).toContain("### decision:decision:01TASK（決定）");
  });
});

describe("cos_digest decision packet", () => {
  it("escalated with options: recommended mark, href and card reason from the packet", () => {
    const d = packet({ background: "背景", on_answer: "再開する", recommendation_reason: "戻しやすい" });
    const p = post(input({ items: [item({ state: "escalated", route: "escalation", decision: d })] }));
    expect(p.text).toContain(
      "- 背景: 背景\n- 人が決めること: どちらで進めるか\n- 選択肢: A 案（推奨） / B 案\n- 推奨の理由: 戻しやすい\n- 答えると: 再開する\n",
    );
    expect(p.cards[0]).toMatchObject({ href: "/tasks/01TASK/decisions", reason: "戻しやすい", state: "escalated" });
  });

  it("an option with a detail puts every option on its own line", () => {
    const d = packet({
      options: [
        { key: "a", label: "A 案", detail: "A の意味" },
        { key: "b", label: "B 案", detail: null },
      ],
    });
    const p = post(input({ items: [item({ state: "fallback", route: "fallback", decision: d })] }));
    expect(p.text).toContain("- 選択肢: \n  - A 案（推奨）: A の意味\n  - B 案\n");
    expect(p.cards[0]?.state).toBe("pending");
  });

  it("no options and empty strings print nothing extra and keep /inbox", () => {
    const d = packet({ summary: "", options: [], background: "", web_path: "" });
    const p = post(input({ items: [item({ state: "escalated", decision: d })] }));
    expect(p.text).not.toContain("選択肢");
    expect(p.text).not.toContain("人が決めること");
    expect(p.cards[0]?.href).toBe("/inbox");
  });

  it("a packet on an item that is not escalated or fallback is ignored", () => {
    const p = post(input({ items: [item({ state: "running", decision: packet() })] }));
    expect(p.text).not.toContain("選択肢");
    expect(p.cards[0]?.href).toBe("/inbox");
  });
});

describe("cos_digest input checks", () => {
  it("an unknown field is rejected", async () => {
    await expect(cosDigest({ ...input(), policy_version: 7 })).rejects.toThrow("unknown field in input: policy_version");
  });

  it("a missing field is rejected (Option fields must be explicit null)", () => {
    const { reason: _reason, ...rest } = item();
    expect(() => parseInput(input({ items: [rest as CosTriageItemReport] }))).toThrow(
      "missing field in input.items[0]: reason",
    );
  });

  it("an extra field inside a nested option is rejected", () => {
    const d = { ...packet(), options: [{ key: "a", label: "A", detail: null, extra: true }] };
    expect(() => parseInput(input({ items: [item({ decision: d as CosTriageDecision })] }))).toThrow(
      "unknown field in input.items[0].decision.options[0]: extra",
    );
  });

  it("an unknown route value is rejected", () => {
    expect(() =>
      parseInput(input({ items: [item({ route: "email" as CosTriageItemReport["route"] })] })),
    ).toThrow("unknown value of input.items[0].route: email");
  });

  it("a wrong type is rejected", () => {
    expect(() => parseInput({ ...input(), run_state: 3 })).toThrow("input.run_state must be a string");
  });
});

describe("cos_digest dispatch", () => {
  it("is reachable through __celerisDispatch and returns { commands }", async () => {
    const out = await globalThis.__celerisDispatch("cos_digest", input({ run_state: "stopped" }), ctx);
    expect(Object.keys(out as object)).toEqual(["commands"]);
  });

  it("serialises commands in the Rust field order", () => {
    const json = JSON.stringify(decide(input({ run_state: "interrupted" })).commands);
    expect(json.startsWith('[{"kind":"post_digest","thread":"thread-1","idempotency_key":')).toBe(true);
    expect(json).toContain('"actor":"cos","reason":"理由 0","operation_id":null}],"run_id":"run-1"}');
    expect(json.endsWith(',{"kind":"requeue_run","run_id":"run-1"}]')).toBe(true);
  });
});
