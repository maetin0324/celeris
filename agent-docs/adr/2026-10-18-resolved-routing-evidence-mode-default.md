---
title: "ADR: ResolvedRouting.evidence_mode の欠落は型の既定で読む（旧 event を書き換えずに読む）"
status: accepted
date: 2026-10-18
tasks: [01M57DXA25E8KB5FC75FN0TXY7]
---
# ResolvedRouting.evidence_mode の欠落を読み込み時に解釈する

- 関連: 2026-10-17-evidence-mode-none-pinned-latest（RoutingOverride の規則）

## 文脈
9c4af7f9 で `ResolvedRouting` に必須の `evidence_mode` を足したが serde の既定が無く、`evidence_mode` の無い
旧 `routing_resolved`（本番の routing_decided 等に残る）を読むと `missing field evidence_mode` になる。
`GET /llm/accounts` は stats の `catch_up` で events 全件を型付きで読むため、旧 event 1 件で 500 になった。

## 決定
- D1: `ResolvedRouting` の `Deserialize` を 1 か所（型の実装）で決める。`evidence_mode` が無ければ
  `evidence_table_id` あり→`pinned`、なし→`latest`（`RoutingOverride` と同じ規則）。events は書き換えない。
  専用の互換分岐は足さない。
- D2: 型付きで events を読み直す経路（`events_since`）が読めない行に当たったら、event id と task id を
  エラー文（problem detail）に含める。
- D3: 回帰試験: 旧 shape の `llm_route_selected`・`routing_decided`・`worker_finished`・`worker_progress` を
  含む DB で `GET /llm/accounts` が 200 と usage を返す。
- D4: 一覧 API が 1 件の読めない event で 500 になる作りは変えない（読めない event を黙って飛ばすと
  stats が静かに欠ける）。原因の event id・task id を problem detail に出して特定できるようにする。
  `routing_resolved` を持つ型は `ResolvedRouting`（`RouteDecision.routing_resolved`）だけで、
  `worker_finished`・`worker_progress` は型に当該欄が無く（未知の欄は読み飛ばす）旧 shape でも読める。
