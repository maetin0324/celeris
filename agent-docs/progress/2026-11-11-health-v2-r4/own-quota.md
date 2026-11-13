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

- 条件0〜2: artifacts の `acceptance.py`（ADR D1 から field を抽出、純粋 module の語・薄い入口・QuotaBook・guard を検査）→ exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と同じ基点で変更12 path を確認。ADR 本体と `token_budget.rs` は `$CELERIS_WU_BASE` との差分ゼロ。

## 検査の結果（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh`: exit 0。3859 件 pass（11 skipped、413 秒）。
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `sh scripts/dev/progress-index.sh --check`: ok。`bash scripts/dev/check-health.sh`: exit 0（all checks passed）。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）: 変更 path は task-dispatch の routing_runtime・dispatcher の入口と試験、ADR-0082、architecture-map、進捗だけ。
- `leases.rs`（lease の高 risk path）は `self.eval_runs.len()` を `self.routing.eval_run_count()` に直した 1 行だけで、lease の判定は変えていない。

## 今回の補正の brief（2026-11-13）

1. 根本原因: 抽出済みの見積り関数に単独 run とグループの計算が集中し、較正を渡す分岐が表形式の試験に無かった。
2. owner: `routing_runtime/quota_estimate.rs`。
3. 再利用: 既存の提案型と `task_core::quota::decide_window`。
4. 選択: 大きい判断関数を残す案に代え、終わりの形の選択・単独 run・閉じたグループの提案を分ける。設計は既存 ADR D3 のまま。
5. 新状態・fallback・port: 無し。
6. 撤去: 主関数内の旧い計算本体とコメント内の検査対象語。
7. 不変条件: 観測優先、較正の最小件数、estimated を較正標本にしない。既存の表は較正無しだけなので `quota_estimate_calibration_priority_table` を追加した。

## 未解決

コードの未解決事項は無し。最初の検査は sandbox が検査枠の lock 書き込みを拒み exit 2、必要な権限で再実行して exit 0。
試験終了時の一時 file 削除に Permission denied の警告が3行出たが、試験と doc test は成功した。
範囲外の script_exhaustive.rs の未使用 import と、依存 deno_core の警告は残る。

## 提案

無し。

## commit（件名）

- `phase: own-quota: quota の見積りの判断を純粋関数に分け、quota の状態を RoutingRuntime へ移す`
- `phase: own-quota: 進捗に検査の結果を書く`

- `phase 1: split quota estimate proposals and cover calibration priority` — 判断を終わりの形ごとの関数へ分割し、較正の表形式試験を追加。
