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
- D1 **task 欄** `RoutingOverride`（`task_core::route_select::task_override`）:
  `{mode: fixed|decision_assisted, lower_policy?: string, evidence_table_id?: string, online_update: bool = true}`。
  置き場は migration 0084 の `task_routing_overrides(task_id PK, override_json, created_at)`（`TaskStore::task_routing_override`）。
  `POST /tasks` が task の作成と同じ transaction で 1 行書き、同時に追記専用の `routing_override_pinned` event を 1 件追記する。
  行は以後変えない（PK で 2 回目の書き込みは失敗）。行の無い task・既存の DB は従来どおり。
  `Task.routing`（tasks.json）には置かない: `NewTaskSpec`・`TaskRouting` を変えると task-ops・celerisctl・全 spec 生成箇所に波及し、
  この unit の範囲（task-core・task-dispatch・task-api・celeris）を越えるため。
- D2 **解決** `resolve_routing(config_mode, task_override)`: task 欄があれば config より優先、無ければ従来（config の mode・
  `DEFAULT_POLICY_VERSION`・最新の根拠表・オンライン更新あり）。dispatcher の lane 層（`apply_lane_assist`）と daemon の分類段
  （`ClassifyTick::run`）が task ごとに同じ関数で解決する。fixed に固定した task は分類器も lane 補助も走らない。config が fixed でも
  decision_assisted に固定した task は分類・補助が走る。
- D3 **下段** `lower_policy` は lane 規則の版。現行は `mlsys-min-1` だけを受理し、未知の版は作成時に 400（黙って既定へ倒さない）。
- D4 **根拠表** `evidence_table_id` は `routing_evidence.evidence_version`。指定した task の route 選択はこの版だけを読む
  （`TaskStore::routing_evidence_at`）。無ければ従来どおり最新版。lane 補助の利用可否の下見は従来の読みのまま（lane の引き上げ可否だけ）。
- D5 **オンライン更新** `online_update = false` の task は run の結果を `routing_outcome_recorded` に追記しない
  （`task_core::model_router::feedback::pending_run_outcomes` が task の events に `routing_override_pinned{online_update:false}` を
  見つけたら 0 件を返す。追記の全呼び出し元がこの関数を通る）。決定ログ（`decision_log_*`）は記録する。
- D6 **記録** 欄のある task の `celeris.route-decision/1`（`routing_decided` event の `RouteDecision.routing_resolved`）に
  `{mode, source: task|config, lower_policy, evidence_table_id, online_update}` を載せ、決定ログの `routing_mode` にも mode を書く。
  欄の無い task には書かない（従来の record と同一）。解決の入力（固定そのもの）は作成時の `routing_override_pinned` event に残る。
- D7 **入口** `POST /api/v1/tasks` の `routing_override`（`NewTaskBody`。監査付きの CoS operation 経路 `task.create` も同じ関数）。未知の mode・未知の欄・未知の下段版は 4xx。
  CoS の `create_task` action（Agent 経路）・`celerisctl add` はこの欄を持たない（試行の固定は人・campaign の監査付き API 経路だけ）。作成後の変更口は作らない（試行中に設定を動かさない）。

## 帰結
- `docs/api/v1/*.schema.json`・`docs/protocol/worker-protocol.schema.json`・`web/api/generated/*` を再生成した。
- campaign の B/C アダプタ（後続 unit）は投入時に `routing_override` を付け、全体 config に依存しない。
