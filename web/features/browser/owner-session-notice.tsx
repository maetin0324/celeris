import { useState } from "react";
import { Button } from "../../components/ui/button";
import { CodeBlock } from "../../components/ui/code-block";
import { Notice } from "../../components/ui/notice";
import { BrowserGatewayError, requestOwnerSession } from "./browser-query";

export type OwnerAvailability = { available: boolean; isOwner: boolean; resumeError?: string };

/** notice を出すべき理由。本人として登録済み（available かつ isOwner）なら null で、waits panel は form を出してよい。 */
export function ownerNoticeReason(owner: OwnerAvailability | undefined): "owner_unavailable" | "not_owner" | null {
  if (!owner?.available) return "owner_unavailable";
  if (!owner.isOwner) return "not_owner";
  return null;
}

const CHALLENGE_ERROR_TEXT: Record<string, string> = {
  csrf_failed: "送信元を確認できませんでした。画面を読み込み直してください。",
  unauthenticated: "ログインが切れています。",
  owner_unavailable: "この構成では本人を識別できません。",
  browser_action_pending: "前の操作がまだ終わっていません。少し待ってやり直してください。",
};

function challengeErrorMessage(error: unknown): string {
  const code = error instanceof BrowserGatewayError ? error.code : "request_failed";
  return CHALLENGE_ERROR_TEXT[code] ?? "発行できませんでした。しばらくしてからやり直してください。";
}

/** 登録端末からの自動復帰（ADR 2026-10-07-browser-trusted-devices D3）に失敗したときの文言。 */
export function resumeErrorMessage(code: string | undefined): string | null {
  if (!code) return null;
  if (code === "device_rejected")
    return "登録した端末として確認できませんでした（失効・期限切れ・不一致）。本人確認のコードで承認し直してください。";
  if (code === "unauthenticated") return null;
  return "登録した端末での自動復帰ができませんでした（Celeris に接続できません）。画面を読み込み直すか、本人確認のコードで承認してください。";
}

/** `celerisctl` に渡す確定コマンド。`<path>` は web gateway の起動設定（CELERIS_WEB_OWNER_SOCKET）の socket path に置き換える。 */
export function ownerApproveCommand(challenge: string): string {
  return `celerisctl browser owner-session approve ${challenge} --socket <path>`;
}

/** 発行の呼び出しだけを切り出す（component の state 更新と分離してテストできるようにする）。 */
export async function issueOwnerChallenge(): Promise<{ challenge: string } | { error: string }> {
  try {
    const result = await requestOwnerSession();
    return { challenge: result.challenge };
  } catch (error) {
    return { error: challengeErrorMessage(error) };
  }
}

export function ChallengeCommand({ challenge }: { challenge: string }) {
  return (
    <div className="space-y-2" data-testid="browser-owner-challenge">
      <p>
        ローカルの端末で次を実行し、本人として確定してください（5 分間有効）。
        <code className="font-mono">{"<path>"}</code> は web gateway の起動設定（
        <code className="font-mono">CELERIS_WEB_OWNER_SOCKET</code>）が指す socket の path に置き換えます。
      </p>
      <CodeBlock label="本人確認の確定コマンド">{ownerApproveCommand(challenge)}</CodeBlock>
    </div>
  );
}

/**
 * 本人でない（または owner_unavailable の）セッションに、操作 form の代わりに出す案内。
 * - owner_unavailable: パスワード認証が無効で本人を識別できない。登録のしようがないのでボタンは出さない。
 * - not_owner: challenge 発行ボタンと `celerisctl browser owner-session approve <challenge> --socket <path>` の手順を出す。
 */
export function OwnerSessionNotice({ owner }: { owner: OwnerAvailability | undefined }) {
  const reason = ownerNoticeReason(owner);
  const [challenge, setChallenge] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);

  if (reason === null) return null;

  if (reason === "owner_unavailable") {
    return (
      <Notice tone="warning" title="本人確認を利用できません" data-testid="browser-owner-unavailable">
        パスワード認証を有効にした単一所有者の構成でだけ、credential の登録・承認・Live View を使えます。
      </Notice>
    );
  }

  return (
    <Notice
      tone="warning"
      title="このセッションは本人として登録されていません"
      data-testid="browser-owner-request"
      action={
        <Button
          size="sm"
          disabled={pending}
          onClick={async () => {
            setPending(true);
            setError(null);
            const result = await issueOwnerChallenge();
            if ("challenge" in result) setChallenge(result.challenge);
            else setError(result.error);
            setPending(false);
          }}
        >
          本人確認のコードを発行
        </Button>
      }
    >
      <p>登録・承認・credential 入力・Live View は、本人として登録したセッションでだけ操作できます。</p>
      {resumeErrorMessage(owner?.resumeError) ? (
        <p role="alert" className="text-danger-foreground" data-testid="browser-owner-resume-failed">
          {resumeErrorMessage(owner?.resumeError)}
        </p>
      ) : null}
      {challenge ? <ChallengeCommand challenge={challenge} /> : null}
      {error ? (
        <p role="alert" className="text-danger-foreground">
          {error}
        </p>
      ) : null}
    </Notice>
  );
}
