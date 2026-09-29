import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * celeris ADR-0079 D6 / §7 R4b (d): GUI に「配送」の語を残さない（「成果の取り込み」「main に取り込み済み」に改めた）。
 * 対象は `app/` の全ソース（`help.tsx` を含む）と、画面に出る偽の celeris の fixture。`app/celeris/types.ts` は
 * celeris の schema（Rust の doc コメント）から生成した型の注釈で画面には出ないので除く（API の欄名
 * `delivered_release` 等も互換のため変えない。D6）。
 */
const GUI_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const GENERATED = path.join(GUI_DIR, "app", "celeris", "types.ts");

function sources(dir: string): string[] {
  const out: string[] = [];
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...sources(p));
    else if (/\.(tsx?|mjs|json)$/.test(entry.name) && p !== GENERATED) out.push(p);
  }
  return out;
}

describe("語: 配送 → 成果の取り込み（ADR-0079 D6）", () => {
  it("app/ と画面用の fixture に「配送」が無い", () => {
    const files = [
      ...sources(path.join(GUI_DIR, "app")),
      path.join(GUI_DIR, "scripts", "lib", "celeris-fixture.mjs"),
      ...sources(path.join(GUI_DIR, "test", "fixtures", "api")),
    ];
    expect(files.some((f) => f.endsWith(path.join("routes", "help.tsx")))).toBe(true);
    const hits = files.filter((f) => fs.readFileSync(f, "utf8").includes("配送")).map((f) => path.relative(GUI_DIR, f));
    expect(hits).toEqual([]);
  });
});
