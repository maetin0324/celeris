import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ScreenFrame } from "./screen-frame";

describe("ScreenFrame", () => {
  it("既存の呼び出し（title・route）は h1・data-screen・準備中の枠だけを出す", () => {
    const html = renderToStaticMarkup(<ScreenFrame title="タスク" route="/tasks" />);
    expect(html).toContain('data-screen="/tasks"');
    expect(html).toMatch(/<h1 tabindex="-1"[^>]*>タスク<\/h1>/);
    expect(html).toContain("準備中");
    expect(html).not.toContain("パンくず");
    expect(html).not.toContain("page-actions");
    expect(html).not.toContain("page-description");
  });

  it("breadcrumb・description・actions を見出しの前後に出す", () => {
    const html = renderToStaticMarkup(
      <ScreenFrame
        title="詳細"
        route="/x"
        breadcrumb={[{ label: "一覧" }, { label: "詳細" }]}
        description="補足の文"
        actions={<button type="button">実行</button>}
      >
        <p>本文</p>
      </ScreenFrame>,
    );
    expect(html).toContain('<nav aria-label="パンくず"');
    expect(html).toContain('aria-current="page"');
    expect(html.indexOf("パンくず")).toBeLessThan(html.indexOf("<h1"));
    expect(html.indexOf("<h1")).toBeLessThan(html.indexOf("補足の文"));
    expect(html.indexOf("補足の文")).toBeLessThan(html.indexOf("実行"));
    expect(html).toContain('data-slot="page-actions"');
    expect(html).not.toContain("準備中");
  });
});

describe("ScreenFrame の互換", () => {
  it("slot を使わないときは h1 と中身が兄弟（従来の DOM）", () => {
    const html = renderToStaticMarkup(
      <ScreenFrame title="ホーム" route="/">
        <p>本文</p>
      </ScreenFrame>,
    );
    expect(html).toMatch(/^<div[^>]*data-screen="\/"><h1 [^>]*>ホーム<\/h1><p>本文<\/p><\/div>$/);
    expect(html).not.toContain("page-header");
  });
});
