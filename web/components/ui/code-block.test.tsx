import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { CodeBlock, LogSurface } from "./code-block";

describe("CodeBlock", () => {
  it("token の色と等幅書体で、横溢れは内側でだけ scroll する名前付きの region", () => {
    const out = renderToStaticMarkup(<CodeBlock label="設定ファイル">{"a = 1"}</CodeBlock>);
    expect(out).toMatch(/^<section aria-label="設定ファイル" tabindex="0"/);
    for (const token of ["bg-code", "text-code-foreground", "font-mono", "overflow-x-auto", "min-w-0", "max-w-full"])
      expect(out).toContain(token);
    expect(out).toContain('<pre class="whitespace-pre"><code>a = 1</code></pre>');
    expect(out).toContain('data-wrap="false"');
    expect(out).toMatch(/(?:^|\s)text-label(?:\s|")/);
  });

  it("wrap で折り返し、原文の HTML は文字列のまま出す", () => {
    const out = renderToStaticMarkup(
      <CodeBlock label="出力" wrap>
        {"<script>x</script>"}
      </CodeBlock>,
    );
    expect(out).toContain("whitespace-pre-wrap");
    expect(out).toContain('data-wrap="true"');
    expect(out).toContain("&lt;script&gt;");
    expect(out).not.toContain("<script>");
  });

  it("className と属性を渡せる", () => {
    const out = renderToStaticMarkup(
      <CodeBlock label="x" className="mt-2" data-testid="code">
        x
      </CodeBlock>,
    );
    expect(out).toContain("mt-2");
    expect(out).toContain('data-testid="code"');
  });
});

describe("LogSurface", () => {
  it("既定は max-h-96 で縦横とも内側で scroll する", () => {
    const out = renderToStaticMarkup(<LogSurface label="stdout">line</LogSurface>);
    expect(out).toMatch(/^<section aria-label="stdout" tabindex="0"/);
    expect(out).toContain("overflow-auto");
    expect(out).toContain("max-h-96");
    expect(out).toContain("bg-code");
    expect(out).toContain('<pre class="whitespace-pre">line</pre>');
  });

  it("size で高さ上限を変え、wrap で折り返す", () => {
    const out = renderToStaticMarkup(
      <LogSurface label="stderr" size="sm" wrap>
        line
      </LogSurface>,
    );
    expect(out).toContain("max-h-64");
    expect(out).not.toContain("max-h-96");
    expect(out).toContain("whitespace-pre-wrap");
  });
});
