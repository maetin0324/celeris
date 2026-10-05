import { useQuery } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";
import { apiGet } from "../../api/client";
import type {
  LlmSourcesView,
  ProviderCheckResponse,
  Providers,
  ProviderView,
  RoutingCatalogView,
  Tier,
} from "../../api/generated/types";
import { accountKeys, providerKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { ADAPTERS, buildProviderBody, TIERS } from "./providers-form";
import {
  adapterLabel,
  celerisModelsFor,
  providerLlmSourceDisplay,
  sourceKindLabel,
  sourceTierScopeNote,
  tiersResolvingTo,
} from "./providers-llm-source";
import { RoutingCatalogList } from "./routing-catalog-section";
import { DeploymentStateList } from "./routing-state-view";

type Sender = ReturnType<typeof useActionResult>;
const inputClass = "block w-full min-h-11 rounded border p-2";

function TierChecks({ value, onChange }: { value: Tier[]; onChange: (next: Tier[]) => void }) {
  return (
    <fieldset className="flex flex-wrap gap-3">
      <legend>tiers</legend>
      {TIERS.map((tier) => (
        <label key={tier} className="inline-flex min-h-11 items-center gap-1">
          <input
            type="checkbox"
            className="size-11"
            checked={value.includes(tier)}
            onChange={(e) => onChange(e.target.checked ? [...value, tier] : value.filter((t) => t !== tier))}
          />
          {tier}
        </label>
      ))}
    </fieldset>
  );
}

/** 実行枠の要約（ADR-0132 D6）: adapter・受ける tier・要求モデルと、使う LLM source。 */
export function ProviderSummary({ item }: { item: ProviderView }) {
  const source = providerLlmSourceDisplay(item.llm_source);
  const celerisModels = celerisModelsFor(item);
  return (
    <>
      <p className="text-sm break-words">
        {item.adapter} / concurrency {item.concurrency} / tiers {item.tiers.join(", ")}
        {item.model ? ` / model ${item.model}` : ""}
      </p>
      <p className="text-sm break-words">harness: {adapterLabel(item.adapter)}</p>
      <p className="text-sm break-words" data-source-ref={item.llm_source?.source ?? ""}>
        LLM source: {source.label}
        {source.origin ? `（${source.origin}）` : ""}
        {celerisModels.length > 0 ? ` / 使うモデル ${celerisModels.join(", ")}` : ""}
      </p>
      {source.note && <p className="text-sm break-words">{source.note}</p>}
    </>
  );
}

function ProviderCard({ item, sender }: { item: ProviderView; sender: Sender }) {
  const [concurrency, setConcurrency] = useState(String(item.concurrency));
  const [model, setModel] = useState(item.model ?? "");
  const [tiers, setTiers] = useState<Tier[]>(item.tiers);
  const base = `/api/providers/${encodeURIComponent(item.id)}`;
  const check = sender.results[`check:${item.id}`];
  const checked = check?.ok ? (check.response as ProviderCheckResponse | undefined) : undefined;
  const mutate = (kind: "patch" | "delete") => {
    const target =
      kind === "patch"
        ? {
            id: `patch:${item.id}`,
            path: base,
            method: "PATCH" as const,
            body: buildProviderBody({ concurrency, model, tiers }, "patch"),
          }
        : { id: `delete:${item.id}`, path: base, method: "DELETE" as const };
    void sender.run([target, { id: "reload", path: "/api/reload", body: {} }]);
  };
  return (
    <li className="min-w-0 rounded border p-3 space-y-2" aria-label={`プロバイダ ${item.id}`}>
      <h3 className="font-semibold break-words">{item.id}</h3>
      <ProviderSummary item={item} />
      {item.cooldown && (
        <p className="text-sm">
          cooldown: {item.cooldown.reason}（{item.cooldown.until} まで）
        </p>
      )}
      {item.last_check && (
        <p className="text-sm">
          前回の確認: {item.last_check.result}（{item.last_check.at}）
        </p>
      )}
      <div className="grid gap-2 sm:grid-cols-2">
        <label className="block">
          concurrency
          <input
            className={inputClass}
            type="number"
            min={0}
            value={concurrency}
            onChange={(e) => setConcurrency(e.target.value)}
          />
        </label>
        <label className="block">
          model
          <input className={inputClass} value={model} onChange={(e) => setModel(e.target.value)} />
        </label>
      </div>
      <TierChecks value={tiers} onChange={setTiers} />
      <div className="flex flex-wrap gap-2">
        <Button disabled={sender.pending} onClick={() => mutate("patch")}>
          変更を保存
        </Button>
        <Button
          disabled={sender.pending}
          onClick={() => void sender.run([{ id: `check:${item.id}`, path: `${base}/check`, body: {} }])}
        >
          接続を確認
        </Button>
        <Button
          disabled={sender.pending}
          onClick={() => {
            if (window.confirm(`プロバイダ ${item.id} を削除しますか`)) mutate("delete");
          }}
        >
          削除
        </Button>
      </div>
      <ActionResultView result={sender.results[`patch:${item.id}`]} />
      <ActionResultView result={sender.results[`delete:${item.id}`]} />
      <ActionResultView result={check} />
      {checked && (
        <p role="status">
          確認結果: {checked.result}
          {checked.detail ? `（${checked.detail}）` : ""}
        </p>
      )}
    </li>
  );
}

function CreateForm({ sender }: { sender: Sender }) {
  const [id, setId] = useState("");
  const [adapter, setAdapter] = useState<string>("codex");
  const [concurrency, setConcurrency] = useState("");
  const [model, setModel] = useState("");
  const [tiers, setTiers] = useState<Tier[]>([]);
  return (
    <form
      className="rounded-lg border border-neutral-300 p-3 space-y-2 min-w-0"
      aria-label="プロバイダを追加"
      onSubmit={(event) => {
        event.preventDefault();
        void sender
          .run([
            {
              id: `create:${id.trim()}`,
              path: "/api/providers",
              body: buildProviderBody({ id, adapter, concurrency, model, tiers }, "create"),
            },
            { id: "reload", path: "/api/reload", body: {} },
          ])
          .then((outcomes) => {
            if (outcomes[0]?.ok) setId("");
          });
      }}
    >
      <h2 className="text-lg font-semibold">プロバイダを追加</h2>
      <div className="grid gap-2 sm:grid-cols-2">
        <label className="block">
          新規 id
          <input className={inputClass} required value={id} onChange={(e) => setId(e.target.value)} />
        </label>
        <label className="block">
          adapter
          <select className={inputClass} value={adapter} onChange={(e) => setAdapter(e.target.value)}>
            {ADAPTERS.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        </label>
        <label className="block">
          新規 concurrency
          <input
            className={inputClass}
            type="number"
            min={0}
            value={concurrency}
            onChange={(e) => setConcurrency(e.target.value)}
          />
        </label>
        <label className="block">
          新規 model
          <input className={inputClass} value={model} onChange={(e) => setModel(e.target.value)} />
        </label>
      </div>
      <TierChecks value={tiers} onChange={setTiers} />
      <Button type="submit" disabled={sender.pending}>
        追加
      </Button>
      <ActionResultView result={sender.results[`create:${id.trim()}`]} />
    </form>
  );
}

/** 「LLM source」節の中身: 種類・ID・到達性・今どの celeris/<tier> の解決先か。 */
export function LlmSourceList({ data }: { data: LlmSourcesView }) {
  if (data.sources.length === 0) return <p>供給元がありません。</p>;
  return (
    <ul className="space-y-2">
      {data.sources.map((source) => {
        const resolving = tiersResolvingTo(source.id, data.celeris_tiers);
        const scope = sourceTierScopeNote(source.kind);
        const reach = source.reachable === false ? "届かない" : source.reachable ? "到達可" : "未確認";
        return (
          <li key={source.id} className="rounded border p-2 text-sm break-words" aria-label={`LLM source ${source.id}`}>
            <span className="font-semibold">{source.id}</span>（{sourceKindLabel(source.kind)}）/{" "}
            {source.enabled ? "有効" : "無効"} / {reach}
            {resolving.length > 0 && ` / 解決先: ${resolving.join(", ")}`}
            {scope && <span className="block">{scope}</span>}
            {source.deployments && source.deployments.length > 0 && (
              <div className="mt-2">
                <DeploymentStateList states={source.deployments} />
              </div>
            )}
          </li>
        );
      })}
    </ul>
  );
}

/** 画面の節立て: adapter / harness の実行枠と LLM source を別の見出しで並べる。 */
export function ProvidersSections({
  count,
  adapters,
  sources,
}: {
  count: number;
  adapters: ReactNode;
  sources: ReactNode;
}) {
  return (
    <>
      <section className="space-y-2 min-w-0" aria-label="プロバイダ一覧">
        <h2 className="text-lg font-semibold">adapter / harness の実行枠（{count}）</h2>
        <p className="text-sm">
          道具（claude-code・codex・acp・paperqa・langmem・ldr など）ごとの実行枠です。各枠の LLM source
          は、その道具がどの供給元のモデルを使うかを示します。celeris/&lt;tier&gt; は実行時に proxy
          が供給元を選ぶ抽象モデルです。
        </p>
        {adapters}
      </section>
      <section className="space-y-2 min-w-0" aria-label="供給元の一覧">
        <h2 className="text-lg font-semibold">LLM source</h2>
        <p className="text-sm">
          モデルを供給するもの（Claude OAuth・Codex OAuth・OpenAI 互換）です。Qwen などの OpenAI 互換源は celeris/cheap
          にだけ使われ、frontier / standard は Claude / GPT から選ばれます。
        </p>
        {sources}
      </section>
    </>
  );
}

export function ProvidersScreen() {
  const query = useQuery({
    queryKey: providerKeys.list(),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<Providers>("/api/providers", signal),
  });
  const llm = useQuery({
    queryKey: accountKeys.list({ section: "llm" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<LlmSourcesView>("/api/llm/sources", signal),
  });
  const catalog = useQuery({
    queryKey: accountKeys.list({ section: "routing-catalog" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<RoutingCatalogView>("/api/llm/routing/catalog", signal),
  });
  const sender = useActionResult(providerKeys.all);
  const reload = sender.results.reload;
  return (
    <ScreenFrame title="プロバイダ" route="/providers">
      <FetchFrame query={query}>
        {query.data && (
          <div className="space-y-4 min-w-0">
            <ProvidersSections
              count={query.data.items.length}
              adapters={
                query.data.items.length === 0 ? (
                  <p>プロバイダがありません。</p>
                ) : (
                  <ul className="space-y-3">
                    {query.data.items.map((item) => (
                      <ProviderCard key={item.id} item={item} sender={sender} />
                    ))}
                  </ul>
                )
              }
              sources={<FetchFrame query={llm}>{llm.data && <LlmSourceList data={llm.data} />}</FetchFrame>}
            />
            <section className="space-y-2 min-w-0" aria-label="モデルの catalog">
              <h2 className="text-lg font-semibold">モデルの catalog</h2>
              <p className="text-sm">
                routing が参照する model と deployment（供給元 ×
                model）です。設定に無い価格・品質・能力は「不明」と表示します。
              </p>
              <FetchFrame query={catalog}>{catalog.data && <RoutingCatalogList data={catalog.data} />}</FetchFrame>
            </section>
            <CreateForm sender={sender} />
            {reload && !reload.ok && <p role="alert">設定の reload に失敗しました。</p>}
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
