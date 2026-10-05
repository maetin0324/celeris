import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Notice, noticeRole } from "./notice";

describe("Notice", () => {
  it("danger は role=alert、warning（既定）は role=status", () => {
    expect(noticeRole).toEqual({ danger: "alert", warning: "status" });
    const danger = renderToStaticMarkup(<Notice tone="danger">本文</Notice>);
    expect(danger).toMatch(/^<div role="alert" data-slot="notice" data-tone="danger"/);
    expect(danger).toContain("bg-danger");
    expect(danger).toContain("text-danger-foreground");
    const warning = renderToStaticMarkup(<Notice>本文</Notice>);
    expect(warning).toMatch(/^<div role="status" data-slot="notice" data-tone="warning"/);
    expect(warning).toContain("bg-warning");
  });

  it("見出し・本文・操作を順に出し、icon は装飾", () => {
    const html = renderToStaticMarkup(
      <Notice tone="danger" title="保存できませんでした" action={<button type="button">再試行</button>}>
        接続を確かめてください
      </Notice>,
    );
    const order = ['aria-hidden="true"', "保存できませんでした", "接続を確かめてください", "再試行"].map((s) =>
      html.indexOf(s),
    );
    expect(order.every((i) => i >= 0)).toBe(true);
    expect([...order].sort((a, b) => a - b)).toEqual(order);
    expect(html).toContain("font-semibold");
  });

  it("操作・見出しが無いときは枠を出さず、className と属性を渡せる", () => {
    const html = renderToStaticMarkup(
      <Notice className="mt-2" data-testid="n">
        本文だけ
      </Notice>,
    );
    expect(html).not.toContain("font-semibold");
    expect(html).not.toContain("shrink-0 flex-wrap");
    expect(html).toContain("mt-2");
    expect(html).toContain('data-testid="n"');
  });
});
