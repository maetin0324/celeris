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
- `corepack pnpm@12.6.0 -C web install --offline --frozen-lockfile` — 再試行（attempt 2）では exit 0。

## 再試行（前回 check の不合格への対処）

- `web lint` が base（c40b3669）の時点で既に落ちていた: `components/content/artifact-preview.test.tsx`・`components/ui/{confirm-dialog,drawer,gallery}.test.tsx` の import 並び（biome organizeImports）4 件。`biome check --write` でこの 4 file の import 行だけ直した（範囲外だが check を通すのに必要な機械的修正）。
- `grep -q '/api/v1/chat/threads' fake-daemon.mjs` が落ちていた: 経路は path を分解して照合しており文字列が無かった。`handle` の上に受ける経路の一覧の注釈を足した（挙動は不変）。
- 計画の check 全体（install --offline --frozen-lockfile → typecheck → lint → test）を同じ cwd で実行: exit 0。lint は 4 warnings（styles.css の !important、既存）・0 error、test は 62 files / 394 件 + server 47 件 成功。grep check も exit 0。

## 未解決と提案

この fixture は UI 試験用で、CoS の実行は自動進行しない。e2e は hold と emit を使って出来事を進める。実 daemon の処理・権限・容量制限を検証する場合は task-api 側の試験を使う。

## 再試行 2（attempt 2 の範囲 check 不合格）

- 範囲 check が上の 4 file（import 並びの修正）を範囲外として落とした。4 file を base に戻すと `corepack pnpm@12.6.0 -C web lint` は base と同じ 4 error（organizeImports）で exit 1、戻さないと範囲 check が exit 1。計画の check 同士が両立しないので plan_issue として申告した。
- 4 file を戻した状態で確かめた結果: typecheck exit 0、vitest 62 files / 394 件成功、server 47 件成功、fake-daemon.test.ts 9/9、build exit 0、e2e functional 243 件成功・8 skip。範囲 check は exit 0。
- 提案: fixture の範囲 check の allow に `web/components/(content/artifact-preview|ui/(confirm-dialog|drawer|gallery))\.test\.tsx` を足す（作業ツリーは import 修正を残した HEAD のまま）。または base の lint 不合格を別の葉で直し、この葉の check から lint を外す。
