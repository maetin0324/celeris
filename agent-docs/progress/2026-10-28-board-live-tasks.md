---
title: ボードで作成の古い終端でない task が「新しい順 200 件」の外に落ちる不具合の修正
tasks: [01M61C9EHBCMPEZA0FPC00GP6B]
status: done
updated: 2026-10-28
---
# ボードで作成の古い終端でない task が「新しい順 200 件」の外に落ちる不具合の修正

完了日: 2026-10-28

## 変更
- `board: 終端でない task を全件読み、終端の列だけ新しい 200 件に切る` — web だけの変更。daemon の API は変えていない。
  - `web/features/projects/board-model.ts`: `boardTasksPath`（status 指定なし・新しい順 200 件の 1 要求）を消し、`fetchBoard` に置き換えた。
    終端でない状態（draft・ready・running・reviewing・blocked）は `status=` で絞った要求を `limit=500` で `next_cursor` を辿って全件読む。
    終端（done・failed・cancelled）は状態ごとに 1 要求、新しい順 200 件。各要求の `total` を列の実件数に使う。`run_state` 指定時は終端を読まない。
  - `boardGroup` は列ごとに `count`（読み切った列は表示件数、切った列は daemon の件数）と `truncated`（読んだ件数と全件数）を返す。
  - `web/features/projects/board-screen.tsx`: 状態の絞り込みと「すべて」の件数を `count` の和にした。切った列は見出しに
    「全 N 件のうち新しい 200 件を表示」と出し、表の上に `BoardTruncatedNotice`（「完了は作成の新しい 200 件まで表示しています。件数は全件の数です（裏方を含む）。」）を出す。
  - 試験: `board-model.test.ts` に、終端 260 件＋作成の古い running 1 件で running が「進行中」に出ること、終端の件数と範囲、`next_cursor` を辿った全件読み、`run_state` で終端を読まないことを追加。`board-truncated-notice.test.tsx` を追加。

## 証拠（提出時の HEAD で実行。基準は git merge-base HEAD main）
- `bash scripts/dev/test-affected.sh` → exit 0（web tsc -b 通過、vitest 100 files / 690 tests passed、server node --test 105 passed）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決
- 裏方（support）の判定は daemon で絞れないので、「裏方も表示」を外した状態で切った終端の列の件数は裏方を含む数になる。画面にその旨を出している。
- 終端の列の続きを読む操作は付けていない（範囲と全件数の表示だけ）。

## 提案
- 終端の続きが要るなら、`GET /tasks` に裏方を除く絞り込みを足し、列ごとに「さらに読む」を付ける。
