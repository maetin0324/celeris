---
title: 戦略層 v1 子 5 critic-review（final review の Critic）
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
wu: critic-review
status: blocked
updated: 2026-11-22
---
# 戦略層 v1 子 5 critic-review: final review の Critic（ADR 2026-11-22-strategy-review-critic D3）

## 状態（2026-11-22、run 01M81BBW37W6NCNPM746SZ891V）
- 実装・試験・size の超過の解消は commit 済み。test-affected・clippy・fmt・strategy_critic_ の試験は提出時の HEAD で通る（下）。
- 止まっている理由: 計画の範囲 check が `crates/task-core/src/strategy_critic.rs`・`crates/task-core/src/lib.rs`・`crates/task-dispatch/src/dispatcher.rs` を
  許さない。ADR D8 の write set は前の 2 つを critic-review に割り当てている（D3.1・D3.4 も `task_core::strategy_critic` と書く）。計画の不備として
  plan_issue を申告した。

## commit（件名）
- strategy-v1 5 critic-review: strategy_critic_ の試験を先に置く（実装前は compile で落ちる）
- strategy-v1 5 critic-review: 戦略の文書を作った task の final review に Critic（暗黙の条件 strategy_critic）を足す
- strategy-v1 5 critic-review: WU の進捗（変更前の失敗・残る size の超過）
- strategy-v1 5 critic-review: 判定されなかった reviewer の条件と Critic の不合格を 1 つの関数にし、review_task・run_reviewer_inner・spawn_review の行を base 以下に戻す
- strategy-v1 5 critic-review: 進捗・ADR の付記（experiments[] を入れない理由、条件のまとめ方）

## 何を足したか
- `task_core::strategy_critic`: `critic_subjects(events)`（`ArtifactProduced` の `strategy/<id>/v<n>.json` から id ごとに最大の版。名前は
  `task_core::strategy::artifact_name` で作り直して一致したものだけを採る）、固定の基準 `CRITIC_RUBRIC`（C1〜C5）。
- `task_worker::protocol`: `ReviewRequest.strategy_critic`（任意。対象の無い task では key ごと出ない）、`ReviewOutput.critic`（任意）。schema を再生成した。
- `task_worker::change_rules::review::critic_section`: `strategy_critic` が `Some` のときだけ Critic の節を prompt に 1 回出す。`None` なら空文字列。
- `task_dispatch::review`: 暗黙の条件 `strategy_critic` を `research_evidence` の次の番号に足す（`review/critic.rs` の `Critic`）。合否は reviewer の同じ run の
  `review.json` の `critic` の節だけで決まる。節が無い・壊れている・対象や候補が文書と違う・理由が空か長すぎる・`ok: false`・対象の衝突・文書の書き換えは不合格。
  決定的な条件が落ちた・reviewer run が無い・run が判定できなかったときも不合格（`review::not_evaluated`）。reviewer の合格の持ち越しは Critic を覆わない。
- `task_dispatch::dispatcher::review_spawn`: `reviewer_run_required(&task, auto_integrates, &critic_subjects)` で reviewer run を起こす。対象がある task は持ち越しを使わない。
  別の reviewer・別の run・dispatcher の LLM 呼び出しは足していない。

## 変更前に落ちた出力（試験を先に commit した HEAD で実行）
`cargo nextest run -p task-core -p task-worker -p task-dispatch -E 'test(/strategy_critic_/)'` → exit 101（compile で落ちる）:
          1 error: could not compile `task-core` (lib test) due to 7 previous errors; 1 warning emitted
          1 error: could not compile `task-dispatch` (lib test) due to 7 previous errors
          1 error: could not compile `task-worker` (lib test) due to 12 previous errors
          1 error[E0422]: cannot find struct, variant or union type `StrategyCriticRequest` in module `crate::protocol`
          1 error[E0422]: cannot find struct, variant or union type `StrategyCriticSubject` in module `crate::protocol`
          2 error[E0425]: cannot find function `critic_section` in this scope
          5 error[E0425]: cannot find function `critic_subjects` in this scope
          2 error[E0425]: cannot find value `CRITIC_HEADING` in this scope
          1 error[E0425]: cannot find value `CRITIC_RUBRIC` in module `task_core::strategy_critic`
          1 error[E0432]: unresolved import `crate::review::reviewer_run_required`
          1 error[E0432]: unresolved imports `task_core::strategy_critic::CriticSubject`, `task_core::strategy_critic::critic_subjects`
          2 error[E0560]: struct `ReviewRequest` has no field named `strategy_critic`
          2 error[E0560]: struct `review::ReviewExtras` has no field named `critic_subjects`
          1 error[E0609]: no field `critic` on type `ReviewOutput`
          2 error[E0609]: no field `strategy_critic` on type `task_worker::ReviewRequest`
（全文は run の artifacts の before*.log）。回帰の試験 `strategy_critic_absent_prompt_unchanged` の期待値は、base で同じ関数を走らせて得た prompt の sha256 を
試験の定数に固定したもの。

## 試験（13 件、prefix `strategy_critic_`）
- task-core（4）: 対象の導出（全 run の最新の版、成果物の無い task は空、同じ版の 2 つの sha256 は衝突、名前の形の判定）。
- task-worker（3）: `strategy_critic_section_appears_once`（節が 1 回だけ）、`strategy_critic_absent_prompt_unchanged`（対象の無い task の prompt が base と同じ
  byte 列。回帰）、`strategy_critic_absent_request_json_unchanged`（`request.json` に key が無い。回帰）。
- task-dispatch（6）: `strategy_critic_missing_section_fails_review`（Critic の判定の無い review は不合格）、`strategy_critic_all_ok_passes`、
  `strategy_critic_subject_sha_mismatch_or_candidate_missing_fails`、`strategy_critic_not_evaluated_when_deterministic_fails`、`strategy_critic_reuse_not_applied`、
  `strategy_critic_absent_reviewer_run_count_unchanged`（対象の無い task の reviewer run は `Check::Reviewer` が無ければ 0 回、あれば 1 回。回帰）。
- 既存の試験で検出できない理由: 既存の review の試験は暗黙の条件 `strategy_critic` も `ReviewRequest.strategy_critic` も持たず、Critic の不合格・節の重複・
  対象の無い task の prompt の byte 列の不変を見ない。

## 変更後（提出時の HEAD で実行）
- `cargo nextest run -p task-core -p task-worker -p task-dispatch -E 'test(/strategy_critic_/) | test(/^review::/)'` → exit 0、50 tests run: 50 passed。
- WU の check の数え方 `cargo nextest list -p task-worker -p task-dispatch | grep -cE 'strategy_critic_[a-z0-9_]*$'` → 9（task-core の 4 件は数えない）。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → 結果は下の「test-affected」。
- 範囲 check（計画の `"scope": true` の command）→ out of scope: `crates/task-core/src/lib.rs`・`crates/task-core/src/strategy_critic.rs`・`crates/task-dispatch/src/dispatcher.rs`。

### size の超過の解消（repo health の size.responsibility-growth）
| 単位 | base | Run #1 | 提出時 |
|---|---|---|---|
| `Dispatcher::spawn_review`（関数の行） | 725 | 736 | 725 |
| dispatcher module の `review_spawn.rs` の本体の行 | 1434 | 1442 | 1434 |
| `review::review_task`（関数の行） | 252 | 278 | 245 |
| `review::run_reviewer_inner`（関数の行） | 199 | 210 | 198 |

同じ「判定されなかった条件を不合格にする」組み立てが `review_task` に 1 か所、`run_reviewer_inner` の `fail_all` に 1 か所あったので、`review::not_evaluated` 1 つに
した。持ち越しの回は `review::without_reviewer_run`。delivery の条件は `reviewer_run_required` に寄せた（D3.2 の条件そのもの）。

### test-affected
TEST_AFFECTED_RESULT

## LoC（`source-size-report.py --exclusive-classes`、script と toml は base の commit から取り出して base と HEAD に当てた。base は `$CELERIS_WU_BASE` の段の head）
| view | base との差 |
|---|---|
| production（全言語） | +528（すべて Rust） |
| 試験（inline +4、external +547） | +551 |
| own_total_with_tests | +1,079 |
| 生成物 `docs/protocol/worker-protocol.schema.json` | +194 |

純増の理由: Critic の対象の導出・wire の 2 つの任意の節・prompt の節・暗黙の条件の判定を足した。新しい Manager・trait・状態機械・DB 表・event の種類・migration は無い。

## 未解決
- 範囲 check と ADR D8 の食い違い（上）。計画の改訂で範囲 check に `crates/task-core/src/strategy_critic(\.rs$|/)`・`crates/task-core/src/lib\.rs$`・
  `crates/task-dispatch/src/dispatcher\.rs$` を足せば、この WU は check の再実行だけで終わる。
- ADR D3.4 の `experiments[]`（前の版の実験 task の記録）は入れていない。X(v) は `cos_operations` の照会で、照会の関数はまだ無く、dispatcher から引くには
  `task_core` の store（高 risk の class `store`）に足す必要がある（ADR の付記）。

## 提案
- `experiments[]` の照会は `task_core` の store に 1 つ置き、experiment-gate の X(v) と review の `experiments[]` の両方がそれを使う（同じ照会を二重に書かない）。
  置く WU は計画の改訂で決める。
