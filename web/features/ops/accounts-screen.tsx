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
import { formatAbsolute, formatRelative, getServerClockOffset } from "../../lib/time";
import { cn } from "../../lib/utils";
import { McpClientsSection } from "./mcp-clients";
import { SecretsSection } from "./secrets-section";

/** account の道具（adapter）の表示名。未知の値はそのまま。 */
export function adapterLabel(adapter: string): string {
  return adapter === "opencode-go" ? "OpenCode Go" : adapter;
}

type Sender = ReturnType<typeof useActionResult>;
// 枠の色は styles.css の @layer base（--color-input）に任せ、ここでは寸法だけを持つ。
const inputClass = "block w-full min-h-11 rounded border p-2";
const fieldErrorClass = "mt-1 text-label text-danger-foreground";
export const ADAPTER_CHOICES = ["claude-code", "codex", "opencode-go"] as const;

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

/** 除外理由（GET /accounts の excluded_reason）の日本語。未知の値はそのまま出す（旧 GUI と同じ語）。 */
export const EXCLUDED_REASON_LABEL: Readonly<Record<string, string>> = {
  not_logged_in: "未ログイン",
  at_capacity: "上限に達しています",
  cooldown: "cooldown 中",
  five_hour_exhausted: "短期枠を使い切りました",
  seven_day_exhausted: "長期枠を使い切りました",
  one_month_exhausted: "1 か月の枠切れ",
  rejected: "拒否されました",
};

export function excludedReasonLabel(reason: string): string {
  const label = Object.hasOwn(EXCLUDED_REASON_LABEL, reason) ? EXCLUDED_REASON_LABEL[reason] : undefined;
  return label ? `${label}（${reason}）` : reason;
}

export type UsageTone = "normal" | "warning" | "danger";

/** 使用率の段階（旧 GUI の usageTone と同じ境目: 70% で注意、90% で上限間近）。 */
export function usageTone(utilization: number): UsageTone {
  if (utilization >= 0.9) return "danger";
  if (utilization >= 0.7) return "warning";
  return "normal";
}

const USAGE_TONE_WORD: Readonly<Record<UsageTone, string>> = { normal: "", warning: "注意", danger: "上限間近" };
// 塗りは状態色の前景（文字色と同じ濃さ）を使い、地の muted と区別できる明度差を保つ。
const USAGE_FILL: Readonly<Record<UsageTone, string>> = {
  normal: "bg-primary",
  warning: "bg-warning-foreground",
  danger: "bg-danger-foreground",
};

/** 残り時間を上位 2 単位で表す（例「2時間15分」「3日4時間」）。0 以下は「リセット時刻を過ぎました」。 */
export function formatRemaining(ms: number): string {
  if (!Number.isFinite(ms)) return "-";
  if (ms <= 0) return "リセット時刻を過ぎました";
  const totalMin = Math.floor(ms / 60_000);
  if (totalMin < 1) return "1分未満";
  const days = Math.floor(totalMin / 1440);
  const hours = Math.floor((totalMin % 1440) / 60);
  const minutes = totalMin % 60;
  const parts: string[] = [];
  if (days) parts.push(`${days}日`);
  if (hours) parts.push(`${hours}時間`);
  if (minutes && parts.length < 2) parts.push(`${minutes}分`);
  return parts.slice(0, 2).join("");
}

type RateWindow = { utilization: number; resets_at: string };

/** 5 時間枠・7 日枠・1 か月枠の残量。残り時間は取得時刻（server 時計に補正）を基準にする。窓が無ければ「不明」（0% とは書かない）。 */
export function UsageBar({
  label,
  window,
  fetchedAtMs,
}: {
  label: string;
  window: RateWindow | null | undefined;
  fetchedAtMs: number;
}) {
  if (!window) {
    return (
      <div className="min-w-0" data-usage-window="none">
        <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 text-label">
          <span className="font-medium text-foreground">{label}</span>
          <span className="text-muted-foreground">不明</span>
        </div>
        <div
          aria-hidden="true"
          className="mt-1 h-2 w-full overflow-hidden rounded-full border border-border bg-muted"
        />
      </div>
    );
  }
  const used = Math.min(100, Math.max(0, Math.round(window.utilization * 100)));
  const left = 100 - used;
  const tone = usageTone(window.utilization);
  const word = USAGE_TONE_WORD[tone];
  const remaining = formatRemaining(Date.parse(window.resets_at) - fetchedAtMs);
  const text = `使用 ${used}% / 残り ${left}%`;
  return (
    <div className="min-w-0" data-usage-tone={tone}>
      <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 text-label">
        <span className="font-medium text-foreground">{label}</span>
        <span className="tabular-nums text-foreground">
          {text}
          {word && (
            <span className={tone === "danger" ? "text-danger-foreground" : "text-warning-foreground"}>（{word}）</span>
          )}
        </span>
      </div>
      {/* biome-ignore lint/a11y/useSemanticElements: 素の <meter> は browser ごとに塗りの色・dark の扱いが揃わないので、token で塗る div に role を付ける。 */}
      <div
        role="meter"
        aria-label={label}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={used}
        aria-valuetext={`${text}${word ? `・${word}` : ""}・リセットまで ${remaining}`}
        className="mt-1 h-2 w-full overflow-hidden rounded-full border border-border bg-muted"
      >
        <div className={cn("h-full", USAGE_FILL[tone])} style={{ width: `${used}%` }} />
      </div>
      <p className="mt-1 break-words text-label text-muted-foreground">
        リセットまで {remaining}（{formatAbsolute(window.resets_at)}）
      </p>
    </div>
  );
}

/** account の状態を 1 つの語にまとめる。重いもの（除外・失敗）から順に見る。色だけにせず必ず文字を出す。 */
export function accountState(item: AccountView): { tone: BadgeTone; label: string; detail?: string } {
  if (item.excluded_reason)
    return { tone: "danger", label: "除外中", detail: excludedReasonLabel(item.excluded_reason) };
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
  fetchedAtMs,
  sender,
  blocked,
  deniedId,
  login,
  setLogin,
}: {
  item: AccountView;
  max: number;
  /** 一覧を取得した時刻（server 時計に補正済み）。残り時間と観測の相対時刻の基準。 */
  fetchedAtMs: number;
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
      <section className="min-w-0 space-y-3" aria-label={`使用量 ${item.id}`}>
        <UsageBar label="短期枠（5時間）" window={item.usage?.five_hour} fetchedAtMs={fetchedAtMs} />
        <UsageBar label="長期枠（7日）" window={item.usage?.seven_day} fetchedAtMs={fetchedAtMs} />
        <UsageBar label="月間枠（1か月）" window={item.usage?.one_month} fetchedAtMs={fetchedAtMs} />
      </section>
      <DataList
        items={[
          {
            key: "state",
            label: "状態",
            value: state.detail ?? "使える状態です",
          },
          {
            key: "observed",
            label: "使用量の観測",
            value: item.usage
              ? `${formatAbsolute(item.usage.observed_at)}（${formatRelative(item.usage.observed_at, fetchedAtMs)}・${item.usage.source}${item.usage.status ? `・${item.usage.status}` : ""}）`
              : "-",
          },
          {
            key: "score",
            label: "選択の score",
            value:
              item.score != null
                ? item.score.toFixed(2)
                : item.excluded_reason
                  ? `除外: ${excludedReasonLabel(item.excluded_reason)}`
                  : "-",
          },
          { key: "adapter", label: "道具", value: adapterLabel(adapter) },
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
          道具（adapter）
          <select className={inputClass} value={adapter} onChange={(e) => setAdapter(e.target.value)}>
            {ADAPTER_CHOICES.map((a) => (
              <option key={a} value={a}>
                {adapterLabel(a)}
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
                        fetchedAtMs={query.dataUpdatedAt + getServerClockOffset()}
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
