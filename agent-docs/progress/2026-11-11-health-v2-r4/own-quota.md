---
title: "健全性v2 R4 own-quota: quota の見積りの判断を純粋関数に分け、quota の状態を RoutingRuntime へ移す"
tasks: [01M6TYVJT4S29ZMP6P0D4831JG]
status: done
updated: 2026-11-13
---
# 健全性v2 R4 own-quota

完了日: 2026-11-13

## 概要

- ADR D1 の表で unit 列が own-quota の 4 field（`quota_activity`・`quota_calibration`・`eval_runs`・`knowledge_probe_cache`）を `Dispatcher` から消した。
  quota の 2 件は下位 owner `routing_runtime/quota.rs` の `QuotaBook`（private field）、`eval_runs`・`knowledge_probe_cache` は `RoutingRuntime` の field。
  `RoutingRuntime` の field は 15 → 18（quota は `QuotaBook` 1 個として数える）。
- `resolve_quota_estimate`（約 210 行）の判断は `routing_runtime/quota_estimate.rs` の純粋関数 `propose_estimate(EstimateInput) -> EstimateProposal`
  （この run の event・閉じたグループの他の member の event・較正へ足す標本）。適用（較正の更新と他の member の event の追記）は
  `RoutingRuntime::settle_quota`（`routing_runtime/quota.rs`）。`Dispatcher::resolve_quota_estimate` は呼ぶだけの約 20 行。旧い本体と
  `decide_quota_window`・`quota_windows_for` は消した（純粋 module に移した）。
- `EvalRunEntry` は owner が `dispatcher` module を使わないよう `routing_runtime` に移した。
- ADR-0082 に付記、`dispatcher.rs` の module map の `quota_book` の行（入口だけを持つ行に直した。module は入口として残る）、`docs/architecture-map.md` を直した。
  基準は `$CELERIS_WU_BASE`。

## 実装前 brief

1. 根本原因: quota の状態 2 件と評価 run・knowledge cache が `Dispatcher` の field で、`resolve_quota_estimate` は判断と書き込み（較正・他 task の event）が 1 関数に混ざっていた。
2. 責務 owner: `RoutingRuntime`（`QuotaBook`）と `quota_estimate::propose_estimate`。
3. 既存の再利用先: `QuotaActivity`・`QuotaCalibrationBook`（`accounts.rs`）と既存の試験、`task_core::quota` の判定関数。
4. 選択肢: field を包むだけ（不採用）／判断を純粋関数に分け適用を owner の method に置く（採用）。
5. 新しい状態・分岐・fallback・port: 無し。`EstimateInput`・`EstimateProposal` は値の束。
6. 撤去対象: `Dispatcher` の 4 field と初期化、`resolve_quota_estimate` の旧い本体、`decide_quota_window`、`quota_windows_for`、`EvalRunEntry` の定義。
7. 不変条件: 見積りの優先順位（measured → apportioned → estimated → unknown）、費用不明で止める・予約と解放・`token_budget.rs` の判定は変えない
   （この unit はその file を触らない）。event の内容と追記の順序は旧い実装と同じ（既存の `quota_activity_tests`・`quota_calibration_tests`・worker/review の quota の試験が同じ assertion で通る）。

## 試験の変更

- 足した試験（`quota_estimate.rs`）: `quota_estimate_table`（終わりの形ごとの窓の数・method・追記先・標本の表）、
  `quota_estimate_closed_group_addresses_other_members`、`quota_estimate_closed_group_without_own_run_is_unknown`、`quota_estimate_windows_per_adapter`。
  既存の試験は `Dispatcher` を通した結果だけを見るので、判断の分岐（暫定 unknown・按分・自分の run が無い閉じ方）を 1 件ずつ検出できなかった。
- `structure_guard_routing_runtime`: field を 18 件に、純粋 module の対象に `quota_estimate.rs` を足し、`quota.rs` が `Dispatcher` を受けないことと
  `QuotaBook` の field が private なことを確かめる。`structure_guard.rs` の `OWNER_STRUCTS` に `QuotaBook` を足した（`#[cfg(test)]` の field は無い）。
- 試験の field 直参照（`eval_runs`）は `routing` の method（`eval_run_count`・`drain_eval_runs`・試験用の `eval_run_handles_for_test`）に置き換えた。assertion は変えていない。

## 証拠（提出時の HEAD で実行）

- 結果は下の「検査の結果」。
