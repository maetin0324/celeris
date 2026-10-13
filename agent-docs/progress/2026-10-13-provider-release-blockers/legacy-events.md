# 旧 routing event の読み込み互換

## 調査結果

verify check 1 の `unknown variant 'legacy'` は、daemon 起動時の dispatcher 初期化が event 履歴を読む経路で発生する。具体的には `SqliteStore::tasks_with_events_impl`（`crates/task-core/src/store/events.rs`）が各 `events.json` を `serde_json::from_str::<Event>` し、`RoutingDecided.record.optimizer` の `RoutingTraceV1.mode` を復元する際に失敗する。これは DB の接続・migration 後、dispatch が始まる前の bootstrap 失敗である。

ADR の provider 廃止差分（`09b65114` の RoutingMode 削除、`a534f497` の provider config 削除、および同系列の hint-drop/store migration）を serde Event payload まで追跡した。保存済み Event 型として追加互換が必要なのは次の一つ。

| Event 型 / payload | 廃止前の値・欄 | 判断 |
| --- | --- | --- |
| `RoutingDecided.record.optimizer: RoutingTraceV1` | `mode: "legacy"` | 読み取り互換が必要。`RoutingMode::Legacy` を deserialize 専用で追加し、現行 serialize は拒否する。 |
| `Created.task` 内の `WorkerHint.adapter` | 旧 struct field | 未知の struct field として serde が無視する。現行型で読み込み失敗しない。 |

provider 設定、provider catalog、adapter 設定、tier binding の削除は Event payload 型ではなく、保存済み event reader に影響しない。`RoutingTraceV1` の optional 欄は旧 JSON に無くても serde default/Option で読み込める。ADR 付記が結論づけたとおり、旧 event 自体の書き換えや event migration は行わない。

## 実装と検証

`RoutingMode::Legacy` は deserialize のみに許可し、新規 serialize できないよう `skip_serializing` を付けた。`task_core::model_router::tests::legacy_routing_event_deserializes_without_becoming_writable` は legacy mode の event payload を deserialize し、Legacy mode の serialize が失敗することを固定する。

`tasks_with_events_impl` が実際の永続 event 読み込み点であり、daemon bootstrap がこの API を呼ぶ。task-core の serde 境界・store 読み込み試験と、旧 `RoutingDecided` row を含む DB で dispatcher bootstrap/tick を通す integration 試験を追加した。完了証拠は [legacy-db.md](legacy-db.md) を参照。
