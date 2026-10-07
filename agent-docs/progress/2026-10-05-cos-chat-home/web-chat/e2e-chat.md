---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---
# web-chat / e2e-chat（会話一覧・streaming・停止/キュー・IME・最新へ）

ADR-2026-10-05-cos-chat-home D6 UI 行のうち会話側を Playwright で検証した。spec は `web/e2e/chat/` に 5 本（threads / content / queue / resync / support）。本文・run の進行は偽 daemon の制御 endpoint（`/__fixture/chat/threads/{t}/emit`・`hold`・`expire`・`disconnect`）と SSE 出来事待ち（`expect.poll`）で決定的に進め、sleep の長さや確率には頼らない。

## 検証した内容（D6 UI 行・会話側）

- **会話一覧**: 新規作成（`?thread=chat-new-1` へ選択）・題名変更（form→保存、revision 経由で反映）・検索（本文にだけ含む語で絞り込み）・archive（一覧から消え `status=archived` に残る）（`threads.spec.ts`）。
- **再開**: 共有 `?thread=` URL で開き再読込しても同じ会話・本文・composer が残る（`threads.spec.ts`）。
- **Markdown/code/表/tool**: 見出し・太字・インラインコード・link（`/console`）・script 文字列（文字のまま）・表（`<table>` 1 枚）・fenced code（言語ラベル付き scroll 領域）・tool の折りたたみ（detail 非表示→展開で CodeBlock）と完了で「成功」に変わる、二重表示なし（`content.spec.ts`）。
- **streaming**: `text_delta` を UTF-8 byte offset で 3 回流し、確定 message（同じ id）で置き換え。streaming 中は caret 付きで 1 項目、確定後 caret 消え・本文 1 回・status 行が消える（`content.spec.ts`）。
- **再接続**: 再読込は cursor（`Last-Event-ID`・`?after=`）から続きを読み、古い event を二重に適用しない。切断（`disconnect`）→ 410（`expire`）→ snapshot 取り直し → 取り直した cursor で再接続。新着も継げる（`resync.spec.ts`）。
- **停止・キュー**: 実行中にキューへ追加→「停止」で `queue_paused`（停止中の送信待ち＋再開ボタン）→ 停止中の送信は `resume_queue=true` →「キューを再開」で元順番に表示。割り込み送信は `mode=interrupt` で active run を `stopping`（`queue.spec.ts`）。
- **入力**: Enter で送信、Shift+Enter は改行のみ、IME composition 中の Enter（`isComposing`・keyCode 229）は送信しない・改行も足さない、完了後の Enter（13）で送信（`queue.spec.ts`）。
- **scroll**: 上へ scroll 中は位置を保ち「最新へ（未読 N 件）」が出る、押すと下端へ。古い頁（`before_seq`）を足しても見ている位置を保つ（`resync.spec.ts`）。

## 見つけた不具合（web/features/chat/ で修正）

1. **`features/chat/data/reducer.ts`（`applySnapshot`）**: 再読込・停止などの refresh（snapshot 取り直し）で `queue` が初期値にリセットされ、キュー表示が消えていた。snapshot の queued message と `thread.queue_paused` からキューを導くよう `queue` を `applySnapshot` で設定（queue event が来ればそのまま置き換え）。
2. **`features/chat/composer/chat-composer.tsx`**: `paused` の判定が `chat.thread?.queue_paused ?? chat.queue.paused` で、snapshot 後の瞬時に `queue_paused` が stale な true になるケースでキュー停止表示が固定されていた。`chat.queue.paused`（snapshot でも queue event でも導かれる正本）に統一。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: exit 0（lockfile up to date）。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `corepack pnpm@12.6.0 -C web lint`: exit 0（biome。既存の styles.css の `!important` warning 4 件のみ）。
- `corepack pnpm@12.6.0 -C web test`: exit 0（vitest + node server 試験。reducer の queue 維持試験 `chat_data_resnapshot_keeps_queue_until_replaced` を追加）。
- `corepack pnpm@12.6.0 -C web build`: exit 0（`dist/` 生成）。
- `WEB_E2E_SCOPE=functional CHAT_SHOT_DIR=<worktree>/agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`: **14 passed**（threads 2 / content 2 / queue 5 / resync 5）。6 枚の screenshot を保存。
- 範囲 check（`git diff --name-only $CELERIS_WU_BASE` + untracked を `web/e2e/chat/`・`web/e2e/support/`・`web/features/chat/`・`agent-docs/progress/2026-10-05-cos-chat-home/web-chat/` で除外）: 範囲外 0 件。
- e2e の strict mode 違反と IME の誤検出を修正:
  - content: 追加メッセージと fixture メッセージに同名の `コード（sh）`・「成功」cell が重なったため、`[data-slot="chat-body"]` の本文で絞る。
  - queue: 生の `keyboard.press("Enter")` は browser 既定で改行を足す artifact。実 IME の確定キーは改行を挿入しないため、`isComposing`・keyCode 229 の `keydown` を dispatch して再現。

## screenshot（desktop 1440 px、各 300 KB 以下、計 6 枚・上限 8 枚以内）

`agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots/` 以下:

- `chat-threads-1440.png` — 会話一覧（新規作成後・検索・archive）
- `chat-resume-1440.png` — `?thread=` の再開（240 件履歴・再読込）
- `chat-content-1440.png` — Markdown/code/表/tool 折りたたみ
- `chat-queue-paused-1440.png` — 停止中のキュー（再開ボタン）
- `chat-resync-1440.png` — 切断→410→snapshot 取り直しの再接続後
- `chat-jump-latest-1440.png` — 上へ scroll 中の「最新へ（未読 2 件）」

## 未解決

- **共有 Markdown 部の表パース（スコープ外・別 WU 担当）**: `components/content/markdown.tsx` の `textBlocks` は、code fence の直後に表が続くと表として解析されない（`parseBlocks` の fence 除去で段落先頭に `\n` が残り、表ヘッダ行が `lines[0]` に来ないため）。直しの例は段落分割後に `raw.replace(/^\n+|\n+$/g, "")` を挟むこと。本 WU は `web/components/` を触れない範囲 check のため、spec（`content.spec.ts`）は表を code fence より前に置いて回避している。修正は `web/components/content/markdown.tsx` を担当する WU（close 段など）で行うこと。
- 会話側の e2e は本 WU で完了。添付（ボタン/D&D/paste）・カード・320 px と下部タブバー・keyboard と mobile-audit は並行の `e2e-mobile` WU が担当。
- e2e 中の `expect.poll` は既定 10 s の保険。fake daemon の SSE は同期配信なので通常は即決着する（sleep 依存ではない）。
