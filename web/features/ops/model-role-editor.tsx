import { useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useId, useState } from "react";
import { apiMutate } from "../../api/client";
import {
  ASSIGNMENT_TIERS,
  type AssignmentList,
  type AssignmentTier,
  type ImpactView,
} from "../../api/model-assignments";
import { modelKeys } from "../../api/queries/keys";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { type LlmModel, modelSourceLabel } from "./models-catalog";

export type RoleMember = { source: string; model_id: string; priority: number };
export type RolePreview = { before: RoleMember[]; after: RoleMember[]; impact: ImpactView };
export const roleMembersPath = (tier: AssignmentTier) => `/api/llm/models/assignments/roles/${tier}`;
const identity = (m: Pick<RoleMember, "source" | "model_id">) => JSON.stringify([m.source, m.model_id]);
export function roleMembers(data: AssignmentList, tier: AssignmentTier): RoleMember[] {
  return data.effective
    .filter((s) => s.tier === tier && s.model_id !== null)
    .map((s) => ({ source: s.source, model_id: s.model_id as string, priority: s.priority ?? 0 }))
    .sort(
      (a, b) => a.priority - b.priority || a.source.localeCompare(b.source) || a.model_id.localeCompare(b.model_id),
    );
}
export function moveMember(members: readonly RoleMember[], index: number, direction: -1 | 1): RoleMember[] {
  const result = [...members];
  const next = index + direction;
  if (index < 0 || next < 0 || index >= result.length || next >= result.length) return result;
  [result[index], result[next]] = [result[next] as RoleMember, result[index] as RoleMember];
  return result.map((m, priority) => ({ ...m, priority }));
}

export function RoleMembershipEditor({
  data,
  models,
  providerAction,
  quotaLabels = {},
}: {
  data: AssignmentList;
  models: readonly LlmModel[];
  providerAction?: ReactNode;
  quotaLabels?: Record<string, string>;
}) {
  const queryClient = useQueryClient();
  const searchId = useId();
  const [search, setSearch] = useState("");
  const [source, setSource] = useState("");
  const [status, setStatus] = useState("");
  const [role, setRole] = useState<AssignmentTier | "">("");
  const [drafts, setDrafts] = useState<Partial<Record<AssignmentTier, RoleMember[]>>>({});
  const [previews, setPreviews] = useState<Partial<Record<AssignmentTier, RolePreview>> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [denied, setDenied] = useState(false);
  const members = (tier: AssignmentTier) =>
    [...(drafts[tier] ?? roleMembers(data, tier))].sort(
      (a, b) => a.priority - b.priority || a.source.localeCompare(b.source) || a.model_id.localeCompare(b.model_id),
    );
  const edit = (tier: AssignmentTier, values: RoleMember[]) => {
    setDrafts((old) => ({ ...old, [tier]: values }));
    setPreviews(null);
    setError(null);
  };
  const modelStatus = (m: LlmModel) => (m.override?.disabled ? "無効化" : m.available ? "利用可" : "消失");
  // Config-only models and disappeared members remain visible and removable.
  const allModels = [...models];
  for (const tier of ASSIGNMENT_TIERS)
    for (const m of members(tier)) {
      if (!allModels.some((item) => identity(item) === identity(m)))
        allModels.push({
          ...m,
          display_name: null,
          available: false,
          first_seen: "",
          last_seen: "",
          capabilities: {},
          override: null,
          routing: { tiers: [], deployments: [] },
        });
    }
  const visible = allModels.filter(
    (m) =>
      (!source || source === m.source) &&
      (!status || status === modelStatus(m)) &&
      `${m.model_id} ${m.display_name ?? ""}`.toLowerCase().includes(search.toLowerCase()),
  );
  const changed = ASSIGNMENT_TIERS.filter((tier) => drafts[tier] !== undefined);
  const reportError = (reason: unknown) => {
    setError(reason instanceof Error ? reason.message : String(reason));
    const code = (reason as { status?: number })?.status;
    if (code === 401 || code === 403) setDenied(true);
  };
  const preview = async () => {
    setBusy(true);
    setError(null);
    try {
      const result: Partial<Record<AssignmentTier, RolePreview>> = {};
      for (const tier of changed)
        result[tier] = await apiMutate<RolePreview>("POST", `${roleMembersPath(tier)}/preview`, {
          members: members(tier),
        });
      setPreviews(result);
    } catch (reason) {
      reportError(reason);
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      for (const tier of changed) {
        await apiMutate("PUT", roleMembersPath(tier), { members: members(tier) });
        setDrafts((old) => {
          const next = { ...old };
          delete next[tier];
          return next;
        });
      }
      setPreviews(null);
    } catch (reason) {
      setPreviews(null);
      reportError(reason);
    } finally {
      await queryClient.invalidateQueries({ queryKey: modelKeys.all });
      setBusy(false);
    }
  };
  return (
    <div className="min-w-0 space-y-4">
      <p className="text-body text-muted-foreground">
        各モデルに役割を複数付けられます。優先度は小さい順です。役割をすべて外したモデルは候補になりません。
      </p>
      {providerAction}
      <div className="flex flex-wrap items-center gap-3">
        <label htmlFor={searchId}>
          モデルを検索
          <Input id={searchId} aria-label="モデルを検索" value={search} onChange={(e) => setSearch(e.target.value)} />
        </label>
        <label>
          供給元
          <select
            className="min-h-11"
            aria-label="供給元で絞り込み"
            value={source}
            onChange={(e) => setSource(e.target.value)}
          >
            <option value="">全供給元</option>
            {[...new Set(allModels.map((m) => m.source))].sort().map((s) => (
              <option key={s} value={s}>
                {modelSourceLabel(s)}
              </option>
            ))}
          </select>
        </label>
        <label>
          状態
          <select
            className="min-h-11"
            aria-label="状態で絞り込み"
            value={status}
            onChange={(e) => setStatus(e.target.value)}
          >
            <option value="">全状態</option>
            {["利用可", "消失", "無効化"].map((s) => (
              <option key={s}>{s}</option>
            ))}
          </select>
        </label>
        <label>
          表示
          <select
            className="min-h-11"
            aria-label="役割ごとの表示"
            value={role}
            onChange={(e) => setRole(e.target.value as AssignmentTier | "")}
          >
            <option value="">全モデル</option>
            {ASSIGNMENT_TIERS.map((t) => (
              <option key={t}>{t}</option>
            ))}
          </select>
        </label>
      </div>
      {!role ? (
        <Table aria-label="モデルごとの役割">
          <TableHeader>
            <TableRow>
              <TableHead>モデル / 供給元</TableHead>
              <TableHead>状態</TableHead>
              {ASSIGNMENT_TIERS.map((t) => (
                <TableHead key={t}>{t} / 優先度</TableHead>
              ))}
            </TableRow>
          </TableHeader>
          <TableBody>
            {visible.map((m) => (
              <TableRow key={identity(m)}>
                <TableCell className="break-all">
                  {m.model_id}
                  <div className="text-muted-foreground">{modelSourceLabel(m.source)}</div>
                </TableCell>
                <TableCell>
                  {modelStatus(m)}
                  <div className="text-muted-foreground">{quotaLabels[m.source]}</div>
                </TableCell>
                {ASSIGNMENT_TIERS.map((tier) => {
                  const values = members(tier);
                  const member = values.find((a) => identity(a) === identity(m));
                  return (
                    <TableCell key={tier}>
                      <div className="flex items-center gap-2">
                        <label className="flex min-h-11 min-w-11 items-center justify-center">
                          <input
                            type="checkbox"
                            className="size-11"
                            aria-label={`${m.source} ${m.model_id} ${tier}`}
                            checked={!!member}
                            disabled={busy || denied}
                            onChange={(e) =>
                              edit(
                                tier,
                                e.target.checked
                                  ? [
                                      ...values,
                                      {
                                        source: m.source,
                                        model_id: m.model_id,
                                        priority: values.length ? Math.max(...values.map((a) => a.priority)) + 1 : 0,
                                      },
                                    ]
                                  : values.filter((a) => identity(a) !== identity(m)),
                              )
                            }
                          />
                        </label>
                        {member && (
                          <Input
                            type="number"
                            min={0}
                            max={4294967295}
                            aria-label={`${m.source} ${m.model_id} ${tier} 優先度`}
                            className="w-16 shrink-0"
                            value={member.priority}
                            disabled={busy || denied}
                            onChange={(e) => {
                              const n = Number(e.target.value);
                              if (Number.isInteger(n) && n >= 0 && n <= 4294967295)
                                edit(
                                  tier,
                                  values.map((a) => (identity(a) === identity(m) ? { ...a, priority: n } : a)),
                                );
                            }}
                          />
                        )}
                      </div>
                    </TableCell>
                  );
                })}
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : (
        <Table aria-label={`${role} の候補と順位`}>
          <TableHeader>
            <TableRow>
              <TableHead>順位</TableHead>
              <TableHead>モデル / 供給元</TableHead>
              <TableHead>並べ替え</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {members(role).map((m, index, values) => (
              <TableRow key={identity(m)}>
                <TableCell>
                  {index + 1}（優先度 {m.priority}）
                </TableCell>
                <TableCell className="break-all">
                  {m.model_id} / {modelSourceLabel(m.source)}
                </TableCell>
                <TableCell>
                  <div className="flex gap-2">
                    <Button
                      aria-label={`${m.model_id} を上へ`}
                      disabled={busy || denied || index === 0}
                      onClick={() => edit(role, moveMember(values, index, -1))}
                    >
                      上へ
                    </Button>
                    <Button
                      aria-label={`${m.model_id} を下へ`}
                      disabled={busy || denied || index === values.length - 1}
                      onClick={() => edit(role, moveMember(values, index, 1))}
                    >
                      下へ
                    </Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      {changed.length > 0 && (
        <div className="space-y-3">
          <p>未保存: {changed.join(", ")}</p>
          <div className="flex gap-2">
            <Button disabled={busy || denied} onClick={() => void preview()}>
              変更の影響を確認
            </Button>
            <Button
              disabled={busy}
              onClick={() => {
                setDrafts({});
                setPreviews(null);
              }}
            >
              変更を破棄
            </Button>
          </div>
          {previews && (
            <section aria-label="役割の変更の影響" className="space-y-2">
              {changed.map((tier) => {
                const p = previews[tier];
                return (
                  p && (
                    <div key={tier}>
                      <p>
                        {tier}: {p.before.map((m) => `${m.model_id} (${m.priority})`).join(", ") || "候補なし"} →{" "}
                        {p.after.map((m) => `${m.model_id} (${m.priority})`).join(", ") || "候補なし"}
                      </p>
                      <p className="text-muted-foreground">
                        適用先: {p.impact.changes.map((c) => c.id).join(", ") || "なし（provider の追加が必要です）"}
                      </p>
                    </div>
                  )
                );
              })}
              <Button variant="primary" disabled={busy || denied} onClick={() => void save()}>
                変更を保存
              </Button>
            </section>
          )}
        </div>
      )}
      {error && (
        <p role="alert" className="text-danger-foreground">
          {error}
        </p>
      )}
      {denied && <p role="alert">この操作を行う権限がありません。</p>}
    </div>
  );
}
