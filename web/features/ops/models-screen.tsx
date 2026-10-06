import { useQuery } from "@tanstack/react-query";
import { useId, useState } from "react";
import { apiGet } from "../../api/client";
import { modelKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { Drawer } from "../../components/ui/drawer";
import { Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import { Select } from "../../components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute } from "../../lib/time";
import { cn } from "../../lib/utils";
import { deniedMessage, isDenied } from "./providers-form";

// GET /api/llm/models の形（ADR 2026-10-06 opencode-go と model catalog の D1/D2/D5）。
export type ModelTier = "frontier" | "standard" | "cheap";
export type ModelOverride = {
  disabled: boolean;
  tier: ModelTier | null;
  alias: string | null;
  note: string | null;
};
export type LlmModel = {
  source: string;
  model_id: string;
  display_name: string | null;
  available: boolean;
  first_seen: string;
  last_seen: string;
  capabilities: Record<string, unknown>;
  override: ModelOverride | null;
  routing: { tiers: string[]; deployments: string[] };
};
export type LlmDiscovery = { source: string; at: string; ok: boolean; error: string | null; count: number };
export type LlmModelCatalog = { items: LlmModel[]; last_discovery: LlmDiscovery[] };

type Sender = ReturnType<typeof useActionResult>;

const TIER_CHOICES: readonly ModelTier[] = ["frontier", "standard", "cheap"];

/** 供給元 id の表示名。未知の id はそのまま出す。 */
export function modelSourceLabel(source: string): string {
  if (source === "claude-oauth") return "Claude（subscription）";
  if (source === "codex-oauth") return "Codex（subscription）";
  if (source === "opencode-go") return "OpenCode Go（subscription）";
  if (source.startsWith("openai-compatible:")) return `self-host ${source.slice("openai-compatible:".length)}`;
  return source;
}

export type ModelState = { label: "利用可" | "消失" | "無効化"; tone: "success" | "neutral" | "warning" };

/** 無効化（人の上書き）を消失より先に見る。色だけにせず必ず語を出す。 */
export function modelState(item: LlmModel): ModelState {
  if (item.override?.disabled) return { label: "無効化", tone: "warning" };
  if (!item.available) return { label: "消失", tone: "neutral" };
  return { label: "利用可", tone: "success" };
}

export type SourceGroup = { source: string; items: LlmModel[]; discovery: LlmDiscovery | null };

/** source ごとにまとめる。発見の記録だけがあって model が 0 件の source も節として残す。 */
export function groupBySource(data: LlmModelCatalog): SourceGroup[] {
  const order: string[] = [];
  const seen = new Set<string>();
  for (const id of [...data.items.map((m) => m.source), ...data.last_discovery.map((d) => d.source)]) {
    if (!seen.has(id)) {
      seen.add(id);
      order.push(id);
    }
  }
  return order.map((source) => ({
    source,
    items: data.items.filter((m) => m.source === source),
    discovery: data.last_discovery.find((d) => d.source === source) ?? null,
  }));
}

const modelBase = (item: Pick<LlmModel, "source" | "model_id">) =>
  `/api/llm/models/${encodeURIComponent(item.source)}/${encodeURIComponent(item.model_id)}/override`;

function OverrideForm({
  item,
  sender,
  blocked,
  onDone,
}: {
  item: LlmModel;
  sender: Sender;
  blocked: boolean;
  onDone: () => void;
}) {
  const ids = useId();
  const current = item.override;
  const [disabled, setDisabled] = useState(current?.disabled ?? false);
  const [tier, setTier] = useState<string>(current?.tier ?? "");
  const [alias, setAlias] = useState(current?.alias ?? "");
  const [note, setNote] = useState(current?.note ?? "");
  const saveId = `override:${item.source}:${item.model_id}`;
  const result = sender.results[saveId];
  const busy = sender.pending || blocked;
  return (
    <form
      noValidate
      className="min-w-0 space-y-3"
      aria-label={`上書き ${item.model_id}`}
      onSubmit={(event) => {
        event.preventDefault();
        void sender
          .run([
            {
              id: saveId,
              path: modelBase(item),
              method: "PUT",
              body: {
                disabled,
                tier: tier === "" ? null : tier,
                alias: alias.trim() === "" ? null : alias.trim(),
                note: note.trim() === "" ? null : note.trim(),
              },
            },
          ])
          .then((out) => {
            if (out[0]?.ok) onDone();
          });
      }}
    >
      <label className="flex min-h-11 items-center gap-2 text-body text-foreground">
        <input type="checkbox" className="size-5" checked={disabled} onChange={(e) => setDisabled(e.target.checked)} />
        無効化（routing の対象から外す）
      </label>
      <div className="min-w-0">
        <label htmlFor={`${ids}-tier`} className="block text-label text-foreground">
          tier を固定
        </label>
        <Select id={`${ids}-tier`} value={tier} onChange={(e) => setTier(e.target.value)}>
          <option value="">なし</option>
          {TIER_CHOICES.map((t) => (
            <option key={t} value={t}>
              {t}
            </option>
          ))}
        </Select>
      </div>
      <div className="min-w-0">
        <label htmlFor={`${ids}-alias`} className="block text-label text-foreground">
          別名（alias）
        </label>
        <Input id={`${ids}-alias`} value={alias} onChange={(e) => setAlias(e.target.value)} />
      </div>
      <div className="min-w-0">
        <label htmlFor={`${ids}-note`} className="block text-label text-foreground">
          メモ
        </label>
        <Input id={`${ids}-note`} value={note} onChange={(e) => setNote(e.target.value)} />
      </div>
      <div className="flex flex-wrap gap-2">
        <Button type="submit" variant="primary" disabled={busy}>
          上書きを保存
        </Button>
        {current && (
          <Button
            disabled={busy}
            onClick={() =>
              void sender
                .run([{ id: `clear:${item.source}:${item.model_id}`, path: modelBase(item), method: "DELETE" }])
                .then((out) => {
                  if (out[0]?.ok) onDone();
                })
            }
          >
            上書きを消す
          </Button>
        )}
      </div>
      <ActionResultView result={result} />
      <ActionResultView result={sender.results[`clear:${item.source}:${item.model_id}`]} />
    </form>
  );
}

function OverrideCell({ item, sender, blocked }: { item: LlmModel; sender: Sender; blocked: boolean }) {
  const [open, setOpen] = useState(false);
  const o = item.override;
  return (
    <div className="flex min-w-0 flex-col items-start gap-1">
      {o?.alias && <span className="break-words">別名: {o.alias}</span>}
      {o?.note && <span className="break-words text-muted-foreground">{o.note}</span>}
      <Drawer
        open={open}
        onOpenChange={setOpen}
        title={`上書き: ${item.model_id}`}
        description={`${modelSourceLabel(item.source)} のモデルの無効化・tier の固定・別名を設定します。`}
        trigger={
          <Button size="sm" aria-label={`上書きを編集 ${item.model_id}`}>
            {o ? "上書きを編集" : "上書き"}
          </Button>
        }
      >
        <OverrideForm item={item} sender={sender} blocked={blocked} onDone={() => setOpen(false)} />
      </Drawer>
    </div>
  );
}

function ModelRow({ item, sender, blocked }: { item: LlmModel; sender: Sender; blocked: boolean }) {
  const state = modelState(item);
  const muted = state.label !== "利用可";
  const tiers = item.routing.tiers;
  const fixed = item.override?.tier ?? null;
  return (
    <TableRow data-model-state={state.label} className={cn(muted && "text-muted-foreground")}>
      <TableCell className="break-all font-medium">{item.model_id}</TableCell>
      <TableCell className="break-words">{item.display_name ?? "-"}</TableCell>
      <TableCell>
        <Badge tone={state.tone}>{state.label}</Badge>
      </TableCell>
      <TableCell className="whitespace-nowrap tabular-nums">{formatAbsolute(item.last_seen)}</TableCell>
      <TableCell className="min-w-0">
        <div className="flex min-w-0 flex-col gap-1">
          {fixed && <span className="font-medium">固定 {fixed}</span>}
          {tiers.length > 0 && <span>tier: {tiers.join(", ")}</span>}
          {item.routing.deployments.length > 0 && (
            <span className="break-all">deployment: {item.routing.deployments.join(", ")}</span>
          )}
          {!fixed && tiers.length === 0 && item.routing.deployments.length === 0 && <span>-</span>}
        </div>
      </TableCell>
      <TableCell>
        <OverrideCell item={item} sender={sender} blocked={blocked} />
      </TableCell>
    </TableRow>
  );
}

function SourceSection({ group, sender, blocked }: { group: SourceGroup; sender: Sender; blocked: boolean }) {
  const { source, items, discovery } = group;
  const label = modelSourceLabel(source);
  const discoverId = `discover:${source}`;
  return (
    <Section
      title={`${label}（${items.length}）`}
      level={2}
      data-source={source}
      actions={
        <Button
          disabled={sender.pending || blocked}
          aria-label={`${label} の発見を実行`}
          onClick={() => void sender.run([{ id: discoverId, path: "/api/llm/models/discover", body: { source } }])}
        >
          発見を実行
        </Button>
      }
      description={
        discovery ? (
          <span data-discovery={discovery.ok ? "ok" : "failed"}>
            最終発見 {formatAbsolute(discovery.at)} / {discovery.ok ? "成功" : "失敗"} / {discovery.count} 件
            {discovery.error ? `（${discovery.error}）` : ""}
          </span>
        ) : (
          "まだ発見を実行していません"
        )
      }
    >
      <div className="mt-3 min-w-0 space-y-2">
        <ActionResultView result={sender.results[discoverId]} />
        {items.length === 0 ? (
          <p className="text-body text-muted-foreground">モデルがありません。「発見を実行」で取得します。</p>
        ) : (
          <Table aria-label={`${label} のモデル`}>
            <TableHeader>
              <TableRow>
                <TableHead>モデル</TableHead>
                <TableHead>表示名</TableHead>
                <TableHead>状態</TableHead>
                <TableHead>最終確認</TableHead>
                <TableHead>tier / deployment</TableHead>
                <TableHead>上書き</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {items.map((item) => (
                <ModelRow key={item.model_id} item={item} sender={sender} blocked={blocked} />
              ))}
            </TableBody>
          </Table>
        )}
      </div>
    </Section>
  );
}

/** 取得済みの catalog を節ごとに出す（hook を持たない。試験はここを描画する）。 */
export function ModelsView({ data, sender, blocked }: { data: LlmModelCatalog; sender: Sender; blocked: boolean }) {
  const groups = groupBySource(data);
  if (groups.length === 0)
    return (
      <p className="text-body text-muted-foreground">モデルがありません。「すべて発見」で各供給元から取得します。</p>
    );
  return (
    <div className="min-w-0 space-y-6">
      {groups.map((g) => (
        <SourceSection key={g.source} group={g} sender={sender} blocked={blocked} />
      ))}
    </div>
  );
}

export function ModelsScreen() {
  const query = useQuery({
    queryKey: modelKeys.list(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<LlmModelCatalog>("/api/llm/models", signal),
  });
  const sender = useActionResult(modelKeys.all);
  const denied = isDenied(sender.results);
  return (
    <ScreenFrame
      title="モデル"
      route="/models"
      breadcrumb={[{ label: "プロバイダ", link: { to: "/providers" } }, { label: "モデル" }]}
      description="供給元（LLM source）が公開するモデルの一覧です。発見で最新にし、無効化・tier の固定・別名を上書きできます。"
      actions={
        <Button
          variant="primary"
          disabled={sender.pending || denied}
          onClick={() => void sender.run([{ id: "discover:all", path: "/api/llm/models/discover", body: {} }])}
        >
          すべて発見
        </Button>
      }
    >
      <FetchFrame query={query} subject="モデル">
        {query.data && (
          <div className="min-w-0 space-y-4">
            {denied && (
              <p
                role="alert"
                className="border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
              >
                {deniedMessage("モデルの発見・上書き")}
              </p>
            )}
            <ActionResultView result={sender.results["discover:all"]} />
            <ModelsView data={query.data} sender={sender} blocked={denied} />
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
