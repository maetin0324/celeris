import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { DataList, DataListRow, DataListTerm, DataListValue } from "./data-list";

const html = (el: ReactElement) => renderToStaticMarkup(el);

describe("DataList", () => {
  it("items から dl > div > dt/dd を組み立てる", () => {
    const out = html(
      <DataList
        items={[
          { label: "担当", value: "UI/UX" },
          { key: "state", label: "状態", value: "実行中" },
        ]}
      />,
    );
    expect(out.startsWith("<dl ")).toBe(true);
    expect(out.match(/<div data-slot="data-list-row"/g)).toHaveLength(2);
    expect(out.match(/<dt /g)).toHaveLength(2);
    expect(out.match(/<dd /g)).toHaveLength(2);
    expect(out.indexOf("担当</dt>")).toBeLessThan(out.indexOf("UI/UX</dd>"));
    expect(out.indexOf("UI/UX</dd>")).toBeLessThan(out.indexOf("状態</dt>"));
  });

  it("狭い幅では縦積み、md 以上で 2 列", () => {
    const out = html(<DataList items={[{ label: "a", value: "b" }]} />);
    expect(out).toContain("flex-col");
    expect(out).toContain("md:flex-row");
    expect(out).toContain("md:w-1/3");
  });

  it("children の dt/dd をそのまま置ける", () => {
    const out = html(
      <DataList aria-label="詳細">
        <DataListRow>
          <DataListTerm>ブランチ</DataListTerm>
          <DataListValue>main</DataListValue>
        </DataListRow>
      </DataList>,
    );
    expect(out).toContain('aria-label="詳細"');
    expect(out).toContain("ブランチ</dt>");
    expect(out).toContain("main</dd>");
  });
});
