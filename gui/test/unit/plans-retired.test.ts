import { existsSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * celeris ADR-0079 R5b-prep: `POST /plans` は R5a で 410（U-R6）。その画面 `/plans/new` と、ナビの「新規 Plan」・
 * 使い方ページのリンクを外した（新しい仕事は root task〈`/tasks/new`〉とその計画で表す）。ルート定義・ナビ・
 * 使い方・中継（`createPlan`）のどこにも `/plans/new` と `POST /plans` が残っていないことを確かめる
 * （コメントは除いて見る。撤去の経緯をコメントに残してよいように）。
 */

const APP = fileURLToPath(new URL("../../app/", import.meta.url));

function read(rel: string): string {
  return readFileSync(`${APP}${rel}`, "utf8");
}

/** `//` と `/* *\/` のコメントを落とす（文字列の中の `//`〈URL〉は `://` なので残す）。 */
function stripComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

describe("/plans/new の撤去（ADR-0079 R5b-prep）", () => {
  it("ルート定義に plans/new が無く、画面のモジュールも無い", () => {
    const routes = stripComments(read("routes.ts"));
    expect(routes).not.toContain("plans/new");
    expect(routes).not.toContain("plans.new");
    expect(existsSync(`${APP}routes/plans.new.tsx`)).toBe(false);
    expect(readdirSync(`${APP}routes`).some((f) => f.startsWith("plans."))).toBe(false);
  });

  it("ナビ（root.tsx）と使い方（help.tsx）に /plans/new へのリンクが無い", () => {
    for (const file of ["root.tsx", "routes/help.tsx"]) {
      const src = stripComments(read(file));
      expect(src, file).not.toContain("/plans/new");
      expect(src, file).not.toContain("新規 Plan");
    }
  });

  it("GUI の中継に POST /plans が無い", () => {
    const src = stripComments(read("celeris/route-actions.server.ts"));
    expect(src).not.toContain('"/plans"');
    expect(src).not.toContain("createPlan");
  });
});
