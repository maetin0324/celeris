// 案の message の採否の欄と外部の文の印（ADR 2026-11-23-project-observability-publish D2・D4）。
// 状態は wire の `message.proposal` だけを表示する。送った後は既存の再取得（session.refresh）で読み直し、
// 画面には採否の正本を持たない。採用は確認の一手を挟み、横に「採用は承認ではありません」を常に出す。
import { useId, useState } from "react";
import type {
  ChatAttachment,
  ChatMessage,
  ChatPostMessageResponse,
  ProposalReviewRequest,
  ProposalReviewState,
  ProposalVerdict,
} from "../../../api/generated/types";
import { Badge, type BadgeTone } from "../../../components/ui/badge";
import { Button } from "../../../components/ui/button";
import { fieldClassName } from "../../../components/ui/input";
import { Notice } from "../../../components/ui/notice";
import { randomId } from "../../../lib/random-id";
import { formatAbsolute } from "../../../lib/time";
import { cn } from "../../../lib/utils";
import {
  EXTERNAL_MARK_TEXT,
  PROPOSAL_ADOPT_NOTICE,
  PROPOSAL_KIND_LABEL,
  PROPOSAL_NOTE_MAX_CHARS,
  PROPOSAL_STATE_LABEL,
  PROPOSAL_VERDICT_LABEL,
  proposalActionable,
  proposalReviewFailure,
  proposalReviewRequest,
} from "../data/proposal";
import { chatMessageElementId } from "./workspace-links";

/** 一覧から吹き出しへ渡す採否の口。無ければ状態だけを出し、操作を出さない。 */
export type ProposalReviewActions = {
  /** POST …/messages/{m}/review（既定は data/client.ts の reviewProposal）。 */
  submit: (message: ChatMessage, body: ProposalReviewRequest) => Promise<ChatPostMessageResponse>;
  /** 送った後・失効の後の読み直し（thread と messages の取り直し）。 */
  reload: () => void;
};

const stateTone: Record<ProposalReviewState, BadgeTone> = {
  pending: "neutral",
  adopted: "success",
  rejected: "danger",
  commented: "info",
  stale: "warning",
};

/** 外部の文の印。wire の `external` がある message だけに出す（人の発言には wire が付けない）。 */
export function ExternalMark({ message }: { message: Pick<ChatMessage, "external"> }) {
  const external = message.external;
  if (!external) return null;
  return (
    <p
      data-slot="chat-external-mark"
      data-untrusted="true"
      className="flex flex-wrap items-center gap-2 text-label text-muted-foreground"
    >
      <Badge tone="warning">{EXTERNAL_MARK_TEXT}</Badge>
      <span data-slot="chat-external-source">{`出典: ${external.source_label}（${external.source_kind}）`}</span>
    </p>
  );
}

type Feedback = { kind: "error"; message: string; stale: boolean } | { kind: "sent"; verdict: ProposalVerdict };

/** 案の状態・採否した人と時刻・採用・却下・コメントの操作。 */
export function ProposalReview({
  message,
  attachments = {},
  actions,
}: {
  message: ChatMessage;
  attachments?: Record<string, ChatAttachment | undefined>;
  actions?: ProposalReviewActions;
}) {
  const [note, setNote] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<Feedback | null>(null);
  const noteId = useId();
  const proposal = message.proposal;
  if (!proposal) return null;
  const state = proposal.review_state;
  const actionable = actions !== undefined && proposalActionable(message);

  const send = async (verdict: ProposalVerdict) => {
    if (!actions) return;
    const built = proposalReviewRequest(message, verdict, note, attachments, randomId());
    if (!built.ok) {
      setFeedback({ kind: "error", message: built.reason, stale: false });
      setConfirming(false);
      return;
    }
    setBusy(true);
    setFeedback(null);
    try {
      await actions.submit(message, built.body);
      setNote("");
      setFeedback({ kind: "sent", verdict });
      actions.reload();
    } catch (error) {
      setFeedback({ kind: "error", ...proposalReviewFailure(error) });
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  };
  const reload = () => {
    setFeedback(null);
    actions?.reload();
  };

  return (
    <section
      data-slot="chat-proposal"
      data-review-state={state}
      data-proposal-kind={proposal.kind}
      aria-label="案の採否"
      className={cn(
        "flex w-full min-w-0 flex-col gap-2 rounded-md border border-border px-3 py-2",
        message.role === "user" && "max-w-[85%]",
      )}
    >
      <p className="flex flex-wrap items-center gap-2 text-label">
        <span className="text-muted-foreground">{PROPOSAL_KIND_LABEL[proposal.kind]}</span>
        <span data-slot="chat-proposal-state" data-review-state={state}>
          {"状態: "}
          <Badge tone={stateTone[state]}>{PROPOSAL_STATE_LABEL[state]}</Badge>
        </span>
      </p>
      {proposal.reviewed_by ? (
        <p data-slot="chat-proposal-reviewed" className="text-label text-muted-foreground">
          {`採否した人: ${proposal.reviewed_by}`}
          {proposal.reviewed_at ? `（${formatAbsolute(proposal.reviewed_at)}）` : null}
          {proposal.review_message_id ? (
            <>
              {" "}
              <a
                href={`#${chatMessageElementId(proposal.review_message_id)}`}
                className="underline underline-offset-2 hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
              >
                採否の発言へ
              </a>
            </>
          ) : null}
        </p>
      ) : null}
      {message.retracted ? (
        <p data-slot="chat-proposal-retracted" className="text-label text-muted-foreground">
          撤回された案は採否できません。
        </p>
      ) : null}
      {actionable ? (
        <>
          <label htmlFor={noteId} className="text-label text-muted-foreground">
            {`文（採用・却下では任意、コメントでは必須。${PROPOSAL_NOTE_MAX_CHARS} 文字まで）`}
          </label>
          <textarea
            id={noteId}
            data-slot="chat-proposal-note"
            value={note}
            onChange={(event) => setNote(event.target.value)}
            rows={2}
            disabled={busy}
            className={cn(fieldClassName, "min-h-16 w-full")}
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button
              size="sm"
              variant="primary"
              data-slot="chat-proposal-adopt"
              disabled={busy || confirming}
              onClick={() => {
                setFeedback(null);
                setConfirming(true);
              }}
            >
              {PROPOSAL_VERDICT_LABEL.adopt}
            </Button>
            <Button size="sm" data-slot="chat-proposal-reject" disabled={busy} onClick={() => void send("reject")}>
              {PROPOSAL_VERDICT_LABEL.reject}
            </Button>
            <Button size="sm" data-slot="chat-proposal-comment" disabled={busy} onClick={() => void send("comment")}>
              {PROPOSAL_VERDICT_LABEL.comment}
            </Button>
          </div>
          <p data-slot="chat-proposal-adopt-notice" className="text-label text-muted-foreground">
            {PROPOSAL_ADOPT_NOTICE}
          </p>
          {confirming ? (
            <Notice
              data-slot="chat-proposal-confirm"
              tone="warning"
              action={
                <div className="flex flex-wrap gap-2">
                  <Button size="sm" variant="primary" disabled={busy} onClick={() => void send("adopt")}>
                    採用を送る
                  </Button>
                  <Button size="sm" variant="ghost" disabled={busy} onClick={() => setConfirming(false)}>
                    やめる
                  </Button>
                </div>
              }
            >
              この案を採用として CoS に送りますか。承認にはなりません。
            </Notice>
          ) : null}
        </>
      ) : null}
      {feedback?.kind === "error" ? (
        <Notice
          data-slot="chat-proposal-error"
          tone={feedback.stale ? "warning" : "danger"}
          action={
            feedback.stale ? (
              <Button size="sm" onClick={reload}>
                読み直す
              </Button>
            ) : undefined
          }
        >
          {feedback.message}
        </Notice>
      ) : null}
      {feedback?.kind === "sent" ? (
        <p data-slot="chat-proposal-sent" role="status" className="text-label text-muted-foreground">
          {`${PROPOSAL_VERDICT_LABEL[feedback.verdict]}を送りました。状態は daemon の記録を読み直して表示します。`}
        </p>
      ) : null}
    </section>
  );
}
