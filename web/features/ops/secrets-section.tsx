import { useQuery } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type { LlmSourcesView, SecretList, SecretView } from "../../api/generated/types";
import { accountKeys } from "../../api/queries/keys";
import { type ActionResult, ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList } from "../../components/ui/data-list";
import { formatAbsolute } from "../../lib/time";
import { deniedMessage, isDenied } from "./providers-form";

type Run = ReturnType<typeof useActionResult>["run"];
// 枠の色は styles.css の @layer base（--color-input）に任せ、ここでは寸法だけを持つ。
const inputClass = "block w-full min-h-11 rounded border p-2";
const labelClass = "block min-w-0 text-label text-foreground";
const fieldErrorClass = "mt-1 text-label text-danger-foreground";
const cardClass = "min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4";
const sectionTitleClass = "text-section font-semibold text-foreground";

const denied = (r: ActionResult | undefined) => r?.status === 403 || r?.status === 401;

/** 使っている所を 1 文にする（置き換え・削除の影響の説明に使う）。 */
export function secretUsageText(item: Pick<SecretView, "used_by">): string {
  if (item.used_by.length === 0) return "参照している設定はありません";
  return item.used_by.map((u) => `${u.scope} ${u.name}（${u.env}）`).join("、");
}

/** secret の値は表示しない。有無・更新日時・使っている所だけを出す。 */
export function secretRows(item: SecretView) {
  return [
    { key: "value", label: "値", value: "表示しません" },
    {
      key: "updated",
      label: "更新日時",
      value: item.updated_at ? formatAbsolute(item.updated_at) : "記録なし",
    },
    { key: "used", label: "使っている所", value: secretUsageText(item) },
  ];
}

/**
 * 値の置き換え。入力の検査をしてから ConfirmDialog で影響を示し、確認後に送る。
 * 値は手元の state にだけ置き、送ったら空にする。
 */
function ReplaceForm({
  item,
  run,
  disabled,
  deniedId,
}: {
  item: SecretView;
  run: Run;
  disabled: boolean;
  deniedId: string | undefined;
}) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const errorId = useId();
  return (
    <div className="min-w-0 space-y-2">
      <div className="min-w-0">
        <label className={labelClass}>
          {item.id} の新しい値
          <input
            ref={inputRef}
            className={inputClass}
            type="password"
            autoComplete="new-password"
            value={value}
            aria-invalid={error ? true : undefined}
            aria-describedby={error ? errorId : undefined}
            onChange={(e) => setValue(e.target.value)}
          />
        </label>
        {error && (
          <p id={errorId} className={fieldErrorClass}>
            {error}
          </p>
        )}
      </div>
      <ConfirmDialog
        title="secret の値を置き換えますか"
        target={`secret ${item.id}`}
        consequence={`今の値は新しい値で上書きされ、次の run から新しい値が使われます。影響する設定: ${secretUsageText(item)}。`}
        reversibility="前の値には戻せません。戻すには前の値をもう一度入力して置き換えます。"
        followUp="一覧のこの secret の更新日時が変わります。"
        confirmLabel={`${item.id} を置き換える`}
        onConfirm={async () => {
          const sent = value;
          setValue("");
          const [r] = await run([
            {
              id: `put:${item.id}`,
              path: `/api/secrets/${encodeURIComponent(item.id)}`,
              method: "PUT",
              body: { value: sent },
            },
          ]);
          if (r && !r.ok) throw new Error(denied(r) ? deniedMessage("secret の保存・置き換え・削除") : r.message);
          setError(null);
        }}
        trigger={
          <Button
            disabled={disabled}
            aria-describedby={deniedId}
            onClick={(event) => {
              // 空のまま確認へ進めない。項目の error を出して入力欄へ戻す（Radix の trigger は preventDefault で開かない）。
              if (value === "") {
                event.preventDefault();
                setError("新しい値を入力してください。");
                inputRef.current?.focus();
              } else setError(null);
            }}
          >
            値を置き換える
          </Button>
        }
      />
    </div>
  );
}

function SecretItem({
  item,
  run,
  results,
  disabled,
  deniedId,
}: {
  item: SecretView;
  run: Run;
  results: Record<string, ActionResult>;
  disabled: boolean;
  deniedId: string | undefined;
}) {
  const [replacing, setReplacing] = useState(false);
  const removed = results[`del:${item.id}`];
  const put = results[`put:${item.id}`];
  return (
    <li className={cardClass} aria-label={`secret ${item.id}`}>
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <h3 className="min-w-0 break-words text-body font-semibold text-foreground">{item.id}</h3>
        <Badge tone={item.fingerprint ? "success" : "warning"}>{item.fingerprint ? "保存あり" : "未設定"}</Badge>
      </div>
      <DataList items={secretRows(item)} />
      <div className="flex flex-wrap gap-2">
        <Button
          aria-expanded={replacing}
          disabled={disabled}
          aria-describedby={deniedId}
          onClick={() => setReplacing((v) => !v)}
        >
          {replacing ? "置き換えをやめる" : "置き換え"}
        </Button>
        <Button
          variant="destructive"
          disabled={disabled}
          aria-describedby={deniedId}
          onClick={() => {
            // parity e2e（ops.spec.ts）が native の確認で進めるため window.confirm のまま、影響と戻し方を書く。
            if (
              window.confirm(
                `secret ${item.id} を削除しますか。影響する設定: ${secretUsageText(item)}。次の run からこの値は使えません。値は元に戻せず、もう一度入力し直す必要があります。`,
              )
            )
              void run([
                { id: `del:${item.id}`, path: `/api/secrets/${encodeURIComponent(item.id)}`, method: "DELETE" },
              ]);
          }}
        >
          secret を削除
        </Button>
      </div>
      {replacing && <ReplaceForm item={item} run={run} disabled={disabled} deniedId={deniedId} />}
      {put?.ok && <ActionResultView result={put} />}
      {removed && !removed.ok && !denied(removed) && <ActionResultView result={removed} />}
    </li>
  );
}

/** 新しい secret の追加。既にある id は置き換え（確認つき）へ回す。値は送る直前に空にする。 */
function SecretForm({
  run,
  disabled,
  deniedId,
  existing,
}: {
  run: Run;
  disabled: boolean;
  deniedId: string | undefined;
  existing: readonly string[];
}) {
  const [id, setId] = useState("");
  const [value, setValue] = useState("");
  const [errors, setErrors] = useState<{ id?: string; value?: string }>({});
  const idRef = useRef<HTMLInputElement>(null);
  const valueRef = useRef<HTMLInputElement>(null);
  const idErrorId = useId();
  const valueErrorId = useId();
  return (
    <form
      noValidate
      className={cardClass}
      aria-label="secret を設定"
      onSubmit={(event) => {
        event.preventDefault();
        const sentId = id.trim();
        const found: { id?: string; value?: string } = {};
        if (sentId === "") found.id = "secret id を入力してください。";
        else if (existing.includes(sentId))
          found.id = "この id は既にあります。一覧の「置き換え」から確認して置き換えてください。";
        if (value === "") found.value = "secret 値を入力してください。";
        setErrors(found);
        if (found.id) return idRef.current?.focus();
        if (found.value) return valueRef.current?.focus();
        const sent = value;
        setValue("");
        void run([
          {
            id: `put:${sentId}`,
            path: `/api/secrets/${encodeURIComponent(sentId)}`,
            method: "PUT",
            body: { value: sent },
          },
        ]).then((out) => {
          const r = out[0];
          if (r?.ok) setId("");
          else if (r && !denied(r)) {
            setErrors({ value: `保存できませんでした: ${r.message}。値をもう一度入力してください。` });
            valueRef.current?.focus();
          }
        });
      }}
    >
      <h3 className="text-body font-semibold text-foreground">secret を追加</h3>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="min-w-0">
          <label className={labelClass}>
            secret id
            <input
              ref={idRef}
              className={inputClass}
              required
              value={id}
              aria-invalid={errors.id ? true : undefined}
              aria-describedby={errors.id ? idErrorId : undefined}
              onChange={(e) => setId(e.target.value)}
            />
          </label>
          {errors.id && (
            <p id={idErrorId} className={fieldErrorClass}>
              {errors.id}
            </p>
          )}
        </div>
        <div className="min-w-0">
          <label className={labelClass}>
            secret 値
            <input
              ref={valueRef}
              className={inputClass}
              type="password"
              autoComplete="new-password"
              required
              value={value}
              aria-invalid={errors.value ? true : undefined}
              aria-describedby={errors.value ? valueErrorId : undefined}
              onChange={(e) => setValue(e.target.value)}
            />
          </label>
          {errors.value && (
            <p id={valueErrorId} className={fieldErrorClass}>
              {errors.value}
            </p>
          )}
        </div>
      </div>
      <Button type="submit" variant="primary" disabled={disabled} aria-describedby={deniedId}>
        secret を保存
      </Button>
    </form>
  );
}

export function SecretsSection() {
  const secrets = useQuery({
    queryKey: accountKeys.list({ section: "secrets" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<SecretList>("/api/secrets", signal),
  });
  const llm = useQuery({
    queryKey: accountKeys.list({ section: "llm" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<LlmSourcesView>("/api/llm/sources", signal),
  });
  const sender = useActionResult(accountKeys.all);
  const deniedNoticeId = useId();
  const blocked = isDenied(sender.results);
  const deniedId = blocked ? deniedNoticeId : undefined;
  const disabled = sender.pending || blocked;
  const items = secrets.data?.items ?? [];
  return (
    <section className="min-w-0 space-y-3" aria-label="secret と LLM source">
      <h2 className={sectionTitleClass}>secret</h2>
      <p className="text-label text-muted-foreground">
        値は表示しません。保存の有無・更新日時・使っている所を確かめ、変えるときは「置き換え」で新しい値を入れます。
      </p>
      {blocked && (
        <p
          id={deniedNoticeId}
          role="alert"
          className="border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
        >
          {deniedMessage("secret の保存・置き換え・削除")}
        </p>
      )}
      <FetchFrame query={secrets} subject="secret">
        {secrets.data &&
          (items.length === 0 ? (
            <p className="text-body text-muted-foreground">secret はまだありません。下の form から追加します。</p>
          ) : (
            <ul className="grid min-w-0 gap-3 xl:grid-cols-2">
              {items.map((item) => (
                <SecretItem
                  key={item.id}
                  item={item}
                  run={sender.run}
                  results={sender.results}
                  disabled={disabled}
                  deniedId={deniedId}
                />
              ))}
            </ul>
          ))}
      </FetchFrame>
      <SecretForm run={sender.run} disabled={disabled} deniedId={deniedId} existing={items.map((s) => s.id)} />
      <h2 className={sectionTitleClass}>LLM source</h2>
      <FetchFrame query={llm} subject="LLM source">
        {llm.data && (
          <ul className="grid min-w-0 gap-2 xl:grid-cols-2">
            {llm.data.sources.map((source) => (
              <li
                key={source.id}
                className="min-w-0 space-y-1 rounded-lg border border-border bg-surface p-3 text-label text-foreground"
                aria-label={`LLM source ${source.id}`}
              >
                <p className="break-words">
                  {source.id}（{source.kind}）
                </p>
                <div className="flex flex-wrap gap-2">
                  <Badge tone={source.enabled ? "success" : "neutral"}>{source.enabled ? "有効" : "無効"}</Badge>
                  {source.reachable === false && <Badge tone="danger">届かない</Badge>}
                </div>
                <p className="break-words">
                  直近 1 時間 {source.last_hour_requests} 件
                  {source.accounts.length > 0 && ` / アカウント ${source.accounts.map((a) => a.id).join(", ")}`}
                </p>
              </li>
            ))}
          </ul>
        )}
      </FetchFrame>
    </section>
  );
}
