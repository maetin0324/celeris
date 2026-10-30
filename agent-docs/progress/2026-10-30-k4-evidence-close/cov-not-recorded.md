---
title: K4 追試の証跡確定 — WU cov-not-recorded（人の op 0 件の集合を exact にしない）
tasks: [01M669QAZE5WEQDJBQS8MRX1AK]
status: done
updated: 2026-10-30
---
# K4 追試の証跡確定 — WU cov-not-recorded

完了日: 2026-10-30

ADR: `agent-docs/adr/2026-10-30-ops-r1-human-time-coverage.md` の付記「人の op 0 件の集合を exact にしない」。

## 背景
final review（reviewer run 01M66FWZSG6WFQ5YFYCPFPVSC5）が受け入れ条件 3 を不合格にした。再生成した t001 の human-ops-report が
`human_time_coverage` を `bound=exact`・`union_secs=0`・`trial_ratio=1.0` と出していた。評価者の op 記録（protocol・attempt）は
あるが人の op は 0 件の集合で、未記録の人の時間を 0 秒と確定していた。

## 確かめたこと
- human-op の schema（`crates/campaign/src/human_ops.rs` の `RecordType`）に、試行ごとに「人の op なし」を宣言する記録は無い。
  新設はせず、人の op 0 件は常に `not_recorded` とした。
- `scripts/campaign/check-out-consistency.py` は `bound` を照合していない（`human_time` は欠測の有無だけを見る）。変更なし。

## 変更
- `crates/campaign/src/human_ops_report.rs`
  - `Coverage` に `not_recorded_trials` と `recorded_trials()` を足した。人の op を記録した試行が 0 なら `missing`（op 記録なし）か
    `not_recorded`（op 記録あり）で `union_secs` null。欠測・未記録の試行・長さの無い区間・切った区間があれば `lower_bound`、
    どれも無いときだけ `exact`。`trial_ratio` の分子は `recorded_trials`。試行・arm・全体で同じ関数を使う。
  - 試行の `human_time` は人の op 0 件なら `source=not_recorded`・値 null。arm は `human_time_not_recorded_trials` に数え、
    `human_union_secs`・`human_sum_secs` に 0 を足さない。
  - report.md: arm 表の人の時間の括弧に未記録試行数、`coverage_line` に人の op を記録した試行・欠測・未記録の試行数、試行別表は
    「未記録」。注記と `HUMAN_TIME_COVERAGE_RULE` を新規則に直した。
- 試験: `crates/campaign/tests/human_ops_report.rs` に回帰 3 件
  （`attempts_without_human_ops_are_not_recorded_not_exact_zero`、`complete_human_op_intervals_stay_exact`、
  `partially_missing_human_ops_are_lower_bound`）。既存の `cutoff_or_job_only_records_leave_human_time_and_rework_unobserved`・
  `unrecorded_human_time_is_missing_with_coverage_not_zero`、`crates/celeris/tests/actor_trace_flow1.rs` の期待値を新規則に直した。
- 文書: `docs/ops/eval-human-ops-runbook.md`・`docs/ops/eval-raw-record-definitions.md`。
- 採点（grade）の code は触っていない。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p celeris-campaign --test human_ops_report` → 10 passed（新規 3 件を含む）
- `bash scripts/dev/test-affected.sh` → exit 0（2867 passed、9 skipped）。1 回目は
  `celeris::routing_eval_run daemon_runs_two_stub_trials_with_hidden_checks_and_eval_records` が「daemon API became ready」の
  待ちで落ちた（human-op と無関係、単独再実行で pass）。2 回目の全体実行で exit 0。
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- K4 の t001・t002 の report の再生成（`not_recorded` になること・採点不変の確認）は WU regrade-v2 の担当。

## 提案
- 評価者が「この試行で人の op は無かった」と確かめた場合に記録できる record（例: 試行ごとの `no_human_op` 宣言）を足せば、
  その試行だけ `exact` 0 を出せる。今回は schema を変えない方針（objective）なので提案に留める。
