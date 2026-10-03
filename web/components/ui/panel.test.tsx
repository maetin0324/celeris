import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Panel, Section } from "./panel";

const html = (el: ReactElement) => renderToStaticMarkup(el);

describe("Panel", () => {
  it("section の aria-label と h2 の見出しを保つ", () => {
    const out = html(<Panel title="実行">本文</Panel>);
    expect(out).toMatch(/^<section aria-label="実行"/);
    expect(out).toContain('<h2 class="text-section font-semibold text-foreground">実行</h2>');
    expect(out).toContain(">本文</div>");
  });

  it("children が無ければ本文の枠を出さない", () => {
    const out = html(<Panel title="空" />);
    expect(out).not.toContain("<div");
  });
});

describe("Section", () => {
  it("既定は h2 で、見出しを accessible name にする", () => {
    const out = html(<Section title="判断待ち">本文</Section>);
    const id = out.match(/aria-labelledby="([^"]+)"/)?.[1];
    expect(id).toBeTruthy();
    expect(out).toContain(`<h2 id="${id}"`);
    expect(out).not.toContain("aria-describedby");
  });

  it("level で h3・h4 を選べる", () => {
    expect(html(<Section title="a" level={3} />)).toContain("<h3 ");
    expect(html(<Section title="a" level={4} />)).toContain("<h4 ");
  });

  it("description を aria-describedby で結ぶ", () => {
    const out = html(<Section title="a" description="説明文" />);
    const id = out.match(/aria-describedby="([^"]+)"/)?.[1];
    expect(out).toContain(`<p id="${id}"`);
    expect(out).toContain("説明文</p>");
  });

  it("actions は見出しの後ろに置き、狭い幅では縦に折り返す", () => {
    const out = html(<Section title="a" actions={<button type="button">再取得</button>} />);
    expect(out).toContain('data-slot="section-actions"');
    expect(out.indexOf("</h2>")).toBeLessThan(out.indexOf("再取得"));
    expect(out).toContain("flex flex-col gap-2 md:flex-row");
  });
});
