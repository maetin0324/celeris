import { describe, expect, it } from "vitest";
import { ApiError } from "../../api/client";
import type { BrowserWait, InboxItem } from "../../api/generated/types";
import {
  answerFailure,
  browserWaitBadge,
  isDestructive,
  isInboxKind,
  nativeTarget,
  needsNativeScreen,
} from "./inbox-model";

const item = (extra: Partial<InboxItem>): InboxItem => ({
  id: "x",
  kind: "decision",
  title: "t",
  options: [{ key: "a", label: "A", effect: "", needs_note: false }],
  blocking: { tasks: [], units: [], summary: "" },
  blocked_by: [],
  answer: { method: "POST", path: "/api/v1/inbox/items/x/answer", body_schema: {} },
  created_at: "2026-10-01T00:00:00Z",
  age_secs: 0,
  links: [],
  ...extra,
});
const problem = (status: number, body: unknown) =>
  new ApiError(status === 409 ? "conflict" : status === 404 ? "not_found" : "validation", {
    method: "POST",
    path: "/api/inbox/items/x/answer",
    status,
    body,
  });

describe("inbox model", () => {
  it("破壊的な選択を見分ける", () => {
    for (const key of ["withdraw", "cancel", "reject", "deny"])
      expect(isDestructive({ key, label: key, effect: "", needs_note: false })).toBe(true);
    expect(isDestructive({ key: "approve", label: "", effect: "", needs_note: false })).toBe(false);
  });

  it("search param の種類を検証する", () => {
    expect(isInboxKind("authorization")).toBe(true);
    expect(isInboxKind("nope")).toBe(false);
    // 人の決定 2026-10-08: KB の取り込み待ちは受信箱の種類に無い（知識画面で扱う）。
    expect(isInboxKind("knowledge_review")).toBe(false);
  });

  it("409 native_action_required は専用画面への誘導にする", () => {
    expect(answerFailure(problem(409, { code: "native_action_required" }))).toMatchObject({ native: true });
    expect(answerFailure(problem(409, { code: "conflict" }))).toMatchObject({ stale: true });
    expect(answerFailure(problem(404, { code: "not_found" }))).toMatchObject({ stale: true });
    expect(answerFailure(problem(422, { error: "note_required" }))).toMatchObject({ field: "note" });
    expect(answerFailure(problem(422, { detail: "だめ" })).message).toBe("だめ");
  });

  it("専用画面の行き先は links → 種類 → task の順", () => {
    expect(nativeTarget(item({ links: [{ label: "L", href: "/knowledge/inbox" }] })).href).toBe("/knowledge/inbox");
    expect(nativeTarget(item({ links: [{ label: "外", href: "//evil" }], kind: "cluster_login" })).href).toBe(
      "/clusters",
    );
    const task = { id: "T 1", title: "x", kind: "task", status: "failed", actions: [] } as unknown as InboxItem["task"];
    expect(nativeTarget(item({ kind: "integration_request", task })).href).toBe("/tasks/T%201");
    expect(needsNativeScreen(item({ kind: "cluster_login" }))).toBe(true);
    expect(needsNativeScreen(item({ options: [] }))).toBe(true);
    expect(needsNativeScreen(item({}))).toBe(false);
  });

  it("browser_wait の badge と run 導線は待ち ID を照合し、無ければ task の待ちへ戻す", () => {
    const task = {
      id: "T1",
      title: "請求書",
      kind: "execute",
      status: "running",
      actions: [],
    } as unknown as InboxItem["task"];
    const approval = item({
      id: "browser_wait:W1",
      kind: "browser_wait",
      title: "承認待ち",
      task,
      links: [{ label: "古い", href: "/tasks/T1" }],
    });
    const credential = item({ id: "browser_wait:W2", kind: "browser_wait", title: "credential 待ち", task });
    const wait = { wait_id: "W1", task_id: "T1", run_id: "R1", state: "pending" } as BrowserWait;
    expect(browserWaitBadge(approval)).toBe("ブラウザの承認待ち");
    expect(browserWaitBadge(credential)).toBe("credential 待ち");
    expect(nativeTarget(approval, [wait]).href).toBe("/browser/runs/T1/R1#browser-waits");
    expect(nativeTarget(credential, [wait]).href).toBe("/tasks/T1#browser-waits");
    expect(nativeTarget(approval, [{ ...wait, task_id: "T2" }]).href).toBe("/tasks/T1#browser-waits");
  });
});
