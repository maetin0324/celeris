---
title: 戦略層 v1 子 6 controller-launch — 親の途中評価の reviewer run の起動・決着と task-dispatch の試験
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: controller-launch
status: running
updated: 2026-11-23
---
# controller-launch（WorkUnit）

状態: 途中（run 1 で起動・決着の実装を置き、yield した）。試験 `project_controller_checkpoint_`（task-dispatch）・ADR の置き場の直し・
`docs/ops/project-controller.md` の見直しは未着手。設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D2.3・D3・D11。
基準はこの WU の `$CELERIS_WU_BASE`。検査は run 1 の終わりの HEAD で実行した。

## 実装前 brief

1. 根本原因: controller-checkpoint の run 2 が done と読まれ、`RequestEvaluation` の効果（何も書かず `applied`）の後に評価の reviewer run を起こす段と、終わりの決着が無い。
2. 責務 owner: run の起動・実行・終わりの記録は `health_review/design_gate.rs` の共通の起動口。起動の条件・試行の記録・決着は `crate::review::parent_evaluation`（新）。tick の呼び出しは `dispatcher/project_wake.rs` の `tick_project_controller`。
3. 既存の再利用先: `start_side_review`・`on_side_review_finished`・`holds_side_review`、`supervisor.has_task_run`、`tree_limit_hold_for`、`parent_checkpoint::{checkpoint_states, check_evaluation, ParentEvaluationRecord, attempt_artifact_name, attempt_of, evaluation_decision_id}`、`strategy_reads::strategy_task_creates`、`project_decider_ts::decision_proposal`、`token_budget::tree_tokens_used`（`TreeTokens` で渡す）。
4. 選択肢: (a) `Dispatcher` に method を足す（method の数が閾値の上で worsened になるので不可）、(b) dispatcher の子の新しい file（dispatcher module の本体の行が増えるので不可）、(c) `crate::review` の自由関数が `&mut Dispatcher` を受け、起動口・`has_task_run`・`tree_limit_hold_for` を呼ぶ。(c) を採った。`review` → `dispatcher` の組は既にある（`review/required.rs`）ので deps.cycle に新しい組は無い。
5. 新しい状態・分岐・fallback・port: 新しい状態は無し（実行中・試行の数は判断の行と成果物の name から毎回導く）。分岐は `SideReviewKind::ParentEvaluation(Subject)`（判断の id と起動の時点の試行の上限）。可視性の変更: `design_gate` の mod・`SideReviewKind`・`start_side_review`・`Dispatcher.supervisor`・`tree_limit_hold_for`・`decision_proposal` を `pub(crate)` にした（行は足さない）。
6. 撤去対象: 無し（新しい経路の追加だけ）。行数の相殺のため `decide_exhausted_notice` を `dispatcher/project_wake.rs` から `crate::project_controller` に移した（中身は同じ。controller-checkpoint の run 2 が `wakes_for_event` を移したのと同じ理由）。
7. 不変条件: strategy の無い root・`enabled` でない案件では評価の run を起こさない、root に手元の run があれば起こさない、試行は試行の記録の name だけで数える、上限で run が無ければ `invalid` の最終の記録、store・migration・events の差分 0。試験は未着手（下の「残り」）。

## run 1 で済んだこと

- `crates/task-dispatch/src/review/parent_evaluation.rs`（新）:
  - `launch(d, settings, tokens)`: `project_controller_active` のうち mode `enabled` の案件ごとに判断の行を読み、root ごとに `checkpoint_states` で実行中の判断と試行の数を導く。判断ごとに `has_task_run(root)` が真なら止め、試行 ≥ 上限なら `invalid` の最終の記録、`tree_limit_hold_for` が真なら止め、strategy root でなければ起こさず、`start_side_review` が run id を返したら試行の記録 `parent-evaluation/<id>.attempt-<n>.json` を書いて `ArtifactProduced`（kind `parent_evaluation_attempt`、`declared: false`）。
  - `settle`: run が `done` なら root の成果物の dir の `review.json` の `parent_evaluation` を読み、`check_evaluation` に通す。`checked`、または `invalid` で試行が上限に達したときだけ最終の記録 `parent-evaluation/<id>.json`（kind `parent_evaluation`）を書く。既にあれば何もしない。
  - `in_flight(events, run_id)`: 最終の記録の無い判断の最後の試行の run で、`WorkerFinished` が無ければ真。
  - 入力の節 `ParentCheckpointRequest`: strategy・直下の子（status・`ReviewerPassed` の有無）・metrics（終端の子の数・木の token・root の token 予算）・前の最終の記録・固定の rubric、`goal.request_sha256` は判断の行の `request_sha256`。
- `health_review/design_gate.rs`: `SideReviewKind::ParentEvaluation(Subject)`、label、終わりの腕（`pe::settle`）、`holds_side_review` の `|| pe::in_flight(...)`。
- `dispatcher/project_wake.rs`: `tick_project_controller` の判断の後に `parent_evaluation::launch(self, &rt.settings.checkpoint, tree_tokens_used)` の 1 行（controller の設定が無効なら呼ばない）。

## 検査（run 1 の終わりの HEAD）

- `cargo build -p task-dispatch`: exit 0。
- `cargo clippy -p task-dispatch -- -D warnings`: exit 0（`--workspace` は未実行）。
- `python3 scripts/dev/repo-health/repo_health.py --repo . --base $CELERIS_WU_BASE --head worktree --policy-version 1 --rules size.responsibility-growth,deps.cycle --out $TMPDIR/rh.json`: exit 0。`size.responsibility-growth`・`deps.cycle` とも pass。dispatcher module の本体の行 32218 → 32200（-18）。最初の版は 32228（+10）で worsened になり、variant を 1 行の tuple にし、`decide_exhausted_notice` を移して直した。
- `run_index_start` を持つ production の file の数: base 8、HEAD 8（`git grep -l run_index_start <rev> -- 'crates/*.rs' | grep -v test | wc -l`）。
- `bash scripts/dev/test-affected.sh`・`cargo fmt --all -- --check`・試験: 未実行。

## 残り（run 2 以降）

1. `crates/task-dispatch/src/dispatcher/tests/project_checkpoint.rs`（新）と `tests/mod.rs` の `mod` の行: `project_controller_checkpoint_` の試験（発火・効果 1 回の冪等・crash の境界・strategy の無い root と無効な案件・shadow で reviewer run 0・写していない評価がある間の no-progress の escalate・suppressed の後の due が open にならない・root の final review の run を試行に数えない・root に手元の run がある間は起こさない・試行の上限で invalid）。proposal は decider の差し替え（`dispatcher/tests/project_wake.rs` の `DecideFn`）で与える。実装は run 1 で先に置いたので、実装前の失敗は `parent_evaluation::launch` の呼び出しの 1 行と終わりの腕を外した状態で試験を走らせて記録する（実装前に試験を置けなかったことも書く）。
2. `request` の未実装の欄: `goal.outcomes`（承認済み Roadmap の Outcome と D6 の `reached`）、子の `criteria`。`facts` の `goal_reached` は今 false 固定、commit の参照は実在を確かめない。D6 の値の読み口（`task_ops::project_controller_view::recorded_outcomes` は snapshot を要る）を決めて入れる。
3. ADR の D2.3・D11 の置き場の記述を本文で直す（起動の条件の置き場を `crates/task-dispatch/src/review/parent_evaluation.rs` の自由関数、呼び出しを `dispatcher/project_wake.rs` の `tick_project_controller` の 1 行にする。D11 の controller-checkpoint の行から dispatcher の新しい file と `dispatcher.rs` の tick の行を消す）。末尾に付記を足さない。
4. `docs/ops/project-controller.md` の「親の途中評価の checkpoint」の手順を見直す（評価の run が起きる条件と、試行の記録・最終の記録の name）。
5. `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check`、範囲 check（`sh "$CELERIS_WU_SCOPE_PATHS"`）。範囲 check で `crates/task-dispatch/src/project_controller.rs`・`tree_units.rs`・`project_decider_ts.rs`・`dispatcher.rs` が範囲外と出たら、行数の相殺と可視性の変更の理由を書いて replan を申告する。
6. この進捗と `controller-checkpoint.md` の status を実態に合わせる。

## 未解決事項

- 試行の上限で run が無いときの `invalid` の最終の記録は `ArtifactProduced.run_id` が空文字になる（最後の試行の run id を入れるかは未決）。
- 範囲 check は未実行。`project_controller.rs`（`decide_exhausted_notice` の移動）・`tree_units.rs`・`project_decider_ts.rs`・`dispatcher.rs`（可視性だけ）が WU の write set に入るかは未確認。

## 提案

- 無し（run 2 の後に書く）。
