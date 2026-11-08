---
title: 健全性v2 G2 design-budget — 設計 review の run に木の上限と token 予算の watch を掛ける（付記 C1〜C4）
tasks: [01M6XHG4PVB3C2V2YT769ZPPSV]
status: done
updated: 2026-11-08
completed: 2026-11-08
---
# 健全性v2 G2 design-budget — 設計 review の run に木の上限と token 予算の watch を掛ける（付記 C1〜C4）

[ADR 2026-11-07 health-review-integration](../../adr/2026-11-07-health-review-integration.md) の付記「二度目の final review の差し戻しへの方針」(c) C1〜C4 を実装した。二度目の final review の差し戻し理由 3.（設計 review の run が `tree_run_limit_hold` より前に起き、木の上限も token 予算の watch も通らない）への修正。WorkUnit `limit-hold` が C1（木の run・費用の上限）を、WorkUnit `run-watch` が C2〜C4（token 予算の watch）を担当した。基準は `$CELERIS_WU_BASE`。ADR は編集していない（付記との違いは各節に書き、unit reclose が ADR に写す）。

## 受け入れ条件と証拠（task の完了の条件）
検査は提出時の HEAD（WorkUnit `run-watch` の最後の commit）で実行した。check-health の基準は `git merge-base HEAD main`。
- 上限超えでは設計 review の run が起きず `kind: limit` の決定が出る → C1 の試験 `health_review_design_budget_tree_runs_limit_holds_the_design_review_run`・`health_review_design_budget_tree_usd_limit_holds_the_design_review_run` が合格（下の「C1」節）。
- 設計 review の run に token 予算の watch が付く → C2〜C4 の試験 5 件が合格（下の「C2〜C4」節）。
- watch の組み立てが 1 つの関数にまとまっている → `token_budget::run_watch` だけが `TokenBudgetWatch::new` を呼ぶ。`grep -rn "TokenBudgetWatch::new" crates/task-dispatch/src --include=*.rs | grep -v /tests/` → `token_budget.rs` の中（`run_watch` の本体と、その doc comment）だけ。
- test-affected・clippy・fmt が exit 0 → 下の「C2〜C4 の証拠」。

# WorkUnit limit-hold（C1）

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

## C1 の未解決
- `crates/task-core/src/tree/limits.rs` の `TreeLimitKind::is_run_time` の doc comment が旧い名前 `tree_run_limit_hold` を指している。この WorkUnit の範囲（`crates/task-dispatch/src/`）の外なので直していない。ADR-0079 の本文も旧い名前を書いている（ADR は編集しない）。reclose で名前を直すか判断してほしい。
- 設計 review の run への token 予算の watch（付記 C2〜C4）は WorkUnit `run-watch` が入れた（下の節）。

# WorkUnit run-watch（C2〜C4）

## 変更
- commit「G2 design-budget: token 予算の watch の組み立てを token_budget::run_watch にまとめ、設計 review の run を final review と共通の run_within_token_budget で止める」
  - C2 `dispatcher/token_budget.rs`: `run_watch(store, task, exclude_run) -> Result<Option<Arc<TokenBudgetWatch>>, StoreError>` を置いた（`budget_scope` → `tree_tokens_used` → `TokenBudgetWatch::new`。予算の無い task は `None`）。使う所は 3 か所で、error の扱いは呼び出し側が決める。
    - `dispatcher/dispatch_run.rs`（`dispatch_one`、worker・planner の run）: `extras.token_budget = token_budget::run_watch(…, Some(&run_id))?`。今のとおり `?`。
    - `dispatcher/review_spawn.rs`（final review）: `pick_reviewer` に引数 `token_budget: Result<Option<Arc<TokenBudgetWatch>>, StoreError>` を足した。`spawn_review` は `run_watch(&*self.store, &task, None)` の結果をそのまま渡し、`pick_reviewer` は `Err` なら今のとおり warn を出して watch 無しで走る。`pick_reviewer` は受け取った 1 つの `Arc` を `ReviewerSink` と `ReviewerRun` の両方に入れる（今と同じ）。
    - `health_review/design_gate.rs`（`start_design_review`）: `token_budget::run_watch(…, None)?` で読み、`pick_reviewer(task, &subject, Ok(watch))` に渡す。読めなければ run を起こさず `Err` を返し、次の tick でやり直す（C1 の判定と同じ扱い）。
    - `token_budget::exhausted`（final review の前の「既に尽きている」判定）も `run_watch` を使う形にした（同じ組み立てを 2 回書かない）。値は今と同じ `(prior, max)`。
  - C3 `review.rs`（dispatcher/ の外）: `run_within_token_budget(adapter, req, run: &ReviewerRun)` を出した。`run.token_budget` が既に上限なら adapter を起こさず、途中で `wake` が鳴ったら `kill_tree_with` で process tree を止める。どちらも `watch.terminal()`（`BudgetExhausted{Tokens}`、usage は最後の累積値）で終える。watch が無ければ adapter の run を待つだけ。`run_reviewer_inner`（final review）と `design_gate.rs` の `run_design_reviewer`（設計 review）の両方がこれを呼ぶ。
  - C4: 新しい分岐は足していない。`BudgetExhausted{Tokens}` で終わった設計 review の run は、`run_end_of` が `RunEnd::BudgetExhausted { kind: Tokens }` を記録し、`settle_design_review` が「done で終わらない run」として判定にしない。次の dispatch で `max_reviewer_retries` まで起こし直し、超えたら `record_design_verdict` が `DesignReviewed { pass: false }` を書く。予算が尽きた後の起こし直しは adapter を起こさない。

### 消した物
- `review_spawn.rs` の `pick_reviewer` の中の組み立て（`budget_scope` と `tree_tokens_used` の入れ子の match、warn 2 種）。warn は 1 種（`failed to read the token budget; reviewing without the mid-run stop`）にまとめた。
- `dispatch_run.rs` の `dispatch_one` の中の組み立て（4 行）。
- `review.rs` の `run_reviewer_inner` の中の watch の処理（`match &run.token_budget` と `tokio::select!`）。`run_within_token_budget` に移した。
- `token_budget::exhausted` の中の `budget_scope` と `tree_tokens_used` の呼び出し。
- `grep -rn "budget_scope\|tree_tokens_used" crates/task-dispatch/src --include=*.rs` の token_budget.rs の外の残りは `worker_finish.rs` の 1 か所（run の終わりの「使った量」の判定で、watch の組み立てではない）だけ。

### 試験
`crates/task-dispatch/src/dispatcher/tests/health_review_design_budget.rs` に 5 件を足した（prefix `health_review_design_budget_`、合計 8 件）。偽の adapter `UsageAdapter` は設計 review の run で `UsageProgress` を 1 回出し、`hang` なら止められるまで終わらない（`std::future::pending`）。run の終わりは spawn した run の `JoinHandle` で待つ（30 秒の `timeout` は失敗したときだけ効く保険）。sleep で待たない。
- `health_review_design_budget_run_watch_reads_the_tree_budget`: `run_watch` は木の全 run の消費（2,000）を前置きにし、`observe` の累積値との和が `max_tokens`（5,000）に達すると `exceeded`。`exclude_run` の行は数えない。予算の無い task は `None`。
- `health_review_design_budget_reviewer_run_and_sink_share_the_watch`: `pick_reviewer` に渡した watch が `ReviewerRun.token_budget` と同じ `Arc`（`Arc::ptr_eq`）で、sink の `usage_progress` が同じ watch を `exceeded` にする。`Ok(None)` と `Err`（final review の warn の経路）では watch が付かない。
- `health_review_design_budget_usage_over_budget_stops_the_design_review_run`: 上限の内（1,000 / 5,000）で起きた設計 review の run で、偽の adapter が 4,000 の `UsageProgress` を出すと run が止まる。`WorkerFinished { role: reviewer }` は `end = BudgetExhausted{Tokens}`、outcome に `5000/5000`、usage は 4,000。判定（`DesignReviewed`）は出ず、起こし直しの run は adapter に届かない（adapter に届いた run は 1 件）。
- `health_review_design_budget_without_budget_runs_to_done`: 予算の無い task では同じ `UsageProgress` を出しても止まらず、`end = Completed` で終わる。
- `health_review_design_budget_exhausted_budget_never_starts_the_adapter`: 木の消費 6,000 が `max_tokens` 5,000 を超えている。設計 review の run は 2 回（`max_reviewer_retries = 1` の起こし直しを含む）とも adapter に届かず `BudgetExhausted{Tokens}`（`6000/5000`）で終わり、その後に `DesignReviewed { pass: false }` が 1 件だけ書かれる。
- 手元の確認: `design_gate.rs` で watch を渡さない形（`Ok(watch.filter(|_| false))`）にすると、3 件目（30 秒の保険で timeout）と 5 件目が落ちた。確認の後に戻した。

## 付記との違い
- C2 は「`review_spawn.rs` の組み立てを移す」と書くが、場所の指定は無い。`pick_reviewer` は `spawn_review` と設計 review の両方から呼ばれるので、`run_watch` の結果（`Result`）を引数で受け取る形にした。final review の warn の扱いは `pick_reviewer` の中に残り、設計 review は `?` の後の `Ok` だけを渡す。こうすると `ReviewerSink` と `ReviewerRun` が持つ `Arc` は常に 1 つで、別々に作る経路が無い。`Dispatcher` の method は増やしていない（458 のまま）。
- 付記に無い変更: `token_budget::exhausted` も `run_watch` を使う形にした（同じ組み立ての重複を消すため。挙動は同じ）。
- C3 の共通の関数は `review::run_within_token_budget` という名前にした。付記は名前を決めていない。`ReviewerRun` を参照で受け取る（`run_id`・`limits`・`sink`・`token_budget` を 1 か所から読む）。
- C4 の「予算が尽きた後の起こし直しは adapter を起こさない」は、`start_design_review` が起こし直しのたびに `run_watch` を読み直すことで成り立つ。止めた run の usage は `run_index_finish` で runs 索引に入るので、次の watch の前置きに加わる。

## C2〜C4 の証拠（提出時の HEAD で実行。check-health の基準は `git merge-base HEAD main`、範囲 check は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。先頭の check-health は `verdict=pass findings=717 new=0 worsened=0 existing=717`（base の 718 から 1 件減った。`review::run_reviewer_inner` の関数の行が 221 → 197 で閾値 200 を下回った）。nextest は 3584 passed・11 skipped。
  - `size.responsibility-growth` の値: `crates/task-dispatch/src/dispatcher` の module の本体の行 35,169 → 35,163、`dispatcher::Dispatcher` の method 数 458（同じ）、`Dispatcher::dispatch_one` 1,114 → 1,111、`Dispatcher::spawn_review` 759（同じ）、`Dispatcher::pick_reviewer` 224 → 214。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `cargo nextest list -p task-dispatch -E 'test(/health_review_design_budget_/)' --message-format oneline | grep -c health_review_design_budget_` → 8。
- `cargo nextest run -p task-dispatch -E 'test(/health_review_design_budget_/)'` → 8 passed。
- 範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）→ 変更した path は `crates/task-dispatch/src/` の 6 file とこの進捗 file だけ。

## 未解決
- C1 の未解決（上の節。`TreeLimitKind::is_run_time` の doc comment と ADR-0079 本文の旧い名前）はそのまま。
- 予算の尽きた task では、LLM を呼ばない設計 review の run の行が最大 `max_reviewer_retries + 1` 件増える（付記 C4 が書く代償のとおり）。

## 提案
- なし。
