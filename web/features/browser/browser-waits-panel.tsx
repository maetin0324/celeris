import { useQuery } from "@tanstack/react-query";
import type { FormEvent } from "react";
import { useId, useState } from "react";
import type { BrowserWait } from "../../api/generated/types";
import { Badge } from "../../components/ui/badge";
import { Button } from "../../components/ui/button";
import { Input } from "../../components/ui/input";
import { Section } from "../../components/ui/panel";
import { browserWaitKind } from "./browser-model";
import {
  answerBrowserCredential,
  answerBrowserDecision,
  BrowserGatewayError,
  type OwnerSession,
  ownerSessionQuery,
} from "./browser-query";
import { OwnerSessionNotice, ownerNoticeReason } from "./owner-session-notice";

const WAIT_ACTION_TEXT: Record<string, string> = {
  approved: "一回だけ承認しました。",
  denied: "拒否しました。この操作は実行されません。",
  registered: "登録しました。使う前に改めて承認を求めます。",
  csrf_failed: "送信元を確認できませんでした。画面を読み込み直してください。",
  unauthenticated: "ログインが切れています。",
  owner_unavailable: "この構成では本人を識別できません。",
  not_owner: "この操作は本人のセッションでだけ行えます。",
  payload_too_large: "入力が大きすぎます。",
  invalid_input: "入力を確認してください。",
  not_found: "この依頼は見つかりません。",
  wait_not_found: "この依頼は見つかりません。",
  wait_not_actionable: "この依頼はもう操作できません。",
  version_conflict: "依頼の状態が変わりました。画面を読み込み直してください。",
  wait_gone: "この依頼は期限切れです。",
  attestation_unavailable: "署名鍵が設定されていないため操作できません。",
  rejected: "Celeris が受け付けませんでした。",
  celeris_unavailable: "Celeris に接続できません。",
  browser_action_pending: "前の操作がまだ終わっていません。少し待って確認してください。",
  request_failed: "操作に失敗しました。",
};

/** 固定コードの文言だけを出す（celeris の応答本文・入力値は出さない）。 */
export function waitActionMessage(code: string): string {
  return WAIT_ACTION_TEXT[code] ?? "操作に失敗しました。";
}

export type WaitActionOutcome = { ok: boolean; message: string };

function outcomeFromError(error: unknown): WaitActionOutcome {
  const code = error instanceof BrowserGatewayError ? error.code : "request_failed";
  return { ok: false, message: waitActionMessage(code) };
}

/** 承認・拒否を送る。呼び出し側は結果が来るまで pending を保つだけでよい。 */
export async function submitDecision(send: () => Promise<{ ok: true; code: string }>): Promise<WaitActionOutcome> {
  try {
    const result = await send();
    return { ok: true, message: waitActionMessage(result.code) };
  } catch (error) {
    return outcomeFromError(error);
  }
}

/** credential を送る。成功・失敗のいずれでも username・password は空にして返す — 呼び出し側の state に値を残さない。 */
export async function submitCredential(
  send: (username: string, password: string) => Promise<{ ok: true; code: string }>,
  username: string,
  password: string,
): Promise<{ username: ""; password: ""; outcome: WaitActionOutcome }> {
  try {
    const result = await send(username, password);
    return { username: "", password: "", outcome: { ok: true, message: waitActionMessage(result.code) } };
  } catch (error) {
    return { username: "", password: "", outcome: outcomeFromError(error) };
  }
}

function OutcomeText({ outcome }: { outcome: WaitActionOutcome | null }) {
  if (!outcome) return null;
  return (
    <p
      role={outcome.ok ? "status" : "alert"}
      className={outcome.ok ? "text-success-foreground" : "text-danger-foreground"}
    >
      {outcome.message}
    </p>
  );
}

function formatDeadline(iso: string): string {
  const parsed = new Date(iso);
  return Number.isNaN(parsed.getTime())
    ? iso
    : parsed
        .toISOString()
        .replace("T", " ")
        .replace(/\.\d+Z$/, "Z");
}

function WaitSummary({ wait }: { wait: BrowserWait }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-label" data-testid="browser-wait-summary">
      <dt className="text-muted-foreground">サイト</dt>
      <dd className="break-all font-mono">{wait.origin}</dd>
      <dt className="text-muted-foreground">用途</dt>
      <dd className="whitespace-pre-wrap break-words">{wait.purpose}</dd>
      <dt className="text-muted-foreground">期限</dt>
      <dd>{formatDeadline(wait.deadline)}</dd>
    </dl>
  );
}

function OperationSummary({ wait }: { wait: BrowserWait }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1 text-label" data-testid="browser-wait-operation">
      <dt className="text-muted-foreground">対象操作</dt>
      <dd className="break-all font-mono">{wait.operation?.action ?? "credential_use"}</dd>
      <dt className="text-muted-foreground">引数 digest</dt>
      <dd className="break-all font-mono">{wait.operation?.args_digest ?? "—"}</dd>
      <dt className="text-muted-foreground">policy revision</dt>
      <dd className="break-all font-mono">
        {wait.policy_revision}（{wait.policy_hash.slice(0, 12)}）
      </dd>
    </dl>
  );
}

function DecisionForm({ wait, csrf }: { wait: BrowserWait; csrf: string }) {
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<WaitActionOutcome | null>(null);

  async function decide(decision: "approve_once" | "deny") {
    if (pending) return;
    setPending(true);
    const result = await submitDecision(() =>
      answerBrowserDecision(wait.wait_id, wait.task_id, wait.version, decision, csrf),
    );
    setOutcome(result);
    setPending(false);
  }

  return (
    <div className="space-y-2" data-testid="browser-decision-form">
      <OperationSummary wait={wait} />
      <div className="flex flex-wrap gap-2">
        <Button
          type="button"
          variant="primary"
          size="sm"
          disabled={pending}
          onClick={() => void decide("approve_once")}
        >
          一回だけ承認
        </Button>
        <Button type="button" variant="destructive" size="sm" disabled={pending} onClick={() => void decide("deny")}>
          拒否
        </Button>
      </div>
      <OutcomeText outcome={outcome} />
    </div>
  );
}

function CredentialForm({ wait, csrf }: { wait: BrowserWait; csrf: string }) {
  const id = useId();
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<WaitActionOutcome | null>(null);

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (pending) return;
    setPending(true);
    const result = await submitCredential(
      (sentUsername, sentPassword) =>
        answerBrowserCredential(wait.wait_id, wait.task_id, wait.version, sentUsername, sentPassword, csrf),
      username,
      password,
    );
    // 成功・失敗のいずれでも必ず空にする。応答・画面に入力値を残さない。
    setUsername(result.username);
    setPassword(result.password);
    setOutcome(result.outcome);
    setPending(false);
  }

  return (
    <form onSubmit={handleSubmit} autoComplete="off" className="space-y-3" data-testid="browser-credential-form">
      <div className="space-y-1">
        <label htmlFor={`${id}-user`} className="block text-label text-foreground">
          ユーザー名
        </label>
        <Input
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
          value={username}
          onChange={(event) => setUsername(event.target.value)}
        />
      </div>
      <div className="space-y-1">
        <label htmlFor={`${id}-pass`} className="block text-label text-foreground">
          パスワード
        </label>
        <Input
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
          value={password}
          onChange={(event) => setPassword(event.target.value)}
        />
      </div>
      <p className="text-label text-muted-foreground">
        値は credential broker
        にだけ渡り、画面・記録には残りません。登録しても使用は承認されず、使う前に一回ごとの承認を求めます。
      </p>
      <Button type="submit" size="sm" disabled={pending}>
        登録する
      </Button>
      <OutcomeText outcome={outcome} />
    </form>
  );
}

function WaitCard({ wait, csrf }: { wait: BrowserWait; csrf: string | null }) {
  const open = wait.state === "pending";
  const kind = browserWaitKind(wait);
  return (
    <div
      className="space-y-2 break-words border-b border-border py-3 last:border-b-0"
      data-testid="browser-wait"
      data-wait-reason={wait.reason}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={open ? "warning" : wait.state === "denied" || wait.state === "expired" ? "danger" : "neutral"}>
          {kind === "credential" ? "credential の登録依頼" : "一回だけの承認依頼"}
        </Badge>
        <span className="text-label text-muted-foreground">{wait.state}</span>
      </div>
      <WaitSummary wait={wait} />
      {kind === "decision" && !open ? <OperationSummary wait={wait} /> : null}
      {open && csrf ? (
        kind === "credential" ? (
          <CredentialForm wait={wait} csrf={csrf} />
        ) : kind === "decision" ? (
          <DecisionForm wait={wait} csrf={csrf} />
        ) : null
      ) : null}
    </div>
  );
}

/**
 * reason ごとに form を出し分ける表示本体。owner は呼び出し側（BrowserWaitsPanel か試験）が渡す —
 * useQuery に依存しないので、本人の状態ごとの出し分けを describe できる。
 * 本人として登録済みのセッションにだけ decision・credential の form を出し、それ以外は OwnerSessionNotice を出す。
 */
export function BrowserWaitsList({ waits, owner }: { waits: readonly BrowserWait[]; owner: OwnerSession | undefined }) {
  if (waits.length === 0) return null;
  const open = waits.filter((w) => w.state === "pending");
  const closed = waits.filter((w) => w.state !== "pending");
  const needsNotice = open.length > 0 && owner !== undefined && ownerNoticeReason(owner) !== null;
  const csrf = owner?.isOwner ? owner.csrfToken : null;
  return (
    <Section title="ブラウザの人待ち" id="browser-waits" data-testid="browser-waits">
      <div className="space-y-4">
        {needsNotice ? <OwnerSessionNotice owner={owner} /> : null}
        <div className="flex flex-col">
          {[...open, ...closed].map((wait) => (
            <WaitCard key={wait.wait_id} wait={wait} csrf={csrf} />
          ))}
        </div>
      </div>
    </Section>
  );
}

/** browser-query.ts の owner-session を取り、表示本体へ渡す。 */
export function BrowserWaitsPanel({ waits }: { waits: readonly BrowserWait[] }) {
  const owner = useQuery(ownerSessionQuery());
  return <BrowserWaitsList waits={waits} owner={owner.data} />;
}
