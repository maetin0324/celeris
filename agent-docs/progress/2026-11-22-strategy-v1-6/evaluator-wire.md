---
title: 戦略層 v1 子 6 evaluator-wire — reviewer run の wire（ReviewRequest.parent_checkpoint・ReviewOutput.parent_evaluation）と prompt の節
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: evaluator-wire
status: running
updated: 2026-11-22
---
# evaluator-wire（WorkUnit）

設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md)（D3.1・D3.2・D3.3・D9.2・D11）。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

**状態: 範囲の内側は実装と検査を終えた。範囲の外の 1 行（下の「範囲外の 1 行」）が要るので `plan_issue` で計画に申告した。**
`crates/task-dispatch/src/review.rs` が直るまで task-dispatch はコンパイルできず、`bash scripts/dev/test-affected.sh` と
`cargo clippy --workspace -- -D warnings` はまだ通らない（未実行ではなく、task-dispatch のコンパイルで落ちることを確かめた）。

## commit（件名）

1. `strategy-v1 6 evaluator-wire: parent_evaluation_absent_ の回帰の試験（base の prompt・request.json の sha256）` — base の状態で通る回帰の試験 3 件。
2. `strategy-v1 6 evaluator-wire: ReviewRequest.parent_checkpoint・ReviewOutput.parent_evaluation と prompt の節` — wire の試験・型・prompt・schema・md とこの file。

## 実装前 brief

1. 根本原因: reviewer の wire に親の途中評価の入力と出力の欄が無く、prompt にも節が無い。dispatcher（後続の controller-checkpoint）が評価の run を起こせない。
2. 責務 owner: wire は `task_worker::protocol`、prompt の節は `task_worker::change_rules::review`（Result 節を組む唯一の関数 `result_section` を含む）、
   prompt の組み立ての分岐は `claude_code::prompt::build_review_prompt`（codex・acp も同じ関数）。評価の形の正本は `task_core::parent_checkpoint::ParentEvaluation`。
3. 既存の再利用先: 子 5 の `strategy_critic` の欄・節・`fixed_rubric` の形、採用前の設計 review の `design` の分岐（受け入れ条件を判定しない run）、
   checkpoint-core の `ParentEvaluation`・`check_evaluation`・`PARENT_EVALUATION_RUBRIC`、`committed_schema_matches_generated` の再生成。
4. 選択肢: (a) task-worker に評価の型を別に持つ（ADR D11 の文）→ task-core の `ParentEvaluation` と同じ形の型が 2 つになる（規約 2）。
   (b) `ReviewOutput.parent_evaluation: Option<ParentEvaluation>`（型つき）→ 未知の欄で `review.json` 全体が読めなくなり、`check_evaluation` の R1 の違反が記録されない。
   (c) 採用: wire は `Option<serde_json::Value>` で読み、schema は `#[schemars(with = "Option<ParentEvaluation>")]` で task-core の型から作る。
   `check_evaluation(raw: Option<&Value>, …)` の入力にそのまま渡せる。
5. 新しい状態・分岐・fallback・port: 欄 2 つ（`ReviewRequest.parent_checkpoint`・`ReviewOutput.parent_evaluation`）、prompt の分岐 1 つ（`build_review_prompt` の `parent_checkpoint` の腕）、
   `result_section` の分岐 1 つ。状態・fallback・port は無し。
6. 撤去対象: `CriticRubricItem` を `RubricItem` に改名し、Critic と親の途中評価で共有した（固定の基準の 1 項目という同じ意味。`RubricItem::from_table` が
   `StrategyCriticRequest::fixed_rubric` と `ParentCheckpointRequest::fixed_rubric` の写しの処理を 1 つにした）。wire の JSON は変わらない（schema の `$defs` の名前だけ）。
7. 不変条件: 節の無い review の prompt・`request.json` が base と同じ byte 列（`parent_evaluation_absent_*`）、節のある wire が schema・md と一致（`parent_evaluation_wire_*`）。

## 実装前の失敗（受け入れ条件 0）

commit 1 の後、wire の試験（`protocol/parent_checkpoint/tests.rs` と `change_rules/review/tests.rs` の `parent_evaluation_wire_*`）を置き、型・欄・節の無い状態で実行した:

```
$ cargo nextest run -p task-worker -p task-core --no-fail-fast -E 'test(/parent_evaluation_wire_|parent_evaluation_absent_/)'
error[E0422]: cannot find struct, variant or union type `ParentCheckpointRequest` in this scope   （ほか ParentCheckpoint* の 7 型）
error[E0425]: cannot find function `parent_checkpoint_section` in this scope
error[E0425]: cannot find value `PARENT_CHECKPOINT_HEADING` in this scope
error[E0560]: struct `ReviewRequest` has no field named `parent_checkpoint`
error[E0609]: no field `parent_evaluation` on type `ReviewOutput`
error: could not compile `task-worker` (lib test) due to 20 previous errors
exit=101
```

`parent_evaluation_absent_*` の 3 件は base で通る回帰で、sha256 は base の commit（commit 1 の時点。型・欄・節は base のまま）で取った値を試験に書いた:

```
$ cargo nextest run -p task-worker --no-fail-fast -E 'test(/parent_evaluation_absent_/)'
     Summary [   0.020s] 3 tests run: 3 passed, 1331 skipped
```

## 実装後（提出時の HEAD）

```
$ cargo nextest run -p task-worker -p task-core --no-fail-fast -E 'test(/parent_evaluation_wire_|parent_evaluation_absent_/)'
     Summary [   0.037s] 8 tests run: 8 passed, 2792 skipped
$ cargo nextest run -p task-worker -p task-core --no-fail-fast
     Summary [  41.288s] 2785 tests run: 2785 passed, 15 skipped
```

## 試験と守る不変条件

既存の試験は `strategy_critic`・`design`・`health` の欄と、最小の final review 1 形の prompt の sha256（`strategy_critic_absent_prompt_unchanged`）だけを見る。
親の途中評価の欄・節が無いので、下の不変条件が壊れても既存の試験では検出できない。

| 試験 | 守る不変条件 |
|---|---|
| `parent_evaluation_absent_prompt_unchanged` | 節の無い review の prompt が base と同じ byte 列（review の文脈なし・最小の final review・人の決定/回答/check/policy 違反/health/差分の設計/Critic を全部持つ final review・採用前の設計 review の 4 形の sha256） |
| `parent_evaluation_absent_request_json_unchanged` | 同じ 4 形の `request.json`（`write_run_request` と同じ `to_string_pretty` と末尾の改行）が base と同じ byte 列で、key `parent_checkpoint` が出ない |
| `parent_evaluation_absent_old_review_json_parses` | 旧い `review.json`（verdict だけ・health・設計・Critic つき）が読め、書き戻しても key `parent_evaluation` が出ず同じ値 |
| `parent_evaluation_wire_roundtrip` | 入力が `request.json` に key として出て読み戻せる。rubric は `PARENT_EVALUATION_RUBRIC` と同じ。出力は値のまま読め、task-core の `ParentEvaluation` として読める |
| `parent_evaluation_wire_malformed_evaluation_still_reads` | 未知の欄 `achieved`・未知の verdict の評価でも `review.json` は読め、`check_evaluation` が `invalid` にする。正しい評価は `checked` |
| `parent_evaluation_wire_schema_matches_md` | schema の `ReviewRequest.parent_checkpoint`・`ReviewOutput.parent_evaluation` と、新しい 14 型の全欄・5 つの列挙の全値が worker-protocol.md の「親の途中評価」の節の code span にある。`context.review` の表の行に `parent_checkpoint?`。`ParentEvaluation` は未知の欄を受けない |
| `parent_evaluation_wire_prompt_section` | 節のある prompt に節がちょうど 1 回出て、判断の id・root・trigger・Outcome と `reached`・strategy の版と候補・子・メトリクス・前の評価・E1〜E6 を持つ。受け入れ条件の判定の節（判定する criterion・worker の申告）は出ない。節の無い review には出ない |
| `parent_evaluation_wire_result_example_passes_shape_check` | 節のある run の Result 節の例の JSON は `ReviewOutput` として読め、`verdicts` は空、`parent_evaluation` は `check_evaluation` の R1 を通る（例を写した評価が invalid にならない） |

## 変更した file（範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の出力と同じ。範囲 check は exit 0）

- `crates/task-worker/src/protocol/parent_checkpoint.rs`（新）: `ParentCheckpointRequest` と子の型 7 つ（`Goal`・`Outcome`・`Strategy`・`Child`・`Criterion`・`Metrics`・`Previous`）、`fixed_rubric`。
- `crates/task-worker/src/protocol/parent_checkpoint/tests.rs`（新）: wire の試験 3 件と sample。
- `crates/task-worker/src/protocol.rs`: 欄 `ReviewRequest.parent_checkpoint`・`ReviewOutput.parent_evaluation`、`CriticRubricItem` → `RubricItem`（`from_table`）、module と re-export。
- `crates/task-worker/src/change_rules/review.rs`: `PARENT_CHECKPOINT_HEADING`・`parent_checkpoint_section`・`result_section` の分岐・`parent_evaluation_example`・`parent_evaluation_lines`。
- `crates/task-worker/src/change_rules/review/tests.rs`: `parent_evaluation_absent_*` 3 件と `parent_evaluation_wire_*` の prompt の 2 件。
- `crates/task-worker/src/claude_code/prompt.rs`: `build_review_prompt` の `parent_checkpoint` の腕（受け入れ条件の判定の節を出さない）。
- `crates/task-worker/src/protocol/{design_review,health}/tests.rs`: `ReviewOutput` の literal に `parent_evaluation: None`。
- `crates/task-core/src/parent_checkpoint.rs`: 評価の 11 型に `JsonSchema` の derive と schema の名前（`ParentEvaluation*`）、`references` の schema は object の配列（`Reference` は範囲外の `strategy/mod.rs` にあり derive が無い）。
- `docs/protocol/worker-protocol.md`: `context.review` の表の行・§10.2 の列挙と説明・新しい節「親の途中評価」・`CriticRubricItem` → `RubricItem`。
- `docs/protocol/worker-protocol.schema.json`: `UPDATE_SCHEMA=1 cargo nextest run -p task-worker -E 'test(committed_schema_matches_generated)'` で再生成。
- この file。

## 範囲外の 1 行（plan_issue）

`ReviewRequest` に欄を足すと、全欄を列挙する literal `crates/task-dispatch/src/review.rs` の `ReviewHumanContext::into_request`（1201 行）がコンパイルできない:

```
$ cargo check -p task-dispatch
error[E0063]: missing field `parent_checkpoint` in initializer of `ReviewRequest`
    --> crates/task-dispatch/src/review.rs:1201:9
```

直しは final review の request に `parent_checkpoint: None` の 1 行（final review は親の途中評価の run ではない）。この path はこの WU の範囲 check の正規表現に無いので書いていない。
計画の範囲に `crates/task-dispatch/src/review\.rs$` を足すよう `plan_issue` で申告した。他の `ReviewRequest {` の literal（task-dispatch の試験・`design_gate.rs`）は
`..Default::default()` を使うので変更は要らない。

## 検査（提出時の HEAD）

1. `cargo nextest run -p task-worker -p task-core --no-fail-fast -E 'test(/parent_evaluation_wire_|parent_evaluation_absent_/)'`: exit 0、8 passed。
2. schema・md の check（plan の check 2）: exit 0。
3. `cargo nextest run -p task-worker -p task-core --no-fail-fast`: exit 0、2785 passed（`committed_schema_matches_generated`・`strategy_critic_absent_prompt_unchanged` を含む）。
4. `cargo clippy -p task-worker -p task-core -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
5. 範囲 check（plan の check 5）: exit 0。
6. `bash scripts/dev/test-affected.sh` と `cargo clippy --workspace -- -D warnings`: task-dispatch のコンパイルで落ちる（上の範囲外の 1 行）。範囲が足された run で実行する。

## LoC（`git diff --numstat $CELERIS_WU_BASE` と新しい file の行数）

- production: +380 行・−22 行（`protocol/parent_checkpoint.rs` +120、`change_rules/review.rs` +185、`protocol.rs` +40・−11、`prompt.rs` +7、`task-core/src/parent_checkpoint.rs` +28・−11）。
  `StrategyCriticRequest::fixed_rubric` の写しの処理は `RubricItem::from_table` に移し、親の途中評価と共有した。
- test: +555 行（`protocol/parent_checkpoint/tests.rs` +234、`change_rules/review/tests.rs` +319、literal の 2 行）。
- 文書・生成物: md +65・−4、schema +498・−17（生成）。
- 純増の理由: ADR D3.1〜D3.3 が求める入力の型 8 つ・出力の欄・prompt の節と Result 節の形を新しく置いた。旧い経路の置き換えではないので、改名した `CriticRubricItem` 以外に消す対象は無い。

## 未解決

- 範囲外の 1 行（上）。直るまで task-dispatch・task-api などの逆依存の crate はコンパイルできない。
- ADR D9.2 は `parent_evaluation_absent_*` を task-worker・task-dispatch に置くと書くが、この WU の範囲は task-worker だけなので task-worker に置いた。
  dispatcher が組む request の試験は controller-checkpoint（`dispatcher/parent_evaluation.rs` が `parent_checkpoint` を組む WU）の範囲。
- `ParentCheckpointCriterion.result` の値（`pass`・`fail`・`not_run`）と `kind` の値は文字列で、値を決めて詰めるのは controller-checkpoint の組み立て。型で閉じていない。
- `references` の schema は object の配列で、`Reference` の 6 種の形は schema に出ない（`task_core::strategy::Reference` に `JsonSchema` の derive が無く、`strategy/mod.rs` は範囲外）。md の節が 6 種を書く。
- ADR の「保証と実装の対応」の表の `parent_evaluation_wire_roundtrip`・`parent_evaluation_wire_schema_matches_md` は試験名のとおり置いた。
  ADR D11 の「evaluator-wire は `parent_evaluation` の型を task-worker の protocol に wire の形で持ち」は、型を task-core の 1 つにした（上の brief 4）。ADR の付記は書いていない（下の提案）。

## 提案

- ADR D3.2・D11 の付記（close-gate か controller-checkpoint が書く）: `ReviewOutput.parent_evaluation` は `Option<serde_json::Value>` で、schema は
  `task_core::parent_checkpoint::ParentEvaluation` から生成する。dispatcher は `check_evaluation(review.parent_evaluation.as_ref(), …)` にそのまま渡す。
- `task_core::strategy::Reference` に `JsonSchema` の derive を足せる WU があれば、`parent_checkpoint.rs` の `#[schemars(with = …)]` 3 か所を外せる
  （撤去の条件: `Reference: JsonSchema`。判断者: 親の closeout で人）。
