import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import {
  ASSIGNMENT_TIERS,
  type AssignmentList,
  type AssignmentTier,
  deleteAssignment,
  getAssignments,
  type ImpactView,
  previewAssignment,
  putAssignment,
  type RoleSlotView,
} from "../../api/model-assignments";
import { accountKeys, modelKeys, providerKeys } from "../../api/queries/keys";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Drawer } from "../../components/ui/drawer";
import { Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import { Select } from "../../components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute } from "../../lib/time";
import { type LlmModel, modelSourceLabel } from "./models-catalog";

/** account pool の adapter 名（accounts の `adapter`）。source → pool の対応は ADR D5。 */
const SOURCE_POOL_ADAPTER: Record<string, string> = {
  "claude-oauth": "claude-code",
  "codex-oauth": "codex",
  "opencode-go": "opencode-go",
};

export type AccountLite = {
  adapter?: string;
  usage?: {
    five_hour?: { utilization: number } | null;
    seven_day?: { utilization: number } | null;
    one_month?: { utilization: number } | null;
  } | null;
};

/** source の pool の最小残量（0..1）。pool が無い・usage を取れない source は null（『不明』）。 */
export function poolRemaining(accounts: readonly AccountLite[] | null | undefined, source: string): number | null {
  const adapter = SOURCE_POOL_ADAPTER[source];
  if (!adapter || !accounts) return null;
  let min: number | null = null;
  for (const account of accounts) {
    if (account.adapter !== adapter || !account.usage) continue;
    for (const window of [account.usage.five_hour, account.usage.seven_day, account.usage.one_month]) {
      if (!window) continue;
      const remaining = Math.min(1, Math.max(0, 1 - window.utilization));
      if (min === null || remaining < min) min = remaining;
    }
  }
  return min;
}

/** `llm_source` は文字列でも `{ source }` でも来る。`opencode-go` と `opencode_go` は同じ source。 */
export function llmSourceName(value: unknown): string | null {
  if (typeof value === "string") return value;
  if (value && typeof value === "object" && "source" in value)
    return llmSourceName((value as { source: unknown }).source);
  return null;
}

export function hasOpencodeGoProvider(providers: readonly { llm_source?: unknown }[] | null | undefined): boolean {
  return (providers ?? []).some((p) => llmSourceName(p.llm_source)?.replaceAll("-", "_") === "opencode_go");
}

export const OPENCODE_GO_PROVIDER_BODY = {
  id: "opencode-go",
  adapter: "acp",
  llm_source: "opencode_go",
  account_pool: "opencode-go",
  tiers: ["frontier", "standard", "cheap"],
  concurrency: 1,
} as const;

export type SourceSlots = { source: string; slots: RoleSlotView[] };

/** source ごとに 3 役割を frontier / standard / cheap の順で並べる。source の順は effective に出た順。 */
export function groupSlots(effective: readonly RoleSlotView[]): SourceSlots[] {
  const order: string[] = [];
  for (const slot of effective) if (!order.includes(slot.source)) order.push(slot.source);
  return order.map((source) => ({
    source,
    slots: ASSIGNMENT_TIERS.flatMap((tier) => effective.filter((s) => s.source === source && s.tier === tier)),
  }));
}

export function availabilityLabel(available: boolean | null): {
  label: "利用可" | "消失" | "不明";
  tone: "success" | "neutral";
} {
  if (available === true) return { label: "利用可", tone: "success" };
  if (available === false) return { label: "消失", tone: "neutral" };
  return { label: "不明", tone: "neutral" };
}

export function quotaLabel(remaining: number | null): string {
  return remaining === null ? "不明" : `残り ${Math.round(remaining * 100)}%`;
}

function errorText(error: unknown): string {
  if (error instanceof ApiError) {
    const body = error.body;
    if (typeof body === "string" && body !== "") return body;
    if (body && typeof body === "object") {
      const r = body as Record<string, unknown>;
      for (const v of [r.detail, r.message, r.error]) if (typeof v === "string") return v;
    }
    if (error.kind === "forbidden" || error.kind === "unauthorized") return "この操作の権限がありません。";
    if (error.kind === "timeout" || error.kind === "network" || error.kind === "aborted")
      return "結果を確認できません。再取得して状態を確認してください。";
  }
  return "操作に失敗しました";
}

function ImpactList({ impact }: { impact: ImpactView }) {
  if (impact.changes.length === 0)
    return <p className="text-body text-muted-foreground">この変更で影響を受ける provider / proxy はありません。</p>;
  return (
    <ul aria-label="影響" className="min-w-0 space-y-2">
      {impact.changes.map((c) => (
        <li key={`${c.kind}:${c.id}:${c.tier}`} data-impact-kind={c.kind} className="min-w-0 break-words text-body">
          <span className="font-medium">
            {c.kind === "provider" ? "provider" : "proxy"} {c.id}
          </span>{" "}
          ({c.tier}): {c.before ?? "未設定"} → {c.after ?? "未設定"}
          {c.excluded_reason && (
            <>
              {" "}
              <Badge tone="warning">{c.excluded_reason}</Badge>
            </>
          )}
        </li>
      ))}
    </ul>
  );
}

function ChangeForm({
  source,
  tier,
  current,
  candidates,
  onDone,
}: {
  source: string;
  tier: AssignmentTier;
  current: string | null;
  candidates: readonly LlmModel[];
  onDone: () => void;
}) {
  const ids = useId();
  const queryClient = useQueryClient();
  const [modelId, setModelId] = useState(candidates.some((m) => m.model_id === current) ? (current ?? "") : "");
  const [note, setNote] = useState("");
  const [impact, setImpact] = useState<{ modelId: string; value: ImpactView } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const previewed = impact !== null && impact.modelId === modelId;

  async function preview() {
    setBusy(true);
    setError(null);
    try {
      const out = await previewAssignment({ source, tier, model_id: modelId });
      setImpact({ modelId, value: out.impact });
    } catch (reason) {
      setImpact(null);
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    setBusy(true);
    setError(null);
    try {
      const trimmed = note.trim();
      await putAssignment(source, tier, trimmed === "" ? { model_id: modelId } : { model_id: modelId, note: trimmed });
      await queryClient.invalidateQueries({ queryKey: modelKeys.all });
      onDone();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      noValidate
      className="min-w-0 space-y-3"
      aria-label={`役割の変更 ${source} ${tier}`}
      onSubmit={(event) => {
        event.preventDefault();
        if (modelId === "") return;
        void (previewed ? apply() : preview());
      }}
    >
      <div className="min-w-0">
        <label htmlFor={`${ids}-model`} className="block text-label text-foreground">
          モデル
        </label>
        <Select
          id={`${ids}-model`}
          value={modelId}
          onChange={(e) => {
            setModelId(e.target.value);
            setImpact(null);
          }}
        >
          <option value="">選択してください</option>
          {candidates.map((m) => (
            <option key={m.model_id} value={m.model_id}>
              {m.display_name ? `${m.model_id}（${m.display_name}）` : m.model_id}
            </option>
          ))}
        </Select>
        {candidates.length === 0 && (
          <p className="mt-1 text-label text-muted-foreground">
            選べるモデルがありません。先に発見を実行してください。
          </p>
        )}
      </div>
      <div className="min-w-0">
        <label htmlFor={`${ids}-note`} className="block text-label text-foreground">
          メモ（任意）
        </label>
        <Input id={`${ids}-note`} value={note} onChange={(e) => setNote(e.target.value)} />
      </div>
      {impact && impact.modelId === modelId && (
        <div className="min-w-0 space-y-2" data-impact>
          <p className="text-label font-semibold text-foreground">影響</p>
          <ImpactList impact={impact.value} />
        </div>
      )}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant={previewed ? "secondary" : "primary"} disabled={busy || modelId === ""}>
          影響を確認
        </Button>
        {previewed && (
          <Button variant="primary" disabled={busy} onClick={() => void apply()}>
            割り当てる
          </Button>
        )}
      </div>
      {error && (
        <p role="alert" className="text-danger-foreground">
          {error}
        </p>
      )}
    </form>
  );
}

function ChangeDrawer({ slot, models }: { slot: RoleSlotView; models: readonly LlmModel[] }) {
  const [open, setOpen] = useState(false);
  const candidates = models.filter((m) => m.source === slot.source && m.available && !m.override?.disabled);
  return (
    <Drawer
      open={open}
      onOpenChange={setOpen}
      title={`役割の変更: ${slot.tier}`}
      description={`${modelSourceLabel(slot.source)} の ${slot.tier} に割り当てるモデルを選びます。確定前に影響を確認できます。`}
      trigger={
        <Button size="sm" aria-label={`${slot.tier} を変更`}>
          変更
        </Button>
      }
    >
      {open && (
        <ChangeForm
          source={slot.source}
          tier={slot.tier}
          current={slot.model_id}
          candidates={candidates}
          onDone={() => setOpen(false)}
        />
      )}
    </Drawer>
  );
}

function ReleaseButton({ slot }: { slot: RoleSlotView }) {
  const queryClient = useQueryClient();
  return (
    <ConfirmDialog
      trigger={
        <Button size="sm" aria-label={`${slot.tier} を解除`}>
          解除
        </Button>
      }
      title="割り当てを解除"
      target={`${modelSourceLabel(slot.source)} の ${slot.tier}（${slot.model_id ?? "未設定"}）`}
      consequence="この割り当てを消し、config の値に戻します。config にも無い役割は未設定になり、その役割の候補から外れます。"
      reversibility="「変更」でもう一度割り当てられます。"
      followUp="「役割の割り当て」の由来 badge と現在のモデルで確認できます。"
      confirmLabel={`${slot.tier} の割り当てを解除`}
      onConfirm={async () => {
        try {
          await deleteAssignment(slot.source, slot.tier);
        } catch (reason) {
          throw new Error(errorText(reason));
        }
        await queryClient.invalidateQueries({ queryKey: modelKeys.all });
      }}
    />
  );
}

function OpencodeGoButton() {
  const queryClient = useQueryClient();
  return (
    <ConfirmDialog
      trigger={<Button variant="primary">opencode go を使う（provider を追加）</Button>}
      title="opencode go の provider を追加"
      target="provider opencode-go（adapter acp・account pool opencode-go）"
      consequence="providers.d に provider を追加して reload します。モデルは役割の割り当てで決まり、割り当てが無い役割はその provider の候補になりません。"
      reversibility="providers 画面から削除できます。"
      followUp="providers 画面と、この画面の opencode-go のカードで確認できます。"
      confirmLabel="provider opencode-go を追加"
      onConfirm={async () => {
        try {
          await apiMutate("POST", "/api/providers", OPENCODE_GO_PROVIDER_BODY);
          await apiMutate("POST", "/api/reload", {});
        } catch (reason) {
          throw new Error(errorText(reason));
        }
        await queryClient.invalidateQueries({ queryKey: providerKeys.all });
        await queryClient.invalidateQueries({ queryKey: modelKeys.all });
      }}
    />
  );
}

function SourceCard({
  group,
  models,
  quota,
  needsProvider,
}: {
  group: SourceSlots;
  models: readonly LlmModel[];
  quota: number | null;
  needsProvider: boolean;
}) {
  const label = modelSourceLabel(group.source);
  return (
    <Section
      title={label}
      level={3}
      data-assignment-source={group.source}
      description={`枠: ${quotaLabel(quota)}`}
      actions={needsProvider ? <OpencodeGoButton /> : undefined}
    >
      <Table aria-label={`${label} の役割の割り当て`}>
        <TableHeader>
          <TableRow>
            <TableHead>役割</TableHead>
            <TableHead>現在のモデル</TableHead>
            <TableHead>由来</TableHead>
            <TableHead>状態</TableHead>
            <TableHead>最終確認</TableHead>
            <TableHead>枠</TableHead>
            <TableHead>操作</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {group.slots.map((slot) => {
            const avail = availabilityLabel(slot.available);
            return (
              <TableRow key={slot.tier} data-role-tier={slot.tier}>
                <TableCell className="font-medium">{slot.tier}</TableCell>
                <TableCell className="break-all">{slot.model_id ?? "未設定"}</TableCell>
                <TableCell>
                  {slot.origin === "assignment" && <Badge tone="info">割り当て</Badge>}
                  {slot.origin === "config" && <Badge tone="neutral">config</Badge>}
                  {slot.origin === null && "-"}
                </TableCell>
                <TableCell>
                  <div className="flex min-w-0 flex-col items-start gap-1">
                    <Badge tone={avail.tone}>{avail.label}</Badge>
                    {slot.excluded_reason && (
                      <Badge tone="warning" data-excluded-reason={slot.excluded_reason}>
                        除外: {slot.excluded_reason}
                      </Badge>
                    )}
                  </div>
                </TableCell>
                <TableCell className="whitespace-nowrap tabular-nums">
                  {slot.last_seen ? formatAbsolute(slot.last_seen) : "-"}
                </TableCell>
                <TableCell className="whitespace-nowrap">{quotaLabel(quota)}</TableCell>
                <TableCell>
                  <div className="flex flex-wrap gap-2">
                    <ChangeDrawer slot={slot} models={models} />
                    {slot.origin === "assignment" && <ReleaseButton slot={slot} />}
                  </div>
                </TableCell>
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </Section>
  );
}

/** 取得済みの値から描画する（hook を持たない。試験はここを描画する）。 */
export function RoleAssignmentsView({
  data,
  models,
  accounts,
  providers,
}: {
  data: AssignmentList;
  models: readonly LlmModel[];
  accounts: readonly AccountLite[] | null;
  /** null は providers を取れていない（ボタンを出さない）。 */
  providers: readonly { llm_source?: unknown }[] | null;
}) {
  const groups = groupSlots(data.effective);
  if (groups.length === 0) return <p className="text-body text-muted-foreground">割り当てできる供給元がありません。</p>;
  const missingGo = providers !== null && !hasOpencodeGoProvider(providers);
  return (
    <div className="min-w-0 space-y-6">
      {groups.map((g) => (
        <SourceCard
          key={g.source}
          group={g}
          models={models}
          quota={poolRemaining(accounts, g.source)}
          needsProvider={g.source === "opencode-go" && missingGo}
        />
      ))}
    </div>
  );
}

/** `/models` の先頭の「役割の割り当て」。割り当て・accounts・providers を取り、取れない部分は『不明』にする。 */
export function ModelAssignmentsSection({ models }: { models: readonly LlmModel[] }) {
  const assignments = useQuery({
    queryKey: modelKeys.list({ section: "assignments" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => getAssignments(signal),
  });
  const accounts = useQuery({
    queryKey: accountKeys.list({ section: "role-quota" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<{ items: AccountLite[] }>("/api/accounts", signal),
  });
  const providers = useQuery({
    queryKey: providerKeys.list({ section: "role-assignments" }),
    queryFn: ({ signal }: { signal: AbortSignal }) =>
      apiGet<{ items: { llm_source?: unknown }[] }>("/api/providers", signal),
  });
  return (
    <Section
      title="役割の割り当て"
      level={2}
      description="供給元ごとに frontier / standard / cheap の役割へ割り当てるモデルを決めます。割り当ては config より優先されます。"
    >
      {assignments.isPending && <p className="text-body text-muted-foreground">読み込み中です。</p>}
      {assignments.isError && (
        <p role="alert" className="text-danger-foreground">
          役割の割り当てを取得できませんでした。
        </p>
      )}
      {assignments.data && (
        <RoleAssignmentsView
          data={assignments.data}
          models={models}
          accounts={accounts.data?.items ?? null}
          providers={providers.data?.items ?? null}
        />
      )}
    </Section>
  );
}
