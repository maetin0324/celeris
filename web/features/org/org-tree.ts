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
