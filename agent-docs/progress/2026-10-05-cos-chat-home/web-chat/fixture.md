---
tasks: [01M48GK6BHSW2GA1JECC7214P4]
status: done
updated: 2026-10-06
---
# web-chat fixture

`web/e2e/support/fake-daemon.mjs` に CoS チャットの状態付き API fixture を追加した。通常会話 2 件、受信箱 thread、legacy thread を用意し、240 件の履歴と全 7 種のカード（operation は CoS 代答・取消/差し戻しへの案内）を seed した。

会話一覧の検索・ページ送り、作成と冪等再送、題名と archive、履歴の前後 cursor、queue/interrupt、queued 取消、stop/resume、run の詳細と event、添付の upload/取得/preview/content/delete/references を扱う。SSE は保存済み event を cursor から再生して live に続ける。`/__fixture/chat/hold` で実行中の run を維持し、`/__fixture/chat/threads/{t}/emit` で任意の event を即時配信、`.../expire` で 410 を再現できる。制御 endpoint の型と使い方は `fake-daemon.d.mts` に記載した。

## 検証

- `corepack pnpm@12.6.0 -C web exec vitest run e2e/support/fake-daemon.test.ts` — 9/9 成功。検索・長い履歴・カード・冪等性・queue・SSE 再開/410・添付を確認。
- `corepack pnpm@12.6.0 -C web test` — 394 件と server 47 件が成功。
- `corepack pnpm@12.6.0 -C web typecheck` — 成功。
- `corepack pnpm@12.6.0 -C web e2e` — functional 243 件成功、8 件 skip。
- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` — js-tokens の tarball がローカル store に無く失敗。続く pnpm exec が依存を取得し、上記検査は成功。

## 未解決と提案

この fixture は UI 試験用で、CoS の実行は自動進行しない。e2e は hold と emit を使って出来事を進める。実 daemon の処理・権限・容量制限を検証する場合は task-api 側の試験を使う。
