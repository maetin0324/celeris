---
title: "ADR: 試行ごとの routing 設定を task 欄で固定する（routing_override）"
status: accepted
date: 2026-10-16
tasks: [01M51QR9CCRP89GWXZDR4XB3S6]
---
# 試行ごとの routing 設定を task 欄で固定する

- 関連: 2026-10-15-mlsys-min-campaign（B/C の比較）、2026-10-15-mlsys-min-routing-policy（`[routing] mode`・lane 層・`DEFAULT_POLICY_VERSION`）、2026-10-15-mlsys-min-decision-assisted

## 文脈
本番の全体 config は `[routing] mode = "decision_assisted"`。campaign の B（Celeris+fixed）と C（Celeris+decision_assisted）を同じ daemon で走らせるには、
全体 config を切り替えずに試行ごとの routing 設定（mode・下段方策・根拠表・オンライン更新）を task に固定し、記録する必要がある。

## 決定
- D1 **task 欄** `Task.routing.routing_override: Option<RoutingOverride>`（`task_core::route_select::task_override`）:
  `{mode: fixed|decision_assisted, lower_policy?: string, evidence_table_id?: string, online_update: bool = true}`。
  `TaskRouting`（tasks.json の中）に置くので migration は要らない。欄が無い task は 1 バイトも変わらない。
- D2 **解決** `resolve_routing(config_mode, task_override)`: task 欄があれば config より優先、無ければ従来（config の mode・
  `DEFAULT_POLICY_VERSION`・最新の根拠表・オンライン更新あり）。dispatcher の lane 層（`apply_lane_assist`）と daemon の分類段
  （`ClassifyTick::run`）が task ごとに同じ関数で解決する。fixed に固定した task は分類器も lane 補助も走らない。config が fixed でも
  decision_assisted に固定した task は分類・補助が走る。
- D3 **下段** `lower_policy` は lane 規則の版。現行は `mlsys-min-1` だけを受理し、未知の版は作成時に 422（黙って既定へ倒さない）。
- D4 **根拠表** `evidence_table_id` は `routing_evidence.evidence_version`。指定した task の route 選択はこの版だけを読む
  （`TaskStore::routing_evidence_at`）。無ければ従来どおり最新版。lane 補助の利用可否の下見は従来の読みのまま（lane の引き上げ可否だけ）。
- D5 **オンライン更新** `online_update = false` の task は run の結果を `routing_outcome_recorded` に追記しない
  （`task_ops::routing_outcome::record_routing_outcomes` が 0 件で返る。全呼び出し元に効く）。決定ログ（`decision_log_*`）は記録する。
- D6 **記録** 欄のある task の `celeris.route-decision/1`（`routing_decided` event の `RouteDecision.routing_resolved`）に
  `{mode, source: task|config, lower_policy, evidence_table_id, online_update}` を載せ、決定ログの `routing_mode` にも mode を書く。
  欄の無い task には書かない（従来の record と同一）。
- D7 **入口** `POST /api/v1/tasks` の `routing_override`（`NewTaskSpec`）。未知の mode・未知の欄は 4xx。CoS の `create_task`（Agent 経路）は
  この欄を書かない（試行の固定は人・campaign の監査付き API 経路だけ）。作成後の変更口は作らない（試行中に設定を動かさない）。

## 帰結
- `docs/api/v1/*.schema.json`・`docs/protocol/worker-protocol.schema.json`・`web/api/generated/*` を再生成した。
- campaign の B/C アダプタ（後続 unit）は投入時に `routing_override` を付け、全体 config に依存しない。
