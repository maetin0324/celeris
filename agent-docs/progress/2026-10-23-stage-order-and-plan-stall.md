---
title: 段の scheduler の詰まり（前の段が残ったまま後の段が ready・行の無い依存・黙って止まる）を直す
updated: 2026-10-23
completed: 2026-10-23
tasks: [01M5KHEJRREJS2DK7SH3NMEER6]
status: done
---
# 段の scheduler の詰まりを直す

ADR: `agent-docs/adr/2026-10-23-stage-order-and-plan-stall.md`

## 原因（本番 01M5HDSN3AWKK4ZJ0T1K3QTC8X、2026-10-22T12:10Z〜10-23T04:30Z）
`task_core::phase_ranks` が段の順位を段の有効な行の `seq` の最小値で決めていた。replan は done の行の `seq` を書き換えないので、
前の版で done の parity-close（close 段）の小さい `seq` が残り、close が gap-c より前の段に見えた。parity-final は ready に上がり、
gap-c の 3 unit は `waiting_stage=close` で pending のまま（互いに待つ）。scheduler の現在の段は gap-c で ready が無く、何も起きない。
waits は ready の unit があるので []、生存確認も ready の unit を「走れる」と分類して黙っていた。人の replan（版 01M5KHDY52）でも
採用時の ready の決め直しが `seq` の書き換え前だったので同じ形を作った。

## 決定と実装（commit）
- D1 `d0dcd058`: 未完の行がある段はその行の `seq` だけで順位を決める（今の版の並び = spec.stages の宣言順。前の版の done 行は入らない）。
- D2 `987adc05`: replan の採用は未完の行の `seq` を新しい版の並びへ書き換えてから ready を決める。
- D3 `0a9cb911`: 採用（新規・replan）で、有効な行に解決できない依存は `DependencyWithoutRow`（422 `validation`）で拒む。前の版で done の
  繰り越し行は満たされた依存とする（`newly_ready` は版を問わず done の key を数える）。
- D4（この commit）: `task_core::plan_stall` と `TaskWait::PlanStalled{stage, unit_keys, detail}`（waits に出る）。web の待ちの表示を追加。
- D5（この commit）: 生存確認の `NodeLivenessFacts.plan_stalled` → `StallDetected{reason: plan_stalled}`（`liveness_timeout_secs` 既定 600 秒）、
  `DaemonSnapshot.stalled_tasks[]`、`AttentionItem::Stalled`、受信箱の kind `stalled`。

## 証拠
- 修正前の失敗（D1/D2 の試験を修正前の code で流した記録、run の artifacts `prefix-repro.log`）:
  `stage_order_ignores_stale_seq_of_done_rows_from_earlier_versions`・`stalled_production_state_resumes_from_the_earlier_stage`・
  `replan_adding_earlier_stages_keeps_stage_order_over_done_rows` が FAILED。修正後は pass。
- 試験（受け入れ条件との対応）:
  - 0: `task-core execution_plan::tests::stage_order_ignores_stale_seq_of_done_rows_from_earlier_versions`・`stalled_production_state_resumes_from_the_earlier_stage`、
    `task-ops execution::tests::replan_adding_earlier_stages_keeps_stage_order_over_done_rows`
  - 1: `task-core execution_plan::tests::dependency_on_a_key_without_a_live_row_is_rejected`・`carried_over_done_row_from_an_earlier_version_satisfies_a_dependency`
  - 2: `task-core wait::tests::plan_stall_is_reported_when_nothing_can_be_dispatched`・`plan_stall_is_none_while_the_plan_can_move_or_waits_on_a_named_reason`、
    `task-core tree::r3b_tests::liveness_plan_stalled_node_is_unexplained_even_with_a_ready_unit`、
    `task-dispatch dispatcher::tests::finished_plan::a_plan_that_cannot_dispatch_reports_plan_stalled_and_reaches_the_inbox`（偽の時計で 600 秒後に StallDetected と snapshot、直せば消える）、
    `task-ops inbox::tests::inbox_attention_stalled_from_snapshot_for_ready_tasks_only`、`human_inbox::tests::human_inbox_every_inbox_kind_appears_in_common_form`、
    web `features/tasks/wait-reason.test.ts`（plan_stalled の表示）

## 検査
- `bash scripts/dev/test-affected.sh`: exit 0、6290 tests run: 6290 passed, 16 skipped（全出力は run の artifacts `test-affected.log`）
- web（test-affected は Rust と混ざると web を走らせないので別に実行）: `pnpm install --offline` の後 `vitest run features/tasks features/inbox api/realtime`
  12 files・113 tests pass、`tsc -b` exit 0、`biome check .` exit 0（既存の warning 4 件）
- `cargo clippy --workspace -- -D warnings`: exit 0
- `cargo fmt --all -- --check`: exit 0

## 本番への移行
- config の変更は不要（`[execution.tree].liveness_timeout_secs` の既定 600 秒をそのまま使う）。DB の migration も無い。
- 本番で既に止まっている task は、release 後の最初の tick で D1 の順位で前の段の unit が上がる（未完の行の `seq` は D2 以前の replan でも
  書き換え済み）。行の無い依存で止まった既存の行は D3 では直らないので、waits の `plan_stalled` と受信箱の `stalled` から人が replan する。

## 未解決事項
- daemon が足した phase 付きの行（統合の repair WU）は replan で `seq` を書き換えない。段を並べ替える replan でだけ段の順位を下げうる（ADR D1 の既知の残り）。
- すべて done の段の順位は前の版の `seq` のまま（待ちの判定には効かない。GUI の段の帯の並びだけに出る）。

## 提案
- 段の順位を spec（`ExecutionPlanSpec.stages`）から直接引く形にするなら、`newly_ready`・`unit_waits`・`runnable_work_units` に計画の spec を渡す
  API 変更になる（呼び出し 10 か所余り）。今回は行の `seq` が宣言順と一致することを D2 で保証して避けた。GUI の段の帯の並びを宣言順にしたいときに検討する。
