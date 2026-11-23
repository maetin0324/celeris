---
title: 戦略層 v1 子 6 pivot-chain-test — D-done の pivot から新しい戦略の版と子 5 の gate までを 1 つの試験でつなぐ
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: pivot-chain-test
status: done
updated: 2026-11-23
---
# pivot-chain-test（WorkUnit）

完了日: 2026-11-23。基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## 範囲の訂正と再実行（2026-11-23）

- 最初の提出は、計画の範囲の check が exit 1 で落ちた。範囲の外の path は次の 7 件だった。
  `crates/task-dispatch/src/review/parent_evaluation.rs`・`crates/task-dispatch/src/project_controller.rs`・
  `crates/task-dispatch/src/dispatcher/project_decider_ts.rs`・`crates/task-dispatch/src/dispatcher/project_wake.rs`・
  `crates/task-dispatch/src/dispatcher/tests/project_checkpoint.rs`・`crates/task-api/tests/parent_checkpoint_shadow.rs`・
  `crates/task-api/tests/parent_checkpoint_regression.rs`。
- 原因: 鎖の切れ目の owner は、評価の記録の書き手（task-dispatch の `write_record`）と、snapshot に足回りの root を渡す
  `ProjectDecider::configure`（task-dispatch）だった。最初の計画の範囲（task-api の cos・task-ops の project_*・task-core の strategy）に入っていなかった。
  範囲の中だけで直すと、API の path の検査を弱めるか、試験が dispatcher と違う形で記録を書くことになる。どちらも回避策なので採らず、plan_issue で replan を求めた。
- replan（計画 v6、人が承認）は同じ key で再実行し、範囲に上の 7 件を足した。実装は branch の commit をそのまま使い、再実行で直した code は無い。
- 再実行の範囲の check（7 件を足した版）は、提出時の HEAD で exit 0。
- 同じ段の blind-eval-input も `review/parent_evaluation.rs` を変える。ただし変える hunk は 285 行目付近までで、`write_record`（419 行目付近）とは重ならない。

## 結論

- `crates/task-api/tests/parent_checkpoint_pivot_chain.rs` の `parent_checkpoint_pivot_chain_d_done_pivot_to_gated_v2`（1 件）が、
  D-done の評価から子 5 の gate の通過までを 1 つの試験でつなぎ、pass する。
- 試験は、段の間が 1 か所切れているのを見つけた。評価の記録の path の形が、書く側（dispatcher）と読む側（API・gate・snapshot）で食い違っていた。
  書く側と snapshot の読み手を直した（下の「直した所」）。
- 確認済み: 評価の記録 → replan の参照 → CoS が API で記録を読む → v2 → 人の承認 → gate、の受け渡しが本物の snapshot・Rust の検証・CoS operations の API・gate で通る。
- 未確認: 評価器は slice のスクリプトの D-done の評価で、LLM ではない（独立の評価器の質は blind-eval-runs と close-gate が扱う）。
  CoS の手順は skill の決まった操作を試験が流したもので、CoS の LLM が同じ操作を選ぶかは確かめていない。

## 試験がつないだ段

1. 第二ゲートの shadow の slice（`tests/fixtures/parent_checkpoint_shadow/`、読むだけで変えていない）の D-done までの子 A〜H の遷移を一時 DB に流す。
   本物の `task_ops::project_snapshot::{build, checkpoints}` が `children:4` を出し、`project_proposal::parse_and_validate` が `evaluate` を通す。
2. 評価の記録（slice のスクリプトの D-done: off_track・pivot、`negative_result: true`）を、dispatcher の `write_record` と同じ置き場と形（root の足回りからの相対 path）で root に書く。
   次の wake で snapshot が記録を `checked` と読み、`replan{checkpoint_pivot}`（`context_refs` に `task:<root>`・`artifact:<root>/parent-evaluation/dec-001.json`・戦略の文書）に写す。写しは task を作らない。
3. CoS の LLM の代わりに、skill（`config/skills/cos-operator/strategy-review.md` の「checkpoint の replan を受けたとき」）の決まった操作を CoS の run の bearer で流す。
   - replan の参照から `GET /api/v1/tasks/<root>/artifacts` と `/artifacts/<idx>` で記録を読む（一覧の `forbidden` が false、本文の sha256 が一覧と一致）。
   - v2 の文書の task を `/cos/operations` の `task.create`（`parent` = root、`idempotency_key` = `checkpoint-<判断 id>-arch-v4-cycle-v2`）で起票する。
   - その run が `celeris.strategy/1` の v2 を書く。v2 は `supersedes` に v1 を指し、`change_reason` に判断 id・verdict・next と「実行は成功・知見は前進・当初の目標は未達」を書く。
     記録の `evaluation.refuted` のうち task の参照を持つ項目で、反証された仮説 `h-fixed-plan` の候補 `fixed-plan-a-to-h` を `rejected` にする。
     その反証の証拠は、評価の記録の成果物（`artifact`・sha256）と評価が引いた D の task を参照する。selected は書かない（残る 2 候補の識別実験の版）。
4. 子 5 の gate:
   - 人の答えが無い v2 の起票、CoS（`cos`）が approve と答えた v2 の起票は、どちらも `strategy_unapproved` の 422 で断られ、task は増えない。
   - 同じ決定に人（`human:shadow`）が答えた後、v2 の識別実験（atomic、`cut-boundary-edges`、`max_wall_secs` 3600）の起票が通る。replan の後に増えた task は v2 の文書の task と実験の 2 件だけ。
   - 反証された版 v1（固定計画）へ戻る起票は `strategy_stale`、v2 で却下した候補 `fixed-plan-a-to-h` の起票は `strategy_revisit_rejected` で断られ、task は増えない。
5. Project.request と Roadmap の全版（`roadmap_versions` の JSON）が鎖の前後で同じで、承認済みの版は 1 のまま。

## 変更前に落ちた試験（run 1 で確かめた）

- dispatcher の `crates/task-dispatch/src/review/parent_evaluation.rs` の `write_record` は、`ArtifactRef.path` に**絶対 path** を書いていた。
  worker の成果物（`task_worker::artifact`）、API（`task_api::files::resolve_artifact`）、子 5 の gate（`cos::strategy_gate` の `Reader::file`）は、path を足回りからの相対として扱う。
- 試験が dispatcher と同じ形（絶対 path）で記録を書くと、`GET /api/v1/tasks/<root>/artifacts` が記録を `forbidden: true` と返して落ちた。
  `assertion left == right failed: 記録の path を API が引ける … "forbidden":true`。CoS は skill の手順 1 で記録を読めず、v2 の証拠の参照も gate から引けない。
- 試験が相対 path で書くと、`task_ops::project_snapshot::unmapped_evaluation` が `std::fs::read(&a.path)` で読めず、`status` が `invalid` になって落ちた（写しは pivot でなく escalate になる）。
  `assertion left == right failed: snapshot が記録を読めた left: "invalid" right: "checked"`。
- 同じ読み方（`std::fs::read(&artifact.path)`）で、snapshot は戦略の文書（worker の成果物で相対 path）も読めていなかった。推定: 本番の daemon では、反証の trigger は文書の bytes が無いまま判定していた（本番の挙動は未確認）。
- `parent_checkpoint_shadow_` は、記録を workspace の外の絶対 path に書いて snapshot が直に読むので、この食い違いでは落ちなかった（API と gate を通さない）。

## 直した所（実装前 brief の要点）

- 根本原因: 記録の書き手が成果物の path の契約（足回りからの相対）を破り、snapshot の読み手がそれに合わせて path を直に読んでいた。
- owner: 書き手は `review/parent_evaluation.rs` の `write_record`、読み手は `task_ops::project_snapshot::checkpoints`（architecture-map の task-ops の project_*）。
- 選択肢: (a) API の `resolve_artifact` に絶対 path を通させる案は、path の検査を弱めるので採らなかった。(b) 書き手は相対 path を書き、読み手は owner の足回りを解いて読む案を採った。
- 変更:
  - `write_record` は `artifacts_dir_for(root, dir)` の file を `dir` からの相対 path で記録する。
  - `project_snapshot::checkpoints` は引数 `workspace_roots: &WorkspaceRoots` を受ける。文書と評価の記録は、記録した task の足回り（`task_ops::workspace::local_dir`）を基準に読む（新しい helper `artifact_bytes`）。
  - `ProjectDecider::configure` は `workspace_roots` も受ける（dispatcher の `self.config.workspace_roots`）。`TsProjectDecider` がそれを `checkpoint_section` に渡す。
- 新しい状態・分岐・fallback・port: 無し（引数 1 つと読み口の helper 1 つ。新しい表・event の種類・trait・状態機械は無い）。
- 撤去: snapshot の `std::fs::read(&artifact.path)`（2 か所）と、記録の絶対 path。
- 不変条件と試験:
  - 記録の path は相対: `dispatcher/tests/project_checkpoint.rs` の `final_record` が `is_relative` を確かめ、root の足回りで読む。
  - snapshot は相対の記録を読む: task-ops の `project_controller_checkpoint_continue_maps_none_effect_and_returns_to_rules` の記録を相対 path に変えた。
  - 鎖の全体: この WU の chain の試験。
- 呼び出し元の更新: task-api の試験 `parent_checkpoint_shadow.rs`・`parent_checkpoint_regression.rs` の `ps::checkpoints` に 1 行（`workspace_roots`）を足した。
  shadow の試験の記録は workspace の外の絶対 path のままで、`Path::join` が絶対 path をそのまま返すので読める。
  shadow の試験は同じ段の blind-eval-input も変えるので、段の統合で衝突しうる（この WU の変更は呼び出しの 1 行だけ）。

## 証拠コマンドと結果（提出時の HEAD で実行）

- 範囲の check（task-dispatch の 5 file と task-api の 2 試験を足した版）: exit 0（範囲の外の path は 0 件）。
- `cargo nextest run -p task-api --test parent_checkpoint_pivot_chain`: 1 passed。
- `cargo nextest run -p task-ops -p task-dispatch -p task-api -p task-core -E 'test(/project_controller_checkpoint|parent_checkpoint|strategy|project_checkpoint/)'`: 136 passed。
  内訳（同じ filter の `cargo nextest list`）: `parent_checkpoint_pivot_chain_` 1 件、`parent_checkpoint_shadow_` 11 件、
  task-dispatch の `project_checkpoint` 11 件（記録の path が相対であることを `final_record` が確かめる）、
  task-ops の `project_controller_checkpoint_continue_maps_none_effect_and_returns_to_rules` 1 件、子 2・5 の `strategy` 79 件。
  その他は `parent_checkpoint_regression_`・`_live_` と task-core の `parent_checkpoint` の試験。
- `cargo nextest run -p task-ops -E 'test(/checkpoint/)'`: 7 passed。
- `bash scripts/dev/test-affected.sh`: exit 0。nextest は 7042 passed・30 skipped。web の tsc -b は通過し、vitest は 105 files・745 passed。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。

## LoC（`git diff --numstat $CELERIS_WU_BASE HEAD -- crates`）

- production: +79 / −24（純増 55）。`task-ops/src/project_snapshot.rs` +29 −7（test module を除く）、`task-dispatch/src/dispatcher/project_decider_ts.rs` +34 −12（rustfmt の折り返しを含む）、
  `task-dispatch/src/project_controller.rs` +10 −3、`task-dispatch/src/review/parent_evaluation.rs` +5 −1、`task-dispatch/src/dispatcher/project_wake.rs` +1 −1。
  純増の理由: 読み口が足回りを解くための引数と helper（成果物の path の契約に合わせる最小の変更）。
- test: +961 / −5。新しい試験 `parent_checkpoint_pivot_chain.rs` +940、`project_snapshot.rs` の test module +15 −4、`project_checkpoint.rs` +4 −1、shadow・regression の試験 各 +1。

## 回帰条件

- strategy の無い task・controller off・shadow では reviewer run を起こさない挙動は変えていない（`parent_checkpoint_regression_` と `parent_checkpoint_shadow_` が pass）。
- store・migration・events・`cos/operations.rs`・`task-ops/src/lifecycle.rs`・ADR・slice の fixture・`task_core::parent_checkpoint` は変えていない。

## 未解決・提案

- 本番の DB に絶対 path で書かれた評価の記録があれば、snapshot は `Path::join` で読めるが、API の一覧は `forbidden` のまま。
  評価の記録を書く release は本番に入っていないはずだが、未確認。入っていた場合の移行は close-gate で人に確かめる。
- CoS の LLM が skill の手順どおりに v2 を作るかは、実 LLM の run で確かめていない（子 8 の pilot の対象）。
