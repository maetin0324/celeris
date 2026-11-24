---
tasks: [01M7XQA74FP70G3T3KSTP02S0W]
wu: eval-input
status: done
completed: 2026-11-24
---
# 戦略層 v1 子 8 eval-input: 評価器の request・facts に承認済み Roadmap の Outcome・子の criteria・D6 の到達を入れる

## 結論
- 子 6 の途中の親 Evaluator の入力の欠け（人 rmaeda の 2026-11-24 の判断。子 6 の進捗の「提案」1）を、owner の `crates/task-dispatch/src/review/parent_evaluation.rs` の `request()`・`facts()` で直した。
  - `request()` の `goal.outcomes` は D6 と同じ Outcome（承認済み Roadmap があればその生きている Outcome）の id・title・acceptance・reached で埋まる。`reached` は `unverified_outcomes(snapshot, recorded_outcomes(..))` に入らないこと（記録があり `root_satisfies` を満たす root がある）。
  - 子の `criteria` は受け入れ条件と今の試行の review の判定（`pass`・`fail`・`unknown`）で埋まる。組み立ては snapshot の root の行と同じ関数 `task_ops::project_snapshot::acceptance_criteria`（`root_row` から切り出した）。
  - `facts()` の `goal_reached` は同じ D6 の値（見る Outcome が 1 件以上あり、すべて到達のときだけ真）。
  - 型は `task_core::project_proposal::SnapshotCriterion` の 1 つにし、wire の旧い型と schema の定義を消した（`docs/protocol/worker-protocol.schema.json` は `UPDATE_SCHEMA=1` で再生成、`worker-protocol.md` の記述を直した）。
- 真になる条件（見る Outcome の集合、Roadmap の無い案件の扱い）は ADR `agent-docs/adr/2026-11-22-parent-checkpoint-evaluator.md` の付記（2026-11-24、WU eval-input）で決めた。D3.1・R4 の「未実装」は付記で置き換えた（決定の本文は書き換えていない）。
- 評価器の prompt の文面（`crates/task-worker/src/change_rules/`）は変えていない（差分 0）。新しい表・event・状態機械・API は足していない。controller の mode の既定は変えていない。

## 実装前 brief
1. 根本原因: `request()` が `goal.outcomes` と子の `criteria` を `Vec::new()` で渡し、`facts()` が `goal_reached: false` を固定していた。症状は評価器の prompt の「（Outcome の記録は無い）」と、D4 R4 で有効な `axes.goal` が `met` にならないこと。
2. 責務 owner: `task_dispatch::review::parent_evaluation` の `request`・`facts`（入力と事実の組み立て）。到達の規則の owner は `task_ops::project_controller_view::recorded_outcomes` と `task_core::project_proposal::{root_satisfies, unverified_outcomes}`、criteria の組み立ての owner は `task_ops::project_snapshot`。
3. 既存の再利用先: `project_snapshot::build`（snapshot）、`recorded_outcomes`・`unverified_outcomes`（D6・I1）、`root_row` の criteria の組み立て、controller の画面の `summary`（試験で D6 の比較先に使う）。
4. 選択肢: (a) 評価器の側で Roadmap と events を読み直して写す → 二重実装なので不採用。(b) `summary` の snapshot の組み立てを `reach` に切り出し、`summary` と評価器が同じ関数を読む → 採用。到達は I1 と同じ `unverified_outcomes` で決める。Roadmap の無い案件を常に空にする案は controller の判定と食い違うので不採用（ADR の付記 1）。
5. 新しい状態・分岐・fallback・port: 無し。新しい pub 関数は `project_snapshot::acceptance_criteria` と `project_controller_view::reach` の 2 つ（既存の組み立ての切り出し）。`reach` の `OpsError` は同じ file の記録の書き込みと `dispatcher/project_resume.rs` と同じく `StoreError::Invalid` に写す（`dispatcher.rs` は WU の範囲の外なので変えない）。
6. 撤去対象: wire の旧い criteria の型（`protocol/parent_checkpoint.rs`）、その re-export（`protocol.rs`）、schema の定義、`worker-protocol.md` の旧い記述（`not_run`）、`request()` の `Vec::new()` 2 か所と `facts()` の `false` 固定、その doc の「未実装」。
7. 不変条件と試験: 下の「守る不変条件」。

## 守る不変条件と、既存の試験で検出できない理由
- `request` の `goal.outcomes` は承認済み Roadmap の Outcome（id・title・acceptance）で、`reached` は controller の画面（`summary`）の D6 の値と同じ。
- 子の `criteria` は子の受け入れ条件と今の試行の判定で、snapshot の root の行と同じ組み立て。
- `facts` の `goal_reached` は D6（見る Outcome がすべて到達のときだけ真、0 件は偽）。
- Outcome のある案件で、評価器の prompt に「（Outcome の記録は無い）」が出ない。
- 既存の試験で検出できない理由: `project_controller_checkpoint_*`（dispatcher）は Roadmap の無い案件で起動と決着の経路だけを見る。task-api の `parent_checkpoint_live_*`・`parent_checkpoint_blind_*` は request の欄を試験の中で組み立てる。どれも `request` が欄を空で渡し、`facts` が `goal_reached` を偽に固定しても通る。

## 試験（`crates/task-dispatch/src/review/parent_evaluation/tests.rs`）
- `parent_evaluation_input_outcomes_criteria_and_goal_reached_follow_d6`: 承認済み Roadmap（o1 到達・o2 未達）・strategy root・受け入れ条件つきの子 2 件を store に置く。`request()` の `goal.outcomes` が 2 件で reached が `[true, false]` と `summary` の値に一致、子 A の criteria が `[reviewer] pass`・`[command] fail`、子 B が `[human] unknown`、`facts().goal_reached` が偽、`parent_checkpoint_section` の prompt に「（Outcome の記録は無い）」が無く `- outcome \`o1\`: o1 を仕上げる（reached=true）` がある。o2 も到達させると reached がすべて真で `goal_reached` が真。
- `parent_evaluation_input_no_outcomes_is_not_reached`: Roadmap も Outcome block も無い案件で `outcomes` は空、`goal_reached` は偽、子の criteria は入る。

### 変更前の code で落ちた出力
変更前（基準 `$CELERIS_WU_BASE`）の code に試験の file と `mod tests;` だけを足して実行した（`cargo nextest run -p task-dispatch parent_evaluation_input_`、exit 100）。行番号は `cargo fmt` の前の file のもの。
```
thread 'review::parent_evaluation::tests::parent_evaluation_input_outcomes_criteria_and_goal_reached_follow_d6' panicked at crates/task-dispatch/src/review/parent_evaluation/tests.rs:305:5:
assertion `left == right` failed
  left: []
 right: [("o1", "o1 を仕上げる", ["o1 の検査が通る"], true), ("o2", "o2 を仕上げる", ["o2 の検査が通る"], false)]

thread 'review::parent_evaluation::tests::parent_evaluation_input_no_outcomes_is_not_reached' panicked at crates/task-dispatch/src/review/parent_evaluation/tests.rs:375:5:
assertion `left == right` failed
  left: 0
 right: 1

Summary [   1.312s] 2 tests run: 0 passed, 2 failed, 1975 skipped
```
変更後: `cargo nextest run -p task-dispatch parent_evaluation_input_` → 2 tests run: 2 passed。

## 検査（提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）
- `bash scripts/dev/test-affected.sh` → exit 0。対象は celeris・celeris-campaign・celeris-mcp・celerisctl・llm-proxy・task-api・task-dispatch・task-ops・task-worker と web（typecheck+unit）。nextest 7138 passed・32 skipped、web の vitest 108 files・773 tests passed、repo_health_gate: all checks passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- `UPDATE_SCHEMA=1 cargo nextest run -p task-worker committed_schema_matches_generated` で schema を再生成し、再生成なしでも test-affected の中で一致（pass）。
- `git grep -n ParentCheckpointCriterion -- crates/ docs/protocol/` → 0 件。
- `git diff --stat "$CELERIS_WU_BASE" -- crates/task-worker/src/change_rules/` → 差分 0。

## LoC（`git diff --numstat "$CELERIS_WU_BASE" HEAD`）
- production Rust: +143・−65（純増 +78）。内訳: `review/parent_evaluation.rs` +54・−7、`project_controller_view.rs` +33・−19、`project_snapshot.rs` +50・−25、`protocol.rs` +2・−3、`protocol/parent_checkpoint.rs` +4・−11。
- 試験: `review/parent_evaluation/tests.rs` +415（新規）、`protocol/parent_checkpoint/tests.rs` +2・−2。
- 文書・生成物: ADR +42、`worker-protocol.md` +2・−1、`worker-protocol.schema.json` +25・−23（生成）。
- 純増の理由: 欠けていた入力（Outcome の写し・criteria・goal_reached）を組み立てる `goal()` と、既存の組み立てを 2 か所から呼ぶための切り出し（`acceptance_criteria`・`review_verdicts`・`reach`）の doc。新しい Manager・trait・状態機械・表は無い。

## 未確認
- 本番の案件（agent-platform）の承認済み Roadmap と Outcome の到達の状態は、本番の DB を読んで確かめていない。
- `request` は評価の run を起こすたびに、`facts` は決着のたびに snapshot を 1 回作る（`summary` と同じ費用）。本番の件数での時間は測っていない。
- 子の `criteria` は今の store の値で、checkpoint の時点で切らない（`status`・`reviewer_passed` と同じ。ADR の付記 4）。replay の時点の再現は WU eval-seed の seed の側で扱う。
- E-mid・D-done の盲検の入力（fixture `crates/task-api/tests/fixtures/parent_checkpoint_live/`）の書き直しは WU eval-seed の範囲で、この WU は変えていない。

## 親の目的への寄与
- 改善 C・D（途中の親の独立評価）: 評価器が親 Outcome と子の受け入れ条件の結果を入力に持つようになった（確認済み: 上の試験）。評価の質が上がるかは子 8 の盲検の評価と人の rubric で測る（未確認）。

## 再実行（attempt 2）
- 前回の run の後の範囲の check が `crates/task-dispatch/src/dispatcher.rs`（`ops_to_store` を `pub(crate)` にした 1 行）を範囲の外として落とした。`dispatcher.rs` を基準に戻し、`goal()` の `reach` のエラーは `StoreError::Invalid(e.to_string())` に写した（同じ file の既存の写し方）。範囲の check は exit 0。

## commit
- strategy-v1 8 eval-input: 評価器の request・facts に承認済み Roadmap の Outcome・子の criteria・D6 の到達を入れる
- strategy-v1 8 eval-input: WU の進捗（変更前に落ちた出力・不変条件・検査の結果）
- strategy-v1 8 eval-input: 範囲の外の dispatcher.rs を戻し、reach のエラーを review の中で写す
