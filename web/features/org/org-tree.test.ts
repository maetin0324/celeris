import { describe, expect, it } from "vitest";
import type { OrgNode } from "../../api/generated/types";
import { buildOrgTree, flattenOrgTree, orgSettingState } from "./org-tree";

const at = "2026-10-04T00:00:00Z";
const node = (id: string, parent_id?: string, extra: Partial<OrgNode> = {}): OrgNode => ({
  id,
  name: id,
  kind: parent_id ? "section" : "secretary",
  parent_id,
  created_at: at,
  updated_at: at,
  ...extra,
});

describe("flattenOrgTree", () => {
  it("親の直後に子を深さ付きで並べる", () => {
    const rows = flattenOrgTree(
      buildOrgTree([
        node("cos"),
        node("eng", "cos", { position: 1 }),
        node("ui", "eng"),
        node("ops", "cos", { position: 2 }),
      ]),
    );
    expect(rows.map((row) => [row.node.id, row.depth, row.childCount])).toEqual([
      ["cos", 0, 2],
      ["eng", 1, 1],
      ["ui", 2, 0],
      ["ops", 1, 0],
    ]);
  });
});

describe("orgSettingState", () => {
  it("自分の profile に値があれば own", () => {
    expect(orgSettingState(node("ui", "eng", { profile: { skills_mounts: ["review"] } }))).toBe("own");
  });
  it("空の profile は親があれば inherited、根なら default", () => {
    expect(orgSettingState(node("ui", "eng", { profile: { skills: [], run: null, model: {} } }))).toBe("inherited");
    expect(orgSettingState(node("cos"))).toBe("default");
  });
});
