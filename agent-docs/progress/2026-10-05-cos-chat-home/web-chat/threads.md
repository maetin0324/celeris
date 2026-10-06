---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---
# web-chat / threads

ADR D5 の会話一覧を `web/features/chat/threads/` に実装した。home は `ChatThreads` に選択中の thread id・人待ち件数・`onSelectThread` を渡し、callback を `?thread=` へ写す。route、gui、crates、data 層は変更していない。

一覧は `listThreads({status:"open", q})` の FTS 結果を使用し、受信箱を固定先頭に、残りを `updated_at` と id の降順に並べる。受信箱の人待ち件数は thread API 型に無いため home から渡す。legacy には印を付ける。新規作成は失敗時も同じ `client_thread_id` を使い、成功後に選択 callback を呼ぶ。題名変更と archive は `expected_revision` を送る。409 は操作に応じた文言を表示し、archive 成功時だけ一覧から除く。desktop は sidebar、狭い幅は既存 Drawer を使う。

## 証拠

- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile`: 初回は pnpm store の `use-sidecar@1.1.3` 欠落で exit 1。`install --frozen-lockfile` で lockfile を変えず取得後、offline 再実行 exit 0。
- `corepack pnpm@12.6.0 -C web typecheck`: exit 0。
- `corepack pnpm@12.6.0 -C web exec biome check features/chat/threads`: exit 0。
- `corepack pnpm@12.6.0 -C web exec vitest run features/chat/threads`: 2 files、7 tests passed。`chat_threads_*` で新規の冪等 key、題名変更の revision と 409、q 検索結果、archive の 409、受信箱固定と legacy 表示を確認。実ブラウザで mobile Drawer の Escape・選択後の focus 復元と操作ボタン 44 px 以上を確認。
- `corepack pnpm@12.6.0 -C web lint`: exit 0（既存 `styles.css` 警告 4）。`corepack pnpm@12.6.0 -C web test`: exit 0（vitest 67 files/428 tests、server 57 tests）。`corepack pnpm@12.6.0 -C web check:boundaries`: exit 0。
- 再試行: 前回の check は `threads.test.tsx` の Biome format 違反 1 件で失敗した。`corepack pnpm@12.6.0 -C web exec biome format --write features/chat/threads` で修正し、`install --offline --frozen-lockfile && typecheck && lint && test` を同じ worktree で再実行して exit 0（lint は既存警告 4、vitest 428 件・server 57 件成功）。

## 未解決

- 受信箱の人待ち件数は thread API に含まれない。home の組み立て時に既存の受信箱データから `inboxWaitingCount` を渡す。
- 受信箱は一覧の最初の取得頁から記憶するため、100 件より古い受信箱が最初の頁に含まれない場合は、ページ送りで取得するまで表示できない。API に `kind=inbox` フィルタは無い。

## 提案

- home の組み立て時に `onSelectThread` を URL query と同期し、人待ち件数を既存 inbox データから渡す。
