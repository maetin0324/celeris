---
title: web-chat 全体検査と ADR 付記（ホームを CoS チャットに作り直し）
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---

# web-chat 全体検査（統合後 HEAD）と D5・D6 突き合わせ

ADR 2026-10-05-cos-chat-home D5・D6 に従い、web のホーム（`web/routes/index.tsx`・`web/features/chat/`・`web/server/chat.js`）を CoS チャット中心に作り直した（会話一覧・streaming・添付・カード）。本 leaf（reclose）は実装を変えず、polish 段（fix-visual・shots）を統合した HEAD で全体検査を再実行し、`agent-docs/adr/2026-10-05-cos-chat-home.md` の D6 小節に新しい screenshot と試験名を足す。工程ごとの個別証拠は `agent-docs/progress/2026-10-05-cos-chat-home/web-chat/{data,fixture,gateway,cards,composer,messages,threads,home,e2e-chat,e2e-cards,e2e-attach,e2e-narrow,fix-visual,shots,reclose}.md`。

## final review の 4 指摘への修正（polish 段、実装は polish 段のみ）

| # | 指摘 | 修正（commit） | 検証する試験 |
|---|---|---|---|
| (1) | streaming 中の画面が無い | `2cfc4400`（shots）: fake-daemon の hold 制御 endpoint で run を保持し `text_delta` 3 回流した生成途中（caret 付きの本文＋「考え中」status＋停止ボタン）の `chat-streaming-1440.png` を追加 | e2e `content.spec.ts`「streaming は text_delta を流し、確定 message に置き換えて二重表示にしない」（`expect` で caret 1 個・本文 1 項目・「停止」ボタンの可視性を撮る前に検証） |
| (2) | 会話一覧の「旧会話」badge が 1 文字ずつ縦積み | `bc0f30e4`（fix-visual）: badge は `shrink-0 whitespace-nowrap` で一行を維持し、題名を `min-w-0 truncate` で省略。共通 Badge の折り返し設定は不変 | vitest `chat_threads_*`（長い題名と両 badge の class/構造）＋ e2e `threads.spec.ts`「長い題名の旧会話でも badge は一行に収まり、題名を省略する」（badge の矩形が一行分の高さ・会話ボタン内に収まることと題名の実際の省略を 1440 px で検証） |
| (3) | tool 状態「成功」の白地・薄緑が読めない | `bc0f30e4`（fix-visual）: tool の成功・失敗・実行中は既存 token `text-success-foreground`・`text-danger-foreground`・`text-running-foreground` を使う。白地 contrast は 7.13:1・8.31:1・7.27:1、hover accent 背景でも 6.07:1・7.07:1・6.18:1（いずれも WCAG AA 4.5:1 以上）。`web/styles.css` の token 不変 | vitest `chat_messages_*`（3 状態のラベル・前景色 class と実 token の contrast を固定） |
| (4) | 狭い幅は 320 の 1 枚だけで、drawer 開き・送信後添付（画像表示）が無い。thumbnail が白い四角 | `2cfc4400`（shots）: `chat-drawer-320.png`（drawer 開いた 320）と `chat-attachment-sent-320.png`（送信後の会話内画像 preview）を追加。白い thumbnail は mobile.spec の upload が 1px 白 PNG だったことと fake-daemon の preview endpoint が固定 1px PNG を返していたこと。両方を 96×64 の青・黄格子（`COLORED_PNG`）に変更し、送信前後で `naturalWidth=96`（>0）と送信後の可視性を検証 | e2e `mobile.spec.ts`「320 px で添付・カードは横溢れを出さず、カードの操作領域は 44 px 以上」（送信前 preview の `naturalWidth` が 96 になり送信後画像が表示されるまで poll）＋「320/390 px: 狭い幅では会話一覧は drawer になり、開閉と Escape の focus 復元が成り立つ」 |

## 全体検査結果（統合後 HEAD `b1e61693`、2026-10-07）

| 検査 | コマンド | 結果 |
|---|---|---|
| workspace 試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest 141 binaries・4355 passed・0 failed・14 ignored（doc-test 10 binaries 込み）。nextest 77.2 s、doctest 8.5 s、151 binaries 計 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| web typecheck | `corepack pnpm@12.6.0 -C web typecheck` | exit 0（`tsc -b`） |
| web lint | `corepack pnpm@12.6.0 -C web lint` | exit 0（`biome check .`。既存 styles.css の `!important` 警告 4 件のみ） |
| web 単体 | `corepack pnpm@12.6.0 -C web test` | exit 0。vitest 70 files / 490 passed、node server 57 passed |
| web build | `corepack pnpm@12.6.0 -C web build` | exit 0（既存の bundle size 警告あり） |
| web e2e 全体 | `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test` | exit 0。278 passed・8 skipped・0 failed |
| web e2e チャット | `WEB_E2E_SCOPE=functional … exec playwright test e2e/chat` | exit 0。31 passed（threads 3・content 2・queue 5・resync 5・cards 6・attachments 2・mobile 8） |
| web boundaries/parity/secrets | `corepack pnpm@12.6.0 -C web run check:boundaries / check:parity / check:secrets` | 各 exit 0 |
| web mobile-audit | `corepack pnpm@12.6.0 -C web run mobile-audit` | exit 0。32 paths × 4 widths（360/390/412/1440 px）、`/` と `/console` を含む |
| ADR 採番 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（149 files） |
| docs layout | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| docs links | `sh scripts/dev/check-doc-links.sh` | exit 0 |

試験はすべて決定的。web 単体は fake timer と試験側から流す `ReadableStream`、e2e は fake-daemon の制御 endpoint（`/__fixture/chat/threads/{t}/emit`・`hold`・`expire`・`disconnect`）と `expect.poll` の出来事待ち。sleep の長さ・確率・CPU 負荷には頼らない。keyboard は `visualViewport` の fake と resize イベント。

## crates/ と gui/ にこの task の差分が無いことの確認

- web-chat 工程（data〜e2e-narrow・polish）の差分は `git diff --name-only c40b3669 HEAD` で **101 files**、すべて `web/`・`agent-docs/`。`crates/`・`gui/` は **0 files**。
- 本 leaf の WU base `b1e61693` との差分は、再生成した 4 枚の screenshot（chat-content・chat-mobile・chat-queue-paused・chat-streaming。build と fixture の差で像素がわずかに動くため）と本 leaf の文書（本ファイル・ADR 付記・reclose.md）だけ。`git diff --quiet "$CELERIS_WU_BASE" -- crates/ gui/` は exit 0（crates/・gui/ 未変更）。
- なお main と task ブランチ（cos 工程込み）の差分には `crates/`・`gui/` が含まれるが、これは cos-run / store-api 等の並行工程由来であり web-chat のものではない（`c40b3669` は web-chat 以前に main を取り込んだ merge）。

## スクリーンショット（人による UX 確認用）

`agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots/`（計 **11 枚**、最大 127,836 bytes）。polish 段（fix-visual・shots）を統合した本 HEAD `b1e61693` で `build` 後、`CHAT_SHOT_DIR="$PWD/agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots" WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat`（31 passed）で再生成した。

| 画像 | 幅 | 写っている状態 | 出典（spec / 試験） |
|---|---|---|---|
| chat-threads-1440.png | 1440 | 会話一覧（新規・検索・編集・保管）と本文。旧会話 badge は一行 | threads.spec「新しい会話を作る、題名を変える、検索で見つけ、保管で一覧から消す」 |
| chat-resume-1440.png | 1440 | `?thread=` 再開で長い会話の末尾（履歴 226〜240）と composer が残る | threads.spec「共有された ?thread= URL で開き、再読込しても同じ会話と本文・composer が残る」 |
| chat-content-1440.png | 1440 | Markdown・code（コピー）・表・tool 折りたたみ（read・grep の濃い緑「成功」） | content.spec「Markdown・code・表・tool の折りたたみと展開を 1 回だけ表示する」 |
| chat-streaming-1440.png | 1440 | **生成途中**：「進捗はこうです。」「2 段落」の caret 付き本文・「考え中」status・停止ボタン | content.spec「streaming は text_delta を流し、確定 message に置き換えて二重表示にしない」 |
| chat-queue-paused-1440.png | 1440 | 順番待ちメッセージ・キュー停止中・取消・キュー再開 | queue.spec（停止でキュー停止・resume で再開） |
| chat-resync-1440.png | 1440 | cursor 410 後の snapshot 再取得。認可・計画承認・通知カードと「最新へ 2」 | resync.spec「stream 欠落のあと cursor が 410 で失効したら、snapshot を取り直して続きから読む」 |
| chat-jump-latest-1440.png | 1440 | 上スクロール中の「最新へ 2」と未読 | resync.spec「上へ scroll 中は位置を保ち、未読と『最新へ』が出て、押すと下端へ戻る」 |
| chat-cards-1440.png | 1440 | task・決定・質問の人待ちカード、その場回答ボタン・詳細 link、CoS 代答の取消・差し戻し | mobile.spec「カードの操作（選択肢・取消/差し戻し・詳細）は desktop でも 44 px 以上」 |
| chat-mobile-320.png | 320 | CoS 代答カードと**送信前の色付き thumbnail**（96×64 青黄格子）、composer と下部タブバー | mobile.spec「320 px で添付・カードは横溢れを出さず、カードの操作領域は 44 px 以上」 |
| chat-drawer-320.png | 320 | **drawer を開いた**会話一覧（新規・検索・会話項目・編集・保管・閉じる）。badge 一行 | mobile.spec「320/390 px: 狭い幅では会話一覧は drawer になり、開閉と Escape の focus 復元が成り立つ」 |
| chat-attachment-sent-320.png | 320 | **送信後の会話内画像 preview**（96×64 色付き）と 208 B・送信待ちの取消、composer と下部タブバー | mobile.spec「320 px で添付・カードは横溢れを出さず、カードの操作領域は 44 px 以上」 |

## 未解決（実装を直すべき点・正直に）

- **共有 Markdown の表パース**: `web/components/content/markdown.tsx` の `textBlocks` は code fence の直後に表が続くと表として解析しない（fence 除去で段落先頭に `\n` が残るため）。e2e は表を fence より前に置いて回避済み。`web/components/` は web-chat の範囲外なので未修正（e2e-chat.md の未解決に記録）。
- **CoS 代答カードの詳細 link の実 route 不在**: `card.href` をそのまま使うが、実 daemon が代答カードに置く `/cos/operations/{o}` のような href に web の route が無く、404 画面になる。fixture では実在 route（`/tasks/{id}`・`/inbox`・`/?thread=…`）で検証済み。実 route の追加は API/schema に触れるため別 task（cards.md の提案）。
- **triage が作るカードの href は `/?thread=<inbox>`**: 元の待ちの画面（`/tasks/{id}` など）へ直接行けない。元待ちの href を Card に足す案は API schema の変更のため別 task（cards.md の提案）。
- **override の本文の食い違い**: data 層は `gui-api.md` §3.129・生成型 `OverrideBody {action, reason}` に従った（ADR D2 の表の `{idempotency_key, expected_event_id, mode, reason}` と食い違う。実装と生成型を正とした。data.md の未解決）。
- **受信箱の badge 件数**: thread API に含まれず、home が既存の受信箱データ（`inboxItemsQuery` の `counts.total`）から `inboxWaitingCount` を渡す。100 件より古い受信箱は最初の頁に入らない限り表示できない（API に `kind=inbox` フィルタが無い。threads.md の未解決）。
- **keyboard は `visualViewport` fake**: 実端末 OS の keyboard・IME の実機確認は含まない（e2e-narrow.md）。
