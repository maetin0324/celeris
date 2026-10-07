---
title: CoS チャット web — チャット内カード（task・質問・決定・認可・plan gate・知らせ・CoS 代答）
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---
# web-chat / cards

ADR 2026-10-05-cos-chat-home D5 の 6 項目目と D3「代答・取り消し・差し戻し」（D6 の受け入れ表「代答の修正」の web 側）に従い、`web/features/chat/cards/` にカード部品と vitest を作った。route（`web/routes/`）にはまだ組み込んでいない（home 葉が組む）。`gui/`・`crates/`・API schema・data 層（`web/features/chat/data/`）は変えていない。

## 作ったもの

| file | 内容 |
|---|---|
| `card-model.ts` | 種類・actor・状態の表示名、`isAnswerKind`（decision・question・approval・plan_gate）、`isClosedState`（answered・superseded・revoked・returned・needs_remediation・expired など）、`canOverride`（kind=operation・actor=cos・state=applied のときだけ）、`pendingHumanCount`、`overrideFailure`（409 → 人と CoS の競合、403・404・422・通信失敗）、`internalHref`（web 内の path だけをリンクにする） |
| `card-actions.ts` | `sendCardAnswer`：受信箱と同じ `POST /inbox/items/{id}/answer` に `{option}` を送る。理由が要る選択肢（`needs_note`）は送らず、詳細の画面へ誘導する。失敗は受信箱の `answerFailure` で読む。`sendOverride`：`POST /cos/operations/{o}/override` に `{action: revoke/return, reason}` を送る。理由が空なら送らない。API は `CardApi` で差し替えられる |
| `chat-card.tsx` | `ChatCardView`：props だけで描く。種類・状態の badge、actor（人/CoS/システム）、題名、理由、`card.href` への「詳細」link（44 px、router があれば SPA 遷移）。答えられる種類は受信箱の項目の選択肢をボタンにする（推奨は primary、取り消しにくい選択は ConfirmDialog）。閉じた状態・404（回答済み/失効）・回答後は操作を消す。CoS 代答には「取消」「差し戻し」。結果としては、409 の競合（role=alert）・一時停止したタスクの件数・needs_remediation の修正タスクへのリンクを出す。`ChatCardItem`：`inboxItemQuery(card.id)`（カードの id は triage の source_key で、受信箱の項目 id と同じ）で選択肢を読み、送信後に `inboxKeys.all` を invalidate する。`ChatCardList`：message.cards の並び |
| `override-dialog.tsx` | 取消・差し戻しの確認 dialog（radix AlertDialog）。ConfirmDialog と同じ作法で、初期 focus は「戻る」、送信中は閉じない。API が必須とする理由の textarea を足した。理由の不備だけは dialog を開いたままにし、成功・競合・通信失敗は閉じて結果をカードに残す |
| `pending-badge.tsx` | `PendingCountBadge`：受信箱 thread 用の人待ち件数。0 件は何も出さない。tone は info、99+ で止める。読み上げは「人待ち N 件」の 1 文 |
| `chat-card.test.tsx` | 試験 21 件（`chat_cards_*`） |

## 証拠

| 条件 | コマンド | 結果 |
|---|---|---|
| 0 部品と vitest がある / 1 各 kind の表示・その場回答・revoke/return・409 | `corepack pnpm@12.6.0 -C web exec vitest run features/chat/cards` | exit 0、21 passed |
| web 単体試験全体 | `corepack pnpm@12.6.0 -C web test` | exit 0、vitest 66 files / 442 passed、node --test 57 pass |
| 型 | `corepack pnpm@12.6.0 -C web typecheck` | exit 0 |
| lint | `corepack pnpm@12.6.0 -C web lint` | exit 0（警告 4 件は既存の styles.css） |
| build・境界・秘密 | `pnpm -C web build`・`check:boundaries`・`check:secrets` | いずれも exit 0 |
| 範囲 | `git diff --name-only $CELERIS_WU_BASE` + untracked | `web/features/chat/cards/` と本ファイルだけ |

試験は 7 種すべての表示（題名・状態・actor・理由・詳細 link・44 px）、選択肢（推奨・確認 dialog・自由文は詳細へ）、読み込み中・404・回答済み、閉じた状態で操作が消えること、送信 body と既定 API の endpoint（fetch を差し替えて `/api/inbox/items/{id}/answer` と `/api/v1/cos/operations/{o}/override`）、409・404・native の扱い、取消・差し戻しの body と空の理由、409 の競合表示、needs_remediation の修正タスク link、badge を確かめる。DOM の環境が無い（vitest は node）ため、操作の流れは `sendCardAnswer`・`sendOverride` と、結果の props による静的描画に分けて試した。sleep・負荷は使っていない。

## 未解決

- 実際のクリック・dialog の開閉・focus 復帰は e2e（e2e-mobile 葉、fake-daemon の fixture）で確かめる。ここでは trigger の `aria-haspopup="dialog"` だけを見た。
- カードの「詳細」は `card.href` をそのまま使う。CoS 代答の href `/cos/operations/{o}` に対応する web の route は無い（現時点では 404 画面になる）。home 葉か後続で route を作るか、受信箱へ向けるかを決める必要がある。
- override の応答には新しい状態の card が含まれない。カードの状態（superseded など）は SSE の card event で置き換わる前提で、それまでは結果を局所状態で出し、操作を消す。

## 提案

- triage が作るカードの `href` は `/?thread=<inbox>` で、元の待ちの画面ではない。人が元の画面（`/tasks/{id}` など）へ直接行けるように、Card に元の待ちの href（または InboxItem.links）を足す案がある（API schema の変更なので別 task）。
