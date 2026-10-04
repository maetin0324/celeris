import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { copyText, ShortId, shortId } from "./short-id";

const ulid = "01M44C5GXRV021GW7QDHTNC054";

describe("shortId", () => {
  it("長い値だけ先頭を残して「…」を付ける", () => {
    expect(shortId(ulid)).toBe("01M44C5G…");
    expect(shortId(ulid, 12)).toBe("01M44C5GXRV0…");
    expect(shortId("abc")).toBe("abc");
    expect(shortId("12345678")).toBe("12345678");
  });
});

describe("copyText", () => {
  it("clipboard に書けたら true、無い・拒否なら false", async () => {
    const written: string[] = [];
    expect(await copyText("x", { writeText: async (t) => void written.push(t) })).toBe(true);
    expect(written).toEqual(["x"]);
    expect(await copyText("x", undefined)).toBe(false);
    expect(
      await copyText("x", {
        writeText: async () => {
          throw new Error("denied");
        },
      }),
    ).toBe(false);
  });
});

describe("ShortId", () => {
  it("等幅の省略表示、title と読み上げは全文、コピー操作は名前付きで 44px", () => {
    const html = renderToStaticMarkup(<ShortId value={ulid} label="タスク ID" />);
    expect(html).toContain(`title="${ulid}"`);
    expect(html).toContain("font-mono");
    expect(html).toContain("truncate");
    expect(html).toContain('<span aria-hidden="true">01M44C5G…</span>');
    expect(html).toContain(`<span class="sr-only">タスク ID ${ulid}</span>`);
    expect(html).toMatch(/<button type="button" aria-label="タスク IDをコピー"[^>]*min-h-11 min-w-11/);
    expect(html).toContain('role="status"');
  });

  it("copyable=false ではコピー操作を出さない", () => {
    const html = renderToStaticMarkup(<ShortId value={ulid} copyable={false} />);
    expect(html).not.toContain("<button");
    expect(html).toContain(`<span class="sr-only">ID ${ulid}</span>`);
  });
});
