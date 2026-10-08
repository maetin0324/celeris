import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useId, useState } from "react";
import type { BrowserSitePolicyRecord, SitePolicyPutBody } from "../../api/generated/types";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { Button } from "../../components/ui/button";
import { ConfirmDialog } from "../../components/ui/confirm-dialog";
import { Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import { deleteSitePolicy, saveSitePolicy, sitePoliciesQuery, sitePolicyError } from "./site-policy-query";

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
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function save(event: FormEvent) {
    event.preventDefault();
    if (pending) return;
    setPending(true);
    setError(null);
    try {
      await onSave(policyId.trim(), {
        exact_origin: origin.trim(),
        login_url: login.trim(),
        password_selector: password.trim(),
        submit_selector: submit.trim() || null,
      });
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
                    <dt>password / submit selector</dt>
                    <dd>
                      {policy.password_selector} / {policy.submit_selector || "なし"}
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
