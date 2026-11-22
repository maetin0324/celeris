---
title: 戦略層 v1 子 6 checkpoint-core — task_core::parent_checkpoint（発火条件と評価の出力の決定的な検査）
tasks: [01M7XQ9QWK58ESMW42Q1TGT3WZ]
wu: checkpoint-core
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# checkpoint-core（WorkUnit）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-parent-checkpoint-evaluator](../../adr/2026-11-22-parent-checkpoint-evaluator.md)（D1.2・D1.3・D3.2・D3.4・D4・D5.2・D11）。
基準はこの WU の `$CELERIS_WU_BASE`。検査は提出時の HEAD で実行した。

## commit（件名）

1. `strategy-v1 6 checkpoint-core: parent_checkpoint_contract_ の試験と型の署名（実装前）` — 試験 25 件と、型・署名だけで本体が `todo!()` の module。
2. `strategy-v1 6 checkpoint-core: task_core::parent_checkpoint の実装と WU の進捗` — 本体の実装とこの file。

## 変更した file（範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の出力と同じ）

- `crates/task-core/src/parent_checkpoint.rs`（新）: `CheckpointSettings`・`triggers`・`check_evaluation`・`references_to_resolve`・`ParentEvaluationRecord`・`allowed_actions`・`action_allowed`・`PARENT_EVALUATION_RUBRIC` と型。
- `crates/task-core/src/parent_checkpoint/tests.rs`（新）: 試験 25 件（prefix `parent_checkpoint_contract_`）。
- `crates/task-core/src/lib.rs`: `pub mod parent_checkpoint;` の 1 行と doc comment の 1 行。
- この進捗 file。

store・migration・events・`ProjectAction`（`project_proposal.rs`）は変えていない（下の検査 3）。

## 実装前の失敗（受け入れ条件 0）

commit 1 の状態（型と署名だけ、本体は `todo!("実装前")`）で実行した:

```
$ cargo nextest run -p task-core -E 'test(/parent_checkpoint_contract_/)' --no-fail-fast
     Summary [   0.035s] 25 tests run: 0 passed, 25 failed, 1436 skipped
exit=100
```

25 件すべてが `not yet implemented: 実装前` の panic で落ちた。base（`$CELERIS_WU_BASE`）には module `parent_checkpoint` が無いので、試験は base ではコンパイルできない。

## 実装後（提出時の HEAD）

```
$ cargo nextest run -p task-core -E 'test(/parent_checkpoint_contract_/)'
     Summary [   0.043s] 25 tests run: 25 passed, 1436 skipped
exit=0
```

## 試験と守る不変条件（受け入れ条件 1）

既存の試験（`strategy_*`・`strategy_critic_*`・`project_controller_*`）は戦略の文書の形・Critic の対象・controller の既存の行動だけを見る。
base には親の途中評価の発火・評価の検査・行動の表の関数が無いので、下の不変条件が壊れても既存の試験では検出できない。

| 試験 | 守る不変条件 | 受け入れ条件 1 の項目 |
|---|---|---|
| `children_every_n` | 子の終端の数が N の倍数に達したときだけ `children:<k>` が due。N 未満（1 < 2、N = 3 で 2 件）では出ない。評価済みの k は再び due にならない | 発火・不発火（N 未満） |
| `children_jump_max_only` | 終端は done・failed・cancelled。c が 2 段以上進んだら最大の k だけ | 発火 |
| `review_fail_retry_not_counted` | `review_fail` で `ready` に戻る再試行は終端にも失敗にも数えない | 不発火 |
| `no_strategy_root_no_triggers` | strategy の無い root は子 4 件・失敗・予算 95% でも trigger 0 件 | 不発火（strategy の無い root） |
| `child_failed` | 一度でも `failed` に遷移した子は `child_failed:<id>` 1 件（2 回失敗・再開しても 1 件） | 発火 |
| `refute_new_evidence_only` | 参照した版より新しい版の、参照した版に無い refute の証拠だけ。持ち越し・参照した版そのもの・別の strategy は数えない。同じ証拠は最初の版の key 1 つ | refute |
| `refute_unreadable_document_ignored` | 記録が引けない・JSON でない・name と中身の版が違う文書は数えない | refute |
| `budget_thresholds` | `max_tokens` に対する割合が 50%・80% 以上で `budget:<p>`。評価済みは再び due にならない。`max_tokens` が無い・0 なら出ない。u64 の上限でも桁あふれしない | 予算の閾値 |
| `trigger_state_order` | 状態は evaluated → requested → due の順に導く（requested は due にならず同じ trigger の判断は 2 回作られない）。出力は時刻順 | 発火 |
| `settings_ranges` | N 1〜8・閾値は狭義の昇順で 1〜99・4 件まで・試行 1〜3。範囲外は設定エラー | — |
| `arch_v4_children_times` | ADR D8.1 の子の終端の時刻で N = 2 は B（11-17T21:55:57Z）・D（11-18T20:39:25Z）・F（11-19T04:54:54Z）で発火 | 発火 |
| `unsupported_not_on_track` | 参照の無い・解決しない改善の主張は unsupported で、verdict は on_track にならず、continue は ask_human になる | unsupported の主張 |
| `url_only_is_unsupported` | `url`・`knowledge`・`chat` だけの主張は解決していても支持に数えない | unsupported の主張 |
| `on_track_with_resolved_measure` | 解決した参照の測定が 1 件以上・unsupported 0 のときだけ on_track。1 件でも参照の無い知見があれば unknown | unsupported の主張 |
| `goal_met_needs_d6` | goal の met は D6 の `goal_reached` だけが決める。自己申告の met は unknown・unsupported +1・on_track 無効 | goal の自己申告の拒否 |
| `execution_from_facts` | execution は終端の子の事実で決まり、申告と違えば違反に記録する | — |
| `negative_result_axes` | 負の研究結果は {succeeded, advanced, not_met} と支持された反証で表し、`negative_result` は事実から導く | — |
| `epistemic_advanced_needs_support` | advanced は支持された知見・反証があるときだけ | — |
| `shape_invalid` | 節が無い・schema 違い・未知の欄（`achieved` など）・未知の値・空の主張は invalid で unknown・ask_human | — |
| `input_limits` | 各列 16 件・文 2,000 文字（文字で数える）・metric 等 200 文字・参照 16 件・全体 262,144 byte の上限。byte の上限は形の検査より先 | 入力長の上限 |
| `references_to_resolve` | 呼び出し側が引く参照は task・artifact・commit だけで重複しない | — |
| `allowed_actions_table` | 写していない評価に対する行動は D5.2 の表だけ。off_track・unknown から continue は出ない。escalate の question_key は `checkpoint:<判断 id>` | — |
| `action_kinds_match_project_action` | 表の kind の文字列が `ProjectAction::kind()` と同じ | — |
| `record_roundtrip` | 成果物の name の作りと読みが 1 つの定義。記録 `celeris.parent-evaluation-record/1` を書いて読み戻せ、invalid の記録は評価を写さない | — |
| `rubric_fixed` | rubric は E1〜E6 の固定の定数 | — |

## 検査（提出時の HEAD）

1. `cargo nextest run -p task-core -E 'test(/parent_checkpoint_contract_/)'`: exit 0、25 passed。
2. `cargo clippy --workspace -- -D warnings`: exit 0（`deno_core` の vendor の既存の warning 1 件だけで、`-D warnings` の対象外）。`cargo fmt --all -- --check`: exit 0。
3. 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` は `crates/task-core/src/lib.rs`・`parent_checkpoint.rs`・`parent_checkpoint/tests.rs` とこの file だけ。
   `git diff --name-only $CELERIS_WU_BASE -- crates/task-core/src/store crates/task-core/migrations crates/task-core/src/model/events.rs crates/task-core/src/project_proposal.rs` は 0 行（受け入れ条件 2）。
4. `bash scripts/dev/test-affected.sh`: 結果は下の「test-affected」の節。

## LoC（`git diff --numstat $CELERIS_WU_BASE`）

- production: +1,026 行（`parent_checkpoint.rs` 1,024 行、うち空行 85・comment 79。`lib.rs` +2）。
- test: +982 行（`parent_checkpoint/tests.rs`）。
- 純増の理由: ADR の D1.2〜D5.2 が求める純関数と型（設定の範囲・trigger 4 種・評価の 13 の型・R1〜R7・記録・表）を新しく置いた。旧い経路の置き換えではないので消す対象は無い（ADR の撤去対象「無し」）。

## 実装で決めたこと（ADR の範囲の内側）

- 依存の向き: `parent_checkpoint` は `strategy`（`Reference`・`StrategyDocument`・`Stance`）・`strategy_critic`（`strategy_artifact`）・`model`（`Status`）だけを import する。
  task-core には `strategy`・`project_proposal`・`store` を含む既存の強連結成分がある（`strategy` → `project_proposal::sha256_hex`）。
  `project_proposal` か `store` が `parent_checkpoint` を import すると、既存の循環に新しい組が入り repo health の `deps.cycle` が fail になる。
  そこで行動の表は `AllowedAction::kind()`・reason_code・question_key の文字列で照合する（`action_allowed`）。`ProjectAction` の型を引かない。
- 反証の key: 同じ証拠 id が v2 と v3 に持ち越されたとき、最初に現れた版の key（`refute:<id>:v2:<e>`）1 つだけを出す。ADR D1.2 の文のままだと版ごとに key が変わり、同じ反証で 2 回評価する。
- R6: 支持された知見・反証があれば申告が none でも advanced（違反を記録）。支持が無ければ申告の advanced は unknown、none はそのまま。
- R4: D6 で到達済みのとき申告の not_met は違反に記録し、goal は met。
- R4 を R3 より先に計算する（goal の自己申告の met を unsupported に数えてから on_track の可否を決める）。
- invalid の記録: verdict unknown・next ask_human。execution は事実（R5）、goal は D6 の値、epistemic は unknown。`evaluation` は写さない（`null`）。
- R1 の形に、空の claim・statement・hypothesis・metric・before・after・resolve_by・reason を invalid とする規則を足した（空の主張に参照を付けて on_track にする抜け道を塞ぐ）。
- 全体の上限 `EVALUATION_MAX_BYTES = 262,144`（strategy の文書と同じ値だが、意味が違うので定数は別）。

## 未解決

- `test-affected.sh` の結果は下の節（提出時の HEAD で実行）。
- 評価の質（LLM の reviewer の出力が answer key と合うか）はこの WU の範囲外。ADR「未確認」のまま。
- `ChildTransition.at` と `StrategyDocumentFacts.at` は RFC 3339 の UTC の文字列で、文字列の順を時刻の順として使う。events の ts の書式が混ざる場合の扱いは呼び出し側（controller-checkpoint）で確かめていない。

## 提案

- controller-checkpoint への引き継ぎ（ADR D11 の後続の WU）:
  - `project_proposal::validate` の `checkpoint_action_mismatch` は、`parent_checkpoint` を import せずに照合する。snapshot（`task_ops::project_snapshot`。別の crate なので循環にならない）が
    `allowed_actions` の結果を文字列（kind・reason_code）で載せ、validate はその文字列と `ProjectAction::kind()` を比べる。`StrategyRef` も `parent_checkpoint` に置かない（`store` から引くため）。
  - snapshot の trigger は `triggers` の `TriggerState::Due` だけを出す。`requested` には判断の表の `pending`・`applied`・`suppressed` の `triggers` を、`evaluated` には記録（`ParentEvaluationRecord.triggers`）を入れる。
  - `CheckpointSettings::new` を config の `[project_controller.checkpoint]` の読み込みで呼び、範囲の検査を config 側に書き直さない。
- evaluator-wire（並行の WU）への提案: `ReviewOutput.parent_evaluation` の型を task-worker に別に持たず、`task_core::parent_checkpoint::ParentEvaluation` を使えば同じ schema の型が 1 つになる（規約 2）。
  並行で走るので、統合（integrate-contract）か controller-checkpoint で二重の定義が残っていないかを確かめる。
- `strategy::Reference::is_internal` は private なので、ここでは同じ 1 行の `matches!` を `is_internal` として持った（意味は子 2 D2.3 と同じ）。
  `strategy/mod.rs` を書ける WU が `is_internal` を `pub` にし、こちらはそれを使うと定義が 1 つになる。撤去の条件: `Reference::is_internal` が pub になったとき。判断者: 親の closeout で人。
- ADR D1.2 の反証の key の付記（最初に現れた版の key だけ）は、この WU の範囲に ADR が無いので書かなかった。close-gate か controller-checkpoint が ADR の付記に写す。
