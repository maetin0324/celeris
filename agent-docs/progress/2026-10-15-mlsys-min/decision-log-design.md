---
title: "判断・実行・成果・消費ログと campaign export の設計（unit log-adr）"
tasks: [01M4Z716SMR90GX0WDEHA9B02Z]
status: done
updated: 2026-10-15
completed: 2026-10-15
---
# 判断・実行・成果・消費ログの設計（unit log-adr）

決定は `agent-docs/adr/2026-10-15-decision-log-export.md`（番号なし）。実装はしていない。

## 読んだもの（再利用点の洗い出し）

- `routing_audit.rs`（run 単位の `RoutingAudit`、`RoutingOutcomeState`）、`execution_metrics.rs`（`ExecutionMetrics`、quota・`cost_usd_complete`）。
- `store/routing_log.rs`（`llm_proxy_requests` の相関欄 decision/snapshot/run/task/source/model/account）、`store/route_records.rs`（`route_decisions/outcomes/judgments` と `route_decisions_export`）。
- `model.rs`: `RunMetrics`・`Usage`・`RunRole`（worker/reviewer/planner のみ）・`WorkerStarted/Finished`、`execution::RunEnd`、`RoutingRecord.work_unit_id`、`RouteDecision`（`propensity`・`exclusions`・`candidates`）、`RoutingOutcome`、`RequestSourceAttempt.fallback_reason`。
- `route_select.rs`（dispatch 側）は `RouteDecision` を作る側で、欄追加は `log-wire` の範囲。

## 足りない欄（要約）

campaign_id/arm、parent_run_id、routing_mode・classifier_*（confidence は別名）、prompt/question/provider_model 版、
3 階層ラベル（`campaign_verdict` 新規）、消費 5 区分の欠測/推定表現、jev・cos の役割行、cluster 待機、介入の 3 区分。詳細表は ADR の「既存欄の再利用と追加欄」。

## 気づいた既存の穴

- `RunRole` に cos/jev が無い。run でない消費は `usage` 行（`role`）で表し、`RunRole` は変えない方針にした。
- `Usage.cost_usd` は単価表の推定で実支出ではない（名前が紛らわしい）。export で `list_cost_usd`＋estimated に読み替える。
- `routing-eval::aggregate` は error/cancelled を分母から除く（design.md §5）。campaign 集計に流用しない。

## 証拠

- 受け入れ 0: ADR の D1〜D8 が (1)〜(8) に対応（D1 鍵、D2 提案/実行/除外/fallback/版、D3 ラベル・失敗・冪等、D4 classifier_confidence、D5 消費、D6 役割、D7 介入、D8 export）。
- 受け入れ 1: ADR 末尾の表「既存欄の再利用と追加欄」。
- コードの変更なし（文書のみ）。

## 未解決事項

- 追加欄の置き場（列／別表）は `log-core` が決める。`cos` 消費の run 外の取得元の有無は `log-wire` で確認が要る。
- `timeout` の判定（`budget_exhausted{wall}` との対応）は `RunEnd` の `BudgetKind` を実装時に確認する。

## 提案

- `log-core` は `dedup_key` の UNIQUE 制約を最初に入れ、再処理試験を先に書く。
