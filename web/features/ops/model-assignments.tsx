import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, apiGet, apiMutate } from "../../api/client";
import { ASSIGNMENT_TIERS, type AssignmentList, getAssignments, type RoleSlotView } from "../../api/model-assignments";
import { accountKeys, modelKeys, providerKeys } from "../../api/queries/keys";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Section } from "../../components/ui/panel";
import { RoleMembershipEditor } from "./model-role-editor";
import type { LlmModel } from "./models-catalog";

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
  return (
    <RoleMembershipEditor
      data={data}
      models={models}
      quotaLabels={Object.fromEntries(
        data.effective.map((m) => [m.source, quotaLabel(poolRemaining(accounts, m.source))]),
      )}
      providerAction={
        providers !== null &&
        !hasOpencodeGoProvider(providers) &&
        data.effective.some((m) => m.source === "opencode-go") ? (
          <OpencodeGoButton />
        ) : undefined
      }
    />
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
      description="モデルごとに frontier / standard / cheap を設定し、役割内の候補と順位を管理します。"
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
