---
title: 監査修正4 budget-reserve（予約と実消費の整合）
updated: 2026-10-16
status: done
tasks: [01M530Y2NMKB1PAAXYH1W77893]
---

# 監査修正4 budget-reserve

設計は [ADR](../../adr/2026-10-16-campaign-budget-reservation.md)。完了日: 2026-10-16。

## 着手時の確認

err-taxonomy で台帳に `reserved`/`observed_usage` は入ったが、予算判定（runner の `arm_use`・`tokens_charged_total`）は
完了記録だけで、in-flight の予約を引いていなかったため実装した。

## 変更

- `budget.rs`（新規）: `spend`/`hold_of`/`admit`/`effective_cap`/`max_overshoot_tokens`。
- `runner.rs`: 新規投入前の gate（Admit/Defer/Exhausted）。summary に予約・残り・超過式。`RunReport.reserve_deferred`。
- `tests/budget_reserve.rs`: mock で残りが予約を下回る投入見送り・Exhausted 打ち切り・並列 1・usage 欠測/通信断/更新待ち・
  cancel 停止未確認の予約維持と二重計上なし・再起動後の同値。

## 想定最大超過（report）

- tokens: `max_parallel × (poll 間隔 + 終端確認) × 消費速度`。並列 1・60s・120s・1,000 tokens/s で 18 万。wall は task の wall 上限で硬く抑える。
- 実行側の token 上限は未実装（提案: task に token 上限を渡す。watch-driver / closeout で検討）。

## 未解決事項・提案

- 同期 executor には予約 gate を入れていない（台帳を持たない）。
- 起動前に実 poll 間隔・消費速度の実測で式の値を埋める（runbook は closeout）。
