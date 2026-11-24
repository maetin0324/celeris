---
title: planner の run の供給側の失敗を計画の不採用に数えず account を cooldown にする（WorkUnit planner-supply）
tasks: [01M87M20BAB5C21EVARQ7BWCCN]
status: blocked
updated: 2026-11-24
---
# planner の run の供給側の失敗を計画の不採用に数えず account を cooldown にする（WorkUnit planner-supply）

状態: blocked（WorkUnit の範囲の check が `crates/task-ops/src/tree.rs` を許さない。下の「未解決」の 1 つめ。計画の直しを
plan_issue で申告した）。設計は [ADR 2026-11-24-planner-supply-failure-cooldown](../../adr/2026-11-24-planner-supply-failure-cooldown.md)
の D1〜D4・D7（1〜5）。基準は `$CELERIS_WU_BASE`（この WorkUnit の base）。

## commit（件名）

- planner: 供給側の失敗で終わった planner run を計画の試行に数えず account を cooldown にする
- planner: 供給側の失敗の扱いを free 関数に寄せ、計画の検証を on_planner_finished の外に出す
- planner: 計画の再試行の文面と planner の出力の読み取りを task_ops に移し、供給側の失敗の扱いを詰める
- 試験: 設計 review の run の認証の失敗は cooldown にし判定に流さない
- planner: planner の出力の読み取りを task_core::execution_plan に移し、task_ops::execution を base に戻す
- progress: planner-supply の WorkUnit の結果

## 変えたこと

- planner（`dispatcher/planner_flow.rs`）: run の結果が `provider_failure_outcome` で供給側の失敗（AuthFailed・Throttled・
  Exhausted・起動の失敗）なら、`planner_supply_failure` が worker と同じ `record_supply_failure` で account（pool）か
  source を cooldown にする。AuthFailed は常に、Throttled・Exhausted は `consecutive_requeues` が `max_requeues` 未満の間、
  計画の試行に数えず `WorkerFinished.end = HarnessError{Supply}`（`task_ops::tree::SUPPLY_FAILED_END`）・`Trigger::Requeue`・
  進捗「planner の run が供給側の失敗で終わりました（…）」で閉じる。上限に達した Throttled・Exhausted は理由
  「requeue limit (N) reached: adapter: …」で従来の不正な試行の経路（`give_up_or_retry_planner`）に入る。
  旧い経路（adapter の `Err` をすべて「planner run did not finish cleanly: infra error: …」として数える）は供給側の失敗では
  通らない。供給側でない `Err`（Io・Serde・Other）と `Ok` の異常終了は従来どおり数える。
- 数え方（`task_ops::tree`）: `planner_attempts_in_window` は `planner_run_supply_failed` の run を数えない。
  `planner_retry_pending` は供給側の失敗で終わった planner の `WorkerFinished` を見て真になる（replan の planner を起こし直す。
  replan の依頼は 1 回目の dispatch で消費済みのため）。真実は events で、新しい列・カウンタは無い。
- 共有の関数（`dispatcher/route_failure.rs`）: `record_supply_failure`（pool の account は `record_account_failure`、
  pool を使わない source と起動の失敗は `record_source_failure`）。worker（`worker_finish.rs`）・planner・side review・
  task の final review（`review_verdict.rs` の同じ分岐を置き換えた）が呼ぶ。AuthFailed の `AUTH_FAILED_HOLD_SECS` の保持、
  `notify_account_auth_failed` の bad_news、`GET /api/v1/llm/accounts` の `cooldown.reason = auth_failed` は既存の経路のまま。
- side review（`health_review/design_gate.rs` の `on_side_review_finished`）: 供給側の失敗なら同じ関数で cooldown にし、
  `end = HarnessError{Supply}` にして判定（`settle_design_review`・`parent_evaluation::settle`）に流さない。
- repo-health の `size.responsibility-growth`（dispatcher module の本体の行、`Dispatcher` の method 数、
  `on_planner_finished` の行）を悪化させないため:
  - planner の再試行・拒否の文面と、それを events から読む関数（`planner_retry_message`・`planner_blocked_question`・
    `planner_rejections_since_last_plan`・`plan_invalid_answer_position`・`plan_invalid_replan_note`、`PLANNER_BLOCKED_*`）を
    `task_ops::tree` に移した。同じ文面を読む `planner_retry_pending` と `PLANNER_RETRY_*` は元からここにある。
    dispatcher 側の別名の定数（`planner_flow::PLANNER_RETRY_PREFIX/SUFFIX`）は消した。
  - `parse_planner_output`（planner の出力を `apply_delta`・`carry_done_units_v3` で読む純関数）を `task_core::execution_plan`
    に移した（差分の schema を持つ module。2 回目の run で `task_ops::execution` から移し直し、`crates/task-ops/src/execution.rs` は
    base に戻した）。
  - 計画の file の検証を `planner_plan_validation`（free 関数）に出した。`record_supply_failure` は free 関数にした。
  - dispatcher module の本体の行: base 32202 → HEAD で base より 9 行少ない（`check-health.sh` の verdict=pass、worsened=0）。

## 試験（守る不変条件と、既存の試験で検出できなかった理由）

既存の planner の試験は pool の account を使わず、adapter の `Err` で終わる planner run を作らない。既存の認証の失敗の試験
（`account_auth_failure.rs`）は worker の run だけを見る。side review の試験は run を `Done` でしか終わらせない。

`crates/task-dispatch/src/dispatcher/tests/planner_supply.rs`（偽の adapter・account a/b の pool・時計を `NOW` に固定）:

1. `planner_auth_failure_is_not_a_plan_attempt_and_the_next_planner_runs_on_another_account`: a の planner run が
   "Failed to authenticate …" の `AuthFailed` → `end = Supply`、計画の拒否の進捗 0、`plan_invalid` 0、attempts 0。
   a は `auth_failed` の cooldown（until = NOW + `AUTH_FAILED_HOLD_SECS`）・`logged_in = false`・`excluded_reason = cooldown`、
   bad_news 1 件。次の planner run は b で、計画が採用される。修正前は 2 本とも a に回り `plan_invalid` が出た。
2. `planner_throttle_and_exhaustion_are_not_plan_attempts_and_cool_the_account_down`: Throttled（retry_after 120 秒）は
   until = NOW + 120、Exhausted は `fallback_cooldown_secs` の NOW + 300。どちらも数えず、次は b。
3. `planner_throttle_at_the_requeue_limit_is_counted_as_an_invalid_attempt`: `max_requeues = 0` では Throttled を
   「requeue limit (0) reached」の不正な試行に数える（拒否の進捗 1）。cooldown は付き、次の試行の b で採用。
4. `planner_with_no_usable_account_waits_for_a_relogin_without_plan_invalid`: a・b とも認証の失敗 → planner run は a・b に
   1 本ずつ、その後は起こさない。task は `ready`・attempts 0・`planner_attempts_in_window = 0`・`plan_invalid` 0。人に出るのは
   account ごとの「再ログインが必要」の bad_news 2 件、一覧の `auth_failed`、task の timeline の「route refused: no declared
   route candidate … is eligible」（除外理由 `Cooldown`、保持中は 1 回だけ）。a の再ログイン（`record_account_login`）の後は
   a で計画が採用される。

`crates/task-dispatch/src/dispatcher/tests/reviewer_side_run.rs`:

5. `reviewer_side_run_supply_failure_cools_down_and_reads_no_verdict`: 設計 review の run が `AuthFailed` → `DesignReviewed`
   なし、`WorkerFinished.end = Supply`、source の cooldown（`ProviderThrottled{reason: auth_failed}`）。

## 証拠（2 回目の run の提出時の HEAD で実行。基準は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh` → exit 0。cargo nextest 8912 passed（34 skipped）、web の tsc -b と vitest 108 files /
  773 tests passed、test-ts-verify 20 passed、repo-health verdict=pass（fail_grade=0・new=0・worsened=0）。
- `cargo clippy --workspace -- -D warnings` → exit 0。
- `cargo fmt --all -- --check` → exit 0。
- WorkUnit の範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` の path を許す接頭辞で絞る command）→ exit 1、範囲外は
  `crates/task-ops/src/tree.rs` の 1 件（1 回目の run は `execution.rs` と `tree.rs` の 2 件）。

## ADR の Context の訂正

- ADR は「side review は `review.json` が無いことを不合格として数える」と書いたが、`settle_design_review` は `Done` 以外の
  run を元から判定に流さない（次の dispatch が起こし直す）。設計 review の穴は cooldown が付かないことだけだった。

## 未解決

- WorkUnit の範囲の check が `crates/task-ops/src/tree.rs` を許さない。replan の planner run は起動の時点で replan の依頼
  （`ExecutionHintSet{replan}`・途中確認の replan・「もう一度だけ試します」の印）を消費するので、供給側の失敗で終わった run の
  後に planner を起こし直すには `task_ops::tree::planner_retry_pending` が `end = HarnessError{Supply}` の planner の
  `WorkerFinished` を見て真を返す必要がある。この関数は dispatch（`wu_dispatch_gate`）と生存確認（`node_liveness_facts` の
  `planner_pending`）の両方が読む owner で、task-dispatch に同じ判定を別に書くと、再ログイン待ちの間に生存確認が節点を
  停滞と誤って分類する。範囲に `^crates/task-ops/src/tree\.rs$` を足すよう plan_issue で申告した。
- 親の途中評価（`parent_evaluation`）の試行の数え方は確かめていない（未確認）。供給側の失敗では `settle` を呼ばない形にした。
- 使える account が無いとき、`task_waits`（GUI の待ちの理由）には理由が出ない。人に見えるのは bad_news・account の一覧・
  task の timeline の「route refused」。待ちの理由に出すかは提案に書く。
- eval trial・report の run の供給側の失敗の扱いは確かめていない（ADR D4 のとおり未確認）。
- 再起動の後の扱いの試験（ADR D7-8・9）は WorkUnit restart-close が足す。

## 提案

- route の選択が「全 account が cooldown」で断ったとき、`task_waits` に `Capacity` か専用の理由を載せ、GUI の task の
  待ちの理由に「再ログイン待ち」と出す。
