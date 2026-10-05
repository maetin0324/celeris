---
tasks: [01M4577C9412HCDQEV1AFTT69C]
unit: escalation
status: done-in-branch
completed: 2026-10-05
---

# Phase 3 escalation: 品質失敗の軌跡と監査用の決定値

`task-core::retry_policy` に設定可能な `EscalationThresholds` と構造化された `EscalationAudit` を追加した。既定の閾値は同じ lane で品質失敗 2 回、total attempts は 4 回。`for_task_with_thresholds` は設定値を task/profile の試行上限で丸める。`decide_trajectory` は既存 policy の判定と組織・task・WorkUnit の ceiling を合わせ、人/system の明示 lane は固定する。`LowQuality` は worker の自己申告になり得るため、既定の escalation 対象から外した。`reopen` 後は event 列内の reopen 位置を区間 ID として履歴を数え直す。`RoutingRecord.escalation` は optional で、旧 event と未知欄を読める。

## 検証

| コマンド | 結果 |
| --- | --- |
| `cargo test -p task-core routing_trajectory_escalates_one_lane_and_respects_caps -- --nocapture` | 1 passed |
| `UPDATE_SCHEMA=1 cargo test -p task-core event_row_schema_matches_committed` | 1 passed、`docs/api/v1/event.schema.json` 更新 |
| `cargo test -p task-core --quiet` | 730 passed、0 failed |
| `cargo clippy -p task-core -p task-ops -p task-dispatch -p task-api --all-targets -- -D warnings` | exit 0 |
| `cargo fmt --all -- --check`、`git diff --check` | exit 0 |
| `sh scripts/dev/check-adr-numbers.sh`、`sh scripts/dev/check-doc-links.sh`、`sh scripts/dev/check-doc-layout.sh scripts/dev/docs-layout.tsv` | すべて exit 0 |

## 未解決と提案

- dispatcher の retry lane 選択への `decide_trajectory` 配線と `RoutingRecord.escalation` の `Some` 記録は後続 `dispatch-esc` WorkUnit で行う。今は全 callsite が `None` を入れる。
- API schema、gui/web の生成型は audit API の WorkUnit で更新する。event schema は task-core の固定 schema 試験に必要なのでこの unit で再生成した。
- event の区間 ID は event 配列の index に基づく。store 側の単調 seq に置き換えるなら、後続で `attempt_history_with_interval` の入力を event row に拡張する。
