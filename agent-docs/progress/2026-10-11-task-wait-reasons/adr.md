---
title: "葉 adr: 待ちの理由の ADR"
tasks: [01M4NAKDCCS5XYAAJ518MVZ134]
status: done
updated: 2026-10-11
---

# 葉 adr

- 書いたもの: `agent-docs/adr/2026-10-11-task-wait-reasons.md`（見送り箇所の棚卸し D1〜D13・M1〜M9、`TaskWait` の型、
  task-core の `db_waits`／`plan_waits`／`unit_waits`、DaemonSnapshot の `task_waits`、API 欄、消す旧欄の表、web 表示、5 件の例）。
- 読んだ根拠: `task-core store/tasks.rs` `ready_tasks_impl`、`store/projects.rs` `halted_by_ancestry_locked`、
  `execution_plan/scheduling.rs` `runnable_work_units`／`newly_ready`／`earlier_phases_done`、
  `task-dispatch dispatcher.rs` `dispatch_ready`／`is_idle`、`dispatcher/dispatch_run.rs` `dispatch_one`、`dispatcher/work_units.rs`
  `wu_dispatch_gate`／`resolve_child_dependencies`、`dispatcher/provider_select.rs`、`write_set_gate.rs`、`task-ops view.rs`
  `awaits_only_children`／`ExecutionView`、`tree_view.rs` `TreeNodePhase`／`node_phase`、`daemon.rs` `DaemonSnapshot`。
- 見つけたこと: 「子待ち」の判定が view.rs と dispatcher で 2 つある（ADR Context）。ADR D3 で task-core の `plan_waits` 1 つに置き換える。
- コード差分: なし（`git diff --stat $CELERIS_WU_BASE -- crates/ web/ gui/` が空）。

## 未解決事項

- `HumanGate` の判定を task-ops の `gate_waits` に寄せる際、dispatcher の `human_gate_hold` の現行の引数（active plan・events）との突き合わせは core 葉で行う。

## 提案

- （なし）
