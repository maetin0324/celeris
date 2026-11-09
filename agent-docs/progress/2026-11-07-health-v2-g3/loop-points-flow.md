---
title: 健全性v2 G3 loop-points flow — review の修復の class を rule ごとに直し、Celeris でない repo の流れの試験と全経路の試験を通す
tasks: [01M6ZDF9Q4BWD9DX6S9KX3KKS1]
status: done
updated: 2026-11-09
completed: 2026-11-09
---
# 健全性v2 G3 loop-points flow — review の修復の class を rule ごとに直し、Celeris でない repo の流れの試験と全経路の試験を通す

WorkUnit `flow`（task 01M6ZDF9Q4BWD9DX6S9KX3KKS1）。[ADR 2026-11-08](../../adr/2026-11-08-health-enforcement-loop.md) の D1.4（同梱の検査器）・D3.2・D4.1 を、Celeris でない repo で WU の check と direct の review の流れに通して確かめた。基準は `$CELERIS_WU_BASE`。

結論: Celeris でない repo の流れの試験 2 件を足し、どちらも合格した。direct の review の試験が見つけた本体の不具合（task の review の修復が health の class を rule ごとに数えていなかった）を、replan で範囲に入った `review_verdict.rs` で直した。7 経路の `health_enforce_` の試験は欠けが無く、52 件すべてが合格する。

## 変更（commit の件名）
- 「G3 loop-points flow: Celeris でない repo の WU の check と direct の review の流れの試験を足す」（`crates/task-dispatch/src/dispatcher/tests/health_enforce.rs` だけ）
  - `write_checker`: 既存の stub の検査器（`STUB`・`ENGINE`）の file を repo 相対の配置で書く関数に切り出した。`commit_base_checker`（base の検査器）と `configure_failing_bundled_checker`（同梱の検査器）が同じ file を使う。stub の本文は増やしていない。
  - `configure_failing_bundled_checker`: 同じ stub を release の同梱の配置（`<dir>/scripts/dev/repo-health/`・`<dir>/CHECKER_TREE`）に書き、`gate::configure_bundled_checker` で process に設定する。nextest は試験ごとに process を分けるので、土台の合格の stub と衝突しない。dispatcher は土台の `new_test_dispatcher`（合格の stub を設定しない）で作る。
  - `init_non_celeris_repo`: base に `scripts/dev/repo-health` が無く、test 台本 `scripts/test.sh` が check-health を呼ばない repo。試験の中でこの 2 点を assert する。
  - `RepairCommitAdapter`: review の repair の WU（key `repair-*`）で作業ツリーに `FIXED` を commit する（stub は `FIXED` があれば合格）。
- 「G3 loop-points flow: review の修復の class を段の統合と同じ rule ごとの bucket で数える」
  - 根本原因: `crates/task-dispatch/src/dispatcher/review_verdict.rs` の `try_review_repair` が、同じ class の修復の数えを `classify_review_failure` の `RepairClass::Health` の `bucket()`（`health`）で独自に書いていた。repair WU の title（`repair (health): 修復`）と、title から作る `RepairScheduled.class` も `health` だった。ADR D3.2・D4.1 と `RepairClass::Health` の doc comment は、class を rule ごとの `health:<rule_id>` と定める。段の統合（`phase_integration.rs`）は `health_review::check_item::repair_count` でそう数えていた。
  - 直し: `try_review_repair` は `failing_details`（失敗の reason）を先に作り、段の統合と同じ `check_item::repair_count(class, &failing_details, &repairs)` で bucket と同じ bucket の修復の数を得る。title は `repair ({bucket}): 修復` にした。health 以外の class の bucket は `class.bucket()` のままなので、数えは前と同じ（title に `,` が無い）。独自の数えは消し、同じ数え方を 2 か所に書いていない。
  - `crates/task-dispatch/src/health_review/check_item.rs`: `repair_count` の可視性を `pub(super)`（`phase_integration` だけ）から `pub(in crate::dispatcher)` に広げた。ほかは変えていない。
  - 新しい状態・分岐・fallback・互換分岐は無い。dispatcher module のコード行は 2 行減った（`size.responsibility-growth` は worsened 0）。
  - class が `health` であることを期待していた既存の試験は無かった（`repair (health)` と class の `"health"` の grep で 0 件）。

## 試験（守る不変条件）
- `health_enforce_non_celeris_work_unit_check_routes_to_repair`: Celeris でない repo の WU の check が同梱の検査器（証跡の `checker` が `CheckerSource::Bundled` の版）で health を評価し、new の error の `deps.violation` を `WorkUnitChecksFailed`（cmd `celeris:repo-health/1`、detail `health:deps.violation:…（evidence: src/lib.rs）`）として WU の retry に回す。retry が直した新しい HEAD で評価し直して Pass にし、WU は done になる。replan と質問は起きない。既存の `health_enforce_work_unit_check_*` は base の検査器を使い、`health_enforce_entry_non_celeris_*` は入口だけを見るので、同梱の検査器が WU の check の流れで当たることはこの試験だけが確かめる。
- `health_enforce_non_celeris_direct_review_routes_to_repair`: Celeris でない repo の reviewing の task（project の repo、review 前の同期あり）の review が同梱の検査器で health を評価し（`RequiredChecksEvaluated.results[].health`、base = `target_sha`、head = 候補）、Fail を `review_repair` に回す。修復の後の review は新しい候補で Pass になり、task は done になる。review の修復の `RepairScheduled`（origin `Review`）の class は rule ごとの `["health:deps.violation"]`（D3.2・D4.1）。既存の `health_enforce_direct_review_*` は review を直接呼ぶ unit の試験で、`try_review_repair` が記録する class と数え方を通らない。この試験は直す前の本体で `left: ["health"]`、`right: ["health:deps.violation"]` で落ち、直した後に合格した（回帰の試験）。

## 7 経路の試験の確かめ（`grep -rhoE "fn health_enforce_<経路>_[a-z0-9_]+" crates/ | sort -u`）
- direct_review 6、work_unit_check 3、phase_integration 4、reviewer_reuse 6、pre_review_sync 1、integration_repair 4、delivery 3、non_celeris 2。health_enforce_ の試験は全部で 52 件。欠けている経路は無い。

## 証拠（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `cargo nextest run -p task-core -p task-dispatch -p celeris --no-tests=fail -E 'test(/health_enforce_/)'` → exit 0: 52 tests run: 52 passed, 3692 skipped。
- `grep -q 'repair_count' crates/task-dispatch/src/dispatcher/review_verdict.rs` と旧い式の不在の grep（計画の check）→ exit 0。
- 7 経路と non_celeris の `fn health_enforce_<経路>_` の grep（計画の check と同じ loop）→ exit 0（欠けなし）。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` と計画の正規表現）→ 範囲外の path なし（変更は `dispatcher/review_verdict.rs`・`health_review/check_item.rs`・`dispatcher/tests/health_enforce.rs`・この進捗 file）。
- `cargo fmt --all -- --check` → exit 0。`cargo clippy --workspace -- -D warnings` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 0。nextest は 3652 tests run: 3652 passed (11 slow), 11 skipped。check-health は verdict pass・exit 0（fail_grade 0、new 1、worsened 0）。new の 1 件は optional の `dup.contract` の warning（`task_core::required_checks::health::UnavailableReason` と `route_select::availability::UnavailableReason` の同名）で、この unit の変更ではない。repo_health_gate の 7 件も合格。

## 経緯
- 最初の run は本体の不具合を見つけたが、plan_issue を checkpoint にだけ書いて done を返したため、計画の check が同じ 1 件で落ちた。2 回目の run は `yield.plan_issue` で replan を申告し、replan で `review_verdict.rs` と `check_item.rs` が範囲に入った。この run で本体を直した。

## 未解決
- 無し。

## 提案
- `check_item::repair_count` の doc comment は「段の統合の修復の bucket」と書いたままである。task の review の修復も同じ関数を使うようになったので、record か次の変更で「段の統合と task の review の修復の bucket」に直すとよい（この unit の指示が check_item.rs の変更を可視性に限ったため、ここでは変えていない）。
