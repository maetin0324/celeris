// chat の案の人の採否と外部の文の印（ADR 2026-11-23-project-observability-publish D2）。
// - 案の状態（pending・adopted・rejected・commented・stale）は daemon が人の採否の message から導いて wire の
//   `message.proposal.review_state` に載せる。画面は wire の値だけを表示し、本文の語（「承認済み」など）からは導かない（D2.4）。
// - 送る版（expected_revision・sha256・attachment）は人が画面で見た案の版。sha256 は CoS の戦略の案なら添付
//   `strategy/<id>/v<n>.json` の sha256、それ以外は本文の UTF-8 の SHA-256（`task_core::chat_citation::body_sha256`）。
// - 採用は承認ではない。CoS に戦略の版と人の承認（決定 `strategy-approve-v<n>`）へ進めるよう指示するだけ（D2.3）。

import { isApiError, problemCode, problemDetail } from "../../../api/client";
import type {
  ChatAttachment,
  ChatMessage,
  ChatWorkspaceFile,
  ProposalKind,
  ProposalReviewRequest,
  ProposalReviewState,
  ProposalVerdict,
} from "../../../api/generated/types";
import { sha256Hex } from "../../../lib/sha256";
import { messageRevision } from "./revision";

export const PROPOSAL_STATE_LABEL: Record<ProposalReviewState, string> = {
  pending: "未判定",
  adopted: "採用済み",
  rejected: "却下",
  commented: "コメントあり",
  stale: "失効（採否の後に案が編集・撤回された）",
};

export const PROPOSAL_KIND_LABEL: Record<ProposalKind, string> = {
  external_llm: "外部 LLM の案",
  human_import: "人が取り込んだ要約・引用",
  cos_strategy: "CoS の戦略の案",
};

export const PROPOSAL_VERDICT_LABEL: Record<ProposalVerdict, string> = {
  adopt: "採用",
  reject: "却下",
  comment: "コメント",
};

/** 採用の操作の横に常に出す注意（D2.3: 採用は承認ではない）。 */
export const PROPOSAL_ADOPT_NOTICE =
  "採用は承認ではありません。CoS に戦略の版（strategy/<id>/v<n>.json）と人の承認（決定 strategy-approve-v<n>）へ進めるよう指示します。";

/** D2.4: wire の `external` がある message の印。色だけに頼らず文字で出す。 */
export const EXTERNAL_MARK_TEXT = "外部の文（未検証）";

/** D2.1: note の上限（文字数。Rust の `NOTE_MAX_CHARS` と同じく code point で数える）。 */
export const PROPOSAL_NOTE_MAX_CHARS = 2000;

export const PROPOSAL_VERDICT_REQUIRES_HUMAN = "proposal_verdict_requires_human";
export const PROPOSAL_REVIEW_STALE = "proposal_review_stale";
export const PROPOSAL_FORBIDDEN_TEXT = "採否は人だけができます";
export const PROPOSAL_STALE_TEXT = "案が変わりました。読み直してから採否してください";

/** `task_core::strategy_ref::strategy_artifact` と同じ形（`strategy/<id>/v<n>.json`、id に `/` を含まず n は 0 埋め無し）。 */
const STRATEGY_ARTIFACT = /^strategy\/([^/]+)\/v(0|[1-9][0-9]{0,9})\.json$/;

export function isStrategyArtifactPath(path: string): boolean {
  const match = STRATEGY_ARTIFACT.exec(path);
  return match !== null && Number(match[2]) <= 0xffff_ffff;
}

/** CoS の戦略の案の添付（workspace の file のうち `strategy/<id>/v<n>.json` の最初の 1 件）。 */
export function strategyDraftOf(message: Pick<ChatMessage, "workspace_files">): ChatWorkspaceFile | undefined {
  return message.workspace_files?.find((file) => isStrategyArtifactPath(file.path));
}

/** 採否の操作を出してよい案か。撤回された案には出さない（daemon も 409 にする）。 */
export function proposalActionable(message: Pick<ChatMessage, "proposal" | "retracted">): boolean {
  return message.proposal != null && message.retracted == null;
}

/** note の誤り（送る前に画面で止める）。無ければ undefined。 */
export function proposalNoteError(verdict: ProposalVerdict, note: string): string | undefined {
  const chars = Array.from(note).length;
  if (chars > PROPOSAL_NOTE_MAX_CHARS) return `文は ${PROPOSAL_NOTE_MAX_CHARS} 文字までです（今は ${chars} 文字）。`;
  if (verdict === "comment" && note.trim() === "")
    return `コメントには 1〜${PROPOSAL_NOTE_MAX_CHARS} 文字の文が要ります。`;
  return undefined;
}

export type ProposalReviewBuild = { ok: true; body: ProposalReviewRequest } | { ok: false; reason: string };

/** 人が見た案の版で POST …/review の本文を組む。 */
export function proposalReviewRequest(
  message: ChatMessage,
  verdict: ProposalVerdict,
  note: string,
  attachments: Record<string, ChatAttachment | undefined>,
  clientMessageId: string,
): ProposalReviewBuild {
  const proposal = message.proposal;
  if (!proposal) return { ok: false, reason: "この発言は案ではありません。" };
  if (message.retracted) return { ok: false, reason: "撤回された案は採否できません。" };
  const noteError = proposalNoteError(verdict, note);
  if (noteError) return { ok: false, reason: noteError };
  const base = {
    verdict,
    expected_revision: messageRevision(message),
    client_message_id: clientMessageId,
    ...(note.trim() === "" ? {} : { note }),
  };
  if (proposal.kind === "cos_strategy") {
    const draft = strategyDraftOf(message);
    const attachment = draft ? attachments[draft.attachment_id] : undefined;
    if (!draft || !attachment)
      return { ok: false, reason: "戦略の版の添付を読み込めていません。読み直してから採否してください。" };
    return { ok: true, body: { ...base, sha256: attachment.sha256, attachment: draft.path } };
  }
  return { ok: true, body: { ...base, sha256: sha256Hex(message.text) } };
}

export type ProposalReviewFailure = { message: string; stale: boolean };

/** 採否の失敗を人の読める文にする。stale は読み直しを促す。 */
export function proposalReviewFailure(error: unknown): ProposalReviewFailure {
  if (isApiError(error)) {
    const code = problemCode(error.body);
    const detail = problemDetail(error.body);
    if (error.status === 403)
      return { message: `${PROPOSAL_FORBIDDEN_TEXT}（${code ?? PROPOSAL_VERDICT_REQUIRES_HUMAN}）`, stale: false };
    if (error.status === 409 && code === PROPOSAL_REVIEW_STALE) return { message: PROPOSAL_STALE_TEXT, stale: true };
    if (error.status === 422 || error.status === 409)
      return { message: detail ?? "採否を送れませんでした。", stale: false };
  }
  const reason = error instanceof Error ? error.message : String(error);
  return { message: `採否を送れませんでした: ${reason}`, stale: false };
}
