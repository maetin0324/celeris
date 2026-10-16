# ADR 2026-10-16: campaign の実行中予算上限（per_case / per_arm）と欠測 tokens の扱い

- 日付: 2026-10-16
- 状態: 採用・実装済み（crates/campaign）
- 関連: 2026-10-11-breaking-changes-by-default、crates/campaign の executor / ledger / runner

## 背景
`Budget` は campaign 全体の `max_tokens` / `max_wall_secs` しか持たず、runner は試行が**終わってから**累積を見て
次の試行を止めるだけだった。1 試行（または 1 arm）が上限を大きく超えても止まらず、tokens が欠測（None）の試行は
0 として数えられ「無料」になっていた。

## 決定
- D1: `Budget` に `per_case`（1 試行の上限）と `per_arm`（1 arm の累積上限）を足す。どちらも `{max_tokens, max_wall_secs}`
  で、manifest で必須（0 は不可、`per_arm >= per_case`）。manifest hash の互換は保たない（breaking-changes-by-default）。
  従来の `max_tokens` / `max_wall_secs` は campaign 全体の上限として残る（全 arm の総量）。
- D2: 実行中の適用は既存経路に載せる。新しいスケジューラは作らない。
  - 非同期 executor: runner は `poll` のたびに `Executor::usage`（子 task・job を含む累積使用量）を見て、実効上限
    （`min(per_case, per_arm の残り)`）を超えたら既存の `cancel_row`（子 task・クラスタ job ごと cancel、台帳を cancelled に）で止め、
    `outcome=cancelled` / `reason=case_budget`（arm の残りが先に尽きた場合は `arm_budget`）で記録する。
  - 同期 executor: `TrialRequest.budget`（実効上限）を渡す。executor が自分で止めてもよい。止めなかった（止められない）場合は
    runner が結果を受け取った時点で超過を判定し、同じく cancelled に直す（使用量は実測値を残す）。
  - arm の残りが 0 以下になった後の同 arm の試行は実行せず `cancelled` / `arm_budget`（使用量なし）で記録する。他 arm は止めない。
  - `Timeout`（`trial_timeout_secs`）は従来どおり優先する。timeout と case_budget が同時に成立したら timeout のまま。
- D3: 欠測 tokens（None）を 0 にしない。
  - 記録の `tokens` は null のまま（0 にしない）。
  - 集計では別に数える: `RunReport.tokens_missing` と summary の `budget.tokens_missing`。
  - 上限判定では**保守的に上限消費とみなす**: 欠測の試行は `per_case.max_tokens` を消費したものとして arm 累積と
    campaign 累積（`tokens_charged`）に加算する。`tokens_total` は従来どおり観測できた値の和（欠測は含めない）で、
    `tokens_charged_total` が判定に使う値。checker も同じ式で再計算する。
  - 実行中の欠測（usage の tokens が None）は tokens 上限では止めない（判断材料が無い）。wall は別に判定する。終了後に
    上のとおり保守的に課金する。
- D4: 最大超過の粒度を明示する。上限は「超えたら止める」で「超えさせない」ではない。
  - 非同期: 1 poll 間隔の間に進んだ使用量（runner の走査 1 回分）まで超過し得る。
  - 同期: 1 試行ぶん（executor が自分で止めない限り上限を超えた完走結果）まで超過し得る。
  - arm 上限: 最後の 1 試行（上の粒度）ぶん超過し得る。
  - 実際の超過量は `RunReport.max_overshoot_tokens` / `max_overshoot_wall_secs` と summary の `budget.max_overshoot_*`、
    `budget.overshoot_granularity`（文字列）に出す。
- D5: 試験は実時間を使わず、注入した mock executor（使用量を台本で返す）で決定的に書く（`tests/budget_cap.rs`）。

## 結果・限界
- `CelerisExecutor` は `usage` を実装していない（None）ため、実 Celeris 経路では当面「終了後判定（同期と同じ粒度）」に
  なる。`max_wall_secs` の task への伝達だけ `min(timeout, per_case wall)` にした。task の累積 tokens の観測は
  後続の接続 WU の範囲。
