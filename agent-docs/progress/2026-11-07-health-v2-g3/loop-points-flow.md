---
title: 健全性v2 G3 loop-points flow — Celeris でない repo の流れの試験と全経路の試験の確かめ
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: blocked
updated: 2026-11-09
---
# 健全性v2 G3 loop-points flow — Celeris でない repo の流れの試験と全経路の試験の確かめ

WorkUnit `flow`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D1.4（同梱の検査器）・D3.2・D4.1 を、Celeris でない repo で WU の check と direct の review の流れに通して確かめた。基準は `$CELERIS_WU_BASE`。

結論: 流れの試験 2 件を足した。WU の check の試験は合格した。direct の review の試験は、同梱の検査器での評価・Fail・review の repair・修復後の再評価・done まで通るが、最後の 1 つの assert（review の修復の class が rule ごとの `health:deps.violation` であること）で落ちる。本体の `try_review_repair` が ADR D3.2・D4.1 と違い、class を `health` 1 つにまとめて数えている。範囲外の本体の変更が要るので plan_issue で replan を申告した（下の「未解決」）。

## 変更（commit の件名）
- 「G3 loop-points flow: Celeris でない repo の WU の check と direct の review の流れの試験を足す」
- 内容（`crates/task-dispatch/src/dispatcher/tests/health_enforce.rs` だけ）
  - `write_checker`: 既存の stub の検査器（`STUB`・`ENGINE`）の file を repo 相対の配置で書く関数に切り出した。`commit_base_checker`（base の検査器）と `configure_failing_bundled_checker`（同梱の検査器）が同じ file を使う。stub の本文は増やしていない。
  - `configure_failing_bundled_checker`: 同じ stub を release の同梱の配置（`<dir>/scripts/dev/repo-health/`・`<dir>/CHECKER_TREE`）に書き、`gate::configure_bundled_checker` で process に設定する。nextest は試験ごとに process を分けるので、土台の合格の stub と衝突しない。dispatcher は土台の `new_test_dispatcher`（合格の stub を設定しない）で作る。
  - `init_non_celeris_repo`: base に `scripts/dev/repo-health` が無く、test 台本 `scripts/test.sh` が check-health を呼ばない repo。試験の中でこの 2 点を assert する。
  - `RepairCommitAdapter`: review の repair の WU（key `repair-*`）で作業ツリーに `FIXED` を commit する（stub は `FIXED` があれば合格）。

## 試験（守る不変条件）
- `health_enforce_non_celeris_work_unit_check_routes_to_repair`（合格）: Celeris でない repo の WU の check が同梱の検査器（証跡の `checker` が `CheckerSource::Bundled` の版）で health を評価し、new の error の `deps.violation` を `WorkUnitChecksFailed`（cmd `celeris:repo-health/1`、detail `health:deps.violation:…（evidence: src/lib.rs）`）として WU の retry に回す。retry が直した新しい HEAD で評価し直して Pass にし、WU は done。replan と質問は起きない。既存の `health_enforce_work_unit_check_*` は base の検査器を使い、`health_enforce_entry_non_celeris_*` は入口だけを見るので、同梱の検査器が WU の check の流れで当たることはこの試験だけが確かめる。
- `health_enforce_non_celeris_direct_review_routes_to_repair`（最後の assert で不合格）: Celeris でない repo の reviewing の task（project の repo、review 前の同期あり）の review が同梱の検査器で health を評価し（`RequiredChecksEvaluated.results[].health`、base = `target_sha`、head = 候補）、Fail を `review_repair` に回す。修復の後の review は新しい候補で Pass になり、task は done になる。ここまでは合格。最後に `RepairScheduled`（origin `Review`）の class が `["health:deps.violation"]` であることを assert し、実際は `["health"]` で落ちる。既存の `health_enforce_direct_review_*` は review を直接呼ぶ unit の試験で、`try_review_repair` が記録する class と数え方を通らない。

## 7 経路の試験の確かめ（`grep -rhoE "fn health_enforce_<経路>_[a-z_]+" crates/ | sort -u`）
- direct_review 6、work_unit_check 3、phase_integration 4、reviewer_reuse 6、pre_review_sync 1、integration_repair 4、delivery 3、non_celeris 2。health_enforce_ の試験は全部で 52 件。欠けている経路は無い。

## 証拠（この run の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-core -p task-dispatch -p celeris --no-tests=fail -E 'test(/health_enforce_/)'` → exit 100: 52 件中 51 passed・1 failed（`health_enforce_non_celeris_direct_review_routes_to_repair`。`left: ["health"]`、`right: ["health:deps.violation"]`）。
- 7 経路と non_celeris の `fn health_enforce_<経路>_` の grep（計画の check と同じ loop）→ exit 0（欠けなし）。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と計画の正規表現）→ 範囲外の path なし（変更は `dispatcher/tests/health_enforce.rs` とこの進捗 file）。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 100。check-health は verdict pass・exit 0（fail_grade 0、new 1）。nextest は 3652 件中 3651 passed・1 failed・11 skipped（不合格は上の既知の 1 件だけ）。

## 未解決
- 本体と ADR の食い違い（plan_issue で replan を申告した）: ADR D3.2 は「task の review の `repair_hint` は bucket `health:<rule_id>` を持つ（rule ごとに `max_repairs_per_class` を数えるため）」、D4.1 は「`max_repairs_per_class` の class は `health:<rule_id>`」と書く。段の統合の修復は `health_review::check_item::repair_count` で rule ごとの bucket を作り、`RepairScheduled.class` に `health:deps.violation` を書く。一方、task の review の修復（`crates/task-dispatch/src/dispatcher/review_verdict.rs` の `try_review_repair`）は `classify_review_failure` が返す `RepairClass::Health` の `bucket()`（`health`）で同じ class の修復を数え、repair WU の title（`repair (health): 修復`）と `RepairScheduled.class` にも `health` を書く。health の項目の `repair_hint.class`（`health:deps.violation`）は使われていない。結果として、review の経路では違う rule の Fail が 1 つの上限を分け合い、記録の class も段の統合と揃わない。合格にしてしまう穴ではない（上限が厳しくなる側）。
  - 直す場所の案: `try_review_repair` の同じ class の数えと title を、段の統合と同じ `check_item::repair_count`（reason の `health:<rule_id>:…` から bucket を作る）に通す。dispatcher module の行の純増を避けるなら、数えの部分を `health_review::check_item` 側の関数に寄せる。必要な範囲は `crates/task-dispatch/src/dispatcher/review_verdict.rs`（と、関数を寄せるなら `crates/task-dispatch/src/health_review/check_item.rs`）。この試験はそのまま回帰の試験になる。
- `bash scripts/dev/test-affected.sh` の不合格は上の 1 件だけ。本体が直れば exit 0 になる見込み（未確認）。

## 提案
- 無し（上の修正は replan で unit を足す）。
