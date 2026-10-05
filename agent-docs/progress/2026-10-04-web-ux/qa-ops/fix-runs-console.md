---
title: ops 画面群 visual QA — run ログ・Console の修正（fix-runs-console）
tasks: [01M44C029SCGEZHEK57WEK3QNB]
status: fixed
updated: 2026-10-04
---

# run ログ・Console の修正

対象: `web/features/runs/*`・`web/features/console/*`（route `web/routes/tasks.$id.runs.$runId.tsx` は配置だけなので無変更）。
screenshot は WU の artifacts の `pre/`・`post/`（`--only /tasks/T1/runs/R1` と `--only /` を 360/390/412/1440、build 後に撮影）。

## critique.md の runs/console 指摘への対応

`critique.md` の run ログ・Console 節は「新規指摘はなし」だった（指摘一覧 #1〜#6 に runs/console は無い）。
本葉の objective（長いログ・大量出力の scroll、loading/error/stale の見え方）に沿って再点検し、次を見つけて直した。

| # | 画面 | 重さ | 所見 | 対応 |
|---|---|---|---|---|
| R1 | Console | major | 初回取得の失敗後に stream を張ると hello が空の cache を書き、error が消えて「まだ会話がありません」と誤表示（取得失敗を 0 件と取り違える） | 修正: 取得に成功してから stream を張る（`use-console.ts`） |
| R2 | run ログ・Console | major | 取得失敗は「ページを再読み込みすると取り直します」の文だけで、画面から戻る導線が無い（他画面の FetchFrame は「再試行」を持つ） | 修正: `ErrorNotice`（role=alert・再試行）に揃え、run ログは最初から読み直す `retry` を hook に足した |
| R3 | run ログ | major | header は「未確認」（終了済み）なのに本文は「実行中（追記を追っています）」と食い違う（pre 390px）。終了が分かっても次の polling まで追い続ける | 修正: 終了が分かったら最後の 1 回をすぐ読んで追跡を止める。結果の無い終了 run は「終了（結果の記録なし）」 |
| R4 | run ログ・Console | minor | loading が文字の無い skeleton だけ（他画面は 1 秒後に「読み込み中…」） | 修正: `LoadingState`（遅延で「読み込み中…」、遅いと再取得）に揃えた |
| R5 | run ログ | minor | polling の上限（900 回）で追跡を黙って止める。長い run で続きが出なくなっても分からない | 修正: warning の `Notice`「追記の自動取得を止めました」＋「読み直す」 |
| R6 | run ログ | minor | task 詳細の取得失敗・run が一覧に無いとき header が黙って消える | 修正: Notice で「概要は出せないがログは読める」と伝え、取得失敗は再試行 |
| R7 | run ログ | minor | 本文の面は max-h-screen で、スマホでは面が画面より高く「最新へ」が画面外に出る | 修正: 「最新へ」を `sticky bottom-4` にして画面下端に貼り付けた |
| R8 | Console | minor | progress の「すべて見る」失敗に再試行が無い・「読み込み中」の表記ゆれ | 修正: 再試行ボタンを足し「読み込み中…」に揃えた |

stale（SSE 切断）は critique のとおり shell の接続状態で伝え、取得済みのログ・会話を残す設計のまま（`states.spec.ts` の stale `/`・`/tasks/T1/runs/R1` が通る）。

## 検証

- `corepack pnpm@12.6.0 -C web typecheck` / `lint`（既存の warning 5 件のみ、error 0）/ `test`（vitest 347 passed・node 42）/ `check:boundaries` / `check:parity` / `check:secrets` / `build`: exit 0
- `corepack pnpm@12.6.0 -C web e2e`: exit 0（166 passed・8 skipped。✘ は既存の期待どおり失敗 `long-text /inbox`）
- `web/e2e/parity/runs-files.spec.ts` 通過（h1・accessible name・URL は無変更）
- 新規 `web/e2e/runs/run-log-recovery.spec.ts`: run ログ・Console の 503 から「再試行」で戻れる（2 passed）
- 生の色・任意値 class の追加なし。入力欄（textarea）の枠は base の `--color-input` のまま

## 残課題

- `mobile-audit` は exit 1。違反は `/projects/P1`・`/tasks/T1`・`/tasks/T1/changes` の 3 画面のみ（本葉の範囲外、兄弟葉・他 task の画面）。`/`・`/tasks/T1/runs/R1` に違反は無い
- run ログの event の種類 label に英語（`agent`/`user`/`tool`/`system`）が残る。Console の手順行（`console-labels.ts`）も `agent`/`tool` を使い Celeris の用語として一貫しているため、変えるなら両方を同時に（verify 葉か次の QA で判断）
- polling 上限の Notice（R5）と header の Notice（R6）は e2e で直接は撮れていない（上限 900 回・fixture 側の変種が要る）。必要なら states.ts に変種を足す専用の葉で
- error 状態の screenshot は `--states` の代表画面に run ログ・Console が無いため撮れていない（critique の「状態別 screenshot の欠落」と同じ）
