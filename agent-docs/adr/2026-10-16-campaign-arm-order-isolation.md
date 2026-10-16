---
title: "campaign: arm 実行順の巡回/無作為化と、executor 失敗の試行内隔離"
tasks: [01M526E98AFXB02ZG468J789FB]
status: accepted
date: 2026-10-16
---
# ADR: campaign の arm 順と失敗の隔離

## 背景
`runner::plan` は problem ごとに A,B,C 固定の順で走らせていた。A が常に先に走るため、時刻・cache・外部負荷との順序交絡が A/B/C に偏る。また executor の `Err` は `run()` 全体を `?` で abort していたため、A の 1 件の失敗が B/C を止めた。

## 決定
- D1: manifest に `order`（`abc_rotation` | `seeded_shuffle`、既定 `abc_rotation`）と `seed`（u64、既定 0）を足す（manifest hash に入る）。problem ごとの arm 順を `plan()` が決定的に作る。
  - `abc_rotation`: problem の実行位置 i（`problem_order` 後の添字）で計画 arm 列を i mod n だけ回す。
  - `seeded_shuffle`: sha256(seed ‖ problem_id) を種に Fisher-Yates。problem 位置に依存しないので、再開しても同じ順になる。
  - 依存順 `problem_order`（前提が先）は常に守る。arm 順は problem の内側だけを動かす。
- D2: 各 trial 記録に `arm_rank`（その problem 内の実行位置、1 始まり）を書き、run-meta に `order`・`seed` を書く。checker が `plan()` と突き合わせられる。
- D3: executor の `Err`（sync の execute、async の lookup/submit/track/poll/usage）は run を止めず、その試行だけ `outcome=error, reason=executor_error` として記録する。async なら外部 task を best-effort で cancel して台帳を閉じる。台帳・fs・記録の書き込み失敗は従来どおり run の失敗。
- D4: `executor_error` の試行は使用量を観測できていないので、予算に課金しない（`tokens` は null のまま、`TrialRecord::charged_tokens` が 0 を返す）。欠測を無料扱いしない原則（budget-cap D2）の例外で、「A の恒常的な Err が全体上限を食い潰して B/C を止める」ことを避けるため。件数は `executor_errors` として別計上し、分母には error として残る。
- D5: arm 別の枯渇（`arm_budget`）は他 arm を止めない（budget-cap 済み）。全体枯渇のみ `stop_on_budget` で止める。`Wait` は従来どおり記録せず次へ進み（再実行で続き）、B/C を止めない。
- 新しいスケジューラは作らない。既存 executor/ledger/runner 経路の `plan()` の並びを変えるだけ。
