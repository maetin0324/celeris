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
  `RoutingRuntime::settle_quota`（`routing_runtime/quota.rs`）。各終了経路は `RoutingRuntime::settle_quota` を直接呼ぶ。`Dispatcher::resolve_quota_estimate` の転送関数も削除した。旧い本体と
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

## 前回提出時の証拠（今回の結果は末尾に記録）

- 条件0〜2: artifacts の `acceptance.py`（ADR D1 から field を抽出、純粋 module の語・薄い入口・QuotaBook・guard を検査）→ exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` と同じ基点で変更12 path を確認。ADR 本体と `token_budget.rs` は `$CELERIS_WU_BASE` との差分ゼロ。

## 前回提出時の検査の結果

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

## 前回の未解決の記録

コードの未解決事項は無し。最初の検査は sandbox が検査枠の lock 書き込みを拒み exit 2、必要な権限で再実行して exit 0。
試験終了時の一時 file 削除に Permission denied の警告が3行出たが、試験と doc test は成功した。
範囲外の script_exhaustive.rs の未使用 import と、依存 deno_core の警告は残る。

## 提案

無し。

## commit（件名）

- `phase: own-quota: quota の見積りの判断を純粋関数に分け、quota の状態を RoutingRuntime へ移す`
- `phase: own-quota: 進捗に検査の結果を書く`

- `phase 1: split quota estimate proposals and cover calibration priority` — 判断を終わりの形ごとの関数へ分割し、較正の表形式試験を追加。

## 再試行の brief（旧い転送関数の撤去）

1. 原因: 判断の抽出後も `Dispatcher::resolve_quota_estimate` の転送関数が残り、計画の撤去 check に当たった。
2. owner: 適用は `RoutingRuntime::settle_quota`、判断は `propose_estimate`。
3. 再利用: 既存の純粋関数と quota の試験。終了の値は owner の引数へ直接渡し、不要になった `QuotaRunEnd` を削除する。
4. 選択: 改名ではなく転送関数を削除し、各終了経路から owner を直接呼ぶ。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去: Dispatcher の転送関数とその説明。module map・ADR-0082・architecture-map を直す。
7. 不変条件: 呼び出し位置と値、event の保存先と順序、予算判定を維持する。既存の assertion を変えず検査する。

## 再試行の証拠（提出時の HEAD、基準は `$CELERIS_WU_BASE`）

以下は今回の最終版で実行。同期は行っていない。前回の合格記録とは分けて記録する。

- WU 基準の health で `dispatcher.rs` の本体が 2143 → 2144 行と判定されたため、knowledge cache の期限判定・更新を owner の `knowledge_reachability` にまとめた。cache の get/set method は撤去。probe の注入元は変更しない。
- `knowledge_cache_expiry_and_clear` は URL ごとの分離、期限未満・期限ちょうど、明示の消去後の呼び出し回数を注入時刻で検査する。既存の構造試験はこの境界を検出しない。
- 直接呼び出しの構造体リテラルで各終了関数が2行増えたため、account と adapter を組にした owner の引数を使う。呼ぶ位置と値は同じ。wrapper と転送専用の型を残さない。

- 呼び出し元の修正は quota の引数の受け渡しだけ。`planner_flow.rs`・`worker_finish.rs`・`review_verdict.rs`・`health_review/design_gate.rs` では呼ぶ位置・event の扱いを変えていない。`dispatch_run.rs`・`review_spawn.rs` はコメント内の呼び出し先名だけを直した。
- `#[allow(clippy::too_many_arguments)]` は削除した転送関数から owner の入口へ移した。純粋判断は従来どおり `EstimateInput -> EstimateProposal`。
- 再試行途中の health は構造体リテラルを置いた版で exit 1。最終版の WU 基準 health は exit 0（fail-grade 0、new 0、worsened 0）。検査器・policy・閾値は変更していない。
- 最終版 `clippy --workspace -- -D warnings` は exit 0。最初の `test-affected` は検査枠の lock が sandbox で読み取り専用となり exit 2。必要な権限で同じコマンドを再実行し exit 0。


## 最終結果

- 条件0: ADR D1 の own-quota の4 field が Dispatcher に無い。依頼文の awk/sed/grep check を同じ cwd から実行 → exit 0。
- 条件1: 同 check の `! grep -rn 'fn resolve_quota_estimate' crates/task-dispatch/src/dispatcher` → exit 0。`acceptance-final.py`（指定された成果物ディレクトリ）で純粋 module の禁止語が無いことも確認 → exit 0。
- 条件2: `acceptance-final.py` で QuotaBook の private field・試験 field 無し・OWNER_STRUCTS 登録を確認 → exit 0。構造試験も下記の影響範囲試験に含む。
- `bash scripts/dev/test-affected.sh` → exit 0、3860 passed、11 skipped、402.517秒。script bundle SKIP 0件。quota の表形式5試験、構造試験、cache 境界試験を含む。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/check-health.sh --base "$CELERIS_WU_BASE"` → exit 0、fail-grade 0。`test-affected` 内の health も exit 0。
- `python3 scripts/dev/check-architecture-map.py` → exit 0、1028 path。`sh scripts/dev/progress-index.sh --check` → exit 0。`git diff --check` → exit 0。
- `sh "$CELERIS_WU_SCOPE_PATHS"` → exit 0、18 path を確認。既存12 path に加えた6 path は quota 終了呼び出し4件と呼び出し名のコメント2件だけ。ADR 本体と `token_budget.rs` は `$CELERIS_WU_BASE` との差分ゼロ。
- commit: `phase 1: remove dispatcher quota forwarding and own cache lifecycle`。

## 未解決（今回）

コード・試験の不合格は無し。試験後の一時 file 削除で Permission denied が3件出た（コマンドは exit 0）。
依存 deno_core の deprecated 警告と範囲外 script_exhaustive.rs の unused import 警告は変更していない。
本番への移行は不要。提案は無し。
