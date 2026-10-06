import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { TaskRoutingView } from "~/celeris/types";
import { TaskRoutingPanel } from "~/components/TaskRoutingPanel";
import routingShadowFixture from "../fixtures/api/routing-shadow.json";

const view = routingShadowFixture as unknown as TaskRoutingView;

describe("routing_shadow の監査表示", () => {
  it("routing_shadow を primary の review・model と別欄に表示する", () => {
    const html = renderToStaticMarkup(<TaskRoutingPanel view={view} />);
    const primary = html.indexOf('data-testid="task-routing-review"');
    const shadow = html.indexOf('data-testid="task-routing-shadow"');
    expect(primary).toBeGreaterThan(-1);
    expect(shadow).toBeGreaterThan(primary);
    expect(html).toContain("primary-model");
    expect(html).toContain("candidate-model");
    expect(html).toContain("判断のみ · 完了");
    expect(html).toContain("実行 · 失敗");
    expect(html).toContain("実行 · 見送り");
    expect(html).toContain("異なる");
    expect(html).toContain("同じ");
    expect(html).toContain("timeout");
    expect(html).toContain("cap_exceeded");
    expect(html).toContain("UTC 2026-10-05");
    expect(html).toContain("予約 500 tokens / $0.05 effective");
    expect(html).toContain("確定 500 tokens / $0.05 effective");
  });

  it("routing_shadow の欠測は不明とし、記録のない旧 run には shadow 欄を出さない", () => {
    const html = renderToStaticMarkup(<TaskRoutingPanel view={view} />);
    expect(html).toContain("source 不明 / model 不明");
    expect(html).toContain("入力 不明 / 出力 不明");
    expect(html).toContain("primary との差</dt><dd>不明");
    const oldView: TaskRoutingView = { ...view, runs: [{ ...view.runs[0], routing_shadow: undefined }] };
    expect(renderToStaticMarkup(<TaskRoutingPanel view={oldView} />)).not.toContain(
      'data-testid="task-routing-shadow"',
    );
  });
});
