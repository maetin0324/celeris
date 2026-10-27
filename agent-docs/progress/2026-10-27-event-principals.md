---
title: 判断・操作の event に実際の主体を記録する
updated: 2026-10-27
status: done
tasks: [01M5XXF7Q58YE3PEHHND8DBWT5]
---
# 判断・操作の event に実際の主体を記録する

完了日: 2026-10-27。
基準: `git merge-base HEAD main`。

## 変更

- [ADR](../adr/2026-10-27-event-principals.md)。
- `Answered.by`・`IntegrationAnswered.by`・`Retried.by`・`Created.created_by` を追加（導入前の event は欄なしで読む）。
  API・CoS operations・MCP は A1 の主体、daemon の記録は `daemon`、worker の後続 task は `worker:<run_id>`、
  celerisctl の直接書き込みは `human:cli`。`TaskStore::create_task_by` を追加。
- plan-gate の承認・replan・取り下げは `Event::PlanGateDecided { plan_id, action, by, note }`。
  `WorkerProgress` の自由文に by を埋める旧い記録は削除。
- `CommentAuthorKind` に `cos` / `mcp` / `operator` を追加。comment の種別は主体から決まり、comment 由来の
  `Answered.by` は comment の主体。`post_human_comment`（主体なし）は削除。
- migration 0096（SCHEMA_VERSION 96）: `task_comments` の CHECK を広げ、`author_kind = human` の行を author から直す。
  events は書き換えない。

## 証拠（提出時の HEAD で実行）

- 試験: `store::principal_tests`（作成・retry の主体、migration 0096 で誤記録 0 件・events 件数不変）、
  `task-ops comment::tests::a_comment_records_its_principal_kind_and_the_answer_carries_the_principal`、
  `retry::tests`・`gate::tests` の by、`task-api tests/api_principals.rs`・`tests/execution.rs` の PlanGateDecided、
  `celeris-mcp mcp_integration task_plan_gate_*`。
- `bash scripts/dev/test-affected.sh`、`cargo clippy --workspace -- -D warnings`、`cargo fmt --all -- --check` は報告本文に記す。

## 本番への移行

- migration 0096 は daemon の起動時に自動で走る。人の config 変更は不要。

## 未解決事項

- `Edited`・`Approved` などの既存の `by`（`human` に丸めた値）は範囲外。

## 提案

- 評価の集計（人と AI の判断の区別）は `by` / `created_by` と comment の `author_kind` を使う。欄の無い旧い event は主体不明として扱う。
