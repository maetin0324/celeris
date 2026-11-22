---
title: 戦略層 v1 子 5 critic-review（final review の Critic）
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
wu: critic-review
status: done
updated: 2026-11-22
---
# 戦略層 v1 子 5 critic-review: final review の Critic（ADR 2026-11-22-strategy-review-critic D3）

## 状態（2026-11-22、run 01M81DXF1ZJEQ31B1P1VHTQDDG）
- 実装・試験・size の超過の解消は前の run で commit 済み。WU の 6 つの check は提出時の HEAD ですべて exit 0（下の「変更後」）。
- 前の run（01M81BBW37W6NCNPM746SZ891V）は、範囲 check が `crates/task-core/src/strategy_critic.rs`・`crates/task-core/src/lib.rs`・
  `crates/task-dispatch/src/dispatcher.rs` を許さず plan_issue で止まった。計画 v2 で範囲 check が直り、この run の再実行で out of scope は 0 件。
- 計画 v2 の決定を ADR に反映した: D3.4 の入力から前の版の実験 task の記録の欄を外し（実装しない）、C4・C5 を「文書の evidence が参照する task を
  読み取り専用の DB で読み、実験の結果と失敗を確かめる」形に直した。`CRITIC_RUBRIC` の C4・C5、D3.6 の文、付記の未決の行も同じ意味に直した。

## commit（件名）
- strategy-v1 5 critic-review: strategy_critic_ の試験を先に置く（実装前は compile で落ちる）
- strategy-v1 5 critic-review: 戦略の文書を作った task の final review に Critic（暗黙の条件 strategy_critic）を足す
- strategy-v1 5 critic-review: WU の進捗（変更前の失敗・残る size の超過）
- strategy-v1 5 critic-review: 判定されなかった reviewer の条件と Critic の不合格を 1 つの関数にし、review_task・run_reviewer_inner・spawn_review の行を base 以下に戻す
- strategy-v1 5 critic-review: 進捗・ADR の付記（experiments[] を入れない理由、条件のまとめ方）
- strategy-v1 5 critic-review: WU の進捗（提出時の HEAD の test-affected の結果）
- strategy-v1 5 critic-review: ADR D3.4 から前の版の実験の記録の欄を外し、C4・C5 を文書の evidence の task の参照から読む形にする
- strategy-v1 5 critic-review: WU の進捗（計画 v2 の check の再実行）

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

## 変更後（提出時の HEAD で実行。計画 v2 の WU の check）
- 試験の数 `cargo nextest list -p task-core -p task-worker -p task-dispatch | grep -cE 'strategy_critic_[a-z0-9_]*$'` → 13（4 以上）、exit 0。
- `cargo nextest run -p task-core -p task-worker -p task-dispatch -E 'test(/strategy_critic_/)'` → exit 0、13 tests run: 13 passed、4702 skipped。
- D3.4 の節の check（`awk '/^#### D3\.4/{f=1;next} /^#### D3\.5/{f=0} f'` の出力に `experiments` が無い）→ exit 0。
- `bash scripts/dev/test-affected.sh && cargo clippy --workspace -- -D warnings && cargo fmt --all -- --check` → 3 つとも exit 0（下の「test-affected」）。
- 範囲 check（計画 v2 の `"scope": true` の command、`sh "$CELERIS_WU_SCOPE_PATHS"` の 18 path）→ out of scope の path は 0 件、exit 0。

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
`bash scripts/dev/test-affected.sh` → exit 0（提出時の HEAD で実行。基準は `git merge-base HEAD main`）:
- repo-health: verdict=pass、fail_grade=0、new=0、worsened=0（size.responsibility-growth・progress.index も pass）。
- cargo nextest（対象 14 crate）: 8655 tests run: 8655 passed（17 slow）、31 skipped。script bundle の SKIP 0 件。
- web（task-core の変更で走る）: tsc -b 通過、vitest 105 files・745 tests passed、node --test 110 passed・fail 0。test-ts-verify も ok。

## LoC（`source-size-report.py --exclusive-classes`、script と toml は base の commit から取り出して base と HEAD に当てた。base は `$CELERIS_WU_BASE` の段の head）
| view | base との差 |
|---|---|
| production（全言語） | +528（すべて Rust） |
| 試験（inline +4、external +547） | +551 |
| own_total_with_tests | +1,079 |
| 生成物 `docs/protocol/worker-protocol.schema.json` | +194 |

純増の理由: Critic の対象の導出・wire の 2 つの任意の節・prompt の節・暗黙の条件の判定を足した。新しい Manager・trait・状態機械・DB 表・event の種類・migration は無い。

## 未解決
- 未確認: reviewer が C4・C5 で証拠の task を読み取り専用の DB で実際に読むか（判定の質）は、この WU の試験では確かめていない（reviewer は stub）。
  ADR の「未確認と残る穴」のとおり子 6・子 8 の pilot で確かめる。前の版の終端の実験 task をすべて証拠が参照することは experiment-gate の D4.2 が保証する（未実装）。

## 提案
- 無し。
