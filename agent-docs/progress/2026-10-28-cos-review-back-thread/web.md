---
title: web の cron 画面から review thread の履歴を読み取り専用で開く
tasks: [01M61HW6RGHHRVAGRMQASDF9YY]
status: done
updated: 2026-10-28
---
# web の cron 画面から review thread の履歴を読み取り専用で開く

完了日: 2026-10-28

ADR [2026-10-28-cos-review-back-thread](../../adr/2026-10-28-cos-review-back-thread.md) の D1（web 側）・D6。

## 変更

- `phase web: cron の画面から review thread の履歴を読み取り専用で開く` — 次の内容。
  - `web/features/cron/queries.ts`: `reviewThreadQuery(threadId)`。`GET /api/chat/threads/{t}/messages?limit=50` を読み、
    新しい順（seq の降順）に並べる。
  - `web/features/cron/cron-screen.tsx`:
    - job の行（一覧・詳細）に、`review_thread_id` がある job（`cos_review` で review thread ができた後）だけ「見直しの記録を読む
      （読み取り専用）」の link を置く（`/cron/$id#review-thread`）。
    - 詳細画面に節「見直しの記録（読み取り専用）」（`id="review-thread"`）を置き、review thread の message を新しい順に最大 50 件
      表示する。投稿欄・添付・停止などの操作は置かない。`review_thread_id` が無い job では節を出さない。
  - `web/features/chat/threads/model.ts`: `homeThreads` で `kind = review` を除く。`load`（一覧）と `replace`（open・既読）の
    両方で使い、API が review を返してもホームに出ない。

## 試験

- `web/features/cron/cron-screen.test.tsx`（新設）: 一覧で review thread のある job だけに導線がある、詳細で履歴が新しい順に出て
  `<textarea`・`<form`・`type="file"`・「送信」が無い、`review_thread_id` の無い job に節が無い、空の表示、query の path と並び。
- `web/features/chat/threads/model.test.ts`: 一覧と既読の応答に review thread が来てもホームの thread に入らない。

## 証拠（提出時の HEAD で実行。基準は $CELERIS_WU_BASE）

- `bash scripts/dev/test-affected.sh` → exit 0。対象は web だけ（`--list`: web: typecheck+unit）。tsc -b 通過、vitest 101 files・
  696 passed、server の node --test 105 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `pnpm exec biome check features/cron features/chat/threads` → 指摘なし。

## 未解決事項

- 統括 thread の報告の message（provenance `cos_review_report`）から review thread の画面へ移る導線（ADR D6 の 3 点目）は
  この葉の objective に無く、作っていない。
- `cos_review` の job の「手動実行」ボタンは画面に残る（API が 422 で拒否し、結果欄に出る）。

## 提案

- close の葉で、報告の message の provenance から `/cron/<cos_review の job id>#review-thread` へ移る link を足すかを決める。
