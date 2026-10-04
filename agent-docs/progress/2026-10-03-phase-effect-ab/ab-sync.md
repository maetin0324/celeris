---
task: phase-effect-ab
wu: ab-sync
status: done
completed: 2026-10-03
---
# phase_effect_ab: scenario review_sync（ab-sync）

## やったこと
- `crates/task-dispatch/src/dispatcher/tests/phase_effect_ab/review_sync.rs` に scenario review_sync を実装
  （`phase_effect_ab.rs` は不変。`super::{AbMetric, Variant, print_ab_metric}` を使う）。
  - 一時 git repo（`init_test_repo`）の初回 commit を古い base とし、A/B の worktree を切って同じ `README.md` を変える。
  - A を review → 統合（試験内の `git merge --no-ff`）で main へ入れてから、B を store に入れて流す。
  - 偽アダプタ `SyncScenarioAdapter`（id `instant`）: reviewer run は合格の `review.json`、repair run は B の worktree を
    main へ rebase して衝突を `task A\ntask B\n` で解く。新 session=40000 token、resume=4000 token。
    時計は偽アダプタが進める `SimClock`（reviewer 90 秒・repair 150 秒）。
- pre-review sync の gate が無かったので、`Dispatcher` に `#[cfg(test)] test_skip_pre_review_sync: bool`（既定 false）と
  `pre_review_sync_enabled()`（review_spawn.rs、本番は常に true）を足し、review 前同期のループの条件に入れた。

## 模擬した部分
- 統合（main への取り込み）は `crates/celeris` の配送が持ち task-dispatch に無いので、試験内の `git merge --no-ff` で模擬した。
- off の統合衝突の後は既存の入口を dispatcher に通す: `Trigger::Rereview`（Done → Reviewing、ADR-0051）を
  `store.apply_transition` で適用し、`Dispatcher::try_integration_repair`（IntegrationRepair の repair WU、Reviewing → Ready）を呼ぶ。
  以後の repair run と再 review は `run_until_idle` の dispatcher が走らせる。
- implement run は両 variant に共通なので含めず、task は reviewing から始める（target_sync.rs と同じ組み立て）。

## 証拠
- `cargo test -p task-dispatch --lib phase_effect_ab::review_sync -- --nocapture` → exit 0、1 passed
  ```
  ab-metric review_sync off runs=4 wall_secs=420 input_tokens=160000 fresh_sessions=4
  ab-metric review_sync on runs=3 wall_secs=330 input_tokens=120000 fresh_sessions=3
  ```
  off = A review + B review（古い base）+ 統合後 repair + B 再 review、on = A review + B review 前 repair + B review。
  `assert!(on.runs < off.runs)`、wall_secs・input_tokens も on < off を assert。
- `cargo clippy -p task-dispatch --all-targets -- -D warnings` → exit 0
- `cargo test -p task-dispatch --lib` → 583 passed, 0 failed
- `cargo test --workspace` はこの葉では未実行（範囲は task-dispatch のみ。段の統合で走る）。

## 未解決事項
- review・repair run はどれも新しい session なので fresh_sessions は runs と同じ。resume の差分 token はこの scenario では出ない。

## 提案
- 本番に pre-review sync の切替は要らない（cfg(test) のみ）。配送側の統合衝突 → repair を task-dispatch から通す試験入口が
  あれば、off の模擬（Rereview + try_integration_repair の直呼び）を置き換えられる。
