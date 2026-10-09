import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { type MouseEvent, type ReactNode, useRef, useState } from "react";
import { ApiError } from "../../../api/client";
import type { ChatCard, InboxItem, InboxOption, OverrideMode } from "../../../api/generated/types";
import { inboxItemQuery } from "../../../api/queries/inbox-notifications";
import { inboxKeys } from "../../../api/queries/keys";
import { Badge } from "../../../components/ui/badge";
import { Button } from "../../../components/ui/button";
import { ConfirmDialog } from "../../../components/ui/confirm-dialog";
import { Notice } from "../../../components/ui/notice";
import { DecisionLocateButton } from "../../decisions/decision-locate";
import { decisionIdFromItemId } from "../../decisions/decision-model";
import { type AnswerOutcome, isDestructive } from "../../inbox/inbox-model";
import { type CardApi, defaultCardApi, sendCardAnswer, sendOverride } from "./card-actions";
import {
  ACTOR_LABELS,
  CARD_KIND_LABELS,
  canOverride,
  cardStateView,
  internalHref,
  isAnswerKind,
  isClosedState,
  type OverrideOutcome,
  overrideTarget,
  taskHref,
} from "./card-model";
import { OverrideDialog } from "./override-dialog";

// チャット内の小さなカード（ADR 2026-10-05-cos-chat-home D5）。題名・状態・actor・理由と『詳細』へのリンクを出し、
// 決定・質問・認可・plan gate はその場で答え、CoS 代答は取消・差し戻しできる。自由文は詳細画面へ誘導する。
// ChatCardView は props だけで描く（試験はこちら）。ChatCardItem が受信箱の項目の取得と送信をつなぐ。

// スマホでも押せるよう、カード内のリンクも 44px の高さを持たせる（mobile-audit）。
const linkClass = "inline-flex min-h-11 min-w-11 items-center underline wrap-anywhere";

/** web 内の path へ SPA のまま移る。router の外（試験の静的描画）では普通のリンクとして振る舞う。 */
export function CardLink({ href, children }: { href: string; children: ReactNode }) {
  const router = useRouter({ warn: false });
  function onClick(event: MouseEvent<HTMLAnchorElement>) {
    if (!router || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) return;
    event.preventDefault();
    void router.navigate({ href });
  }
  return (
    <a href={href} onClick={onClick} className={linkClass}>
      {children}
    </a>
  );
}

export type CardAnswerState =
  | { status: "loading" }
  /** 受信箱の項目が無い（404）。回答済みか失効。 */
  | { status: "gone" }
  | { status: "error"; message: string }
  | { status: "ready"; item: InboxItem }
  | { status: "answered"; label: string; removed: boolean };

export type CardAnswerProps = {
  state: CardAnswerState;
  pending: boolean;
  failure: Extract<AnswerOutcome, { ok: false }> | null;
  onAnswer: (option: InboxOption) => Promise<AnswerOutcome>;
};

export type CardOverrideProps = {
  pending: boolean;
  outcome: OverrideOutcome | null;
  onOverride: (mode: OverrideMode, reason: string) => Promise<OverrideOutcome>;
};

export type ChatCardViewProps = {
  card: ChatCard;
  /** 決定・質問・認可・plan gate で、まだ閉じていないときに渡す。 */
  answer?: CardAnswerProps;
  /** CoS 代答のとき渡す。 */
  override?: CardOverrideProps;
};

function AnswerOptions({ card, detail, answer }: { card: ChatCard; detail: string | null; answer: CardAnswerProps }) {
  const { state, pending, failure } = answer;
  if (state.status === "loading")
    return (
      <p role="status" className="text-label text-muted-foreground">
        選択肢を読み込んでいます。
      </p>
    );
  if (state.status === "gone")
    return <p className="text-label text-muted-foreground">この待ちは既に答えられたか、失効しています。</p>;
  if (state.status === "answered")
    return (
      <p role="status" className="text-label">
        「{state.label}」で答えました。{state.removed ? "" : "反映を待っています。"}
      </p>
    );
  if (state.status === "error")
    return (
      <Notice tone="danger" title="選択肢を読めません">
        {state.message}
      </Notice>
    );
  const { item } = state;
  const freeText = item.options.filter((option) => option.needs_note);
  const quick = item.options.filter((option) => !option.needs_note);
  return (
    <div className="flex min-w-0 flex-col gap-2">
      {quick.length > 0 ? (
        <ul aria-label={`「${card.title}」の選択肢`} className="flex flex-wrap gap-2">
          {quick.map((option) => {
            const recommended = item.recommended === option.key;
            const label = `${option.label}${recommended ? "（推奨）" : ""}`;
            return (
              <li key={option.key}>
                {isDestructive(option) ? (
                  <ConfirmDialog
                    trigger={
                      <Button variant="destructive" size="sm" disabled={pending}>
                        {label}
                      </Button>
                    }
                    title={`「${option.label}」を選びますか`}
                    target={card.title}
                    consequence={option.effect || "この判断を確定します。"}
                    reversibility="答えた判断はチャットからは取り消せません。必要なら詳細の画面から操作し直します。"
                    followUp="このカードと受信箱で確かめられます。"
                    confirmLabel={`「${card.title}」を${option.label}`}
                    onConfirm={async () => {
                      const outcome = await answer.onAnswer(option);
                      if (!outcome.ok && !outcome.stale) throw new Error(outcome.message);
                    }}
                  />
                ) : (
                  <Button
                    variant={recommended ? "primary" : "secondary"}
                    size="sm"
                    disabled={pending}
                    title={option.effect || undefined}
                    onClick={() => void answer.onAnswer(option)}
                  >
                    {label}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      ) : null}
      {freeText.length > 0 || item.options.length === 0 ? (
        <p className="text-label text-muted-foreground">
          {item.options.length === 0
            ? "この待ちは詳細の画面で操作します。"
            : `${freeText.map((option) => `「${option.label}」`).join("・")}は理由を書いて詳細の画面で答えます。`}
          {detail ? <CardLink href={detail}>詳細で答える</CardLink> : null}
        </p>
      ) : null}
      {failure ? (
        <Notice tone="danger">
          {failure.message}
          {failure.native || failure.field === "note" ? (
            detail ? (
              <CardLink href={detail}>詳細で答える</CardLink>
            ) : null
          ) : null}
        </Notice>
      ) : null}
    </div>
  );
}

function OverrideResult({ outcome }: { outcome: OverrideOutcome }) {
  if (!outcome.ok)
    return (
      <Notice tone="danger" title={outcome.conflict ? "競合しました（409）" : "代答を修正できませんでした"}>
        {outcome.message}
      </Notice>
    );
  const done = outcome.mode === "revoke" ? "取り消しました" : "差し戻しました";
  return (
    <div role="status" className="flex min-w-0 flex-col gap-1 text-label">
      <p>
        代答を{done}。
        {outcome.pausedTaskIds.length > 0 ? `${outcome.pausedTaskIds.length} 件のタスクを一時停止しました。` : ""}
      </p>
      {outcome.state === "needs_remediation" ? (
        <p>
          戻せない外部への影響があるため、修正のタスクを起票しました。
          {outcome.remediationTaskId ? (
            <CardLink href={taskHref(outcome.remediationTaskId)}>修正のタスクを開く</CardLink>
          ) : null}
        </p>
      ) : null}
    </div>
  );
}

/** カード 1 枚の表示。状態は props だけから決める。 */
export function ChatCardView({ card, answer, override }: ChatCardViewProps) {
  const kind = CARD_KIND_LABELS[card.kind];
  const detail = internalHref(card.href);
  const closed = isClosedState(card.state);
  const showAnswer = answer !== undefined && isAnswerKind(card.kind) && !closed;
  // The saved card is a historical snapshot. Live answer results also update its badge,
  // while AnswerOptions keeps the answer/closure explanation visible without buttons.
  const displayState =
    showAnswer && answer.state.status === "gone"
      ? "closed"
      : showAnswer && answer.state.status === "answered"
        ? "answered"
        : card.state;
  const state = cardStateView(displayState);
  const overridden = override?.outcome?.ok === true;
  const decisionId = card.kind === "decision" ? decisionIdFromItemId(card.id) : null;
  const showOverride = override !== undefined && canOverride(card) && !overridden;
  return (
    <article
      aria-label={`${kind}: ${card.title}`}
      data-card-kind={card.kind}
      data-card-state={displayState}
      className="flex min-w-0 max-w-full flex-col gap-2 rounded-md border border-border bg-surface p-3 text-body text-foreground"
    >
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <Badge tone="neutral">{kind}</Badge>
        <Badge tone={state.tone}>{state.label}</Badge>
        <span className="text-label text-muted-foreground">{ACTOR_LABELS[card.actor]}</span>
      </div>
      <p className="min-w-0 break-words font-medium">{card.title}</p>
      {card.reason ? <p className="min-w-0 break-words text-label text-muted-foreground">理由: {card.reason}</p> : null}
      {showAnswer ? <AnswerOptions card={card} detail={detail} answer={answer} /> : null}
      {override?.outcome ? <OverrideResult outcome={override.outcome} /> : null}
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        {showOverride ? (
          <>
            <OverrideDialog
              mode="revoke"
              target={card.title}
              disabled={override.pending}
              onConfirm={(reason) => override.onOverride("revoke", reason)}
            />
            <OverrideDialog
              mode="return"
              target={card.title}
              disabled={override.pending}
              onConfirm={(reason) => override.onOverride("return", reason)}
            />
          </>
        ) : null}
        {detail ? <CardLink href={detail}>詳細</CardLink> : null}
        {decisionId ? <DecisionLocateButton decisionId={decisionId} /> : null}
      </div>
    </article>
  );
}

function useCardAnswer(card: ChatCard, api: CardApi, enabled: boolean): CardAnswerProps {
  const queryClient = useQueryClient();
  const query = useQuery({ ...inboxItemQuery(card.id), enabled, retry: false });
  const pendingRef = useRef(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<Extract<AnswerOutcome, { ok: false }> | null>(null);
  const [answered, setAnswered] = useState<{ label: string; removed: boolean } | null>(null);

  async function onAnswer(option: InboxOption): Promise<AnswerOutcome> {
    const item = query.data;
    if (pendingRef.current || !item) return { ok: false, message: "送信中です。" };
    pendingRef.current = true;
    setPending(true);
    setFailure(null);
    try {
      const outcome = await sendCardAnswer(api, item, option);
      if (outcome.ok) setAnswered({ label: option.label, removed: outcome.removed });
      else setFailure(outcome);
      if (outcome.ok || outcome.stale) await queryClient.invalidateQueries({ queryKey: inboxKeys.all });
      return outcome;
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  }

  let state: CardAnswerState;
  if (answered) state = { status: "answered", ...answered };
  else if (query.error instanceof ApiError && query.error.status === 404) state = { status: "gone" };
  else if (query.data) state = { status: "ready", item: query.data };
  else if (query.error)
    state = { status: "error", message: "受信箱の項目を読めませんでした。詳細の画面で確かめてください。" };
  else state = { status: "loading" };
  return { state, pending, failure, onAnswer };
}

function useCardOverride(card: ChatCard, api: CardApi): CardOverrideProps {
  const queryClient = useQueryClient();
  const pendingRef = useRef(false);
  const [pending, setPending] = useState(false);
  const [outcome, setOutcome] = useState<OverrideOutcome | null>(null);

  async function onOverride(mode: OverrideMode, reason: string): Promise<OverrideOutcome> {
    if (pendingRef.current) return { ok: false, message: "送信中です。", conflict: false, stale: false };
    pendingRef.current = true;
    setPending(true);
    try {
      const next = await sendOverride(api, overrideTarget(card), mode, reason);
      // 入力の不備はダイアログ側で示す。結果・競合はカードに残す。
      if (next.ok || next.conflict || next.stale) setOutcome(next);
      if (next.ok || next.stale) await queryClient.invalidateQueries({ queryKey: inboxKeys.all });
      return next;
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  }
  return { pending, outcome, onOverride };
}

/** カード 1 枚。受信箱の項目（選択肢）を読み、回答・代答の修正を送る。QueryClientProvider の中で使う。 */
export function ChatCardItem({ card, api = defaultCardApi }: { card: ChatCard; api?: CardApi }) {
  const answerable = isAnswerKind(card.kind) && !isClosedState(card.state);
  const answer = useCardAnswer(card, api, answerable);
  const override = useCardOverride(card, api);
  return (
    <ChatCardView
      card={card}
      answer={answerable ? answer : undefined}
      override={card.kind === "operation" ? override : undefined}
    />
  );
}

/** メッセージに付いたカードの並び。 */
export function ChatCardList({ cards, api }: { cards: readonly ChatCard[]; api?: CardApi }) {
  if (cards.length === 0) return null;
  return (
    <ul aria-label="カード" className="flex min-w-0 flex-col gap-2">
      {cards.map((card) => (
        <li key={`${card.kind}:${card.id}`} className="min-w-0">
          <ChatCardItem card={card} api={api} />
        </li>
      ))}
    </ul>
  );
}
