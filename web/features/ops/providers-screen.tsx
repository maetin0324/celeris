import { useQuery } from "@tanstack/react-query";
import { type ReactNode, type RefObject, useId, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { LlmSourcesView, ProviderCheckResponse, Providers, ProviderView, Tier } from "../../api/generated/types";
import { accountKeys, providerKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { DataList } from "../../components/ui/data-list";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "../../components/ui/table";
import { formatAbsolute } from "../../lib/time";
import {
  ADAPTERS,
  buildProviderBody,
  checkResultLabel,
  deniedMessage,
  isDenied,
  type ProviderFieldErrors,
  providerState,
  TIERS,
  validateProviderForm,
} from "./providers-form";
import {
  adapterLabel,
  celerisModelsFor,
  providerLlmSourceDisplay,
  sourceKindLabel,
  sourceTierScopeNote,
  tiersResolvingTo,
} from "./providers-llm-source";

type Sender = ReturnType<typeof useActionResult>;
// 枠の色は styles.css の @layer base（--color-input）に任せ、ここでは寸法だけを持つ。
const inputClass = "block w-full min-h-11 rounded border p-2";
const labelClass = "block min-w-0 text-label text-foreground";
const fieldErrorClass = "mt-1 text-label text-danger-foreground";
const cardClass = "min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4";
const sectionTitleClass = "text-section font-semibold text-foreground";

/** 可視の label・項目の error（aria-describedby）を持つ 1 入力欄。 */
function TextField({
  label,
  value,
  onChange,
  error,
  inputRef,
  type = "text",
}: {
  label: string;
  value: string;
  onChange: (next: string) => void;
  error?: string;
  inputRef?: RefObject<HTMLInputElement | null>;
  type?: "text" | "number";
}) {
  const errorId = useId();
  return (
    <div className="min-w-0">
      <label className={labelClass}>
        {label}
        <input
          ref={inputRef}
          className={inputClass}
          type={type}
          min={type === "number" ? 0 : undefined}
          value={value}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
          onChange={(e) => onChange(e.target.value)}
        />
      </label>
      {error && (
        <p id={errorId} className={fieldErrorClass}>
          {error}
        </p>
      )}
    </div>
  );
}

function TierChecks({ value, onChange }: { value: Tier[]; onChange: (next: Tier[]) => void }) {
  return (
    <fieldset className="flex min-w-0 flex-wrap gap-3">
      <legend className="text-label text-foreground">tiers</legend>
      {TIERS.map((tier) => (
        <label key={tier} className="inline-flex min-h-11 items-center gap-1 text-label text-foreground">
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

/** 最初の error の入力欄へ focus を移す（入力順: id → concurrency）。 */
function focusFirst(
  errors: ProviderFieldErrors,
  refs: { id?: RefObject<HTMLInputElement | null>; concurrency: RefObject<HTMLInputElement | null> },
) {
  if (errors.id) refs.id?.current?.focus();
  else if (errors.concurrency) refs.concurrency.current?.focus();
}

/** 403 の理由文。変更の操作はこの id へ aria-describedby で結ぶ。 */
export function DeniedNotice({ id, actions }: { id: string; actions: string }) {
  return (
    <p
      id={id}
      role="alert"
      className="border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
    >
      {deniedMessage(actions)}
    </p>
  );
}

/** 実行枠の要約（ADR-0132 D6）: adapter・受ける tier・要求モデルと、使う LLM source。 */
export function ProviderSummary({ item }: { item: ProviderView }) {
  const source = providerLlmSourceDisplay(item.llm_source);
  const celerisModels = celerisModelsFor(item);
  return (
    <div className="min-w-0 space-y-1 text-label text-foreground">
      <p className="break-words">
        {item.adapter} / concurrency {item.concurrency} / tiers {item.tiers.join(", ")}
        {item.model ? ` / model ${item.model}` : ""}
      </p>
      <p className="break-words">harness: {adapterLabel(item.adapter)}</p>
      <p className="break-words" data-source-ref={item.llm_source?.source ?? ""}>
        LLM source: {source.label}
        {source.origin ? `（${source.origin}）` : ""}
        {celerisModels.length > 0 ? ` / 使うモデル ${celerisModels.join(", ")}` : ""}
      </p>
      {source.note && <p className="break-words text-muted-foreground">{source.note}</p>}
    </div>
  );
}

function lastCheckText(check: { result: string; at: string; detail?: string | null } | null | undefined) {
  if (!check) return "まだ確認していません";
  return `${checkResultLabel(check.result)}・${formatAbsolute(check.at)}${check.detail ? `（${check.detail}）` : ""}`;
}

/** 実行枠の現在値と状態を 1 行ずつ並べる表。状態は文字の badge で示す。 */
export function ProviderStatusTable({ items }: { items: readonly ProviderView[] }) {
  return (
    <Table aria-label="実行枠の状態">
      <TableHeader>
        <TableRow>
          <TableHead>実行枠</TableHead>
          <TableHead>状態</TableHead>
          <TableHead>道具</TableHead>
          <TableHead>tiers</TableHead>
          <TableHead>同時実行</TableHead>
          <TableHead>前回の確認</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {items.map((item) => {
          const state = providerState(item, formatAbsolute);
          return (
            <TableRow key={item.id} data-provider={item.id}>
              <TableHead scope="row" className="align-top text-foreground">
                {item.id}
              </TableHead>
              <TableCell>
                <Badge tone={state.tone} data-state={state.label} className="whitespace-nowrap">
                  {state.label}
                </Badge>
              </TableCell>
              <TableCell className="whitespace-nowrap">{item.adapter}</TableCell>
              <TableCell>{item.tiers.join(", ") || "なし"}</TableCell>
              <TableCell>
                {item.in_use != null ? `${item.in_use} / ${item.concurrency}` : `上限 ${item.concurrency}`}
              </TableCell>
              <TableCell>{lastCheckText(item.last_check)}</TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function ProviderCard({
  item,
  sender,
  blocked,
  deniedId,
}: {
  item: ProviderView;
  sender: Sender;
  blocked: boolean;
  deniedId: string | undefined;
}) {
  const [concurrency, setConcurrency] = useState(String(item.concurrency));
  const [model, setModel] = useState(item.model ?? "");
  const [tiers, setTiers] = useState<Tier[]>(item.tiers);
  const [errors, setErrors] = useState<ProviderFieldErrors>({});
  const concurrencyRef = useRef<HTMLInputElement>(null);
  const base = `/api/providers/${encodeURIComponent(item.id)}`;
  const check = sender.results[`check:${item.id}`];
  const checked = check?.ok ? (check.response as ProviderCheckResponse | undefined) : undefined;
  const state = providerState(
    checked
      ? { ...item, last_check: { at: checked.checked_at, result: checked.result, detail: checked.detail } }
      : item,
    formatAbsolute,
  );
  const disabled = sender.pending || blocked;
  const removed = sender.results[`delete:${item.id}`];
  return (
    <li className={cardClass} aria-label={`プロバイダ ${item.id}`}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <h3 className="min-w-0 break-words text-body font-semibold text-foreground">{item.id}</h3>
        <Badge tone={state.tone} data-state={state.label}>
          {state.label}
        </Badge>
      </div>
      <DataList
        items={[
          { key: "state", label: "状態", value: state.detail },
          {
            key: "check",
            label: "前回の確認",
            value: lastCheckText(
              checked ? { result: checked.result, at: checked.checked_at, detail: checked.detail } : item.last_check,
            ),
          },
          {
            key: "credential",
            label: "認証情報",
            value:
              item.env_keys.length > 0
                ? `環境変数 ${item.env_keys.join(", ")}（値は表示しません）`
                : item.account_id
                  ? `アカウント ${item.account_id}`
                  : "設定なし",
          },
          {
            key: "stats",
            label: "これまでの run",
            value: `${item.stats.runs} 件（完了 ${item.stats.done}・エラー ${item.stats.error}）`,
          },
        ]}
      />
      <ProviderSummary item={item} />
      <form
        noValidate
        className="min-w-0 space-y-3"
        aria-label={`実行枠 ${item.id} の設定`}
        onSubmit={(event) => {
          event.preventDefault();
          const found = validateProviderForm({ concurrency }, "patch");
          setErrors(found);
          if (Object.keys(found).length > 0) {
            focusFirst(found, { concurrency: concurrencyRef });
            return;
          }
          void sender
            .run([
              {
                id: `patch:${item.id}`,
                path: base,
                method: "PATCH",
                body: buildProviderBody({ concurrency, model, tiers }, "patch"),
              },
              { id: "reload", path: "/api/reload", body: {} },
            ])
            .then((out) => {
              const r = out[0];
              if (r && !r.ok && r.status !== 403 && r.status !== 401) {
                setErrors({ concurrency: `保存できませんでした: ${r.message}` });
                concurrencyRef.current?.focus();
              }
            });
        }}
      >
        <div className="grid gap-3 sm:grid-cols-2">
          <TextField
            label="concurrency"
            type="number"
            value={concurrency}
            onChange={setConcurrency}
            error={errors.concurrency}
            inputRef={concurrencyRef}
          />
          <TextField label="model" value={model} onChange={setModel} />
        </div>
        <TierChecks value={tiers} onChange={setTiers} />
        <div className="flex flex-wrap gap-2">
          <Button type="submit" disabled={disabled} aria-describedby={deniedId}>
            変更を保存
          </Button>
          <Button
            disabled={disabled}
            aria-describedby={deniedId}
            onClick={() => void sender.run([{ id: `check:${item.id}`, path: `${base}/check`, body: {} }])}
          >
            接続を確認
          </Button>
          <Button
            variant="destructive"
            disabled={disabled}
            aria-describedby={deniedId}
            onClick={() => {
              if (
                window.confirm(
                  `実行枠 ${item.id} を削除しますか。この枠に割り当てる run は止まり、設定は元に戻せません（同じ id で追加し直す必要があります）。`,
                )
              )
                void sender.run([
                  { id: `delete:${item.id}`, path: base, method: "DELETE" },
                  { id: "reload", path: "/api/reload", body: {} },
                ]);
            }}
          >
            削除
          </Button>
        </div>
      </form>
      {sender.results[`patch:${item.id}`]?.ok && <ActionResultView result={sender.results[`patch:${item.id}`]} />}
      {removed && removed.status !== 403 && removed.status !== 401 && <ActionResultView result={removed} />}
      {check && !check.ok && check.status !== 403 && check.status !== 401 && <ActionResultView result={check} />}
      {checked && (
        <p role="status" className="text-label text-foreground">
          確認結果: {checked.result}（{checkResultLabel(checked.result)}）
          {checked.detail ? `（${checked.detail}）` : ""}
        </p>
      )}
    </li>
  );
}

function CreateForm({ sender, blocked, deniedId }: { sender: Sender; blocked: boolean; deniedId: string | undefined }) {
  const [id, setId] = useState("");
  const [adapter, setAdapter] = useState<string>("codex");
  const [concurrency, setConcurrency] = useState("");
  const [model, setModel] = useState("");
  const [tiers, setTiers] = useState<Tier[]>([]);
  const [errors, setErrors] = useState<ProviderFieldErrors>({});
  const idRef = useRef<HTMLInputElement>(null);
  const concurrencyRef = useRef<HTMLInputElement>(null);
  const created = sender.results.create;
  return (
    <form
      noValidate
      className={cardClass}
      aria-label="プロバイダを追加"
      onSubmit={(event) => {
        event.preventDefault();
        const found = validateProviderForm({ id, concurrency }, "create");
        setErrors(found);
        if (Object.keys(found).length > 0) {
          focusFirst(found, { id: idRef, concurrency: concurrencyRef });
          return;
        }
        void sender
          .run([
            {
              id: "create",
              path: "/api/providers",
              body: buildProviderBody({ id, adapter, concurrency, model, tiers }, "create"),
            },
            { id: "reload", path: "/api/reload", body: {} },
          ])
          .then((outcomes) => {
            const r = outcomes[0];
            if (r?.ok) setId("");
            else if (r && r.status !== 403 && r.status !== 401) {
              setErrors({ id: `追加できませんでした: ${r.message}` });
              idRef.current?.focus();
            }
          });
      }}
    >
      <h2 className={sectionTitleClass}>プロバイダを追加</h2>
      <div className="grid gap-3 sm:grid-cols-2">
        <TextField label="新規 id" value={id} onChange={setId} error={errors.id} inputRef={idRef} />
        <label className={labelClass}>
          adapter
          <select className={inputClass} value={adapter} onChange={(e) => setAdapter(e.target.value)}>
            {ADAPTERS.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        </label>
        <TextField
          label="新規 concurrency"
          type="number"
          value={concurrency}
          onChange={setConcurrency}
          error={errors.concurrency}
          inputRef={concurrencyRef}
        />
        <TextField label="新規 model" value={model} onChange={setModel} />
      </div>
      <TierChecks value={tiers} onChange={setTiers} />
      <Button type="submit" variant="primary" disabled={sender.pending || blocked} aria-describedby={deniedId}>
        追加
      </Button>
      {created?.ok && <ActionResultView result={created} />}
    </form>
  );
}

function reachState(reachable: boolean | null | undefined) {
  if (reachable === false) return { tone: "danger" as const, label: "届かない" };
  if (reachable) return { tone: "success" as const, label: "到達可" };
  return { tone: "neutral" as const, label: "未確認" };
}

/** 「LLM source」節の中身: 種類・ID・到達性・今どの celeris/<tier> の解決先か。 */
export function LlmSourceList({ data }: { data: LlmSourcesView }) {
  if (data.sources.length === 0) return <p className="text-body text-muted-foreground">供給元がありません。</p>;
  return (
    <ul className="grid min-w-0 gap-2 xl:grid-cols-2">
      {data.sources.map((source) => {
        const resolving = tiersResolvingTo(source.id, data.celeris_tiers);
        const scope = sourceTierScopeNote(source.kind);
        const reach = reachState(source.reachable);
        return (
          <li
            key={source.id}
            className="min-w-0 space-y-1 rounded-lg border border-border bg-surface p-3 text-label text-foreground"
            aria-label={`LLM source ${source.id}`}
          >
            <p className="break-words">
              <span className="font-semibold">{source.id}</span>（{sourceKindLabel(source.kind)}）
            </p>
            <div className="flex flex-wrap gap-2">
              <Badge tone={source.enabled ? "success" : "neutral"}>{source.enabled ? "有効" : "無効"}</Badge>
              <Badge tone={reach.tone}>{reach.label}</Badge>
            </div>
            {source.reachable === false && source.unreachable_reason && (
              <p className="break-words">理由: {source.unreachable_reason}</p>
            )}
            {resolving.length > 0 && <p className="break-words">解決先: {resolving.join(", ")}</p>}
            {scope && <p className="break-words text-muted-foreground">{scope}</p>}
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
      {/* 名前は aria-label の固定語にする（見出しの「adapter」を名前に流すと、入力欄の label 探索と重なる）。 */}
      <section className="min-w-0 space-y-3" aria-label="プロバイダ一覧">
        <h2 className={sectionTitleClass}>adapter / harness の実行枠（{count}）</h2>
        <p className="text-label text-muted-foreground">
          {"道具（claude-code・codex・acp・paperqa・langmem・ldr など）ごとの実行枠です。" +
            "表の「状態」で、接続を確かめたか・休止中か・失敗しているかを読みます。" +
            "各枠の LLM source は、その道具がどの供給元のモデルを使うかを示します。" +
            "celeris/<tier> は実行時に proxy が供給元を選ぶ抽象モデルです。"}
        </p>
        {adapters}
      </section>
      <section className="min-w-0 space-y-3" aria-label="供給元の一覧">
        <h2 className={sectionTitleClass}>LLM source</h2>
        <p className="text-label text-muted-foreground">
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
  const sender = useActionResult(providerKeys.all);
  const deniedNoticeId = useId();
  const denied = isDenied(sender.results);
  const deniedId = denied ? deniedNoticeId : undefined;
  const reload = sender.results.reload;
  return (
    <ScreenFrame title="プロバイダ" route="/providers">
      <FetchFrame query={query} subject="プロバイダ">
        {query.data && (
          <div className="min-w-0 space-y-6">
            {denied && <DeniedNotice id={deniedNoticeId} actions="実行枠の追加・変更・接続の確認・削除" />}
            <ProvidersSections
              count={query.data.items.length}
              adapters={
                query.data.items.length === 0 ? (
                  <p className="text-body text-muted-foreground">
                    プロバイダがありません。下の「プロバイダを追加」から登録してください。
                  </p>
                ) : (
                  <>
                    <ProviderStatusTable items={query.data.items} />
                    <ul className="grid min-w-0 gap-3 xl:grid-cols-2">
                      {query.data.items.map((item) => (
                        <ProviderCard key={item.id} item={item} sender={sender} blocked={denied} deniedId={deniedId} />
                      ))}
                    </ul>
                  </>
                )
              }
              sources={
                <FetchFrame query={llm} subject="LLM source">
                  {llm.data && <LlmSourceList data={llm.data} />}
                </FetchFrame>
              }
            />
            <CreateForm sender={sender} blocked={denied} deniedId={deniedId} />
            {reload && !reload.ok && reload.status !== 403 && reload.status !== 401 && (
              <p role="alert" className="text-body text-danger-foreground">
                設定の reload に失敗しました。変更は保存済みでも、反映には daemon の再読込が要ります。
              </p>
            )}
          </div>
        )}
      </FetchFrame>
    </ScreenFrame>
  );
}
