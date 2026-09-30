import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { apiGet } from "../../api/client";
import type { AccountCheckResponse, AccountList, AccountLoginStart, AccountView } from "../../api/generated/types";
import { accountKeys } from "../../api/queries/keys";
import { ActionResultView, useActionResult } from "../../components/actions/use-action-result";
import { FetchFrame } from "../../components/fetch-state/fetch-frame";
import { ScreenFrame } from "../../components/shell/screen-frame";
import { Button } from "../../components/ui/button";
import { McpClientsSection } from "./mcp-clients";
import { SecretsSection } from "./secrets-section";

type Sender = ReturnType<typeof useActionResult>;
const inputClass = "block w-full min-h-11 rounded border p-2";
export const ADAPTER_CHOICES = ["claude-code", "codex"] as const;

/** ログイン中の状態（url・user_code）は account 単位で画面の state に持つ。取り直しでは消さない。 */
type Logins = Record<string, AccountLoginStart>;

function accountKey(item: AccountView) {
  return `${item.adapter ?? "claude-code"}:${item.id}`;
}

function AccountCard({
  item,
  sender,
  login,
  setLogin,
}: {
  item: AccountView;
  sender: Sender;
  login: AccountLoginStart | undefined;
  setLogin: (key: string, value: AccountLoginStart | null) => void;
}) {
  const [code, setCode] = useState("");
  const adapter = item.adapter ?? "claude-code";
  const key = accountKey(item);
  const base = `/api/accounts/${encodeURIComponent(item.id)}`;
  const q = `?adapter=${encodeURIComponent(adapter)}`;
  const check = sender.results[`check:${key}`];
  const checked = check?.ok ? (check.response as AccountCheckResponse | undefined) : undefined;
  // サーバが login_pending を返すなら、url が手元に無くてもコード入力欄を残す。
  const awaiting = item.login_pending || login !== undefined;
  return (
    <li className="min-w-0 rounded border p-3 space-y-2" aria-label={`アカウント ${item.id}`}>
      <h3 className="font-semibold break-words">{item.id}</h3>
      <p className="text-sm break-words">
        {adapter} / {item.logged_in ? "ログイン済み" : "未ログイン"} / 使用中 {item.in_use}
        {item.cooldown ? ` / cooldown ${item.cooldown.reason}（${item.cooldown.until} まで）` : ""}
      </p>
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={sender.pending}
          onClick={() => void sender.run([{ id: `check:${key}`, path: `${base}/check${q}`, body: {} }])}
        >
          確認
        </Button>
        <Button
          disabled={sender.pending}
          onClick={() =>
            void sender.run([{ id: `login:${key}`, path: `${base}/login${q}`, body: {} }]).then((out) => {
              if (out[0]?.ok) setLogin(key, out[0].response as AccountLoginStart);
            })
          }
        >
          ログイン開始
        </Button>
        <Button
          disabled={sender.pending}
          onClick={() => {
            if (window.confirm(`アカウント ${item.id} を削除しますか`))
              void sender.run([{ id: `delete:${key}`, path: `${base}${q}`, method: "DELETE" }]);
          }}
        >
          削除
        </Button>
      </div>
      <ActionResultView result={check} />
      {checked && (
        <p role="status">
          確認結果: {checked.result}
          {checked.detail ? `（${checked.detail}）` : ""}
        </p>
      )}
      <ActionResultView result={sender.results[`delete:${key}`]} />
      <ActionResultView result={sender.results[`login:${key}`]} />
      {awaiting && (
        <section className="rounded border p-2 space-y-2" aria-label={`ログイン ${item.id}`}>
          <p role="status">ログイン待ち</p>
          {login && (
            <p className="text-sm break-words">
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
          <label className="block">
            認証コード
            <input
              className={inputClass}
              type="password"
              autoComplete="off"
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
          <div className="flex flex-wrap gap-2">
            <Button
              disabled={sender.pending || code.trim() === ""}
              onClick={() => {
                const sent = code.trim();
                setCode("");
                void sender
                  .run([{ id: `code:${key}`, path: `${base}/login/code${q}`, body: { code: sent } }])
                  .then((out) => {
                    if (out[0]?.ok) setLogin(key, null);
                  });
              }}
            >
              コードを送る
            </Button>
            <Button
              disabled={sender.pending}
              onClick={() =>
                void sender.run([{ id: `cancel:${key}`, path: `${base}/login${q}`, method: "DELETE" }]).then((out) => {
                  if (out[0]?.ok) setLogin(key, null);
                })
              }
            >
              ログインを取り消す
            </Button>
          </div>
          <ActionResultView result={sender.results[`code:${key}`]} />
          <ActionResultView result={sender.results[`cancel:${key}`]} />
        </section>
      )}
    </li>
  );
}

function CreateForm({ sender }: { sender: Sender }) {
  const [id, setId] = useState("");
  const [adapter, setAdapter] = useState<string>("claude-code");
  return (
    <form
      className="rounded-lg border border-neutral-300 p-3 space-y-2 min-w-0"
      aria-label="アカウントを追加"
      onSubmit={(event) => {
        event.preventDefault();
        void sender
          .run([{ id: `create:${id.trim()}`, path: "/api/accounts", body: { id: id.trim(), adapter } }])
          .then((out) => {
            if (out[0]?.ok) setId("");
          });
      }}
    >
      <h2 className="text-lg font-semibold">アカウントを追加</h2>
      <div className="grid gap-2 sm:grid-cols-2">
        <label className="block">
          アカウント id
          <input className={inputClass} required value={id} onChange={(e) => setId(e.target.value)} />
        </label>
        <label className="block">
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
      <Button type="submit" disabled={sender.pending}>
        アカウントを追加
      </Button>
      <ActionResultView result={sender.results[`create:${id.trim()}`]} />
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
  const setLogin = (key: string, value: AccountLoginStart | null) =>
    setLogins((prev) => {
      const next = { ...prev };
      if (value) next[key] = value;
      else delete next[key];
      return next;
    });
  return (
    <ScreenFrame title="アカウント" route="/accounts">
      <div className="space-y-4 min-w-0">
        <FetchFrame query={query}>
          {query.data && (
            <div className="space-y-4 min-w-0">
              <section className="space-y-2 min-w-0" aria-label="アカウント一覧">
                <h2 className="text-lg font-semibold">アカウント一覧（{query.data.items.length}）</h2>
                {query.data.items.length === 0 ? (
                  <p>アカウントがありません。</p>
                ) : (
                  <ul className="space-y-3">
                    {query.data.items.map((item) => (
                      <AccountCard
                        key={accountKey(item)}
                        item={item}
                        sender={sender}
                        login={logins[accountKey(item)]}
                        setLogin={setLogin}
                      />
                    ))}
                  </ul>
                )}
              </section>
              <CreateForm sender={sender} />
            </div>
          )}
        </FetchFrame>
        <SecretsSection />
        <McpClientsSection />
      </div>
    </ScreenFrame>
  );
}
