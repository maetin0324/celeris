import { useQuery } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";
import { apiGet } from "../../api/client";
import type {
  AccountCheckResponse,
  AccountList,
  AccountLoginStart,
  AccountView,
  ProviderCheckResult,
} from "../../api/generated/types";
import { accountKeys } from "../../api/queries/keys";
import { type ActionResult, ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Badge, type BadgeTone } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { DataList } from "../../components/ui/data-list";
import { Section } from "../../components/ui/panel";
import { formatAbsolute } from "../../lib/time";
import { McpClientsSection } from "./mcp-clients";
import { SecretsSection } from "./secrets-section";

type Sender = ReturnType<typeof useActionResult>;
// 枠の色は styles.css の @layer base（--color-input）に任せ、ここでは寸法だけを持つ。
const inputClass = "block w-full min-h-11 rounded border p-2";
const fieldErrorClass = "mt-1 text-label text-danger-foreground";
export const ADAPTER_CHOICES = ["claude-code", "codex"] as const;

/** ログイン中の状態（url・user_code）は account 単位で画面の state に持つ。取り直しでは消さない。 */
type Logins = Record<string, AccountLoginStart>;

function accountKey(item: AccountView) {
  return `${item.adapter ?? "claude-code"}:${item.id}`;
}

const checkLabel: Readonly<Record<ProviderCheckResult, string>> = {
  ok: "正常",
  auth_failed: "認証失敗",
  throttled: "利用制限中",
  spawn_failed: "起動失敗",
};

function checkResultLabel(result: string): string {
  return Object.hasOwn(checkLabel, result) ? checkLabel[result as ProviderCheckResult] : result;
}

/** account の状態を 1 つの語にまとめる。重いもの（除外・失敗）から順に見る。色だけにせず必ず文字を出す。 */
export function accountState(item: AccountView): { tone: BadgeTone; label: string; detail?: string } {
  if (item.excluded_reason) return { tone: "danger", label: "除外中", detail: item.excluded_reason };
  const check = item.last_check?.result;
  if (check === "auth_failed" || check === "spawn_failed")
    return { tone: "danger", label: checkResultLabel(check), detail: item.last_check?.detail ?? undefined };
  if (item.cooldown)
    return {
      tone: "warning",
      label: "休止中",
      detail: `${item.cooldown.reason}（${formatAbsolute(item.cooldown.until)} まで）`,
    };
  if (check === "throttled") return { tone: "warning", label: checkResultLabel(check) };
  if (item.login_pending) return { tone: "info", label: "ログイン待ち", detail: "認証コードを送ると完了します" };
  if (!item.logged_in)
    return { tone: "warning", label: "未ログイン", detail: "期限切れか未作成です。ログインを開始してください" };
  return { tone: "success", label: "ログイン済み" };
}

/** 403・401 の結果があれば、この画面の操作を止める。 */
function isDenied(results: Record<string, ActionResult>) {
  return Object.values(results).some((r) => r.status === 403 || r.status === 401);
}

export function AccountCard({
  item,
  max,
  sender,
  blocked,
  deniedId,
  login,
  setLogin,
}: {
  item: AccountView;
  max: number;
  sender: Sender;
  blocked: boolean;
  deniedId: string | undefined;
  login: AccountLoginStart | undefined;
  setLogin: (key: string, value: AccountLoginStart | null) => void;
}) {
  const [code, setCode] = useState("");
  const [codeError, setCodeError] = useState<string | null>(null);
  const codeRef = useRef<HTMLInputElement>(null);
  const codeErrorId = useId();
  const adapter = item.adapter ?? "claude-code";
  const key = accountKey(item);
  const base = `/api/accounts/${encodeURIComponent(item.id)}`;
  const q = `?adapter=${encodeURIComponent(adapter)}`;
  const check = sender.results[`check:${key}`];
  const checked = check?.ok ? (check.response as AccountCheckResponse | undefined) : undefined;
  const state = accountState(item);
  const disabled = sender.pending || blocked;
  // サーバが login_pending を返すなら、url が手元に無くてもコード入力欄を残す。
  const awaiting = item.login_pending || login !== undefined;
  const lastCheck = checked
    ? { result: checked.result, at: checked.checked_at, detail: checked.detail }
    : item.last_check;
  return (
    <li
      className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4"
      aria-label={`アカウント ${item.id}`}
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <h3 className="min-w-0 break-words text-body font-semibold text-foreground">{item.id}</h3>
        <Badge tone={state.tone} data-state={state.label}>
          {state.label}
        </Badge>
      </div>
      <DataList
        items={[
          {
            key: "state",
            label: "状態",
            value: state.detail ?? "使える状態です",
          },
          { key: "adapter", label: "adapter", value: adapter },
          {
            key: "credential",
            label: "認証情報",
            value: item.logged_in ? "保存あり（値は表示しません）" : "保存なし",
          },
          {
            key: "check",
            label: "最終確認",
            value: lastCheck
              ? `${checkResultLabel(lastCheck.result)}・${formatAbsolute(lastCheck.at)}${lastCheck.detail ? `（${lastCheck.detail}）` : ""}`
              : "まだ確認していません",
          },
          { key: "in_use", label: "実行中の run", value: `${item.in_use} / 上限 ${max}` },
          {
            key: "stats",
            label: "これまでの run",
            value: `${item.stats.runs} 件（完了 ${item.stats.done}・エラー ${item.stats.error}）`,
          },
        ]}
      />
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={disabled}
          aria-describedby={deniedId}
          onClick={() => void sender.run([{ id: `check:${key}`, path: `${base}/check${q}`, body: {} }])}
        >
          確認
        </Button>
        <Button
          disabled={disabled}
          aria-describedby={deniedId}
          onClick={() =>
            void sender.run([{ id: `login:${key}`, path: `${base}/login${q}`, body: {} }]).then((out) => {
              if (out[0]?.ok) setLogin(key, out[0].response as AccountLoginStart);
            })
          }
        >
          ログイン開始
        </Button>
        <ConfirmDialog
          trigger={
            <Button variant="destructive" disabled={disabled} aria-describedby={deniedId}>
              削除
            </Button>
          }
          title="アカウントを削除しますか"
          target={item.id}
          consequence="このアカウントの登録を削除し、以後の run で選べなくなります。"
          reversibility="削除は元に戻せません。必要ならアカウントを追加し直してください。"
          followUp="アカウント一覧で削除結果を確認できます。"
          confirmLabel={`${item.id} を削除`}
          onConfirm={async () => {
            const [result] = await sender.run([{ id: `delete:${key}`, path: `${base}${q}`, method: "DELETE" }]);
            if (result && !result.ok) throw new Error(result.message);
          }}
        />
      </div>
      <ActionResultView result={check} />
      {checked && (
        <p role="status" className="text-label text-foreground">
          確認結果: {checked.result}（{checkResultLabel(checked.result)}）
        </p>
      )}
      <ActionResultView result={sender.results[`delete:${key}`]} />
      <ActionResultView result={sender.results[`login:${key}`]} />
      {awaiting && (
        <section
          className="min-w-0 space-y-2 rounded-md border border-border bg-background p-3"
          aria-label={`ログイン ${item.id}`}
        >
          <p role="status" className="text-label font-medium text-foreground">
            ログイン待ち
          </p>
          {login && (
            <p className="break-words text-label text-foreground">
              {/^https?:\/\//.test(login.url) ? (
                <a className="underline" href={login.url} target="_blank" rel="noreferrer">
                  認証ページを開く
                </a>
              ) : (
                login.url
              )}
              {login.user_code ? ` / user code: ${login.user_code}` : ""}
            </p>
          )}
          <form
            noValidate
            className="space-y-2"
            onSubmit={(event) => {
              event.preventDefault();
              const sent = code.trim();
              if (sent === "") {
                setCodeError("認証コードを入力してください。");
                codeRef.current?.focus();
                return;
              }
              setCode("");
              setCodeError(null);
              void sender
                .run([{ id: `code:${key}`, path: `${base}/login/code${q}`, body: { code: sent } }])
                .then((out) => {
                  const r = out[0];
                  if (r?.ok) setLogin(key, null);
                  else if (r && r.status !== 403 && r.status !== 401) {
                    setCodeError(r.message);
                    codeRef.current?.focus();
                  }
                });
            }}
          >
            <label className="block text-label text-foreground">
              認証コード
              <input
                ref={codeRef}
                className={inputClass}
                type="password"
                autoComplete="off"
                value={code}
                aria-invalid={codeError ? true : undefined}
                aria-describedby={codeError ? codeErrorId : undefined}
                onChange={(e) => setCode(e.target.value)}
              />
            </label>
            {codeError && (
              <p id={codeErrorId} className={fieldErrorClass}>
                {codeError}
              </p>
            )}
            <div className="flex flex-wrap gap-2">
              <Button type="submit" variant="primary" disabled={disabled} aria-describedby={deniedId}>
                コードを送る
              </Button>
              <Button
                disabled={disabled}
                aria-describedby={deniedId}
                onClick={() =>
                  void sender
                    .run([{ id: `cancel:${key}`, path: `${base}/login${q}`, method: "DELETE" }])
                    .then((out) => {
                      if (out[0]?.ok) setLogin(key, null);
                    })
                }
              >
                ログインを取り消す
              </Button>
            </div>
          </form>
          <ActionResultView result={sender.results[`cancel:${key}`]} />
        </section>
      )}
    </li>
  );
}

function CreateForm({ sender, blocked, deniedId }: { sender: Sender; blocked: boolean; deniedId: string | undefined }) {
  const [id, setId] = useState("");
  const [adapter, setAdapter] = useState<string>("claude-code");
  const [error, setError] = useState<string | null>(null);
  const idRef = useRef<HTMLInputElement>(null);
  const errorId = useId();
  const created = sender.results.create;
  return (
    <form
      noValidate
      className="min-w-0 space-y-3 rounded-lg border border-border bg-surface p-4"
      aria-label="アカウントを追加"
      onSubmit={(event) => {
        event.preventDefault();
        const value = id.trim();
        if (value === "") {
          setError("アカウント id を入力してください。");
          idRef.current?.focus();
          return;
        }
        setError(null);
        void sender.run([{ id: "create", path: "/api/accounts", body: { id: value, adapter } }]).then((out) => {
          const r = out[0];
          if (r?.ok) setId("");
          else if (r && r.status !== 403 && r.status !== 401) {
            setError(r.message);
            idRef.current?.focus();
          }
        });
      }}
    >
      <h2 className="text-section font-semibold text-foreground">アカウントを追加</h2>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="min-w-0">
          <label className="block text-label text-foreground">
            アカウント id
            <input
              ref={idRef}
              className={inputClass}
              required
              value={id}
              aria-invalid={error ? true : undefined}
              aria-describedby={error ? errorId : undefined}
              onChange={(e) => setId(e.target.value)}
            />
          </label>
          {error && (
            <p id={errorId} className={fieldErrorClass}>
              {error}
            </p>
          )}
        </div>
        <label className="block min-w-0 text-label text-foreground">
          adapter
          <select className={inputClass} value={adapter} onChange={(e) => setAdapter(e.target.value)}>
            {ADAPTER_CHOICES.map((a) => (
              <option key={a} value={a}>
                {a}
              </option>
            ))}
          </select>
        </label>
      </div>
      <Button type="submit" variant="primary" disabled={sender.pending || blocked} aria-describedby={deniedId}>
        アカウントを追加
      </Button>
      {created?.ok && <ActionResultView result={created} />}
    </form>
  );
}

export function AccountsScreen() {
  const query = useQuery({
    queryKey: accountKeys.list({ section: "accounts" }),
    queryFn: ({ signal }: { signal: AbortSignal }) => apiGet<AccountList>("/api/accounts", signal),
  });
  const sender = useActionResult(accountKeys.all);
  const [logins, setLogins] = useState<Logins>({});
  const deniedNoticeId = useId();
  const denied = isDenied(sender.results);
  const deniedId = denied ? deniedNoticeId : undefined;
  const setLogin = (key: string, value: AccountLoginStart | null) =>
    setLogins((prev) => {
      const next = { ...prev };
      if (value) next[key] = value;
      else delete next[key];
      return next;
    });
  return (
    <ScreenFrame title="アカウント" route="/accounts">
      <div className="min-w-0 space-y-6">
        <FetchFrame query={query} subject="アカウント">
          {query.data && (
            <div className="min-w-0 space-y-4">
              {denied && (
                <p
                  id={deniedNoticeId}
                  role="alert"
                  className="border-l-2 border-danger-foreground bg-danger px-3 py-2 text-body text-danger-foreground"
                >
                  この操作を行う権限がありません。アカウントの追加・確認・ログイン・削除は止めています。管理者に権限を確認してください。
                </p>
              )}
              <Section
                title={`アカウント一覧（${query.data.items.length}）`}
                description="状態の欄で、使えるか・ログインが要るか・失敗しているかを確かめます。認証情報の値は表示しません。"
              >
                {query.data.items.length === 0 ? (
                  <p className="text-body text-muted-foreground">
                    アカウントがありません。下の「アカウントを追加」から登録してください。
                  </p>
                ) : (
                  <ul className="grid min-w-0 gap-3 xl:grid-cols-2">
                    {query.data.items.map((item) => (
                      <AccountCard
                        key={accountKey(item)}
                        item={item}
                        max={query.data.max_runs_per_account}
                        sender={sender}
                        blocked={denied}
                        deniedId={deniedId}
                        login={logins[accountKey(item)]}
                        setLogin={setLogin}
                      />
                    ))}
                  </ul>
                )}
              </Section>
              <CreateForm sender={sender} blocked={denied} deniedId={deniedId} />
            </div>
          )}
        </FetchFrame>
        <SecretsSection />
        <McpClientsSection />
      </div>
    </ScreenFrame>
  );
}
