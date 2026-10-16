---
title: "campaign 実行中の予算上限（per_case / per_arm）と欠測 tokens の扱い"
tasks: [01M526E98AFXB02ZG468J789FB]
status: done
updated: 2026-10-16
completed: 2026-10-16
---
# budget-cap

## 実装済み
- ADR `2026-10-16-campaign-budget-cap`（D1〜D5: 上限・適用経路・欠測の保守課金・最大超過の粒度・試験方針）。
- `Budget.per_case` / `per_arm`（`Cap{max_tokens,max_wall_secs}`、manifest 必須、`per_arm >= per_case`）。case.json・fixtures・試験も更新。
- `TrialRequest.budget`（実効上限 = min(per_case, per_arm の残り)）、`Executor::usage`（既定 None）。
- runner: 非同期は poll ごとに usage を見て超過なら `cancel_row`（子 task・job ごと）→ `cancelled/case_budget`（arm 側が先なら `arm_budget`）。同期は完了後に判定。arm が尽きた後の同 arm は `cancelled/arm_budget`（未実行）。他 arm は止めない。
- 欠測 tokens: 記録は null のまま、`tokens_missing` を別計上、上限判定は per_case.max_tokens を消費したとみなす（`tokens_charged_total`）。checker も同式で再計算し、per-case 超過の未 cancel を検出。
- RunReport / summary.json `budget`: `case_budget_cancelled`・`arm_budget_cancelled`・`tokens_missing`・`max_overshoot_*`・`overshoot_granularity`。

## 証拠
- `cargo test -p celeris-campaign --test budget_cap`: 6 passed（実時間なし、注入 executor）。
- `bash scripts/dev/test-parallel.sh`: passed 5473 / failed 0。`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`: 通過。

## 未解決・提案
- `CelerisExecutor` は `usage` 未実装（実 Celeris 経路は当面完了後判定）。task 累積 tokens の観測は後続 WU（routing-pin 付近）で接続する。
- 欠測の保守課金は campaign 全体上限（`stop_on_budget`）にも効く。欠測が多いと早く止まる。
