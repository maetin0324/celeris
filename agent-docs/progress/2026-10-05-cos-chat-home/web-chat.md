---
title: web-chat 全体検査と ADR 付記（ホームを CoS チャットに作り直し）
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-07
---

# web-chat 全体検査（統合後 HEAD）と D5・D6 突き合わせ

ADR 2026-10-05-cos-chat-home D5・D6 に従い、web のホーム（`web/routes/index.tsx`・`web/features/chat/`・`web/server/chat.js`）を CoS チャット中心に作り直した（会話一覧・streaming・添付・カード）。本葉（close）は実装を変えず、統合後の HEAD で全体検査を流し、`agent-docs/adr/2026-10-05-cos-chat-home.md` の D5・D6 に『実装との突き合わせ（web-chat）』の付記を足す。工程ごとの個別証拠は `agent-docs/progress/2026-10-05-cos-chat-home/web-chat/{data,fixture,gateway,cards,composer,messages,threads,home,e2e-chat,e2e-cards,e2e-attach,e2e-narrow}.md`。

## 全体検査結果（統合後 HEAD ed9ae61b、2026-10-07）

| 検査 | コマンド | 結果 |
|---|---|---|
| workspace 試験 | `bash scripts/dev/test-parallel.sh` | exit 0。nextest 141 binaries・4355 passed・0 failed・14 ignored（doc-test 10 binaries 込み）。nextest 84.1 s、doctest 9.4 s、151 binaries 計 |
| clippy | `cargo clippy --workspace -- -D warnings` | exit 0 |
| web typecheck | `corepack pnpm@12.6.0 -C web typecheck` | exit 0（`tsc -b`） |
| web lint | `corepack pnpm@12.6.0 -C web lint` | exit 0（`biome check .`。既存 styles.css の `!important` 警告 4 件のみ） |
| web 単体 | `corepack pnpm@12.6.0 -C web test` | exit 0。vitest 70 files / 486 passed、node server 57 passed |
| web build | `corepack pnpm@12.6.0 -C web build` | exit 0（既存の bundle size 警告あり） |
| web e2e 全体 | `WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test` | exit 0。277 passed・8 skipped・0 failed |
| web e2e チャット | `WEB_E2E_SCOPE=functional … exec playwright test e2e/chat` | exit 0。30 passed（threads 2・content 2・queue 5・resync 5・cards 6・attachments 2・mobile 8） |
| web boundaries/parity/secrets | `corepack pnpm@12.6.0 -C web run check:boundaries / check:parity / check:secrets` | 各 exit 0 |
| web mobile-audit | `corepack pnpm@12.6.0 -C web run mobile-audit` | exit 0。32 paths × 4 widths（360/390/412/1440 px）、`/` と `/console` を含む |
| ADR 採番 | `sh scripts/dev/check-adr-numbers.sh` | exit 0（149 files） |
| docs layout | `sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | exit 0 |
| docs links | `sh scripts/dev/check-doc-links.sh` | exit 0 |

試験はすべて決定的。web 単体は fake timer と試験側から流す `ReadableStream`、e2e は fake-daemon の制御 endpoint（`/__fixture/chat/threads/{t}/emit`・`hold`・`expire`・`disconnect`）と `expect.poll` の出来事待ち。sleep の長さ・確率・CPU 負荷には頼らない。keyboard は `visualViewport` の fake と resize イベント。

## crates/ と gui/ にこの task の差分が無いことの確認

- web-chat 工程（data〜e2e-narrow）の差分は `git diff --name-only c40b3669 HEAD` で **94 files**、すべて `web/`（73）・`agent-docs/`（21）。`crates/`・`gui/` は **0 files**。
- 本 leaf の作業ツリー base `ed9ae61b` との差分は文書（本ファイルと ADR 付記）だけで、`crates/`・`gui/` は未変更。
- なお main と task ブランチ（cos 工程込み）の差分には `crates/`・`gui/` が含まれるが、これは cos-run / store-api 等の並行工程由来であり web-chat のものではない（`c40b3669` は web-chat 以前に main を取り込んだ merge）。

## スクリーンショット（人による UX 確認用）

`agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots/`（計 8 枚）。`build` 後、`CHAT_SHOT_DIR="$PWD/agent-docs/progress/2026-10-05-cos-chat-home/web-chat/screenshots" WEB_E2E_SCOPE=functional corepack pnpm@12.6.0 -C web exec playwright test e2e/chat/<spec>` で再生成。

| 画像 | 幅 | 内容 | 出典 |
|---|---|---|---|
| chat-threads-1440.png | 1440 | 会話一覧と本文 | threads.spec |
| chat-resume-1440.png | 1440 | `?thread=` 再開で本文・composer が残る | threads.spec |
| chat-content-1440.png | 1440 | Markdown・code・表・tool 折りたたみ | content.spec |
| chat-queue-paused-1440.png | 1440 | 停止中のキューと再開 | queue.spec |
| chat-resync-1440.png | 1440 | cursor 410 後の snapshot 再取得 | resync.spec |
| chat-jump-latest-1440.png | 1440 | 上スクロール中の「最新へ」と未読 | resync.spec |
| chat-mobile-320.png | 320 | 添付 preview と CoS 代答カード | mobile.spec |
| chat-cards-1440.png | 1440 | 会話一覧と task・決定・質問カード | mobile.spec |

## 未解決（実装を直すべき点・正直に）

- **共有 Markdown の表パース**: `web/components/content/markdown.tsx` の `textBlocks` は code fence の直後に表が続くと表として解析しない（fence 除去で段落先頭に `\n` が残るため）。e2e は表を fence より前に置いて回避済み。`web/components/` は web-chat の範囲外なので未修正（e2e-chat.md の未解決に記録）。
- **CoS 代答カードの詳細 link の実 route 不在**: `card.href` をそのまま使うが、実 daemon が代答カードに置く `/cos/operations/{o}` のような href に web の route が無く、404 画面になる。fixture では実在 route（`/tasks/{id}`・`/inbox`・`/?thread=…`）で検証済み。実 route の追加は API/schema に触れるため別 task（cards.md の提案）。
- **triage が作るカードの href は `/?thread=<inbox>`**: 元の待ちの画面（`/tasks/{id}` など）へ直接行けない。元待ちの href を Card に足す案は API schema の変更のため別 task（cards.md の提案）。
- **override の本文の食い違い**: data 層は `gui-api.md` §3.129・生成型 `OverrideBody {action, reason}` に従った（ADR D2 の表の `{idempotency_key, expected_event_id, mode, reason}` と食い違う。実装と生成型を正とした。data.md の未解決）。
- **受信箱の badge 件数**: thread API に含まれず、home が既存の受信箱データ（`inboxItemsQuery` の `counts.total`）から `inboxWaitingCount` を渡す。100 件より古い受信箱は最初の頁に入らない限り表示できない（API に `kind=inbox` フィルタが無い。threads.md の未解決）。
- **keyboard は `visualViewport` fake**: 実端末 OS の keyboard・IME の実機確認は含まない（e2e-narrow.md）。
