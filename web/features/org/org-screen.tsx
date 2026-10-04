import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { EffectiveProfile, OrgCreateBody, OrgKind, OrgList, OrgNode } from "../../api/generated/types";
import { orgKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button, buttonClassName } from "../../components/ui/button";
import { DataList, type DataListItem } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { cn } from "../../lib/utils";
import { Route } from "../../routes/org.index";
import { OrgSkills } from "./org-skills";
import { buildOrgTree, flattenOrgTree, type OrgSettingState, orgSettingState } from "./org-tree";

const kindLabel: Record<OrgKind, string> = { secretary: "CoS", department: "部", section: "課" };
// 入力欄の枠は --color-input（DESIGN.md「入力欄」）。
const fieldClass =
  "block min-h-11 w-full min-w-0 rounded-md border border-input bg-surface px-3 py-2 text-body text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const labelClass = "block space-y-1 text-label font-medium text-foreground";
// 木の字下げ。任意値を使わず段ごとの固定 class にし、4 段目以降は同じ幅に留めて 360px で溢れさせない。
const indentClass = ["pl-0", "pl-4", "pl-8", "pl-12"] as const;

const settingView: Record<OrgSettingState, { tone: BadgeTone; label: string }> = {
  own: { tone: "info", label: "独自設定" },
  inherited: { tone: "neutral", label: "継承" },
  default: { tone: "neutral", label: "既定" },
};

function SettingBadge({ node }: { node: OrgNode }) {
  const view = settingView[orgSettingState(node)];
  return <Badge tone={view.tone}>{view.label}</Badge>;
}

function mountCount(node: OrgNode, profiles: readonly EffectiveProfile[]) {
  const profile = profiles.find((item) => item.node_id === node.id);
  return (profile?.skills_mounts ?? node.profile?.skills_mounts ?? []).length;
}

function Tree({
  items,
  profiles,
  selected,
  onSelect,
}: {
  items: readonly OrgNode[];
  profiles: readonly EffectiveProfile[];
  selected?: string;
  onSelect: (id: string) => void;
}) {
  const rows = flattenOrgTree(buildOrgTree(items));
  return (
    <ul className="divide-y divide-border rounded-md border border-border">
      {rows.map(({ node, depth }) => {
        const current = selected === node.id;
        return (
          <li key={node.id} className={indentClass[Math.min(depth, indentClass.length - 1)]}>
            <Button
              variant="ghost"
              aria-current={current ? "page" : undefined}
              className={cn(
                "w-full flex-col items-stretch gap-1 rounded-none px-3 py-2 text-left",
                current && "bg-accent",
              )}
              onClick={() => onSelect(node.id)}
            >
              <span className={cn("break-words", current && "font-semibold")}>
                {kindLabel[node.kind]} · {node.name}
              </span>
              <span className="flex flex-wrap items-center gap-x-3 gap-y-1 text-label font-normal text-muted-foreground">
                <span className="break-all">{node.id}</span>
                <span>skill {mountCount(node, profiles)}</span>
                <SettingBadge node={node} />
              </span>
            </Button>
          </li>
        );
      })}
    </ul>
  );
}

const listOrNone = (values?: readonly string[] | null) => (values && values.length > 0 ? values.join(", ") : "なし");

/** 設定の現在値（継承を解いた実効値）。値の無い項目は「既定」「なし」と書き、空欄を作らない。 */
function settingItems(node: OrgNode, items: readonly OrgNode[], profile?: EffectiveProfile): DataListItem[] {
  const parent = items.find((item) => item.id === node.parent_id);
  return [
    { label: "種類", value: kindLabel[node.kind] },
    { label: "ID", value: <span className="break-all">{node.id}</span> },
    { label: "親", value: parent ? parent.name : "なし" },
    { label: "分野", value: node.genre || "なし" },
    { label: "状態", value: <SettingBadge node={node} /> },
    { label: "継承", value: profile?.chain?.length ? profile.chain.join(" → ") : "なし" },
    { label: "能力タグ", value: listOrNone(profile?.skills ?? node.profile?.skills) },
    { label: "mount する skill", value: listOrNone(profile?.skills_mounts ?? node.profile?.skills_mounts) },
    { label: "実行環境", value: profile?.run ?? "既定" },
    { label: "モデル階層", value: profile?.tier ?? "既定" },
    { label: "上限の段", value: profile?.max_lane ?? "既定" },
    { label: "既定の harness", value: profile?.harness_default ?? "既定" },
    { label: "使える harness", value: listOrNone(profile?.harnesses_allowed) },
    { label: "レビューの段", value: profile?.review_tier ?? "既定" },
    { label: "最大試行回数", value: profile?.max_attempts ?? "既定" },
    { label: "外部の道具", value: listOrNone(profile?.tools) },
    { label: "禁止の道具", value: listOrNone(profile?.deny_tools) },
  ];
}

function ChildrenTable({
  node,
  items,
  profiles,
}: {
  node: OrgNode;
  items: readonly OrgNode[];
  profiles: readonly EffectiveProfile[];
}) {
  const children = items.filter((item) => item.parent_id === node.id && item.id !== node.id);
  if (children.length === 0) return <p className="text-label text-muted-foreground">配下の担当はありません。</p>;
  return (
    <Table aria-label="配下の担当の一覧">
      <TableHeader>
        <tr>
          <TableHead>名前</TableHead>
          <TableHead>種類</TableHead>
          <TableHead>skill</TableHead>
          <TableHead>状態</TableHead>
        </tr>
      </TableHeader>
      <TableBody>
        {children.map((child) => (
          <TableRow key={child.id}>
            <TableCell className="break-words">{child.name}</TableCell>
            <TableCell>{kindLabel[child.kind]}</TableCell>
            <TableCell>{mountCount(child, profiles)}</TableCell>
            <TableCell>
              <SettingBadge node={child} />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function NodeDetail({ node, list }: { node: OrgNode; list: OrgList }) {
  const profiles = list.effective_profiles ?? [];
  const profile = profiles.find((item) => item.node_id === node.id);
  const policy = profile?.policy?.length ? profile.policy : node.profile?.policy;
  return (
    <div className="space-y-6">
      <Section
        level={3}
        title={node.name}
        description={node.brief || `${kindLabel[node.kind]} · ${node.id}`}
        actions={
          <a className={buttonClassName} href={`/org/${encodeURIComponent(node.id)}`}>
            話す
          </a>
        }
      >
        <DataList aria-label="設定の現在値" items={settingItems(node, list.items, profile)} />
      </Section>
      <Section level={4} title="方針">
        {policy?.length ? (
          <Markdown source={policy.join("\n\n")} />
        ) : (
          <p className="text-label text-muted-foreground">policy はありません。</p>
        )}
      </Section>
      <Section level={4} title="配下の担当">
        <ChildrenTable node={node} items={list.items} profiles={profiles} />
      </Section>
      <OrgSkills node={node} profile={profile} />
      <NodeForm node={node} items={list.items} />
    </div>
  );
}

function NodeForm({ node, items }: { node: OrgNode; items: OrgNode[] }) {
  const sender = useActionResult(orgKeys.all);
  const [name, setName] = useState(node.name);
  const [brief, setBrief] = useState(node.brief ?? "");
  const [genre, setGenre] = useState(node.genre ?? "");
  const [parentId, setParentId] = useState(node.parent_id ?? "");
  const [kind, setKind] = useState<OrgKind>(node.kind);
  return (
    <section className="space-y-3 rounded-lg border border-border bg-surface p-4" aria-label="担当の編集">
      <h3 className="text-body font-semibold text-foreground">担当を変更</h3>
      <label className={labelClass}>
        名前
        <input className={fieldClass} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className={labelClass}>
        一言
        <textarea className={fieldClass} value={brief} onChange={(e) => setBrief(e.target.value)} />
      </label>
      <label className={labelClass}>
        分野
        <input className={fieldClass} value={genre} onChange={(e) => setGenre(e.target.value)} />
      </label>
      <label className={labelClass}>
        種類
        <select className={fieldClass} value={kind} onChange={(e) => setKind(e.target.value as OrgKind)}>
          {Object.entries(kindLabel).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </label>
      <label className={labelClass}>
        親
        <select className={fieldClass} value={parentId} onChange={(e) => setParentId(e.target.value)}>
          <option value="">なし</option>
          {items
            .filter((item) => item.id !== node.id)
            .map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
              </option>
            ))}
        </select>
      </label>
      <div className="flex flex-wrap gap-2">
        <Button
          variant="primary"
          disabled={sender.pending || !name.trim()}
          onClick={() =>
            void sender.run([
              {
                id: "patch",
                method: "PATCH",
                path: `/api/org/${encodeURIComponent(node.id)}`,
                body: { name, brief, genre: genre || null, kind, parent_id: parentId || null },
              },
            ])
          }
        >
          保存
        </Button>
        {node.kind !== "secretary" && (
          <Button
            variant="destructive"
            disabled={sender.pending}
            onClick={() => {
              if (window.confirm(`${node.name} を削除しますか？`))
                void sender.run([{ id: "delete", method: "DELETE", path: `/api/org/${encodeURIComponent(node.id)}` }]);
            }}
          >
            削除
          </Button>
        )}
      </div>
      <ActionResultView result={sender.results.patch} />
      <ActionResultView result={sender.results.delete} />
    </section>
  );
}

function CreateForm({ items, selected }: { items: OrgNode[]; selected?: string }) {
  const sender = useActionResult(orgKeys.all);
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  const [parent, setParent] = useState(selected ?? "cos");
  const [kind, setKind] = useState<OrgKind>("section");
  return (
    <section className="space-y-3 rounded-lg border border-border bg-surface p-4" aria-label="担当の追加">
      <h2 className="text-section font-semibold text-foreground">担当を追加</h2>
      <label className={labelClass}>
        ID
        <input className={fieldClass} value={id} onChange={(e) => setId(e.target.value)} />
      </label>
      <label className={labelClass}>
        名前
        <input className={fieldClass} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className={labelClass}>
        種類
        <select className={fieldClass} value={kind} onChange={(e) => setKind(e.target.value as OrgKind)}>
          <option value="department">部</option>
          <option value="section">課</option>
        </select>
      </label>
      <label className={labelClass}>
        親
        <select className={fieldClass} value={parent} onChange={(e) => setParent(e.target.value)}>
          {items.map((item) => (
            <option key={item.id} value={item.id}>
              {item.name}
            </option>
          ))}
        </select>
      </label>
      <Button
        variant="primary"
        disabled={sender.pending || !id.trim() || !name.trim()}
        onClick={() => {
          const body: OrgCreateBody = { id: id.trim(), name: name.trim(), kind, parent_id: parent || null };
          void sender.run([{ id: "create", path: "/api/org", body }]);
        }}
      >
        追加
      </Button>
      <ActionResultView result={sender.results.create} />
    </section>
  );
}

export function OrgScreen() {
  const search = Route.useSearch();
  const navigate = useNavigate();
  const query = useQuery({ queryKey: orgKeys.list(), queryFn: ({ signal }) => apiGet<OrgList>("/api/org", signal) });
  const selected = query.data?.items.find((node) => node.id === search.selected);
  return (
    <ScreenFrame title="組織" route="/org">
      <FetchFrame query={query}>
        {query.data && (
          <div className="grid gap-4 lg:grid-cols-5">
            <div className="min-w-0 space-y-4 lg:col-span-2">
              <Section
                title="組織の木"
                description={`${query.data.items.length} 件。選ぶと担当の詳細に設定の現在値が出ます。`}
              >
                <Tree
                  items={query.data.items}
                  profiles={query.data.effective_profiles ?? []}
                  selected={search.selected}
                  onSelect={(id) => void navigate({ to: "/org", search: { ...search, selected: id } })}
                />
              </Section>
              <CreateForm items={query.data.items} selected={search.selected} />
            </div>
            <section
              className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4 lg:col-span-3"
              aria-label="担当の詳細"
            >
              <h2 className="text-section font-semibold text-foreground">担当の詳細</h2>
              {selected ? (
                <NodeDetail key={selected.id} node={selected} list={query.data} />
              ) : (
                <p className="text-label text-muted-foreground">木から担当を選んでください。</p>
              )}
            </section>
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
