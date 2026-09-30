import { useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { OrgCreateBody, OrgKind, OrgList, OrgNode } from "../../api/generated/types";
import { orgKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { Route } from "../../routes/org.index";
import { OrgProfile, OrgSkills } from "./org-skills";
import { buildOrgTree, type TreeNode } from "./org-tree";

const kindLabel: Record<OrgKind, string> = { secretary: "CoS", department: "部", section: "課" };
const fieldClass = "block min-h-11 w-full rounded border border-neutral-400 px-3";

function Tree({ nodes, selected, onSelect }: { nodes: TreeNode[]; selected?: string; onSelect: (id: string) => void }) {
  return (
    <ul className="space-y-1 pl-3 border-l">
      {nodes.map(({ node, children }) => (
        <li key={node.id}>
          <Button
            aria-current={selected === node.id ? "page" : undefined}
            className="w-full justify-start text-left"
            onClick={() => onSelect(node.id)}
          >
            {kindLabel[node.kind]} · {node.name}
          </Button>
          {children.length > 0 && <Tree nodes={children} selected={selected} onSelect={onSelect} />}
        </li>
      ))}
    </ul>
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
    <section className="space-y-3 rounded border p-3" aria-label="担当の編集">
      <h3 className="font-semibold">担当を変更</h3>
      <label className="block">
        名前
        <input className={fieldClass} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="block">
        一言
        <textarea className={fieldClass} value={brief} onChange={(e) => setBrief(e.target.value)} />
      </label>
      <label className="block">
        分野
        <input className={fieldClass} value={genre} onChange={(e) => setGenre(e.target.value)} />
      </label>
      <label className="block">
        種類
        <select className={fieldClass} value={kind} onChange={(e) => setKind(e.target.value as OrgKind)}>
          {Object.entries(kindLabel).map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </label>
      <label className="block">
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
    <section className="space-y-3 rounded border p-3" aria-label="担当の追加">
      <h2 className="font-semibold">担当を追加</h2>
      <label className="block">
        ID
        <input className={fieldClass} value={id} onChange={(e) => setId(e.target.value)} />
      </label>
      <label className="block">
        名前
        <input className={fieldClass} value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <label className="block">
        種類
        <select className={fieldClass} value={kind} onChange={(e) => setKind(e.target.value as OrgKind)}>
          <option value="department">部</option>
          <option value="section">課</option>
        </select>
      </label>
      <label className="block">
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
          <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.5fr)]">
            <section className="min-w-0 space-y-3 rounded border p-3" aria-label="組織の木">
              <h2 className="font-semibold">組織の木</h2>
              <Tree
                nodes={buildOrgTree(query.data.items)}
                selected={search.selected}
                onSelect={(id) => void navigate({ to: "/org", search: { ...search, selected: id } })}
              />
              <CreateForm items={query.data.items} selected={search.selected} />
            </section>
            <section className="min-w-0 space-y-3 rounded border p-3" aria-label="担当の詳細">
              <h2 className="font-semibold">担当の詳細</h2>
              {selected ? (
                <div key={selected.id} className="space-y-3">
                  <h3 className="text-lg font-semibold">{selected.name}</h3>
                  <p>
                    {kindLabel[selected.kind]} · {selected.id}
                  </p>
                  {selected.brief && <p>{selected.brief}</p>}
                  {selected.genre && <p>分野: {selected.genre}</p>}
                  <a
                    className="inline-flex min-h-11 min-w-11 items-center underline"
                    href={`/org/${encodeURIComponent(selected.id)}`}
                  >
                    話す
                  </a>
                  <OrgProfile
                    node={selected}
                    profile={query.data.effective_profiles?.find((profile) => profile.node_id === selected.id)}
                  />
                  <OrgSkills
                    node={selected}
                    profile={query.data.effective_profiles?.find((profile) => profile.node_id === selected.id)}
                  />
                  <NodeForm node={selected} items={query.data.items} />
                </div>
              ) : (
                <p>木から担当を選んでください。</p>
              )}
            </section>
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
