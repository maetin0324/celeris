import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TaskSummary } from "../../api/generated/types";
import { boardGroup } from "./board-model";
import { BoardTruncatedNotice } from "./board-screen";

const done = (n: number) =>
  Array.from(
    { length: n },
    (_, i) => ({ id: `d${i}`, status: "done", priority: 10, created_at: `${i}` }) as TaskSummary,
  );

describe("終端の列を切ったときの表示", () => {
  it("切った列の名前と表示範囲を出す", () => {
    const groups = boardGroup(done(200), { done: { loaded: 200, total: 950 }, failed: { loaded: 3, total: 3 } });
    const html = renderToStaticMarkup(<BoardTruncatedNotice groups={groups} showSupport={false} />);
    expect(html).toContain("完了は作成の新しい 200");
    expect(html).toContain("（裏方を含む）");
    expect(html).not.toContain("失敗");
  });
  it("読み切ったときは何も出さない", () => {
    const groups = boardGroup(done(3), { done: { loaded: 3, total: 3 } });
    expect(renderToStaticMarkup(<BoardTruncatedNotice groups={groups} showSupport />)).toBe("");
  });
});
