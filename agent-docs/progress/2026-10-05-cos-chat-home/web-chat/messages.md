---
title: CoS チャット web — メッセージ表示（markdown・tool 折り畳み・streaming・追従と『最新へ』）
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---
# web-chat / messages

ADR 2026-10-05-cos-chat-home D5 の 2・3 項目目を `web/features/chat/messages/` に部品として作った。route（`web/routes/`）には組み込んでいない（home 葉が組む）。data 層（`web/features/chat/data/`）は変えていない。`gui/`・`crates/` も変えていない。

## 作ったもの

| file | 内容 |
|---|---|
| `logic.ts` | 純関数。`isAtBottom`（下端から 48px 以内）・`followReducer`（下端を見ているときだけ追従。離れているときは位置を保ち、末尾に足された件数を未読にする。上に足した古い履歴は数えない。自分の送信（pending）が出たら下端へ移る）・`scrollTopAfterPrepend`・`nextAnnouncement`（初回の snapshot は既読扱い。streaming・draft・running は読まず、確定した返事を 1 回だけ読む）・`groupTools`（message_id → 同じ run の返事 → 末尾の順に付ける）・`statusLabel`（公開要約があればそれ、無ければ「考え中」など）・`formatBytes`・`oldestSeq` |
| `parts.tsx` | `ToolCall`（名前・要約・成功/失敗/実行中の 1 行。detail は展開したときだけ描く。detail が無ければ button にしない）・`ToolCallList`・`AttachmentList`（画像は `preview_url`、他は原名・容量と `download` link。未取得は「添付を読み込み中」）・`StatusLine`・`JumpToLatest`（「最新へ（未読 N 件）」） |
| `message-item.tsx` | `MessageItem`（人は平文の吹き出し、CoS は `Markdown links="target"`、状態の注記、カードの slot `renderCards`）・`DraftItem`（message 未着の streaming 本文）・`PendingItem`（送信中/失敗、再送・破棄） |
| `message-list.tsx` | `MessageList`。`selectTimeline` を描く。scroll 領域は名前付きの `section`（tabIndex 0）。描画直後（`useLayoutEffect`）に「古い頁の足し込み → 追従」の順で位置を決める。上端 64px 以内か「以前のメッセージ」button で `onLoadOlder`。live region は一覧とは別の `aria-live="polite"` 1 つで、確定した返事だけ |
| `history.ts` | `fetchOlder`（`before_seq` = 一番古い seq で頁を取り、`history` action を返す）・`useOlderMessages`・`useAttachmentMap`（`getAttachment` で添付を 1 回ずつ取る）・`attachmentIdsOf` |

`web/components/content/markdown.tsx` は最小に拡張した: fenced code（閉じていない fence は末尾まで code。`CodeBlock` とコピー button 44px）、表（`th scope="col"`、横は内側で scroll）、`links="target"`（link に 44px の操作領域）。既定の出力（段落・見出し・inline link の class）は変えていない。危険な href（`javascript:` など）は従来どおり文字列にする。

## home 葉への引き継ぎ

- `MessageList` には `state`（`useChatSession` の `snapshot.chat`）・`attachments`（`useAttachmentMap(attachmentIdsOf(state))`）・`renderCards`（cards 葉の部品）・`hasOlder/loadingOlder/onLoadOlder`（`useOlderMessages(state, session.dispatch)`）・`onRetryPending`（同じ client_message_id で `session.send`）・`onDiscardPending`（`pending_discard`）を渡す。
- 親は `MessageList` に高さを与える（`flex-1 min-h-0` の column の中に置く）。

## 証拠

| コマンド（cwd = web/ ・ install は `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`） | 結果 |
|---|---|
| `corepack pnpm@12.6.0 exec vitest run features/chat/messages` | 28 passed（試験名 `chat_messages_*`） |
| `corepack pnpm@12.6.0 test` | vitest 66 files / 449 passed、node server 57 pass / 0 fail |
| `corepack pnpm@12.6.0 typecheck` | exit 0 |
| `corepack pnpm@12.6.0 lint` | error 0（warning 4 は既存の styles.css の `!important`） |
| `corepack pnpm@12.6.0 check:boundaries` | exit 0 |
| `corepack pnpm@12.6.0 build` | exit 0 |

受け入れ条件との対応: markdown sanitize（`chat_messages_markdown_sanitizes_html_and_unsafe_links`）・tool 折り畳み（`chat_messages_tool_*`）・二重表示なし（`chat_messages_streaming_draft_then_final_message_is_shown_once`・`..._pending_and_acked_user_message_not_duplicated`）・追従/『最新へ』（`chat_messages_follow_*`）・live region（`chat_messages_live_region_*`）。

## 未解決

- 試験は node 環境の静的描画と純関数で行った（web の vitest は jsdom を持たない）。実 DOM の scroll（追従・位置保持・上端での読み込み）は e2e-chat 葉で確かめる。
- 未読は「末尾に足された timeline 項目」の数（tool・status は数えない）。

## 提案

- なし。
