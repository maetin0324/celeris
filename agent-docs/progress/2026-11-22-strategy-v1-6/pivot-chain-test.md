---
title: 戦略層 v1 子 6 pivot-chain-test — D-done の pivot から新しい戦略の版と子 5 の gate までを 1 つの試験でつなぐ
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: pivot-chain-test
status: running
updated: 2026-11-23
---
# pivot-chain-test（WorkUnit）

状態: 途中（run 1 は区切りで止めた。試験は書いたが、段の間が切れている所を見つけ、まだ直していない）。基準はこの WU の `$CELERIS_WU_BASE`。

## 試験

`crates/task-api/tests/parent_checkpoint_pivot_chain.rs` の `parent_checkpoint_pivot_chain_d_done_pivot_to_gated_v2`（1 件）。つなぐ段:

1. 第二ゲートの shadow の slice（`tests/fixtures/parent_checkpoint_shadow/`、読むだけ）の D-done までの子 A〜H の遷移を一時 DB に流す。
   本物の `task_ops::project_snapshot::{build, checkpoints}` が `children:4` を出し、`project_proposal::parse_and_validate` を通った `evaluate` の後に、
   評価の記録（slice のスクリプトの D-done: off_track・pivot）を root に書く。次の wake で `replan{checkpoint_pivot}` に写す。
2. CoS の skill（`config/skills/cos-operator/strategy-review.md` の「checkpoint の replan を受けたとき」）の決まった操作を CoS の run の bearer で流す。
   replan の参照から `GET /api/v1/tasks/<root>/artifacts` で記録を読む。v2 の文書の task を `/cos/operations` の `task.create`（parent = root）で起票する。
   その run が `celeris.strategy/1` の v2 を書く（反証された仮説の候補 `fixed-plan-a-to-h` を rejected にし、記録の成果物と評価が引いた D の task を反証の証拠に引く）。
3. 人が承認する前の v2 の起票と、CoS が approve と答えた後の v2 の起票は、どちらも `strategy_unapproved` で断られる。人が承認した後は、子 5 の gate が v2 の識別実験（atomic・`cut-boundary-edges`）を通す。
   v1（反証された版）へ戻る起票は `strategy_stale` で、v2 で却下した候補の起票は `strategy_revisit_rejected` で断られる。Project.request と Roadmap の全版は鎖の前後で同じ。

## 変更前に落ちた試験（段の間が切れている所）

評価の記録の path の形が、書く側と読む側で食い違っている。

- dispatcher の `crates/task-dispatch/src/review/parent_evaluation.rs` の `write_record` は、`ArtifactRef.path` に**絶対 path** を書く。
  ところが worker の成果物（`task_worker::artifact`）、API（`task_api::files::resolve_artifact`）、子 5 の gate（`cos::strategy_gate` の `Reader::file`）は、
  path を workspace 相対として扱う。
- 試験が dispatcher と同じ形（絶対 path）で記録を書くと、次の行で落ちる。`GET /api/v1/tasks/<root>/artifacts` がこの記録を `forbidden: true` と返す。
  `assertion left == right failed: 記録の path を API が引ける … "forbidden":true`。
  このため CoS は skill の手順 1 で記録を読めない。v2 の証拠の `artifact` 参照も、同じ `resolve_artifact` を通るので gate から引けない。
- 試験が workspace 相対の path（成果物の契約）で記録を書くと、`task_ops::project_snapshot::unmapped_evaluation` が落ちる。
  この関数は `std::fs::read(&a.path)` で読むので記録を読めず、`status` が `invalid` になる。その結果、写しは pivot（replan）でなく escalate になる。
  `assertion left == right failed: snapshot が記録を読めた left: "invalid" right: "checked"`。
- `parent_checkpoint_shadow_` が通るのは、試験が記録を workspace の外の絶対 path に書き、snapshot がそれを直に読むから。API と gate は通していない。

## 未解決（次の run）

- 直す案: `write_record` は workspace 相対の path（`artifacts/parent-evaluation/<id>.json`。root が自分の workspace を持たないときは `artifacts_dir_for` からの相対）を書く。
  `project_snapshot::checkpoints` は root の workspace で解いて読む（task-ops の `workspace::local_dir` と roots が要る）。
  `ps::checkpoints` の呼び出し元は 4 か所: task-dispatch の `project_decider_ts.rs`、task-api の試験の `parent_checkpoint_shadow.rs`・`parent_checkpoint_regression.rs`・この試験。
  shadow の試験は同じ段の blind-eval-input も変えるので、統合での衝突に気をつける。
- 直した後の確認: この試験、子 2・5 の strategy の試験、`parent_checkpoint_shadow_` を回す。`bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` も回す。
- LoC（production と test）は、直した後に測る。
