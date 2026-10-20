---
title: replan で同じ段の依存先が変わった WU のブランチと起点を作り直す
updated: 2026-10-20
tasks: [01M5BTS6VKHTYE3HE4XNPBW1CX]
status: done
completed: 2026-10-20
---
# replan で同じ段の依存先が変わった WU のブランチと起点を作り直す

ADR: `agent-docs/adr/2026-10-20-replan-dep-change-wu-rebase.md`

## 実施
- migration 0089: `work_units.rebase_pending`（SCHEMA_VERSION 89）。`WorkUnitRow.rebase_pending` の読み書き。
- `task_core::same_phase_dependencies_changed`: 同じ段の依存先の集合（有無を含む）の変化を判定。
- replan の持ち越し（`task-ops/src/execution.rs`）と replay の再構成（`replay.rs`）: kind != Task で同段依存が変われば
  branch・base_commit・head_commit を消し `rebase_pending = true`。kind task の扱いは不変。
- prepare（`task-dispatch/src/dispatcher/work_units.rs`）: `rebase_pending` の WU は `integration::archive_wu_branch` で
  worktree を外し（`worktree remove --force` + `prune`）、旧ブランチを `<branch>-superseded-<sha12>` へ退避してから、
  依存先の HEAD（無ければ Task ブランチの HEAD）から作り直す。範囲検査 snapshot は worktree の git dir 下なので一緒に作り直し。
  `rebase_pending` は dispatch で下ろす。
- 試験: `crates/task-dispatch/src/dispatcher/tests/replan_rebase.rs`（実 git リポジトリのみ。LLM・ネットワーク・sleep なし）
  - `a_changed_same_phase_dependency_rebuilds_the_unit_from_the_new_dependency`（A→C を B→C へ: base・base_commit・worktree 祖先が B の HEAD、旧ブランチは superseded に残る）
  - `an_unchanged_dependency_reuses_the_branch_and_base`
  - `a_dropped_dependency_rebuilds_the_unit_from_the_task_branch`
  - `same_phase_dependencies_changed_compares_only_same_phase_sets`
- 既存の migration 試験 3 本（downgrade と版数一覧）に 0089 を反映。

## 証拠
- `cargo nextest run -p task-dispatch replan_rebase` → 4 passed
- `bash scripts/dev/test-parallel.sh` → exit 0、6189 tests run: 6189 passed, 14 skipped（commit 15852cab）
- `cargo clippy --workspace -- -D warnings` → exit 0
- `cargo fmt --all -- --check` → exit 0

## 未解決事項
- なし。

## 本番への移行
- migration 0089 は起動時に自動適用（列追加のみ、既定 0）。config の変更なし。

## 提案
- なし。
