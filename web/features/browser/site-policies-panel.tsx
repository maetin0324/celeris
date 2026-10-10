import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useId, useState } from "react";
import type { BrowserSitePolicyRecord, PostLoginAction, SitePolicyPutBody } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import { deleteSitePolicy, saveSitePolicy, sitePoliciesQuery, sitePolicyError } from "./site-policy-query";

/** ログイン後の読み取りで選べる action（ADR 2026-10-09 credential username / post-login D2-6）。 */
export const POST_LOGIN_ACTIONS: ReadonlyArray<{ action: PostLoginAction; label: string }> = [
  { action: "snapshot", label: "snapshot（頁の構造）" },
  { action: "extract", label: "extract（本文の抜き出し）" },
  { action: "screenshot", label: "screenshot（password 欄のある頁では撮らない）" },
  { action: "download", label: "download（読み取り先 origin のファイルだけ）" },
  { action: "click", label: "click（読み取り先 origin の頁だけ）" },
];

export type SitePolicyFields = {
  origin: string;
  login: string;
  password: string;
  submit: string;
  username: string;
  postLogin: boolean;
  readOrigins: string;
  actions: PostLoginAction[];
  acknowledged: boolean;
  /** IdP の同意頁で controller が 1 回だけ押す固定ボタン（任意、post_login と組）。 */
  consentSelector?: string;
  consentChoice?: string;
};

/**
 * 入力から PUT の本文を作る。ログイン後の読み取りを有効にするなら、読み取り先 origin・action と、
 * 頁の内容が LLM に渡ることの確認がそろっていなければ送らない（文言だけを返す）。
 */
export function sitePolicyBody(f: SitePolicyFields): { body: SitePolicyPutBody } | { error: string } {
  const body: SitePolicyPutBody = {
    exact_origin: f.origin.trim(),
    login_url: f.login.trim(),
    password_selector: f.password.trim(),
    submit_selector: f.submit.trim() || null,
    username_selector: f.username.trim() || null,
    post_login: null,
    consent: null,
  };
  if (!f.postLogin && ((f.consentSelector ?? "").trim() || (f.consentChoice ?? "").trim()))
    return { error: "同意頁のボタンはログイン後の読み取りと組で設定します。" };
  if (!f.postLogin) return { body };
  const readOrigins = f.readOrigins
    .split(/\s+/)
    .map((o) => o.trim())
    .filter((o) => o.length > 0);
  if (readOrigins.length === 0) return { error: "ログイン後に読み取る origin を 1 つ以上入れてください。" };
  if (f.actions.length === 0) return { error: "ログイン後に許す操作を 1 つ以上選んでください。" };
  if (!f.acknowledged)
    return { error: "ログイン後の頁の内容（個人情報を含みうる）が LLM に渡ることを確認してください。" };
  const actions = POST_LOGIN_ACTIONS.map((a) => a.action).filter((a) => f.actions.includes(a));
  const consentSelector = (f.consentSelector ?? "").trim();
  const consentChoice = (f.consentChoice ?? "").trim();
  if (!consentSelector && consentChoice)
    return { error: "同意の選択肢だけでは押せません。同意ボタンの selector も入れてください。" };
  const consent = consentSelector ? { selector: consentSelector, choice_selector: consentChoice || null } : null;
  return { body: { ...body, post_login: { read_origins: readOrigins, actions }, consent } };
}

export function SitePolicyForm({
  policy,
  onSave,
  onCancel,
}: {
  policy: BrowserSitePolicyRecord | null;
  onSave: (policyId: string, body: SitePolicyPutBody) => Promise<void>;
  onCancel: () => void;
}) {
  const id = useId();
  const [policyId, setPolicyId] = useState(policy?.policy_id ?? "");
  const [origin, setOrigin] = useState(policy?.exact_origin ?? "");
  const [login, setLogin] = useState(policy?.login_url ?? "");
  const [password, setPassword] = useState(policy?.password_selector ?? "");
  const [submit, setSubmit] = useState(policy?.submit_selector ?? "");
  const [username, setUsername] = useState(policy?.username_selector ?? "");
  const [postLogin, setPostLogin] = useState(!!policy?.post_login);
  const [readOrigins, setReadOrigins] = useState((policy?.post_login?.read_origins ?? []).join("\n"));
  const [actions, setActions] = useState<PostLoginAction[]>(policy?.post_login?.actions ?? ["snapshot", "extract"]);
  const [consentSelector, setConsentSelector] = useState(policy?.consent?.selector ?? "");
  const [consentChoice, setConsentChoice] = useState(policy?.consent?.choice_selector ?? "");
  // 既存の opt-in を開き直しても、保存のたびに改めて確認を求める。
  const [acknowledged, setAcknowledged] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function save(event: FormEvent) {
    event.preventDefault();
    if (pending) return;
    const built = sitePolicyBody({
      origin,
      login,
      password,
      submit,
      username,
      postLogin,
      readOrigins,
      actions,
      acknowledged,
      consentSelector,
      consentChoice,
    });
    if ("error" in built) {
      setError(built.error);
      return;
    }
    setPending(true);
    setError(null);
    try {
      await onSave(policyId.trim(), built.body);
    } catch (reason) {
      setError(sitePolicyError(reason));
    } finally {
      setPending(false);
    }
  }
  return (
    <form
      className="space-y-4"
      onSubmit={save}
      aria-label={policy ? "ログイン先の編集" : "ログイン先の追加"}
      aria-busy={pending}
    >
      <fieldset disabled={pending} className="grid min-w-0 gap-4 sm:grid-cols-2">
        <label htmlFor={`${id}-policy`} className="min-w-0 space-y-1 text-label font-medium">
          policy ID
          <Input
            id={`${id}-policy`}
            required
            pattern="[A-Za-z0-9._-]{1,64}"
            maxLength={64}
            readOnly={!!policy}
            value={policyId}
            onChange={(e) => setPolicyId(e.target.value)}
          />
        </label>
        <label htmlFor={`${id}-origin`} className="min-w-0 space-y-1 text-label font-medium">
          正確な origin（exact_origin）
          <Input
            id={`${id}-origin`}
            required
            value={origin}
            placeholder="https://example.com"
            onChange={(e) => setOrigin(e.target.value)}
          />
        </label>
        <label htmlFor={`${id}-login`} className="min-w-0 space-y-1 text-label font-medium">
          ログイン URL（login_url）
          <Input id={`${id}-login`} required type="url" value={login} onChange={(e) => setLogin(e.target.value)} />
        </label>
        <label htmlFor={`${id}-password`} className="min-w-0 space-y-1 text-label font-medium">
          password selector
          <Input
            id={`${id}-password`}
            required
            value={password}
            placeholder='input[type="password"]'
            onChange={(e) => setPassword(e.target.value)}
          />
        </label>
        <label htmlFor={`${id}-submit`} className="min-w-0 space-y-1 text-label font-medium">
          submit selector（任意）
          <Input id={`${id}-submit`} value={submit} onChange={(e) => setSubmit(e.target.value)} />
        </label>
        <label htmlFor={`${id}-username`} className="min-w-0 space-y-1 text-label font-medium">
          username selector（任意。password 欄と同じ頁）
          <Input
            id={`${id}-username`}
            value={username}
            placeholder='input[name="j_username"]'
            onChange={(e) => setUsername(e.target.value)}
          />
        </label>
      </fieldset>
      <fieldset disabled={pending} className="min-w-0 space-y-3 rounded-md border border-border p-3">
        <legend className="px-1 text-label font-medium">ログイン後の読み取り（post_login・任意）</legend>
        <label className="flex min-h-11 min-w-11 items-start gap-2 text-label">
          <input
            type="checkbox"
            checked={postLogin}
            onChange={(e) => setPostLogin(e.target.checked)}
            aria-describedby={`${id}-post-note`}
          />
          <span>ログインした後、下の origin の頁を agent に読み取らせる</span>
        </label>
        <p id={`${id}-post-note`} className="text-label text-muted-foreground">
          有効にしなければ、ログインした session では終わりまで頁を読み取りません（今の動作）。ログイン画面（IdP）と
          password 欄のある頁は有効にしても読み取りません。
        </p>
        {postLogin ? (
          <div className="space-y-3">
            <label htmlFor={`${id}-read`} className="block min-w-0 space-y-1 text-label font-medium">
              読み取り先 origin（1 行に 1 つ。ログイン先の origin は入れられません）
              <textarea
                id={`${id}-read`}
                className="min-h-16 w-full rounded-md border border-border bg-background p-2 font-mono text-label"
                value={readOrigins}
                placeholder="https://lms.example.ac.jp"
                onChange={(e) => setReadOrigins(e.target.value)}
              />
            </label>
            <fieldset className="space-y-1">
              <legend className="text-label font-medium">ログイン後に許す操作</legend>
              {POST_LOGIN_ACTIONS.map(({ action, label }) => (
                <label key={action} className="flex min-h-11 min-w-11 items-center gap-2 text-label">
                  <input
                    type="checkbox"
                    checked={actions.includes(action)}
                    onChange={(e) =>
                      setActions((now) =>
                        e.target.checked
                          ? [...now.filter((a) => a !== action), action]
                          : now.filter((a) => a !== action),
                      )
                    }
                  />
                  <span>{label}</span>
                </label>
              ))}
            </fieldset>
            <label htmlFor={`${id}-consent`} className="block min-w-0 space-y-1 text-label font-medium">
              IdP の同意頁で押すボタンの selector（任意。1 回だけ押す）
              <Input
                id={`${id}-consent`}
                value={consentSelector}
                placeholder='input[name="_eventId_proceed"]'
                onChange={(e) => setConsentSelector(e.target.value)}
              />
            </label>
            <label htmlFor={`${id}-consent-choice`} className="block min-w-0 space-y-1 text-label font-medium">
              同意の選択肢の selector（任意。推奨: 次回も確認する）
              <Input
                id={`${id}-consent-choice`}
                value={consentChoice}
                placeholder='input[value="_shib_idp_doNotRememberConsent"]'
                onChange={(e) => setConsentChoice(e.target.value)}
              />
            </label>
            <p className="text-label text-muted-foreground">
              同意頁（属性送信の確認）が出たとき、Celeris の controller だけがこのボタンを 1 回押します（agent
              は押せません）。 空なら同意頁で止まり、進捗に同意 form の欄名が出ます。
            </p>
            <label className="flex min-h-11 min-w-11 items-start gap-2 text-label">
              <input type="checkbox" checked={acknowledged} onChange={(e) => setAcknowledged(e.target.checked)} />
              <span>
                ログイン後の頁の内容（氏名・学籍番号・成績・課題など個人情報を含みうる）が LLM
                とその提供元に渡ることを確認しました。credential の使用は毎回人の承認が要ります。
              </span>
            </label>
          </div>
        ) : null}
      </fieldset>
      <p className="text-label text-muted-foreground">
        origin は wildcard を含めず、ログイン URL は同じ origin を指定します。パスワードの値は入力しません。
      </p>
      {error ? (
        <p role="alert" className="text-danger-foreground">
          {error}
        </p>
      ) : null}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" disabled={pending}>
          {pending ? "保存中" : "ログイン先を保存"}
        </Button>
        <ConfirmDialog
          trigger={
            <Button type="button" variant="secondary" disabled={pending}>
              編集を閉じる
            </Button>
          }
          title="ログイン先の編集を閉じる"
          target={policyId || "追加中のログイン先"}
          consequence="保存していない入力を破棄します。"
          reversibility="入力し直せます。"
          followUp="保存済みの設定は一覧で確認できます。"
          confirmLabel="入力を破棄して閉じる"
          onConfirm={onCancel}
        />
      </div>
    </form>
  );
}

export function SitePoliciesPanel({ csrf }: { csrf: string }) {
  const client = useQueryClient();
  const query = useQuery(sitePoliciesQuery());
  const [editing, setEditing] = useState<BrowserSitePolicyRecord | null | undefined>(undefined);
  const [message, setMessage] = useState<string | null>(null);
  return (
    <Section
      title="ログイン先の設定（site policy）"
      description="登録したログイン先を実行課の credential policy に選びます。変更は再起動なしで反映されます。"
    >
      <div className="space-y-4">
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            disabled={editing !== undefined}
            onClick={() => {
              setEditing(null);
              setMessage(null);
            }}
          >
            ログイン先を追加
          </Button>
          <Button variant="secondary" onClick={() => void query.refetch()}>
            一覧を再取得
          </Button>
        </div>
        <FetchFrame query={query} subject="ログイン先の設定">
          {query.data?.items.length === 0 ? <p>ログイン先はまだ登録されていません。</p> : null}
          <ul className="divide-y divide-border" aria-label="ログイン先の一覧">
            {query.data?.items.map((policy) => (
              <li key={policy.policy_id} className="min-w-0 space-y-2 py-3">
                <p className="break-all font-medium">
                  {policy.policy_id} · {policy.exact_origin}
                </p>
                <dl className="space-y-1 break-all text-label">
                  <div>
                    <dt>ログイン URL</dt>
                    <dd>{policy.login_url}</dd>
                  </div>
                  <div>
                    <dt>username / password / submit selector</dt>
                    <dd>
                      {policy.username_selector || "なし"} / {policy.password_selector} /{" "}
                      {policy.submit_selector || "なし"}
                    </dd>
                  </div>
                  <div>
                    <dt>ログイン後の読み取り</dt>
                    <dd>
                      {policy.post_login
                        ? `${policy.post_login.read_origins.join(", ")}（${policy.post_login.actions.join(", ")}）`
                        : "なし（ログイン後は読み取らない）"}
                    </dd>
                  </div>
                  <div>
                    <dt>同意頁で押すボタン</dt>
                    <dd>
                      {policy.consent
                        ? `${policy.consent.selector}${policy.consent.choice_selector ? `（選択 ${policy.consent.choice_selector}）` : ""}`
                        : "なし（同意頁で止まる）"}
                    </dd>
                  </div>
                  <div>
                    <dt>更新時刻</dt>
                    <dd>
                      <time dateTime={policy.updated_at}>{policy.updated_at}</time> ·{" "}
                      {policy.source === "config" ? "初期設定から登録" : "API から登録"}
                    </dd>
                  </div>
                </dl>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="secondary"
                    disabled={editing !== undefined}
                    onClick={() => {
                      setEditing(policy);
                      setMessage(null);
                    }}
                    aria-label={`${policy.policy_id} を編集`}
                  >
                    編集
                  </Button>
                  <ConfirmDialog
                    trigger={
                      <Button variant="secondary" aria-label={`${policy.policy_id} を削除`}>
                        削除
                      </Button>
                    }
                    title="ログイン先を削除"
                    target={policy.policy_id}
                    consequence="このログイン先の登録を削除します。実行課や承認待ちが参照している場合は削除できません。"
                    reversibility="同じ ID と内容で追加し直せます。"
                    followUp="この一覧で削除後の状態を確認できます。"
                    confirmLabel={`${policy.policy_id} を削除`}
                    onConfirm={async () => {
                      try {
                        await deleteSitePolicy(client, policy.policy_id, csrf);
                        if (editing?.policy_id === policy.policy_id) setEditing(undefined);
                        setMessage(`${policy.policy_id} を削除しました。`);
                      } catch (error) {
                        throw new Error(sitePolicyError(error));
                      }
                    }}
                  />
                </div>
              </li>
            ))}
          </ul>
        </FetchFrame>
        {editing !== undefined ? (
          <SitePolicyForm
            key={editing?.policy_id ?? "new"}
            policy={editing}
            onCancel={() => setEditing(undefined)}
            onSave={async (policyId, body) => {
              await saveSitePolicy(client, policyId, body, csrf);
              setEditing(undefined);
              setMessage(`${policyId} を保存しました。`);
            }}
          />
        ) : null}
        {message ? (
          <p role="status" className="text-success-foreground">
            {message}
          </p>
        ) : null}
      </div>
    </Section>
  );
}
