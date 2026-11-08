---
title: 健全性v2 G2 design-budget — 設計 review の run に木の run・費用の上限を掛ける（付記 C1）
tasks: [01M6XHG4PVB3C2V2YT769ZPPSV]
status: running
updated: 2026-11-08
---
# 健全性v2 G2 design-budget — 設計 review の run に木の run・費用の上限を掛ける（付記 C1）

WorkUnit `limit-hold`。[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の付記「二度目の final review の差し戻しへの方針」(c) C1 を実装した。二度目の final review の差し戻し理由 3.（設計 review の run が `tree_run_limit_hold` より前に起き、木の上限を通らない）への修正のうち、木の上限の部分。token 予算の watch（C2〜C4）は並行の WorkUnit `run-watch` が担当する。基準は `$CELERIS_WU_BASE`。ADR は編集していない（付記との違いは下の節に書き、unit reclose が ADR に写す）。

## 変更
- commit「G2 design-budget: 設計 review の run の前に木の run・費用の上限を tree_limit_hold_for で判定し kind: limit の決定を出す」
  - `dispatcher/tree_units.rs`: `Dispatcher::tree_run_limit_hold(task, gate)` を 2 つに分けた。
    - `Dispatcher::tree_limit_hold_for(task, next: task_core::NextRun)`: 判定と決定の要求。中身は旧い本体のまま（`task_ops::tree::tree_counters`・`effective_tree_limits`・`task_core::tree::run_limit_breach`・`open_decision` の重複止め・`limit_decision` と `with_unknown_cost_note`）。木が無効・木の節点でなければ `false`。
    - free fn `tree_units::next_run(gate) -> Option<NextRun>`: gate から run の種類を決める。統合・最終レビュー・Skip は `None`（木の上限で止めない。旧い本体の `return Ok(false)` と同じ）。
  - `dispatcher/dispatch_run.rs`（`dispatch_one`）: `if let Some(next) = tree_units::next_run(&gate) && self.tree_limit_hold_for(&task, next)?` に置き換えた。挙動は変えていない。
  - `health_review/design_gate.rs`（`design_review_hold`）: 実行中の run の待ちと、再試行の上限で不合格を記録する分岐の後に `tree_limit_hold_for(task, NextRun::Worker)` を呼ぶ。`true` なら run を起こさず `Ok(true)`（設計 review の待ちのまま）を返す。人が上限を上げれば次の tick で run が起きる。
  - 旧い名前を指していた注釈（`dispatcher/work_units.rs`・`tree_units.rs` の `decision_self_hold`）を `tree_limit_hold_for` に直した。
  - 試験の部品: `tests/health_review_design_gate.rs` の `DIGEST`・`STORE_PATH`・`stub`・`checker_repo`・`v3_plan_with` と、`tests/tree_gate.rs` の `seed_tree_run` を `pub(super)` にして再利用した（同じ部品を写していない）。

### 消した物
- 関数 `Dispatcher::tree_run_limit_hold`。`grep -rn "tree_run_limit_hold" crates/task-dispatch/` → 0 件。

### 足した試験・直した試験
- 新しい file `crates/task-dispatch/src/dispatcher/tests/health_review_design_budget.rs`（prefix `health_review_design_budget_`、3 件）。高 risk の計画の設計 review を待つ木の root（`DesignReviewRequested` を積んだ状態）と、runs 索引に先に積んだ木の run から始め、`tick` を直接回す。時計に依存しない。
  - `health_review_design_budget_tree_runs_limit_holds_the_design_review_run`: 木の run 数 2 が `max_tree_runs = 2` に達している。設計 review の run（Reviewer の `WorkerStarted`・runs 索引の Reviewer の行）は起きず、adapter に run が届かず、`limit:max_tree_runs` の決定（`DecisionRequested` と決定の行）が木に 1 件だけ出る。2 回目の tick でも同じ。
  - `health_review_design_budget_tree_usd_limit_holds_the_design_review_run`: token の使用量から単価で計算した木の費用 2.4 USD（`claude-sonnet-5` の input 1M = 2.0 USD、cache read 2M = 0.4 USD）が `max_tree_usd` 2 USD を超えている。run 数には余裕がある。同じく run は起きず、`limit:max_tree_usd` の決定が木に 1 件だけ出る。本文に「費用不明」が無い（単価で数えた超過）。2 回目の tick でも同じ。
  - `health_review_design_budget_within_limits_starts_the_design_review_run`: 上限の内（run 数 1 < 2、費用 2.0 < 3 USD）なら、開始の tick で Reviewer の `WorkerStarted` と runs 索引の行が書かれ、決定は出ない。reviewer は `ReviewRequest.design`（risk high、方針の `definition_sha256`）を受け取る。
  - hold の呼び出しを外す（`if false && …`）と、上の 1 件目と 2 件目が落ちることを手元で確かめた（確認の後に戻した）。
- `tests/tree_gate.rs`: `tree_usd_limit_*` の 2 件を `tree_limit_hold_for(&root, NextRun::Worker)` に直した。gate から run の種類への写像を固定する `tree_run_limit_next_run_follows_the_dispatch_gate` を足した（Atomic・RunWorkUnit → Worker、RunPlanner → Planner、統合・最終レビュー・Skip → None）。

## 付記との違い
- C1 は「設計 review の run を起こす直前」に判定すると書く。実装は、実行中の run の待ちと再試行の上限の分岐の後、review 方針の読み込み（`plan_review_policies`。`git archive` と python）と `DesignReviewRequest` の組み立ての前に判定する。止める場合に方針を読まないためで、判定の条件と結果は付記と同じ。
- 付記は gate から `NextRun` を決める部分の置き場所を書いていない。`Dispatcher` の method 数（458）を増やさないため、`tree_units.rs` の free fn `next_run` にした。
- 目的の「token の上限」: 木の上限（`TreeLimits`）に token 数の種類は無い。token の上限は、token の使用量から単価で計算した費用の上限（`max_tree_usd`）として試験した（上の 2 件目）。費用不明の run があるときに止まる（ADR 2026-11-06-budget-usd-caps D2-4）のは既存の判定のままで、この試験では費用不明の run を置いていない。
- `dispatch_one` の上の 2 行の注釈は `tree_limit_hold_for` の doc comment に移した。`dispatch_one` の行数（1,114）を増やさないため。

## 証拠（提出時の HEAD で実行。基準は check-health が `git merge-base HEAD main`、範囲 check が `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。先頭の check-health は `verdict=pass findings=718 new=0 worsened=0 existing=718`。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-dispatch で、nextest は 3579 passed・11 skipped。
  - repo health の `size.responsibility-growth` の値は base と同じ: `crates/task-dispatch/src/dispatcher` の module の本体の行 35,169、`dispatcher::Dispatcher` の method 数 458、`Dispatcher::dispatch_one` の関数の行 1,114。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest list -p task-dispatch -E 'test(/health_review_design_budget_/)' --message-format oneline | grep -c health_review_design_budget_` → 3。
- `cargo nextest run -p task-dispatch -E 'test(/health_review_design_budget_|tree_usd_limit|tree_run_limit_next_run|health_review_design_gate_/)'` → 14 passed。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を `crates/task-dispatch/src/` と この進捗 file で絞る）→ 範囲外の path は 0 件。

## 未解決
- `crates/task-core/src/tree/limits.rs` の `TreeLimitKind::is_run_time` の doc comment が旧い名前 `tree_run_limit_hold` を指している。この WorkUnit の範囲（`crates/task-dispatch/src/`）の外なので直していない。ADR-0079 の本文も旧い名前を書いている（ADR は編集しない）。reclose で名前を直すか判断してほしい。
- 設計 review の run への token 予算の watch（付記 C2〜C4）は WorkUnit `run-watch` の担当で、この WorkUnit では入れていない。

## 提案
- なし。
