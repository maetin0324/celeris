import { useEffect, useRef } from "react";
import { Link, useFetcher } from "react-router";
import type { BrowserWait, BrowserWaitItem as BrowserWaitItemView } from "~/celeris/types";
import { Badge } from "~/components/ui/badge";
import { Button } from "~/components/ui/button";
import { Card, CardBody, CardHeader } from "~/components/ui/card";
import { hintClass, inputClass, labelClass } from "~/components/ui/form";

/**
 * ADR-0080 D5: browser の人待ち（credential の登録依頼・一回だけの承認）。
 * - 表示は「サイト（trusted exact origin）」「task/run」「用途（untrusted な plain text）」「期限」
 * - 登録フォームと承認ボタンは本人（owner grant）の session にだけ出す
 * - 秘密の欄は既定値を持たず、autocomplete/自動補完を切り、送信後に消す。応答は固定コードだけ
 */

/** 画面に渡す本人状態（`~/browser-owner.server` の `BrowserOwnerView` と同じ形）。 */
export interface BrowserOwnerProps {
  available: boolean;
  isOwner: boolean;
  challengePending: boolean;
  csrfToken: string | null;
}

interface ActionData {
  ok: boolean;
  code: string;
  task_status?: string;
  challenge?: string;
}

const CODE_TEXT: Record<string, string> = {
  registered: "登録しました。タスクは再開待ちになり、使う前に改めて承認を求めます。",
  approved: "一回だけ承認しました。",
  denied: "拒否しました。この操作は実行されません。",
  challenge_issued: "本人確認のコードを発行しました。",
  csrf_failed: "送信元を確認できませんでした。画面を読み込み直してください。",
  unauthenticated: "ログインが切れています。",
  owner_unavailable: "この構成では本人を識別できません。",
  not_owner: "この操作は本人のセッションでだけ行えます。",
  payload_too_large: "入力が大きすぎます。",
  invalid_input: "入力を確認してください。",
  wait_not_found: "この依頼は見つかりません。",
  wait_not_actionable: "この依頼はもう操作できません。",
  version_conflict: "依頼の状態が変わりました。画面を読み込み直してください。",
  wait_gone: "この依頼は期限切れです。",
  attestation_unavailable: "GUI の署名鍵が設定されていないため操作できません。",
  rejected: "Celeris が受け付けませんでした。",
  celeris_unavailable: "Celeris に接続できません。",
};

export function browserActionMessage(code: string): string {
  return CODE_TEXT[code] ?? "操作に失敗しました。";
}

export function browserWaitLabel(w: BrowserWait): string {
  return w.reason === "waiting_for_auth" ? "WAITING_FOR_AUTH" : "WAITING_FOR_APPROVAL";
}

function formatDeadline(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime())
    ? iso
    : d
        .toISOString()
        .replace("T", " ")
        .replace(/\.\d+Z$/, "Z");
}

function ActionResult({ data }: { data: ActionData | undefined }) {
  if (!data) return null;
  return (
    <p role="status" className={data.ok ? "text-sm text-success-soft-fg" : "text-sm text-danger-soft-fg"}>
      {browserActionMessage(data.code)}
    </p>
  );
}

export function OwnerSessionNotice({ owner }: { owner: BrowserOwnerProps }) {
  const fetcher = useFetcher<ActionData>();
  if (!owner.available) {
    return (
      <p className="text-sm text-fg-muted" data-testid="browser-owner-unavailable">
        credential の登録・承認と Live View は、パスワード認証を有効にした単一所有者の Celeris でだけ使えます。
      </p>
    );
  }
  if (owner.isOwner) return null;
  const challenge = fetcher.data?.challenge;
  return (
    <div className="space-y-2" data-testid="browser-owner-request">
      <p className="text-sm text-fg-muted">
        このセッションは本人として登録されていません。登録・承認・Live View は本人のセッションでだけ操作できます。
      </p>
      <fetcher.Form method="post" action="/browser/owner-session">
        <Button type="submit" variant="secondary" size="sm" disabled={fetcher.state !== "idle"}>
          このセッションを本人として登録
        </Button>
      </fetcher.Form>
      {challenge ? (
        <p className="text-sm">
          ローカルの端末で次を実行してください（5 分間有効）:{" "}
          <code className="font-mono">celerisctl browser owner-session approve {challenge}</code>
        </p>
      ) : (
        <ActionResult data={fetcher.data} />
      )}
    </div>
  );
}

function WaitSummary({ wait }: { wait: BrowserWait }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm">
      <dt className="text-fg-muted">サイト</dt>
      <dd className="break-all font-mono">{wait.origin}</dd>
      <dt className="text-fg-muted">用途</dt>
      {/* untrusted な plain text。そのまま文字として出す（リンクにしない） */}
      <dd className="whitespace-pre-wrap break-words">{wait.purpose}</dd>
      <dt className="text-fg-muted">実行</dt>
      <dd className="break-all font-mono">
        {wait.run_id} / {wait.session_id}
      </dd>
      <dt className="text-fg-muted">期限</dt>
      <dd>{formatDeadline(wait.deadline)}</dd>
    </dl>
  );
}

export function CredentialRegistrationForm({ wait, csrfToken }: { wait: BrowserWait; csrfToken: string }) {
  const fetcher = useFetcher<ActionData>();
  const formRef = useRef<HTMLFormElement>(null);
  const done = fetcher.state === "idle" && fetcher.data !== undefined;
  // 送信が終わったら秘密の欄を必ず空にする（成功・失敗とも）。
  useEffect(() => {
    if (done) formRef.current?.reset();
  }, [done]);
  const id = `cred-${wait.wait_id}`;
  return (
    <fetcher.Form
      ref={formRef}
      method="post"
      action={`/browser/waits/${encodeURIComponent(wait.wait_id)}/credential`}
      autoComplete="off"
      className="space-y-3"
      data-testid="browser-credential-form"
    >
      <input type="hidden" name="csrf" value={csrfToken} />
      <input type="hidden" name="task_id" value={wait.task_id} />
      <input type="hidden" name="expected_version" value={String(wait.version)} />
      <div className="space-y-1">
        <label htmlFor={`${id}-user`} className={labelClass}>
          ユーザー名
        </label>
        <input
          id={`${id}-user`}
          name="username"
          type="text"
          required
          maxLength={256}
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          data-1p-ignore=""
          data-lpignore="true"
          className={inputClass}
        />
      </div>
      <div className="space-y-1">
        <label htmlFor={`${id}-pass`} className={labelClass}>
          パスワード
        </label>
        <input
          id={`${id}-pass`}
          name="password"
          type="password"
          required
          maxLength={1024}
          autoComplete="off"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          data-1p-ignore=""
          data-lpignore="true"
          className={inputClass}
        />
      </div>
      <p className={hintClass}>
        値は Celeris の credential broker
        にだけ渡り、画面・ログ・タスクの記録には残りません。登録しても使用は承認されず、使う前に一回ごとの承認を求めます。
      </p>
      <Button type="submit" size="sm" disabled={fetcher.state !== "idle"}>
        登録する
      </Button>
      <ActionResult data={fetcher.data} />
    </fetcher.Form>
  );
}

export function ApprovalDecisionForm({ wait, csrfToken }: { wait: BrowserWait; csrfToken: string }) {
  const fetcher = useFetcher<ActionData>();
  return (
    <fetcher.Form
      method="post"
      action={`/browser/waits/${encodeURIComponent(wait.wait_id)}/decision`}
      className="space-y-2"
      data-testid="browser-decision-form"
    >
      <input type="hidden" name="csrf" value={csrfToken} />
      <input type="hidden" name="task_id" value={wait.task_id} />
      <input type="hidden" name="expected_version" value={String(wait.version)} />
      <div className="flex flex-wrap gap-2">
        <Button
          type="submit"
          name="decision"
          value="approve_once"
          variant="success"
          size="sm"
          disabled={fetcher.state !== "idle"}
        >
          一回だけ承認
        </Button>
        <Button
          type="submit"
          name="decision"
          value="deny"
          variant="danger"
          size="sm"
          disabled={fetcher.state !== "idle"}
        >
          拒否
        </Button>
      </div>
      <ActionResult data={fetcher.data} />
    </fetcher.Form>
  );
}

function ApprovalTarget({ wait }: { wait: BrowserWait }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-sm" data-testid="browser-approval-target">
      <dt className="text-fg-muted">対象操作</dt>
      <dd className="font-mono">{wait.operation?.action ?? "credential_use"}</dd>
      {wait.operation?.args_digest ? (
        <>
          <dt className="text-fg-muted">引数 digest</dt>
          <dd className="break-all font-mono">{wait.operation.args_digest}</dd>
        </>
      ) : null}
      {wait.credential ? (
        <>
          <dt className="text-fg-muted">credential</dt>
          <dd className="break-all font-mono">
            {wait.credential.credential_id}（{wait.credential.provider}）
          </dd>
        </>
      ) : null}
      <dt className="text-fg-muted">policy revision</dt>
      <dd className="break-all font-mono">
        {wait.policy_revision}（{wait.policy_hash.slice(0, 12)}）
      </dd>
    </dl>
  );
}

export function BrowserWaitItem({ wait, owner }: { wait: BrowserWait; owner: BrowserOwnerProps }) {
  const open = wait.state === "pending";
  const csrf = owner.isOwner ? owner.csrfToken : null;
  return (
    <div className="space-y-2 break-words" data-testid="browser-wait" data-wait-reason={wait.reason}>
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={open ? "warning" : wait.state === "denied" || wait.state === "expired" ? "danger" : "neutral"}>
          {browserWaitLabel(wait)}
        </Badge>
        <span className="text-sm text-fg-muted">
          {wait.reason === "waiting_for_auth" ? "credential の登録依頼" : "一回だけの承認依頼"}・{wait.state}
        </span>
      </div>
      <WaitSummary wait={wait} />
      {wait.reason === "waiting_for_approval" ? <ApprovalTarget wait={wait} /> : null}
      {open && csrf ? (
        wait.reason === "waiting_for_auth" ? (
          <CredentialRegistrationForm wait={wait} csrfToken={csrf} />
        ) : (
          <ApprovalDecisionForm wait={wait} csrfToken={csrf} />
        )
      ) : null}
    </div>
  );
}

export function BrowserWaitsPanel({ waits, owner }: { waits: BrowserWait[]; owner: BrowserOwnerProps }) {
  if (waits.length === 0) return null;
  const open = waits.filter((w) => w.state === "pending");
  const closed = waits.filter((w) => w.state !== "pending");
  return (
    <section aria-label="Browser waits" data-testid="browser-waits" id="browser-waits">
      <Card>
        <CardHeader title="ブラウザの人待ち" />
        <CardBody className="space-y-4">
          {open.length > 0 ? <OwnerSessionNotice owner={owner} /> : null}
          {[...open, ...closed].map((w) => (
            <BrowserWaitItem key={w.wait_id} wait={w} owner={owner} />
          ))}
        </CardBody>
      </Card>
    </section>
  );
}

/** inbox / 認可画面の一覧（操作は task 画面の本人専用フォームで行う）。 */
export function BrowserWaitInboxList({ items }: { items: BrowserWaitItemView[] }) {
  if (items.length === 0) return null;
  return (
    <ul className="space-y-3" data-testid="browser-wait-inbox">
      {items.map((item) => (
        <li key={item.wait.wait_id} className="space-y-1 rounded-lg border border-border p-3 break-words">
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone="warning">{item.run_state}</Badge>
            <Link to={`/tasks/${encodeURIComponent(item.task.id)}#browser-waits`} className="font-medium">
              {item.task.title}
            </Link>
          </div>
          <p className="text-sm">
            <span className="text-fg-muted">サイト: </span>
            <span className="break-all font-mono">{item.wait.origin}</span>
          </p>
          <p className="whitespace-pre-wrap text-sm">
            <span className="text-fg-muted">用途: </span>
            {item.wait.purpose}
          </p>
          <p className="text-sm text-fg-muted">
            {item.wait.reason === "waiting_for_auth"
              ? "credential の登録依頼"
              : `承認依頼: ${item.wait.operation?.action ?? "credential_use"}`}
            ・期限 {formatDeadline(item.wait.deadline)}
          </p>
        </li>
      ))}
    </ul>
  );
}
