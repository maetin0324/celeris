---
title: web の event 種別一覧に required_checks_evaluated を登録する
updated: 2026-10-23
tasks: []
status: done
---
# web の event 種別一覧に required_checks_evaluated を登録する

完了日: 2026-10-23

## 事実
- release gate の web-pnpm-typecheck（`tsc -b`）が落ちた。`api/realtime/event-kinds.ts` の網羅検査（TS2322）と
  `api/realtime/invalidation-map.ts` の `Record<EventKind, KindSpec>`（TS2741）。
- 原因: 「必須検査の結果を基底・候補 SHA に束縛して main 取り込み前に照合する」phase が Rust 側に
  `Event::RequiredChecksEvaluated`（`required_checks_evaluated`）を足し、generated types には出たが web の一覧に未登録だった。

## 修正
- `web/api/realtime/event-kinds.ts` の `EVENT_KINDS` に `required_checks_evaluated` を追加。
- `web/api/realtime/invalidation-map.ts` に `required_checks_evaluated: { sets: ["T", "E"] }`。
  根拠: この event は状態・attempts を変えず review の証跡を残すだけで、近い `review_target_synced` / `review_target_advanced` と
  同じく TaskDetail と timeline だけを無効化すればよい。
- ほかに Rust にあって web に無い kind は、型検査の網羅検査が通ったことで無いと確認した。

## 検査
- `pnpm typecheck`（web/、`tsc -b`）: exit 0。
- `pnpm test`（vitest 98 files / 682 tests、node --test 98 tests）: exit 0。
