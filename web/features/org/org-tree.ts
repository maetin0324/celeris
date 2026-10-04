import type { OrgNode } from "../../api/generated/types";

export type TreeNode = { node: OrgNode; children: TreeNode[] };

/** API は平らな position 順の配列。親のないノードと孤児を根に残す。 */
export function buildOrgTree(items: readonly OrgNode[]): TreeNode[] {
  const entries = new Map(items.map((node) => [node.id, { node, children: [] as TreeNode[] }]));
  const roots: TreeNode[] = [];
  for (const node of items) {
    const entry = entries.get(node.id);
    if (!entry) continue;
    const parent = node.parent_id && node.parent_id !== node.id ? entries.get(node.parent_id) : undefined;
    if (parent) parent.children.push(entry);
    else roots.push(entry);
  }
  const sort = (nodes: TreeNode[]) => {
    nodes.sort((a, b) => (a.node.position ?? 0) - (b.node.position ?? 0) || a.node.name.localeCompare(b.node.name));
    nodes.forEach((node) => {
      sort(node.children);
    });
  };
  sort(roots);
  return roots;
}

/** 木の深さを先に数えた平らな行（一覧の表示順）。 */
export type TreeRow = { node: OrgNode; depth: number; childCount: number };

export function flattenOrgTree(nodes: readonly TreeNode[], depth = 0): TreeRow[] {
  return nodes.flatMap(({ node, children }) => [
    { node, depth, childCount: children.length },
    ...flattenOrgTree(children, depth + 1),
  ]);
}

export type OrgSettingState = "own" | "inherited" | "default";

/**
 * 担当の設定の状態。自分の profile に値が 1 つでもあれば own、無ければ親から継承（根なら default）。
 * API は担当の稼働状態を持たないので、一覧の「状態」は設定の出どころを示す。
 */
export function orgSettingState(node: OrgNode): OrgSettingState {
  const profile = node.profile ?? {};
  const hasValue = Object.values(profile).some((value) => {
    if (value === null || value === undefined) return false;
    if (Array.isArray(value)) return value.length > 0;
    if (typeof value === "object") return Object.keys(value).length > 0;
    return true;
  });
  if (hasValue) return "own";
  return node.parent_id ? "inherited" : "default";
}
