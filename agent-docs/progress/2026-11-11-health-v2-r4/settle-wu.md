---
title: "健全性v2 R4 settle-wu: settle に WU の run の判断を足し、execution_scheduler::decide を run_settlement から呼ぶ"
tasks: [01M7B2KBNJ72C5F75RAEWSFECG]
status: done
updated: 2026-11-13
completed: 2026-11-13
---
# 健全性v2 R4 settle-wu

## 概要

- ADR 2026-11-11 dispatcher-state-ownership D3 の WU の段を `crates/task-dispatch/src/run_settlement/work_unit.rs` に置いた
  （`run_settlement.rs` から `pub(crate) use` で出す）。`finish_worker_result_with` の WU の分岐は、読み出しと提案の適用だけになった。
- `settle_work_unit(WorkUnitRunInput) -> WorkUnitSettlement` が決めるもの:
  - `[execution] continuation = false` の run（WU の行を変えず retryable な失敗にする。auto-leaf の上限を超えた leaf は除く）。
  - `WuLimits` の値（auto-leaf で測る leaf は continuation と進捗なしの上限を外す）と、`crate::execution_scheduler::decide(` の呼び出し。
    `WuLimits`・`WuDecision`・`ContinuationInputs` は `execution_scheduler` の型をそのまま使い、写していない。
  - auto-leaf の上限（遷移が `continue` のときだけ決定の要求を使い、leaf を `blocked(decision)` にする）。
  - merge の repair WU の退き方（旧 `dispatcher/phase_integration/merge_repair.rs` の `retire_if_failed`・`is_merge_repair`・`FAILED_REASON`
    を移し、旧い定義を消した。`merge_repairs_of` は残る）。
  - 遷移の理由ごとの outcome の文と v1 の task のクラスタ job の wait の trigger。
  - integration repair の打ち切りの理由（失敗・進まない・計画の問題・信頼できない done）。
  - WU の失敗を replan の判定に回すか（置き違えた `result.json` の run は回さない）と、失敗の署名。
- 後段の純粋関数（判断の結果で読むものや副作用が変わる所）:
  - `settle_work_unit_failure(&mut WorkUnitSettlement, ReplanLimits) -> WorkUnitFailureStep`: `replan_or_ask` を呼び、自動 replan
    （`Continue{replan}` と `AutoReplanRequested`）・stalled の質問・使い切りの後の確認（`AskExhausted`）・plan_issue の hint を決める。
    `ReplanLimits` は `worker_finish.rs` が `replan_limits` で読む（失敗の署名が `settle_work_unit` の結果で決まるため、提案の
    `failure` を見て読む）。
  - `integration_repair_exhausted(key, fallback)`: 打ち切りの後の trigger と outcome の文。
  - `hold_for_worker_decisions`: `needed_before: self` の決定の要求で leaf を止める wu_update。`WorkUnitSettlement::reads_worker_decisions`
    が読むかを返す。
  - `work_unit_run_end`（end の無い run を harness_error 相当にする）と `work_unit_saves_checkpoint`（checkpoint を組み立てる条件）。
- `worker_finish.rs` に残したもの（読み出しと副作用、今と同じ順）: auto-leaf の回数（events）、checkpoint の組み立て（git・checkpoint.json・events）、
  WU の行（`work_units_for`）、auto-leaf の決定の要求（`auto_leaf_decision`。上限を超えた leaf だけ）、integration repair の結果の信頼
  （integration repair の WU だけ）、`integration_repair_exhaustion`（git の巻き戻し）、`replan_limits`、`replan_exhausted_ask`（木の節点の
  決定の要求の追記）、完了の要約（`run_index_get`）、`worker_decisions`（result.json）。WU の commit・遷移・依存 WU の昇格・write set・
  run index の終了の書き込み・`test_halt_after_wu_transition` の位置・transaction の範囲は変えていない。
- `worker_finish.rs` から `execution_scheduler::decide` の直接の呼び出しと `WuLimits` の組み立てを消した。

## 実装前 brief

1. 根本原因: WU の run の遷移の結果の使い方（上限の値・auto-leaf・merge の repair・integration repair・replan か質問・決定の保留）が
   `finish_worker_result_with`（`Dispatcher` の method）の中で store・git の読み出しと副作用に混ざり、入力から提案を単体で試せない。
2. 責務 owner: 判断は `run_settlement`（D3 の表）。WU の遷移そのものの正本は `execution_scheduler::decide`。適用は `worker_finish.rs`。
3. 既存の再利用先: `execution_scheduler::{decide, WuLimits, WuDecision, ContinuationInputs}`、`run_settlement::{replan_or_ask, ReplanLimits,
   describe_run_end}`、`task_ops::plan_gate::{failure_signature, replan_stalled_question}`。
4. 選択肢: (a) `settle` 1 つに WU の入力も入れる（checkpoint の組み立てと auto-leaf の回数が `settle` の `run_end` で決まるので、先に読めない）、
   (b) 同じ module に WU の段の関数を置き、判断の結果で読むもの・副作用が変わる所は提案の欄を見て適用側が読み、後段の純粋関数に渡す。
   (b) を採った（Objective の「提案で必要を返して適用側が読む形でもよい」）。auto-leaf の決定の要求と integration repair の信頼は、
   読む条件（上限を超えた・integration repair の WU）が遷移の前に分かるので先に読む（どちらも読み出しだけで書き込みは無い）。
5. 新しい状態・分岐・fallback・port: 無し。
6. 撤去対象: `worker_finish.rs` の WU の分岐の判断の本体（`WuLimits` の組み立て・`decide` の呼び出し・理由ごとの outcome・auto-leaf の
   書き換え・integration repair の理由・replan の分岐・決定の保留の書き換え）と `replan_or_ask`・`AskReason`・`ReplanOrAsk` の import、
   `merge_repair.rs` の `retire_if_failed`・`is_merge_repair`・`FAILED_REASON`。
7. 不変条件: 既存の WU の試験と cex_ の試験が同じ assertion で通る（test-affected）。WU の分岐の境界は下の試験。

## 読み出しの順の変化（書き込みの順は同じ）

- auto-leaf の決定の要求（`auto_leaf_decision`: decision path の読み出しと ULID の生成）は、遷移の前に「上限を超えた leaf」で読む。旧は
  遷移が `continue` のときだけ読んだ。遷移が `continue` でなければ要求は使わず捨てる（記録しない）。
- integration repair の結果の信頼（`integration_repair_result_trusted`: events と worktree の git の読み出し）は、integration repair の WU
  なら遷移の前に読む。旧は遷移が `completed` のときだけ読んだ。どちらも打ち切り（git の巻き戻し）より前に読む。
- WU の行（`work_units_for`）は `[execution] continuation = false` の run でも読む（使わない）。

## 足した試験（`crates/task-dispatch/src/run_settlement/work_unit/tests.rs`）

守る不変条件と既存の試験で検出できない理由は各試験の doc comment に書いた。

- `continuation_disabled_keeps_the_unit_unless_the_auto_leaf_limit_stops_it`
- `auto_leaf_lifts_the_progress_limit_and_only_holds_a_continuing_leaf`
- `a_failed_merge_repair_retires_without_replanning`
- `integration_repair_exhausts_on_failure_or_an_untrusted_completion`
- `a_work_unit_failure_replans_asks_or_hints`
- `worker_decisions_hold_a_completed_leaf_only`
- `checkpoints_are_saved_for_continuations_and_cluster_waits`

## 証拠（提出時の HEAD で実行。基準は `git merge-base HEAD main`、範囲は `$CELERIS_WU_BASE`）

- `bash scripts/dev/test-affected.sh`: TEST_AFFECTED_RESULT
- `cargo clippy --workspace -- -D warnings`: exit 0。`cargo fmt --all -- --check`: exit 0。
- `bash scripts/dev/check-health.sh`: exit 0。verdict=pass、fail_grade=0、worsened=0、new=1。new の 1 件は `migration.remnant`（warning）で
  `crates/task-dispatch/src/host_maintenance.rs` の `legacy` の行（この unit は変えていない。settle-end の進捗と同じ）。
- `cargo nextest run -p task-dispatch -E 'test(run_settlement) | test(cex_) | test(structure_guard_)'`: exit 0、53 passed（settle-end の 46 に
  この unit の 7 本）。cex_ の試験と ADR は変えていない（計画の check の `git diff --quiet "$CELERIS_WU_BASE" -- …` は exit 0）。
- 計画の check（`execution_scheduler::decide(` を run_settlement が呼び、型と関数を写さず、`worker_finish.rs` が `decide` を直接呼ばず、
  run_settlement が `replan_or_ask(` を呼ぶ）: exit 0。純粋さの grep: exit 0。
- 範囲の check（`sh "$CELERIS_WU_SCOPE_PATHS"` に計画の正規表現を当てる）: exit 0。`sh scripts/dev/progress-index.sh --check`: exit 0。
- `dispatcher/` の行: `worker_finish.rs` は 318 行減・133 行増、`merge_repair.rs` は 44 行減・3 行増。`dispatcher.rs` は module map の
  run_settlement の行を同じ 1 行の中で直した。`Dispatcher` の method は足していない。

## 未解決事項

- 無し。

## 提案

- settle-task: atomic task の分岐（`no_progress_question`・`continuation_cap_question` を使う continuation）を同じ形で run_settlement に移すと、
  `worker_finish.rs` に残る `execution_scheduler::` の参照は `ContinuationInputs` の組み立てと段の進み方（`settle_phase`）だけになる。
- `WorkUnitSettlement` の完了の要約（`subject.summary`）は `run_index_get` を読むので `worker_finish.rs` に残した。settle-task で reviewer に
  渡す内容をまとめるなら、各 unit の `completed` を入力に読んで純粋関数にできる。
