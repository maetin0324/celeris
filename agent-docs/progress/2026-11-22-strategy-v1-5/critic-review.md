---
tasks: [01M7XQ9QQXY1DY56EXE0TT8X6W]
wu: critic-review
status: running
---
# 戦略層 v1 子 5 critic-review: final review の Critic（ADR 2026-11-22-strategy-review-critic D3）

## 状態（2026-11-22、run 01M818GEGNKBMZ4WK8T5SFKYNY で区切った）
- 実装と試験は commit 済み（件名「strategy_critic_ の試験を先に置く」「final review に Critic（暗黙の条件 strategy_critic）を足す」）。
- 未了: `bash scripts/dev/test-affected.sh` が repo health の size.responsibility-growth で exit 1（下）。試験の実行まで進まない。

## 変更前に落ちた出力（試験を先に commit した HEAD で実行）
`cargo nextest run -p task-core -p task-worker -p task-dispatch -E 'test(/strategy_critic_/)'` → exit 101（compile で落ちる）:
          1 error: command `/home/rmaeda/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo '--color=auto' test --no-run --message-format json-render-diagnostics --package task-core --package task-worker --package task-dispatch` exited with code 101
          1 error: command `/home/rmaeda/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo '--color=auto' test --no-run --message-format json-render-diagnostics --package task-dispatch --lib` exited with code 101
          1 error: command `/home/rmaeda/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo '--color=auto' test --no-run --message-format json-render-diagnostics --package task-worker` exited with code 101
          1 error: could not compile `task-core` (lib test) due to 7 previous errors; 1 warning emitted
          1 error: could not compile `task-dispatch` (lib test) due to 7 previous errors
          1 error: could not compile `task-worker` (lib test) due to 12 previous errors
          1 error[E0422]: cannot find struct, variant or union type `StrategyCriticRequest` in module `crate::protocol`
          1 error[E0422]: cannot find struct, variant or union type `StrategyCriticSubject` in module `crate::protocol`
          2 error[E0425]: cannot find function `critic_section` in this scope
          5 error[E0425]: cannot find function `critic_subjects` in this scope
          1 error[E0425]: cannot find type `StrategyCriticRequest` in module `crate::protocol`
          2 error[E0425]: cannot find value `CRITIC_HEADING` in this scope
          1 error[E0425]: cannot find value `CRITIC_RUBRIC` in module `task_core::strategy_critic`
          2 error[E0425]: cannot find value `CRITIC_RUBRIC` in this scope
          1 error[E0432]: unresolved import `crate::review::reviewer_run_required`
          1 error[E0432]: unresolved import `task_worker::protocol::Terminal`
          1 error[E0432]: unresolved imports `task_core::strategy_critic::CriticSubject`, `task_core::strategy_critic::critic_subjects`
          1 error[E0433]: cannot find `StrategyCriticRequest` in `protocol`
          2 error[E0560]: struct `ReviewRequest` has no field named `strategy_critic`
          2 error[E0560]: struct `review::ReviewExtras` has no field named `critic_subjects`
          1 error[E0609]: no field `critic` on type `ReviewOutput`
          2 error[E0609]: no field `strategy_critic` on type `task_worker::ReviewRequest`
回帰の試験 `strategy_critic_absent_prompt_unchanged` は base の prompt の sha256（0f7107b4…b5e0）を base で採った値で固定した（base で同じ関数を走らせて得た）。

## 変更後
- `cargo nextest run -p task-core -p task-worker -p task-dispatch --lib -E 'test(/strategy_critic_/)'` → exit 0、13 tests run: 13 passed。
- `cargo clippy --workspace -- -D warnings` → exit 0。`cargo fmt --all -- --check` → exit 0。
- `bash scripts/dev/test-affected.sh` → exit 1（repo-health: new=1 worsened=3）:
  - dispatcher module の本体の行の合計の増加（review_spawn.rs）、`review_task` 278 行・`run_reviewer_inner` 210 行・`spawn_review` 736 行が閾値 200 を超えて増えた。

## 未解決
- size の増加を消す: Critic の分岐を `review/critic.rs` に移し、`review_task`・`run_reviewer_inner`・`spawn_review` の本体の行を base 以下にする。
- ADR D3.4 の `experiments[]`（前の版の実験 task の記録）は入れていない。X(v) は `cos_operations` の照会が要り、dispatcher から引く store の関数が無い（task-core の store・chat は critic-review の write set の外）。提案: experiment-gate か close で、照会の置き場所を決めて足す。
