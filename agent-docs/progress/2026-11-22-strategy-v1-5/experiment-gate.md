---
title: 戦略層 v1 子 5 experiment-gate — CoS の task.create の strategy の検査に実験段階・Critic 未了・却下への逆戻り・失敗の未読を足す
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
status: done
updated: 2026-11-22
completed: 2026-11-22
---
# experiment-gate（WorkUnit。子 5 の段 gate）

完了日: 2026-11-22。設計の正本は [ADR 2026-11-22-strategy-review-critic](../../adr/2026-11-22-strategy-review-critic.md) の D2・D3.7・D4・D5・D7.4・D8。
基準はこの WU の `$CELERIS_WU_BASE`。検査はすべて提出時の HEAD で実行した。

## 結論

- `crates/task-api/src/cos/strategy_gate.rs` の `check` は、子 2 の文書の検査（`validate_document`）に続けて、実行の入口の事実を store から集めて
  `task_core::strategy::check_execution`（contract-review の純関数）に渡す。規則は gate に書いていない。違反は子 2 と同じ 422 の `rules`・`codes`・`violations` に並ぶ。
- 集める事実（`ExecutionFacts`）: 文書を作った task の status が `done` か、`task_core::strategy_critic::critic_subjects`（critic-review）の対象に参照の
  `(name, sha256)` があるか（衝突した版は対象外）、参照した版と鎖の前の版ごとの X(v)（`cos_operations` の `task.create`・`applied` の行の
  `payload_json.strategy` が版の name・sha256 のもの、D2.3）と各 task の終端（`done`・`failed`・`cancelled`）、本文の `execution`・`max_wall_secs`、参照の `candidate`。
- `StrategyRef` に任意の欄 `candidate` を足した（`deny_unknown_fields` のまま、`skip_serializing_if = "Option::is_none"`）。候補の無い参照の
  request_hash は base と同じ式のまま（`operation_hash` は欄の無い値を直列化する）。
- `crates/task-api/src/cos/operations.rs`（高 risk の class `authz`）は変えていない。断った要求は既存の `check_strategy` → `OperationAudit::reject` で
  `rejected` の行と監査を残し、task を作らない。`instructed_by` でも外れない（gate は `instructed_by` を見ない位置にある）。
- strategy の無い `task.create` は gate を通らない（呼び出し側が欄のあるときだけ呼ぶ。子 2 の `strategy_gate_absent_field_keeps_task_create_unchanged` が pass）。
- 新しい試験 `strategy_gate_review_` は 9 件。子 2・子 3 の既存の 10 件と合わせて 19 件、`committed_schema_matches_generated` を含めて 20 件が pass する。

## 実装前 brief

1. 根本原因: 子 2 の gate は承認済み・最新・同じ案件・出典のそろった版なら、compound の大きな計画も同じ候補の 2 件目の実験も、文書の task が review 中でも通す。
   症状は `/cos/operations` の `task.create` に出る（固定の大きな計画を最初に起票できる）。
2. 責務 owner: 規則は `task_core::strategy::review::check_execution`（済み）。事実を store から集めるのは実行の入口 `cos::strategy_gate`。
3. 既存の再利用先: `check_execution`・`ExecutionFacts`・`ExperimentTask`（contract-review）、`critic_subjects`（critic-review）、gate の `Reader::chain`（版の鎖）と
   `refusal`（422 の形）、`operations::check_strategy` の監査、子 2 の試験の `World`・`approved`・`publish`・`op`・`post`。
4. 選択肢: (a) X(v) を task-core の store に新しい照会として足す → 書く範囲（ADR D8）の外で、task-core の chat は子 4 の範囲と重なるので採らない。
   (b) gate の中で規則を書く → 規則の二重化（規約 2）なので採らない。(c) 採用: gate が `SqliteStore::write_tx` の中で SELECT だけを行い、`cos_operations` を 1 回走査して
   鎖の全版の X を Rust で選ぶ（task-core に `cos_operations` を任意の条件で読む公開の口が無い。書かない）。
5. 新しい状態・分岐・fallback・port: 状態・表・event・migration は無し。`StrategyRef.candidate`（任意の欄、ADR D2.2）だけ。fallback は無い。
6. 撤去対象: 無し（子 2 の gate の経路に規則を足すだけ。旧い入口は残らない）。子 2 の試験の基準の要求は D7.4 のとおり直した（下）。
7. 不変条件: 実験段階の版は open の候補ごとに atomic の実験 1 件だけを通す。compound は `selected` のある版で、`task.create` の時点で前の版の実験 task が 1 件以上あり、すべて終端で、すべて版の証拠に参照されているときだけ通す（選ぶ版を実験の結果より先に書くことは gate では止めない。ADR の付記「gate が保証する範囲の訂正」）。却下した候補は起票できない。
   拒否は task を作らず監査に残り、`instructed_by` で外れない。strategy の無い `task.create` は変わらない。試験は下の表。

## 変更

- `crates/task-api/src/cos/strategy_gate.rs`: `StrategyRef.candidate`、`execution_facts`、`strategy_operations`（X(v) の元）、`check_execution` の呼び出し、`mod review_tests`。
- `crates/task-api/src/cos/strategy_gate/review_tests.rs`（新）: `strategy_gate_review_` の 9 件。
- `crates/task-api/src/cos/strategy_gate/tests.rs`: D7.4 の直し（下）と、`review_tests` が使う部品を `pub(super)` にした。
- 生成物: `docs/api/v1/api-v1.schema.json`（`UPDATE_SCHEMA=1 cargo nextest run -p task-api -E 'test(committed_schema_matches_generated)'`）、
  `web/api/generated/schema.json`・`types.ts`（`node web/scripts/gen-types.mjs`、`--check` が exit 0）。どちらも `StrategyRef.candidate` が増えただけ。

commit（件名）:
- strategy-v1 5 experiment-gate: 試験を先に足す（strategy_gate_review_ と子 2 の strategy_gate_ の基準の要求を実験段階に合わせる）
- strategy-v1 5 experiment-gate: CoS の task.create の strategy の検査に実験段階・Critic 未了・却下への逆戻り・失敗の未読を足す
- strategy-v1 5 experiment-gate: WU の進捗と ADR の付記

## 試験（`strategy_gate_review_`、9 件）

既存の試験で検出できない理由: task-core の `strategy_review_contract_` は `check_execution` に渡した事実だけを見る。gate が store から X(v)・task の status・
Critic の対象を引く経路と、422 で task が作られないこと・監査が残ることは、`/cos/operations` を通す試験でしか見えない。子 2 の `strategy_gate_` は D2〜D4 の規則を知らない。

| 試験 | 守る不変条件 | code |
|---|---|---|
| `…_compound_in_experiment_phase_is_422_and_creates_nothing` | 実験段階の版で compound・`execution` の省略・候補の省略・予算（6 h）を 1 秒超える `max_wall_secs` を拒否し、task 0。予算ちょうどの atomic は通る | `strategy_experiment_phase` だけ |
| `…_atomic_experiments_up_to_open_candidates` | open の候補 2 件の実験は通り、同じ候補の 2 件目・文書に無い候補は拒否。作った task は 2 件 | 同上 |
| `…_rejected_candidate_task_is_revisit` | 次の版で却下した候補の起票を拒否し、同じ版の open の候補（足した 3 件目）の実験は通る | `strategy_revisit_rejected` だけ |
| `…_source_task_not_done_is_uncritiqued` | 文書の task が `reviewing`・`ready`（reopen）なら拒否。同じ task に新しい版（未承認）があると、承認済みの古い版は Critic の対象でなく拒否（stale ではない） | `strategy_uncritiqued` だけ |
| `…_failure_unread_is_422` | 前の版の終端の実験（done・failed）の 1 件を証拠が参照しない `selected` の版を拒否 | `strategy_failure_unread` だけ |
| `…_selected_without_experiments_is_422` | v1 で `selected` を書いた版と、前の版の実験 task が終端でない時点の起票を拒否 | `strategy_experiment_phase` だけ |
| `…_selected_version_allows_compound` | 前の版の実験が全部終端（done・cancelled）で全部を参照した `selected` の版は、候補の省略でも `selected` の候補でも compound を通す。却下した候補は拒否 | 通過 2 件、`strategy_revisit_rejected` だけ |
| `…_refusals_are_audited` | 4 つの code の拒否がそれぞれ `action = 'rejected'` の `cos_operations` の行（理由 `rejected: the strategy cannot be executed…` と要求の `strategy.candidate`）を残し、task は実験の 2 件だけ | 4 code |
| `…_instructed_by_cannot_waive` | 人の発言の中継（`instructed_by`）でも実験段階・却下と失敗の未読・Critic 未了の拒否が外れない | `[phase]`、`[unread, revisit]`、`[uncritiqued]` |

「4 つの code がそれぞれその規則だけで出て task が作られない」は、試験の部品 `only` が `codes == [want]` と task の数の不変を毎回確かめる。

## 変更前に新しい試験が落ちた出力

試験を先に commit し（gate の変更なし）、次の 2 段で実行した。log は run の成果物 `tests-before-field.log`・`tests-field-only.log`・`tests-after.log`（artifacts/）。

コマンド（3 段とも同じ）:
`cargo nextest run -p task-api -E 'test(/strategy_gate_/) | test(/discussion_mode_execute_unapproved_strategy/) | test(/chat_discussion_summary_strategy_gate/)' --no-fail-fast`

1. 試験だけ（base の gate）: exit 100、`19 tests run: 1 passed, 18 failed`。`StrategyRef` が `candidate` を知らず（`deny_unknown_fields`）、要求が 400 になる。
   ```
   panicked at crates/task-api/src/cos/strategy_gate/review_tests.rs:59:5:  left: 400  right: 422
   panicked at crates/task-api/src/cos/strategy_gate/review_tests.rs:69:5:  left: 400  right: 200
   ```
2. `StrategyRef.candidate` だけ足した（規則なし）: exit 100、`19 tests run: 10 passed, 9 failed`。子 2・子 3 の既存 10 件は D7.4 の直しで pass し、
   新しい 9 件は base の gate が拒否すべき要求を通して落ちる。
   ```
   strategy_gate_review_compound_in_experiment_phase_is_422_and_creates_nothing  review_tests.rs:57:5  left: 200  right: 422
   strategy_gate_review_atomic_experiments_up_to_open_candidates                 review_tests.rs:57:5  left: 200  right: 422
   strategy_gate_review_rejected_candidate_task_is_revisit                       review_tests.rs:57:5  left: 200  right: 422
   strategy_gate_review_source_task_not_done_is_uncritiqued                      review_tests.rs:57:5  left: 200  right: 422
   strategy_gate_review_selected_without_experiments_is_422                      review_tests.rs:57:5  left: 200  right: 422
   strategy_gate_review_refusals_are_audited                                     review_tests.rs:57:5  left: 200  right: 422
   strategy_gate_review_instructed_by_cannot_waive                               review_tests.rs:478:5 left: 200  right: 422
   strategy_gate_review_failure_unread_is_422                                    review_tests.rs:58:5  left: String("strategy_refute_removed")  right: "strategy_failure_unread"
   strategy_gate_review_selected_version_allows_compound                         review_tests.rs:71:5  left: 422  right: 200
   ```
   最後の 2 件は試験の文書の誤り（実験 task の参照を反証の証拠 `evidence[1]` に足し、子 2 D2.3 の `strategy_refute_removed` に当たった）で、参照を支持の証拠
   `evidence[0]` に移した。主張は変えていない。
3. gate の実装の後: exit 0、`19 tests run: 19 passed`（schema の試験を足して 20 件 pass）。

## 既存の試験の直し方（ADR D7.4、弱めない）

- 基準の要求（`task_body`）に `"execution": "atomic"`、`create` が送る参照に候補が無ければ `candidate: "cut-min-edges"`（open の候補）を足す（`with_candidate`）。
  文書の `supersedes` には候補を書かないので、送る直前に足す。文書の task は fixture のまま `done`（`task()` が `Status::Done`）。
- `strategy_gate_human_approved_latest_document_creates_the_task` の payload の比較は、送った参照（候補を足したもの）と比べる。
- `chat_discussion_summary_strategy_gate_edited_is_stale` の通る起票（`c-current`）は、もう 1 つの open の候補 `continue-pilots` にした。同じ版の同じ候補の
  2 件目にすると、後の `c-edited` に `strategy_experiment_phase` が加わり `codes == ["strategy_stale"]` の主張が成り立たない（主張は変えていない）。
- N-x がちょうどその code になる `strategy_gate_ablation_four_rules` の主張は変えていない（pass）。

## 検査（提出時の HEAD で実行）

- `bash scripts/dev/test-affected.sh`: exit 0。対象 crate は celeris・celeris-campaign・celerisctl・llm-proxy・task-api・task-dispatch・task-worker と web の型検査・unit。
  nextest `6249 tests run: 6249 passed (9 slow), 28 skipped`、web `Test Files 105 passed (105)`・`Tests 745 passed (745)`、`repo_health_gate: all checks passed`。
- `cargo clippy --workspace -- -D warnings`: exit 0。
- `cargo fmt --all -- --check`: exit 0。
- `node web/scripts/gen-types.mjs --check`: exit 0。
- 範囲: `sh "$CELERIS_WU_SCOPE_PATHS"` の path は `crates/task-api/src/cos/strategy_gate.rs`・`strategy_gate/review_tests.rs`・`strategy_gate/tests.rs`・
  `docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`・`web/api/generated/types.ts` と、この進捗・ADR の付記だけ。

## LoC（`git diff --numstat` 基準 `$CELERIS_WU_BASE`、進捗と ADR を除く）

| 区分 | file | 追加 | 削除 |
|---|---|---|---|
| production（Rust） | `crates/task-api/src/cos/strategy_gate.rs` | 123 | 5 |
| test | `strategy_gate/review_tests.rs`（新） | 528 | 0 |
| test | `strategy_gate/tests.rs` | 50 | 27 |
| 生成物 | `docs/api/v1/api-v1.schema.json`・`web/api/generated/schema.json`・`types.ts` | 15 | 0 |

production の純増 118 行の理由: 事実を集める `execution_facts`（約 60 行）と X(v) の照会 `strategy_operations`（約 40 行）、`candidate` の欄と doc。規則は task-core の
`check_execution` にあり、ここには書いていない。

## 未確認と残る穴

- X(v) の照会は `write_tx`（IMMEDIATE の transaction）の中の SELECT だけで、書き込み接続の lock を短く取る。本番の `cos_operations`（子 2 の測定で 3,911 行、照会 9 ms）
  での時間はこの WU で測っていない（未確認）。読み取り専用の公開の口が task-core に要るなら、子 4 の統合の後に task-core の chat の store に足すのが筋（提案）。
- 実験の task が `cos_operations` の行にあり task の行が無い（task は消えない前提）ときは終端でない扱い（選択を止める側）にした。起きる経路は確認していない。
- `candidate` の形（`[a-z0-9-]{1,40}`）は gate で別に検査していない。文書に無い id は `check_execution` の「候補が文書に無い」（`strategy_experiment_phase`）になる。
- Critic の LLM の判定の質、shadow ケース（子 0 の E-mid）は後続の WU（shadow-case）と子 6・子 8 の範囲で、この WU では確かめていない。

## 提案

- task-core の store に `cos_operations` の `task.create`・`applied`・`payload_json.strategy` を読む読み取り専用の関数（`with_read_conn`）を置き、gate の
  `write_tx` の SELECT を置き換える。子 4 が `crates/task-core/src/chat/` を書き終えた後に、子 5 の close か子 6 で行う。

## 親の目的への寄与（確認済み・推定・未確認）

- 確認済み（試験）: 改善 A の「最初から巨大な固定計画を走らせない」を実行の入口で決定的に止める。実験段階の版からは open の候補ごとの atomic の識別実験
  （最大 3 件）しか起票できず、実験が全部終わり、その結果を全部証拠で参照し、Critic を通った版だけが compound の本実装を起票できる。却下した候補には戻れない。
- 推定: アーキv4 型の「子 A〜H を最初に全部起票」は、CoS が strategy を付けて起票する限り `strategy_experiment_phase` で止まる。strategy を付けない起票は
  止まらない（通常の低抽象度 task の経路を変えない設計。ADR D5・D6）ので、抽象的な依頼で CoS が strategy を使うかは CoS の手順（cos-skill）と子 6・子 8 で確かめる（未確認）。
