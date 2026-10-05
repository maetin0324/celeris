import type { ReactElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { Table, TableBody, TableCaption, TableCell, TableHead, TableHeader, TableRow } from "./table";

const html = (el: ReactElement) => renderToStaticMarkup(el);

const sample = (props: { "aria-label"?: string; tabIndex?: number } = {}) => (
  <Table {...props}>
    <TableCaption>実行の一覧</TableCaption>
    <TableHeader>
      <TableRow>
        <TableHead>対象</TableHead>
        <TableHead scope="row">状態</TableHead>
      </TableRow>
    </TableHeader>
    <TableBody>
      <TableRow>
        <TableCell>layout</TableCell>
        <TableCell>実行中</TableCell>
      </TableRow>
    </TableBody>
  </Table>
);

describe("Table", () => {
  it("名前のある横スクロール枠で包み、キーボードで focus できる", () => {
    const out = html(sample({ "aria-label": "実行の表" }));
    expect(out).toMatch(/^<section data-slot="table-container" aria-label="実行の表" tabindex="0"/);
    expect(out).toContain("overflow-x-auto");
  });

  it("名前が無ければ landmark にせず focus 順にも入れない", () => {
    const out = html(sample());
    expect(out.startsWith("<div ")).toBe(true);
    expect(out).not.toContain("tabindex");
  });

  it("tabIndex を受ける", () => {
    expect(html(sample({ tabIndex: -1 }))).toContain('tabindex="-1"');
  });

  it("caption を table の最初の子に置く", () => {
    const out = html(sample());
    expect(out).toMatch(/<table [^>]*><caption [^>]*>実行の一覧<\/caption>/);
  });

  it("TableHead は既定で scope=col、指定で上書きできる", () => {
    const out = html(sample());
    expect(out).toMatch(/<th data-slot="table-head" scope="col"[^>]*>対象<\/th>/);
    expect(out).toMatch(/<th data-slot="table-head" scope="row"[^>]*>状態<\/th>/);
  });

  it("thead・tbody・td を持つ", () => {
    const out = html(sample());
    expect(out).toContain("<thead ");
    expect(out).toContain("<tbody ");
    expect(out).toMatch(/<td [^>]*>layout<\/td>/);
  });
});
