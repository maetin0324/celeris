---
title: 戦略層 v1 子 6 controller-checkpoint — Project Controller の evaluate（snapshot の信号・Rust の検証・reviewer run の効果・結果の写し）
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: controller-checkpoint
status: running
updated: 2026-11-23
---
# controller-checkpoint（WorkUnit）

状態: 途中（run 1 で前半を実装し、yield した）。設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md) の D1〜D6・D9・D11。
基準はこの WU の `$CELERIS_WU_BASE`。検査は run 1 の終わりの HEAD で実行した。

## 実装前 brief

1. 根本原因: 子の done（局所の review）と親 Outcome の到達の間に、途中で親の目的への効き目を判定する口が controller に無い。
2. 責務 owner: 発火・状態の純関数は `task_core::parent_checkpoint`、行動の形と検証は `task_core::project_proposal`、snapshot の組み立ては `task_ops::project_snapshot`、効果は `project_decider_ts`・`project_wake`、評価の run は `design_gate.rs` の共通の起動口。
3. 既存の再利用先: `parent_checkpoint::triggers`・`allowed_actions`・`check_evaluation`（contract の WU）、`strategy_reads::strategy_task_creates`、`token_budget::tree_tokens_used`（呼ぶだけ）、`start_side_review`・`holds_side_review`（review-launcher の WU）、既存の効果の門（shadow は `suppressed`）と `runaway_guard`。
4. 選択肢: 設定を decider に渡す口は (a) `attach` の引数を足す（celeris の試験 2 file が呼ぶので不可）、(b) `DeciderLimits` に持たせる（bootstrap が budget の節だけから作るので設定が届かない）、(c) `ProjectDecider::configure`（新しい既定の method）を `set_project_controller` で 1 回呼ぶ。(c) を採った。木の token の読み口は `tree_tokens_used` が `pub(super)` で decider（crate 直下）から呼べないので、`configure` に fn pointer（`TreeTokens`）で渡す（写しを作らない）。
5. 新しい状態・分岐・fallback・port: 新しい状態は無し（状態は判断の行と成果物の name から毎回導く）。分岐は `ProjectAction::Evaluate`・`ProjectEffect::RequestEvaluation`。`configure` を通らない判定（controller の runtime 外の試験・pilot）は snapshot に `checkpoints` を入れない（strategy root を扱わない）。
6. 撤去対象: `ParentEvaluationRecord.triggers`（D3.4。trigger の正本は判断の行）。
7. 不変条件: strategy root の無い snapshot の形が base と同じ、suppressed の後に同じ trigger が due にならない、試行は試行の記録の name だけで数える（final review の run を数えない）、store・migration・events の差分 0。

## 実装前の失敗（試験を先に置いた）

- `cargo nextest run -p task-core parent_checkpoint_contract_`（実装前）: exit 101。compile error 15 件（`checkpoint_states`・`EvaluateDecision`・`DecisionEffect`・`attempt_artifact_name`・`attempt_of` が無い、`ParentEvaluationRecord::new` の引数が 5 個）。
- 実装後の最初の実行で `parent_checkpoint_contract_attempt_name_roundtrip` が 1 件 fail: `evaluation_decision_id("parent-evaluation/D1.attempt-2.json")` が `Some("D1.attempt-2")` を返し、試行の記録を最終の記録と読んでいた。`evaluation_decision_id` を直した。
- `project_controller_checkpoint_` の task-core の 3 件（検証・act の表・snapshot の形）は実装と同じ run で書き、実装前の実行はしていない（未確認）。

## run 1 で済んだこと

- `task_core::parent_checkpoint`: `checkpoint_states`（判断の行と成果物の name だけから、発火済み・評価済み・実行中・試行の数を導く純関数）、`DecisionEffect`・`EvaluateDecision`・`CheckpointStates::fill`、試行の記録の name（`attempt_artifact_name`・`attempt_of`）、`ParentEvaluationRecord.triggers` の削除。
- `task_core::project_proposal`: `ProjectAction::Evaluate`、snapshot の `checkpoints`（空なら直列化しない）、検証 `checkpoint_root_unknown`・`checkpoint_not_due`・`launch_on_paused`・`checkpoint_evaluation_open`・`checkpoint_action_mismatch`。
- `task_ops::project_snapshot::checkpoints`: strategy root ごとに due・実行中・写していない評価と許す行動を作る。
- `project_decider_ts`: `configure`・`checkpoint_section`（判断を最後まで読む）、`runaway_guard` が `evaluate` を数える、`Evaluate` → `RequestEvaluation`。
- `project_wake`: `RequestEvaluation` は門を通った後に何も書かず `applied`（`effect_ref` = root）。shadow は既存の門で `suppressed`。
- `[project_controller.checkpoint]`（3 欄、`deny_unknown_fields`、範囲外は設定エラー）。
- `docs/ops/project-controller.md` に節「親の途中評価の checkpoint」（設定・状態の導き方・本番で有効にする人の手順）を足した。手順の `PUT …/controller` の確認は run 2 の実装の後に見直す。
- `crates/celeris/tests/project_controller_e2e.rs` の `ProjectAction` の網羅の match に `Evaluate` の腕を 1 つ足した（variant を足すと compile が落ちるため。write set の表の外）。

## 検査（run 1 の終わりの HEAD）

- `cargo nextest run -p task-core -p task-ops -p task-dispatch -p celeris -E 'test(/project_|parent_checkpoint|checkpoint/)'`: exit 0、447 件 pass。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/test-affected.sh`: 未実行（run 2 で実行する）。

## 残り（run 2 以降）

- `dispatcher/parent_evaluation.rs`（新）: 実行中の判断ごとに `has_task_run(root)` が偽・`tree_limit_hold_for` が偽・試行 < 上限のときだけ `start_side_review(root, parent_checkpoint の節, SideReviewKind::ParentEvaluation)`、run id が返れば試行の記録を書いて `ArtifactProduced`。上限で run が無ければ `invalid` の最終の記録。
- `design_gate.rs`: `SideReviewKind::ParentEvaluation { decision_id }`・終わりの腕・`holds_side_review` の評価の半分。
- `review/parent_evaluation.rs`（新）: `review.json` を読み `check_evaluation`、最終の記録を書く。
- `dispatcher/tests/project_checkpoint.rs`（新）: `project_controller_checkpoint_` の dispatcher の試験（発火・効果 1 回の冪等・crash の境界・strategy の無い root と無効な案件で reviewer run 0・no-progress の escalate・suppressed の後の due・final review の run を試行に数えない・root に手元の run がある間は起こさない）。
- `bash scripts/dev/test-affected.sh`、範囲 check、run_index_start を持つ file の数の確認。

## 未解決事項

- D5.2 の `checkpoint_action_mismatch` は「写していない評価があるとき、rule が act でない proposal は invalid」と書くが、D5.1 は evaluate を act より先に置く。両立させるため、`evaluate` の proposal は写していない評価があっても通す実装にした（ts-checkpoint-rule の WU と同じ順）。
- trigger の `at` は RFC 3339 の時刻ではなく大域の event id の 20 桁の文字列（store に task ごとの時刻つきの読み口が無く、store は変えない）。順にだけ使う。

## 提案

- 無し（run 2 の後に書く）。
