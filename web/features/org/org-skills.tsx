import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRef, useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import type { EffectiveProfile, OrgNode, SkillDetailView, SkillList } from "../../api/generated/types";
import { orgKeys, skillKeys } from "../../api/queries/keys";
import { Markdown } from "../../components/content/markdown";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";

const selectClass = "min-h-11 max-w-full rounded border px-2";

function SkillPreview({ name }: { name: string }) {
  const query = useQuery({
    queryKey: ["skills", "detail", name],
    queryFn: ({ signal }) => apiGet<SkillDetailView>(`/api/skills/${encodeURIComponent(name)}`, signal),
  });
  return <FetchFrame query={query}>{query.data && <Markdown source={query.data.skill_md} />}</FetchFrame>;
}

export function OrgSkills({ node, profile }: { node: OrgNode; profile?: EffectiveProfile }) {
  const client = useQueryClient();
  const list = useQuery({
    queryKey: skillKeys.list(),
    queryFn: ({ signal }) => apiGet<SkillList>("/api/skills", signal),
    retry: false,
  });
  const [choice, setChoice] = useState("");
  const [preview, setPreview] = useState<string>();
  const [result, setResult] = useState<{ ok: boolean; message: string }>();
  const busyRef = useRef(false);
  const [busy, setBusy] = useState(false);
  const own = node.profile?.skills_mounts ?? [];
  const effective = profile?.skills_mounts ?? own;
  const inherited = effective.filter((name) => !own.includes(name));
  const options = list.data?.items.filter((item) => !effective.includes(item.name)) ?? [];
  async function change(skill: string, mount: boolean) {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setResult(undefined);
    try {
      await apiMutate(
        mount ? "POST" : "DELETE",
        mount
          ? `/api/org/${encodeURIComponent(node.id)}/skills`
          : `/api/org/${encodeURIComponent(node.id)}/skills/${encodeURIComponent(skill)}`,
        mount ? { skill } : undefined,
      );
      setResult({ ok: true, message: "操作が完了しました" });
      // org の詳細情報は一覧応答に含まれる。関連する org と skill 一覧だけ更新する。
      await Promise.all([
        client.invalidateQueries({ queryKey: orgKeys.list(), exact: true }),
        client.invalidateQueries({ queryKey: skillKeys.list(), exact: true }),
      ]);
    } catch (error) {
      const api = error instanceof ApiError ? error : null;
      if (api?.status === 409)
        await Promise.all([
          client.invalidateQueries({ queryKey: orgKeys.list(), exact: true }),
          client.invalidateQueries({ queryKey: skillKeys.list(), exact: true }),
        ]);
      const body = api?.body;
      const detail = body && typeof body === "object" ? (body as Record<string, unknown>).detail : null;
      setResult({
        ok: false,
        message:
          api?.status === 409
            ? "状態が変わりました。最新の状態を確認してください。"
            : typeof detail === "string"
              ? detail
              : api?.kind === "timeout" || api?.kind === "network"
                ? "結果を確認できません。再取得して状態を確認してください。"
                : "操作に失敗しました",
      });
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  }
  return (
    <section className="space-y-3 rounded border p-3" aria-label="担当の skill">
      <h3 className="font-semibold">skill</h3>
      {own.length === 0 && inherited.length === 0 && <p>mount された skill はありません。</p>}
      <ul className="space-y-2">
        {own.map((name) => (
          <li key={name} className="flex flex-wrap items-center gap-2">
            <Button onClick={() => setPreview(preview === name ? undefined : name)} aria-expanded={preview === name}>
              {name} を表示
            </Button>
            <Button disabled={busy} onClick={() => void change(name, false)}>
              外す
            </Button>
          </li>
        ))}
        {inherited.map((name) => (
          <li key={name} className="flex flex-wrap items-center gap-2">
            <Button onClick={() => setPreview(preview === name ? undefined : name)}>{name} を表示</Button>
            <span>継承</span>
          </li>
        ))}
      </ul>
      {list.isError && <p role="alert">skill の一覧を読めませんでした。</p>}
      {list.data && (
        <div className="flex flex-wrap items-end gap-2">
          <label>
            mount する skill
            <select className={selectClass} value={choice} onChange={(e) => setChoice(e.target.value)}>
              <option value="">選ぶ</option>
              {options.map((item) => (
                <option key={item.name} value={item.name}>
                  {item.name}
                </option>
              ))}
            </select>
          </label>
          <Button disabled={busy || !choice} onClick={() => void change(choice, true)}>
            mount
          </Button>
        </div>
      )}
      {result && <p role={result.ok ? "status" : "alert"}>{result.message}</p>}
      {preview && <SkillPreview name={preview} />}
    </section>
  );
}

export function OrgProfile({ node, profile }: { node: OrgNode; profile?: EffectiveProfile }) {
  return (
    <section className="space-y-2 rounded border p-3" aria-label="profile">
      <h3 className="font-semibold">profile</h3>
      {node.profile?.policy?.length ? (
        <Markdown source={node.profile.policy.join("\n\n")} />
      ) : (
        <p>policy はありません。</p>
      )}
      {profile && (
        <dl className="grid grid-cols-1 gap-2 text-sm sm:grid-cols-2">
          <div>
            <dt>継承</dt>
            <dd>{profile.chain?.join(" → ") ?? "なし"}</dd>
          </div>
          <div>
            <dt>能力タグ</dt>
            <dd>{profile.skills?.join(", ") ?? "なし"}</dd>
          </div>
          <div>
            <dt>実行環境</dt>
            <dd>{profile.run ?? "既定"}</dd>
          </div>
          <div>
            <dt>モデル階層</dt>
            <dd>{profile.tier ?? "既定"}</dd>
          </div>
        </dl>
      )}
    </section>
  );
}
