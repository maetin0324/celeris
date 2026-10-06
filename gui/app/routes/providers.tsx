import { useState } from "react";
import { data, type FetcherWithComponents, isRouteErrorResponse, useFetcher } from "react-router";
import type { ProviderActionResult } from "~/celeris/action-types";
import { type CelerisClient, getCelerisClient } from "~/celeris/client.server";
import { CelerisError, type CelerisRouteErrorData, celerisErrorResponse } from "~/celeris/errors";
import { formString } from "~/celeris/forms";
import {
  buildProviderCreateInput,
  buildProviderPatchInput,
  checkProvider,
  createProvider,
  deleteProvider,
  patchProvider,
} from "~/celeris/providers-admin.server";
import type {
  CatalogDeploymentView,
  CatalogModelView,
  LlmSourcesView,
  LlmSourceView,
  Providers,
  ProviderView,
  RoutingCatalogView,
  Tier,
} from "~/celeris/types";
import { ProviderActionFlash } from "~/components/Flash";
import { HelpLink } from "~/components/HelpLink";
import { RouteRecovery } from "~/components/RouteRecovery";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { checkboxClass, chipLabelClass, hintClass, inputClass, labelClass, selectClass } from "~/components/ui/form";
import { Icon } from "~/components/ui/Icon";
import { Alert, DataItem, EmptyState, Mono, PageHeader, SectionTitle } from "~/components/ui/misc";
import type { Tone } from "~/components/ui/tone";
import {
  llmSourceOriginLabel,
  providerLlmSourceDisplay,
  sourceKindLabel,
  sourceLabel,
  sourceStatusWord,
  sourceTierScopeNote,
  tiersResolvingTo,
} from "~/lib/llm-sources";
import { isTransientStatus } from "~/lib/recovery";
import { revalidateAfterActionErrors } from "~/lib/revalidate";
import {
  billingLabel,
  capabilityLabel,
  contextLimitsLabel,
  deploymentPricingLabel,
  pricingLabel,
  qualityLabel,
} from "~/lib/routing-catalog";
import { formatDuration, secondsBetween } from "~/lib/time-delta";
import { CelerisBanner } from "~/root";
import type { Route } from "./+types/providers";

/**
 * `/providers`（プロバイダ画面、docs/DESIGN.md §4.5、ADR-GUI-0012 D2）の loader が返すデータ。
 * `Providers.items[]`（`ProviderView`）をそのまま表にする（docs/adr/0007 D7）。
 * cooldown の残り時間だけは celeris が値を返さないので、BFF 自身がリクエスト前後に取った
 * `fetchedAt`（ISO 文字列）を基準に画面側で減算する（docs/adr/0007 D3）。
 * ADR-0132 D6: `Providers.items[]` は adapter / harness の実行枠で、モデルの供給元（LLM source）は
 * 別の節に出す。供給元の正本は `GET /llm/sources`（`/accounts` の「LLM source」節と同じ）。
 * `[llm_proxy]` が無効（409 `llm_proxy_unavailable`）・取得失敗のときは `llmSources` を `null` にし、
 * 実行枠の一覧だけを出す（供給元の節が取れないことで画面全体を落とさない）。
 * model routing Phase 1（ADR 2026-10-04-multi-objective-model-routing §9）: `GET /llm/routing/catalog` の
 * model と deployment を別の節に出す。取れないときは `routingCatalog` を `null` にする（同じ扱い）。
 */
export interface ProvidersData {
  providers: Providers;
  llmSources: LlmSourcesView | null;
  llmSourcesUnavailable: boolean;
  routingCatalog: RoutingCatalogView | null;
  routingCatalogUnavailable: boolean;
  fetchedAt: string;
}

/** `GET /providers`・`GET /llm/sources`・`GET /llm/routing/catalog` を呼ぶ。応答はそのまま返す（派生の集計はしない）。 */
export async function loadProviders(client: CelerisClient, request: Request): Promise<ProvidersData> {
  const providers = await client.get<Providers>("/providers", { signal: request.signal });
  let llmSources: LlmSourcesView | null = null;
  let llmSourcesUnavailable = false;
  try {
    llmSources = await client.get<LlmSourcesView>("/llm/sources", { signal: request.signal });
  } catch (e) {
    if (e instanceof CelerisError && e.status === 409 && e.code === "llm_proxy_unavailable") {
      llmSourcesUnavailable = true;
    }
  }
  let routingCatalog: RoutingCatalogView | null = null;
  let routingCatalogUnavailable = false;
  try {
    routingCatalog = await client.get<RoutingCatalogView>("/llm/routing/catalog", { signal: request.signal });
  } catch (e) {
    if (e instanceof CelerisError && e.status === 409 && e.code === "llm_proxy_unavailable") {
      routingCatalogUnavailable = true;
    }
  }
  const fetchedAt = new Date().toISOString();
  return { providers, llmSources, llmSourcesUnavailable, routingCatalog, routingCatalogUnavailable, fetchedAt };
}

// 409 / 422 の action 後も再検証する（docs/adr/0005 D2）。追加・変更・削除の後の一覧更新にも要る。
export const shouldRevalidate = revalidateAfterActionErrors;

export async function loader({ request }: Route.LoaderArgs): Promise<ProvidersData> {
  try {
    return await loadProviders(getCelerisClient(), request);
  } catch (e) {
    throw celerisErrorResponse(e);
  }
}

export function meta(_: Route.MetaArgs) {
  return [{ title: "プロバイダ - Celeris" }];
}

/**
 * 追加・編集・削除・疎通確認（ADR-GUI-0012 D2）。管理系はすべて `providers-admin.server.ts` に任せ、
 * ここはフォームの `intent` を対応する呼び出しに写すだけ（GUI 側で判断ロジックは持たない）。
 */
export async function action({ request }: Route.ActionArgs) {
  const form = await request.formData();
  const intent = form.get("intent");
  const client = getCelerisClient();
  let result: ProviderActionResult;
  switch (intent) {
    case "create":
      result = await createProvider(client, buildProviderCreateInput(form), request.signal);
      break;
    case "patch":
      result = await patchProvider(client, formString(form, "id") ?? "", buildProviderPatchInput(form), request.signal);
      break;
    case "delete":
      result = await deleteProvider(client, formString(form, "id") ?? "", request.signal);
      break;
    case "check":
      result = await checkProvider(client, formString(form, "id") ?? "", request.signal);
      break;
    default:
      throw data({ error: `unknown intent: ${String(intent)}` }, { status: 400 });
  }
  return data(result, { status: result.op.ok ? 200 : result.op.error.status });
}

const TIER_OPTIONS: Tier[] = ["frontier", "standard", "cheap"];
// ADR-0026 D7: "acp"（opencode 等の ACP エージェント経由の OpenAI 互換 LLM）を追加。
// ADR-0027 D3: "paperqa"（関連研究調査、PaperQA2）を追加。
// `command`/`args` はこのフォームには無い（管理 API から設定できない。providers.d/<id>.toml を人が直接編集する）。
export const ADAPTER_OPTIONS = ["fake", "claude-code", "codex", "acp", "paperqa", "local-deep-research"] as const;

export default function ProvidersPage({ loaderData }: Route.ComponentProps) {
  const { providers, llmSources, llmSourcesUnavailable, routingCatalog, routingCatalogUnavailable, fetchedAt } =
    loaderData;
  // celeris の SSE（daemon tick）で自動再検証が走るたびに loader の再取得が起きる（`useCelerisStream`）。
  // 通常の `<Form>` の `actionData` はその再検証のたびに消えてしまう（React Router の仕様）ので、
  // 追加・編集・削除・疎通確認は 1 つの `useFetcher()` にまとめ、その `fetcher.data` を表示する
  // （fetcher の状態は revalidate() の影響を受けない。ADR-GUI-0012 D2）。
  const fetcher = useFetcher<ProviderActionResult>();
  const submitting = fetcher.state !== "idle";
  const [adapter, setAdapter] = useState("codex");

  return (
    <div className="space-y-8 [&_button]:min-h-11 [&_summary]:min-h-11">
      <PageHeader
        icon="cpu"
        title={
          <>
            プロバイダ
            <HelpLink anchor="screens" label="画面ごとの説明" />
          </>
        }
        description="adapter / harness の実行枠（claude-code・codex・acp・paperqa・langmem・ldr など）と、それが使う LLM source（Claude OAuth・Codex OAuth・OpenAI 互換の Qwen など）を分けて示します。ログイン・認証情報はアカウント画面で管理します。"
      />

      <ProviderActionFlash result={fetcher.data} />

      <section aria-labelledby="providers-heading" data-testid="providers-section" className="space-y-4">
        <SectionTitle icon="cpu" id="providers-heading" count={providers.items.length}>
          adapter / harness の実行枠
        </SectionTitle>
        <p className={hintClass}>
          道具（adapter）ごとの実行枠です。各枠の「LLM source」は、その道具がどの供給元のモデルを使うかを示します。
          <Mono className="text-xs">celeris/&lt;tier&gt;</Mono> は実行時に proxy が供給元を選ぶ抽象モデルです。
        </p>
        {providers.items.length === 0 ? (
          <EmptyState icon="cpu" title="プロバイダがありません" />
        ) : (
          <div className="grid items-start gap-4 xl:grid-cols-2">
            {providers.items.map((item) => (
              <ProviderCard key={item.id} item={item} fetchedAt={fetchedAt} fetcher={fetcher} submitting={submitting} />
            ))}
          </div>
        )}
      </section>

      <LlmSourcesOverview llmSources={llmSources} unavailable={llmSourcesUnavailable} />

      <RoutingCatalogOverview catalog={routingCatalog} unavailable={routingCatalogUnavailable} />

      <section aria-labelledby="provider-add-heading" className="space-y-4">
        <SectionTitle icon="plus" id="provider-add-heading">
          プロバイダを追加
        </SectionTitle>
        <Card>
          <CardHeader
            icon="plus"
            title="新規プロバイダ"
            description="providers.d/<id>.toml を書き、続けて reload します（反映は次の tick から）。"
          />
          <CardBody>
            <fetcher.Form method="post" data-testid="provider-add-form" className="space-y-4">
              <input type="hidden" name="intent" value="create" />
              <div className="grid grid-cols-2 gap-4 sm:grid-cols-3">
                <div>
                  <label htmlFor="add-id" className={labelClass}>
                    id
                  </label>
                  <input id="add-id" name="id" type="text" required className={cnField(inputClass)} />
                </div>
                <div>
                  <label htmlFor="add-adapter" className={labelClass}>
                    adapter
                  </label>
                  <select
                    id="add-adapter"
                    name="adapter"
                    value={adapter}
                    onChange={(e) => setAdapter(e.target.value)}
                    className={cnField(selectClass)}
                  >
                    {ADAPTER_OPTIONS.map((a) => (
                      <option key={a} value={a}>
                        {a === "codex" ? "GPT (Codex)" : a === "claude-code" ? "Claude" : a}
                      </option>
                    ))}
                  </select>
                </div>
                <div>
                  <label htmlFor="add-concurrency" className={labelClass}>
                    concurrency
                  </label>
                  <input
                    id="add-concurrency"
                    name="concurrency"
                    type="number"
                    min={0}
                    className={cnField(inputClass)}
                  />
                </div>
                <div className="col-span-2 sm:col-span-3">
                  <label htmlFor="add-model" className={labelClass}>
                    共通モデル（階層別設定が無効な場合）
                  </label>
                  <input id="add-model" name="model" type="text" className={cnField(inputClass)} />
                </div>
                <fieldset className="col-span-2 sm:col-span-3">
                  <legend className={labelClass}>tiers</legend>
                  <div className="mt-1.5 flex flex-wrap gap-2">
                    {TIER_OPTIONS.map((tier) => (
                      <label key={tier} className={chipLabelClass}>
                        <input type="checkbox" name="tiers" value={tier} className={checkboxClass} />
                        {tier}
                      </label>
                    ))}
                  </div>
                </fieldset>
                <div className="col-span-2 sm:col-span-3">
                  <label htmlFor="add-account-pool" className={chipLabelClass}>
                    <input id="add-account-pool" name="account_pool" type="checkbox" className={checkboxClass} />
                    account_pool（adapter が claude-code か codex のときだけ意味があります。[accounts]
                    のそのアダプタのプールから残量でアカウントを選びます）
                  </label>
                </div>
              </div>
              <ModelRoutingFields key={adapter} adapter={adapter} prefix="add" />
              <Button type="submit" variant="primary" disabled={submitting} data-testid="provider-add-submit">
                <Icon name="plus" />
                追加
              </Button>
            </fetcher.Form>
          </CardBody>
        </Card>
      </section>
    </div>
  );
}

/** 実行枠の `llm_source` 参照（ADR-0132 D6）。`celeris` は固定の Qwen ではなく実行時の選択と示す。 */
export function ProviderLlmSourceItem({ item }: { item: ProviderView }) {
  const display = providerLlmSourceDisplay(item.llm_source);
  const origin = llmSourceOriginLabel(display.origin);
  return (
    <DataItem label="LLM source" wide>
      <span data-testid="provider-llm-source" data-source-ref={item.llm_source?.source ?? ""}>
        {display.label}
      </span>
      {origin && (
        <Badge tone="neutral" className="ml-2" data-testid="provider-llm-source-origin">
          {origin}
        </Badge>
      )}
      {display.note && (
        <span className="block text-sm text-fg-subtle" data-testid="provider-llm-source-note">
          {display.note}
        </span>
      )}
    </DataItem>
  );
}

/**
 * 「LLM source」節（ADR-0132 D6）: `GET /llm/sources` の供給元を、実行枠とは別に種類・ID・到達性・
 * 今どの `celeris/<tier>` の解決先かで示す。残量・cooldown などの詳細は `/accounts#llm-sources`。
 */
export function LlmSourcesOverview({
  llmSources,
  unavailable,
}: {
  llmSources: LlmSourcesView | null;
  unavailable: boolean;
}) {
  return (
    <section aria-labelledby="provider-llm-sources-heading" data-testid="provider-llm-sources" className="space-y-4">
      <SectionTitle icon="server" id="provider-llm-sources-heading" count={llmSources?.sources.length}>
        LLM source
      </SectionTitle>
      <p className={hintClass}>
        モデルを供給するものです。Qwen などの OpenAI 互換源は <Mono className="text-xs">celeris/cheap</Mono>{" "}
        にだけ使われ、frontier / standard は Claude / GPT から選ばれます。残量と cooldown は{" "}
        <a href="/accounts#llm-sources" className="inline-flex min-h-11 items-center underline">
          アカウント画面
        </a>
        で確かめます。
      </p>
      {!llmSources ? (
        <EmptyState
          icon="server"
          title={unavailable ? "[llm_proxy] が設定されていません" : "LLM source を取得できませんでした"}
        />
      ) : llmSources.sources.length === 0 ? (
        <EmptyState icon="server" title="供給元がありません" />
      ) : (
        <div className="grid items-start gap-4 xl:grid-cols-2">
          {llmSources.sources.map((source) => (
            <LlmSourceSummaryCard key={source.id} source={source} llmSources={llmSources} />
          ))}
        </div>
      )}
    </section>
  );
}

/**
 * 「model routing catalog」節（ADR 2026-10-04-multi-objective-model-routing §9・§10 Phase 1）:
 * `GET /llm/routing/catalog` の model（能力・context・品質・単価）と deployment（source と model の対応・
 * 課金種別・lane・単価の上書き）を別の一覧に出す。欠測は「不明」で、0 円・品質保証に見せない。
 */
export function RoutingCatalogOverview({
  catalog,
  unavailable,
}: {
  catalog: RoutingCatalogView | null;
  unavailable: boolean;
}) {
  return (
    <section
      aria-labelledby="provider-routing-catalog-heading"
      data-testid="provider-routing-catalog"
      className="space-y-4"
    >
      <SectionTitle icon="server" id="provider-routing-catalog-heading">
        model routing catalog
      </SectionTitle>
      <p className={hintClass}>
        model（モデルそのもの）と deployment（どの LLM source でその model を出すか）を分けて示します。
        値が設定に無いものは「不明」と出し、0 円や品質の保証とはみなしません。
      </p>
      {!catalog ? (
        <EmptyState
          icon="server"
          title={unavailable ? "[llm_proxy] が設定されていません" : "routing catalog を取得できませんでした"}
        />
      ) : (
        <div className="space-y-4">
          <dl className="grid grid-cols-2 gap-x-4 gap-y-3 text-sm">
            <DataItem label="mode">
              <span data-testid="routing-catalog-mode">{catalog.mode}</span>
            </DataItem>
            <DataItem label="catalog version">
              <Mono className="break-all text-xs">{catalog.catalog_version}</Mono>
            </DataItem>
          </dl>
          {catalog.warnings.length > 0 && (
            <ul className="space-y-1 text-sm" data-testid="routing-catalog-warnings">
              {catalog.warnings.map((w) => (
                <li key={w} className="break-words">
                  {w}
                </li>
              ))}
            </ul>
          )}
          <h3 className="text-sm font-semibold">model（{catalog.models.length}）</h3>
          {catalog.models.length === 0 ? (
            <EmptyState icon="server" title="model がありません" />
          ) : (
            <div className="grid items-start gap-4 xl:grid-cols-2">
              {catalog.models.map((model) => (
                <CatalogModelCard key={model.id} model={model} />
              ))}
            </div>
          )}
          <h3 className="text-sm font-semibold">deployment（{catalog.deployments.length}）</h3>
          {catalog.deployments.length === 0 ? (
            <EmptyState icon="server" title="deployment がありません" />
          ) : (
            <div className="grid items-start gap-4 xl:grid-cols-2">
              {catalog.deployments.map((deployment) => (
                <CatalogDeploymentCard key={deployment.id} deployment={deployment} />
              ))}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function CatalogModelCard({ model }: { model: CatalogModelView }) {
  return (
    <Card data-testid="routing-catalog-model" data-model-id={model.id} className="min-w-0">
      <CardHeader
        icon="cpu"
        title={<span className="break-all">{model.id}</span>}
        description={
          <Mono className="break-all text-xs text-fg-subtle">
            {model.family} / {model.revision}
          </Mono>
        }
        actions={<Badge tone="neutral">model</Badge>}
      />
      <CardBody>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 text-sm">
          <DataItem label="単価">
            <span data-testid="routing-catalog-model-pricing">{pricingLabel(model.pricing)}</span>
          </DataItem>
          <DataItem label="品質">
            <span data-testid="routing-catalog-model-quality">{qualityLabel(model.quality)}</span>
          </DataItem>
          <DataItem label="context">
            <span data-testid="routing-catalog-model-context">{contextLimitsLabel(model.context_limits)}</span>
          </DataItem>
          <DataItem label="tools">
            <span data-testid="routing-catalog-model-tools">{capabilityLabel(model.capabilities.tools)}</span>
          </DataItem>
        </dl>
      </CardBody>
    </Card>
  );
}

function CatalogDeploymentCard({ deployment }: { deployment: CatalogDeploymentView }) {
  return (
    <Card data-testid="routing-catalog-deployment" data-deployment-id={deployment.id} className="min-w-0">
      <CardHeader
        icon="server"
        title={<span className="break-all">{deployment.id}</span>}
        description={<Mono className="break-all text-xs text-fg-subtle">{deployment.upstream_model}</Mono>}
        actions={<Badge tone="neutral">deployment</Badge>}
      />
      <CardBody>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 text-sm">
          <DataItem label="LLM source">
            <span className="break-all" data-testid="routing-catalog-deployment-source">
              {deployment.source_ref}
            </span>
          </DataItem>
          <DataItem label="model">
            <span className="break-all" data-testid="routing-catalog-deployment-model">
              {deployment.model_profile_id}
            </span>
          </DataItem>
          <DataItem label="課金">
            <span data-testid="routing-catalog-deployment-billing">{billingLabel(deployment.billing)}</span>
          </DataItem>
          <DataItem label="lane">
            <span>{deployment.allowed_lanes.length > 0 ? deployment.allowed_lanes.join(", ") : "-"}</span>
          </DataItem>
          <DataItem label="単価">
            <span data-testid="routing-catalog-deployment-pricing">{deploymentPricingLabel(deployment)}</span>
          </DataItem>
        </dl>
      </CardBody>
    </Card>
  );
}

function LlmSourceSummaryCard({ source, llmSources }: { source: LlmSourceView; llmSources: LlmSourcesView }) {
  const statusWord = sourceStatusWord(source);
  const tone: Tone = statusWord === "reachable" ? "success" : statusWord === "unreachable" ? "danger" : "neutral";
  const tiers = tiersResolvingTo(source.id, llmSources.celeris_tiers);
  const scope = sourceTierScopeNote(source.kind);
  return (
    <Card data-testid="provider-llm-source-row" data-source-id={source.id} className="min-w-0">
      <CardHeader
        icon="server"
        tone={tone}
        title={<span className="break-all">{sourceLabel(source.id)}</span>}
        description={<Mono className="break-all text-xs text-fg-subtle">{source.id}</Mono>}
        actions={
          <Badge tone={tone} dot data-testid="provider-llm-source-status">
            {statusWord}
          </Badge>
        }
      />
      <CardBody className="space-y-2">
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 text-sm">
          <DataItem label="種類">
            <span data-testid="provider-llm-source-kind">{sourceKindLabel(source.kind)}</span>
          </DataItem>
          <DataItem label="今の解決先の tier">
            <span data-testid="provider-llm-source-tiers">
              {tiers.length > 0 ? tiers.map((t) => `celeris/${t}`).join(", ") : "-"}
            </span>
          </DataItem>
        </dl>
        {scope && (
          <p className="text-sm text-fg-subtle" data-testid="provider-llm-source-scope">
            {scope}
          </p>
        )}
      </CardBody>
    </Card>
  );
}

function cnField(base: string, extra?: string): string {
  return extra ? `${base} mt-1.5 min-h-11 min-w-0 ${extra}` : `${base} mt-1.5 min-h-11 min-w-0 w-full`;
}

function ProviderCard({
  item,
  fetchedAt,
  fetcher,
  submitting,
}: {
  item: ProviderView;
  fetchedAt: string;
  fetcher: FetcherWithComponents<ProviderActionResult>;
  submitting: boolean;
}) {
  const tone: Tone = item.cooldown ? "warning" : "success";

  return (
    <Card data-testid="provider-row" data-provider-id={item.id} className="min-w-0 hover:shadow-md">
      <CardHeader
        className="flex-wrap [&>div:last-child]:w-full sm:[&>div:last-child]:w-auto"
        icon="cpu"
        tone={tone}
        title={<Mono className="text-sm font-semibold text-fg">{item.id}</Mono>}
        description={<span data-testid="provider-adapter">adapter: {item.adapter}</span>}
        actions={
          <>
            {item.account_pool && (
              <Badge tone="teal" data-testid="provider-account-pool">
                account_pool
              </Badge>
            )}
            <Badge tone={tone} dot pulse={!!item.cooldown}>
              {item.cooldown ? "cooldown" : "稼働枠あり"}
            </Badge>
          </>
        }
      />
      <CardBody className="space-y-4">
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 text-sm sm:grid-cols-3">
          <DataItem label="tiers">
            <div className="flex flex-wrap gap-1">
              {item.tiers.map((tier) => (
                <Badge key={tier} tone="neutral">
                  {tier}
                </Badge>
              ))}
            </div>
          </DataItem>
          <DataItem label="concurrency">{item.concurrency}</DataItem>
          <ProviderLlmSourceItem item={item} />
          <DataItem label="旧設定の共通モデル">{item.model ?? "-"}</DataItem>
          <DataItem label="認証アカウント" wide>
            {item.account_pool
              ? `${item.adapter} / ${item.account_id || "プールから自動選択"}`
              : "既存の認証設定を保持"}
          </DataItem>
          <DataItem label="階層 → 実行モデル" wide>
            <div className="space-y-2 break-all">
              {TIER_OPTIONS.map((tier) => {
                const binding = item.tier_models?.[tier];
                return (
                  <p key={tier}>
                    {tier}:{" "}
                    {binding
                      ? `${binding.name} → ${binding.unavailable_reason || binding.model_id || "未対応: 実行モデルID未設定"}`
                      : Object.keys(item.tier_models ?? {}).length > 0
                        ? "未対応: 階層の対応なし"
                        : `旧設定 (${item.model || "アダプター既定"})`}
                  </p>
                );
              })}
            </div>
          </DataItem>
          <DataItem label="in_use">{item.in_use == null ? "-" : item.in_use}</DataItem>
          <DataItem label="env_keys（キー名のみ）" wide>
            {item.env_keys.length > 0 ? (
              <div className="flex flex-wrap gap-1">
                {item.env_keys.map((key) => (
                  <Mono key={key} className="rounded bg-surface-2 px-1.5 py-0.5">
                    {key}
                  </Mono>
                ))}
              </div>
            ) : (
              "-"
            )}
          </DataItem>
        </dl>

        <dl className="grid grid-cols-3 gap-x-4 gap-y-3 text-sm sm:grid-cols-4">
          <DataItem label="runs">{item.stats.runs}</DataItem>
          <DataItem label="done">
            <span data-testid="provider-done">{item.stats.done}</span>
          </DataItem>
          <DataItem label="question">{item.stats.question}</DataItem>
          <DataItem label="error">{item.stats.error}</DataItem>
          <DataItem label="requeue">
            <span data-testid="provider-requeue">{item.stats.requeue}</span>
          </DataItem>
          <DataItem label="lease_expired">{item.stats.lease_expired}</DataItem>
          <DataItem label="tokens (input+output)" wide>
            <span data-testid="provider-tokens" className="tabular-nums">
              {item.stats.input_tokens + item.stats.output_tokens}
            </span>
          </DataItem>
        </dl>

        {/* ADR-0022 D2: 直近の疎通確認。手動で `POST /providers/{id}/check` を叩いたときだけ入り、
            celeris を再起動すると消える（メモリ上の観測値）。 */}
        <Alert
          tone={item.last_check ? (item.last_check.result === "ok" ? "success" : "danger") : "neutral"}
          title="最後の疎通確認"
        >
          <span data-testid="provider-last-check">
            {item.last_check ? `${item.last_check.result}（${item.last_check.at}）` : "未確認"}
          </span>
        </Alert>

        {item.cooldown && (
          <Alert tone="warning" title="cooldown">
            <dl className="grid grid-cols-2 gap-x-4 gap-y-2 sm:grid-cols-3">
              <DataItem label="reason">
                <span data-testid="provider-cooldown-reason">{item.cooldown.reason}</span>
              </DataItem>
              <DataItem label="remaining">
                <span data-testid="provider-cooldown-remaining" className="tabular-nums">
                  {formatDuration(secondsBetween(fetchedAt, item.cooldown.until))}
                </span>
              </DataItem>
              <DataItem label="until">
                <span className="text-fg-subtle">{item.cooldown.until}</span>
              </DataItem>
            </dl>
          </Alert>
        )}

        <div className="flex flex-wrap items-center gap-2 border-t border-border pt-3">
          <fetcher.Form method="post">
            <input type="hidden" name="intent" value="check" />
            <input type="hidden" name="id" value={item.id} />
            <Button type="submit" variant="secondary" size="sm" disabled={submitting} data-testid="provider-check">
              <Icon name="activity" />
              疎通確認
            </Button>
          </fetcher.Form>

          <details className="group min-w-0 w-full">
            <summary className="inline-flex h-8 cursor-pointer list-none items-center gap-1.5 rounded-lg border border-border bg-surface px-3 text-sm text-fg shadow-xs hover:bg-surface-2">
              <Icon name="settings" className="size-4" />
              編集
            </summary>
            <fetcher.Form method="post" className="mt-3 space-y-3 rounded-lg border border-border bg-surface-2/40 p-3">
              <input type="hidden" name="intent" value="patch" />
              <input type="hidden" name="id" value={item.id} />
              <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
                <div>
                  <label className={labelClass} htmlFor={`edit-concurrency-${item.id}`}>
                    concurrency
                  </label>
                  <input
                    id={`edit-concurrency-${item.id}`}
                    name="concurrency"
                    type="number"
                    min={0}
                    defaultValue={item.concurrency}
                    className={cnField(inputClass)}
                  />
                </div>
                <div className="col-span-2">
                  <label className={labelClass} htmlFor={`edit-model-${item.id}`}>
                    共通モデル（階層別設定が無効な場合）
                  </label>
                  <input
                    id={`edit-model-${item.id}`}
                    name="model"
                    type="text"
                    defaultValue={item.model ?? ""}
                    className={cnField(inputClass)}
                  />
                </div>
                <fieldset className="col-span-2 sm:col-span-3">
                  <legend className={labelClass}>tiers</legend>
                  <div className="mt-1.5 flex flex-wrap gap-2">
                    {TIER_OPTIONS.map((tier) => (
                      <label key={tier} className={chipLabelClass}>
                        <input
                          type="checkbox"
                          name="tiers"
                          value={tier}
                          defaultChecked={item.tiers.includes(tier)}
                          className={checkboxClass}
                        />
                        {tier}
                      </label>
                    ))}
                  </div>
                </fieldset>
                <div className="col-span-2 sm:col-span-3">
                  <label className={chipLabelClass} htmlFor={`edit-account-pool-${item.id}`}>
                    <input
                      id={`edit-account-pool-${item.id}`}
                      name="account_pool"
                      type="checkbox"
                      defaultChecked={item.account_pool}
                      className={checkboxClass}
                    />
                    account_pool（adapter が claude-code か codex
                    のときだけ意味があります。そのアダプタのプールから選びます）
                  </label>
                </div>
              </div>
              <ModelRoutingFields adapter={item.adapter} prefix={item.id} item={item} />
              <Button type="submit" variant="primary" size="sm" disabled={submitting} data-testid="provider-edit">
                <Icon name="check" />
                保存
              </Button>
            </fetcher.Form>
          </details>

          <details className="group">
            <summary className="inline-flex h-8 cursor-pointer list-none items-center gap-1.5 rounded-lg border border-danger-border bg-danger-soft px-3 text-sm text-danger-soft-fg shadow-xs hover:bg-danger hover:text-white">
              <Icon name="xCircle" className="size-4" />
              削除
            </summary>
            <fetcher.Form method="post" className="mt-3 rounded-lg border border-danger-border bg-danger-soft/40 p-3">
              <input type="hidden" name="intent" value="delete" />
              <input type="hidden" name="id" value={item.id} />
              <p className="mb-2 text-sm text-fg-muted">
                本当に <span className="font-mono">{item.id}</span> を削除しますか？（providers.d/{item.id}.toml
                を削除して reload します）
              </p>
              <Button type="submit" variant="danger" size="sm" disabled={submitting} data-testid="provider-delete">
                <Icon name="xCircle" />
                削除する
              </Button>
            </fetcher.Form>
          </details>
        </div>
      </CardBody>
    </Card>
  );
}

/**
 * loader が `celerisErrorResponse` で投げた `Response` を判別する（docs/adr/0004-g1-decisions.md D6、
 * `app/routes/daemon.tsx` と同じ方針）。celeris 停止中はバナー、それ以外は status と detail を出す。
 */
export function ErrorBoundary({ error }: Route.ErrorBoundaryProps) {
  if (isRouteErrorResponse(error) && error.data && typeof error.data === "object" && "kind" in error.data) {
    const data = error.data as CelerisRouteErrorData;
    if (data.kind === "unavailable") {
      return (
        <main className="p-4">
          <CelerisBanner celerisApiUrl={data.baseUrl ?? ""} problem={null} />
          <RouteRecovery />
        </main>
      );
    }
    return (
      <main className="p-4">
        <h1 className="text-xl font-semibold">エラー {data.status}</h1>
        <p className="mt-2 text-sm text-fg-muted">{data.detail}</p>
        {isTransientStatus(data.status) && <RouteRecovery />}
      </main>
    );
  }

  return (
    <main className="p-4">
      <h1 className="text-xl font-semibold">エラー</h1>
      <p className="mt-2 text-sm text-fg-muted">予期しないエラーが起きました。</p>
      <RouteRecovery />
    </main>
  );
}

function ModelRoutingFields({ adapter, prefix, item }: { adapter: string; prefix: string; item?: ProviderView }) {
  const [enabled, setEnabled] = useState(item ? Object.keys(item.tier_models ?? {}).length > 0 : true);
  if (adapter !== "codex" && adapter !== "claude-code") return null;
  const names = adapter === "codex" ? ["astra", "sol", "luna"] : ["fable", "opus", "sonnet"];
  return (
    <fieldset className="min-w-0 space-y-3 border-t border-border pt-3">
      <legend className={labelClass}>{adapter === "codex" ? "GPT" : "Claude"} のモデル階層</legend>
      <input type="hidden" name="routing_form" value="1" />
      <label className={chipLabelClass}>
        <input
          type="checkbox"
          name="tier_models_enabled"
          checked={enabled}
          onChange={(e) => setEnabled(e.target.checked)}
          className={checkboxClass}
        />
        階層別モデルを使う
      </label>
      <p className={hintClass}>
        名称は希望名です。実行IDは利用可能なものを明示してください。未設定・未対応の階層では実行を停止します。無効にすると従来の共通モデル設定を使います。
      </p>
      {enabled &&
        TIER_OPTIONS.map((tier, i) => (
          <div key={tier} className="min-w-0 space-y-2 rounded-lg border border-border p-3">
            <strong>{tier}</strong>
            <div className="grid min-w-0 grid-cols-1 gap-3 md:grid-cols-2">
              <label className={labelClass} htmlFor={`${prefix}-name-${tier}`}>
                名称
                <input
                  id={`${prefix}-name-${tier}`}
                  name={`name_${tier}`}
                  defaultValue={item?.tier_models?.[tier]?.name ?? names[i]}
                  className={cnField(inputClass)}
                />
              </label>
              <label className={labelClass} htmlFor={`${prefix}-model-${tier}`}>
                実行モデルID
                <input
                  id={`${prefix}-model-${tier}`}
                  name={`model_${tier}`}
                  defaultValue={item?.tier_models?.[tier]?.model_id ?? ""}
                  className={cnField(inputClass)}
                />
              </label>
            </div>
            <label className={labelClass} htmlFor={`${prefix}-reason-${tier}`}>
              未対応の理由（指定すると実行停止）
              <input
                id={`${prefix}-reason-${tier}`}
                name={`reason_${tier}`}
                defaultValue={item?.tier_models?.[tier]?.unavailable_reason ?? ""}
                className={cnField(inputClass)}
              />
            </label>
          </div>
        ))}
      <input type="hidden" name="credential_key" value={adapter === "codex" ? "OPENAI_API_KEY" : "ANTHROPIC_API_KEY"} />
      <label className={chipLabelClass}>
        <input type="checkbox" name="update_credential_ref" className={checkboxClass} />
        APIキーの参照を更新する（他のキー参照を置き換え）
      </label>
      <label className={labelClass} htmlFor={`${prefix}-credential`}>
        APIキーID（アカウント画面で管理）
        <input
          id={`${prefix}-credential`}
          name="credential_ref"
          defaultValue={item?.credential_refs?.[adapter === "codex" ? "OPENAI_API_KEY" : "ANTHROPIC_API_KEY"] ?? ""}
          className={cnField(inputClass)}
        />
      </label>
      <label className={labelClass} htmlFor={`${prefix}-account`}>
        認証アカウントID（空欄はプール自動選択）
        <input
          id={`${prefix}-account`}
          name="account_id"
          defaultValue={item?.account_id ?? ""}
          className={cnField(inputClass)}
        />
      </label>
      <p className={hintClass}>
        アカウント画面の同じ種類のIDを参照します。account_pool
        を有効にしてください。認証情報はこの画面には入力しません。
      </p>
    </fieldset>
  );
}
