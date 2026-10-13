---
tasks: [01M4T98YHKGT945HH1588SC2VR]
status: done
title: provider 廃止後の routing event 互換設計
updated: 2026-10-13
---

# provider 廃止後の routing event 互換設計

## 決定 1: 旧 `RoutingDecided` を読み取り専用で受ける

- **決定**: `RoutingMode` に `Legacy` を復活させる場合は、既存 `RoutingTraceV1` の `mode` deserialize だけに使う互換値として追加する。`RoutingMode` を構築する設定・policy・dispatch の経路は `Legacy` を選べないままとし、新 event の書き込み値は `shadow` / `enforce` に限定する。独立の Event parser でなく、通常の `Event` deserialize が受け止める。
- **理由**: 本番 event の `record.optimizer.mode = "legacy"` は旧 provider 選択器が `RoutingDecided` に記録した歴史的な監査値で、現在の設定 mode ではない。新しい legacy 書き込みや旧 selector の復活は不要。
- **変更箇所**: `crates/task-core/src/model_router/policy.rs:RoutingMode`; `crates/task-core/src/model_router/trace.rs:RoutingTraceV1`; event 読込 `crates/task-core/src/store/events.rs:events_for_impl`, `tasks_with_events_impl`（いずれも `serde_json::from_str::<Event>`）。起動時の全 task/event 読込は `tasks_with_events_impl` から daemon/bootstrap の store 初期化へ伝播する。
- **実装 leaf の試験案**: `task_core::model_router::tests::routing_old_legacy_mode_event_deserializes_and_is_not_written`; `task_core::store::tests::legacy_routing_decided_event_loads_from_store`; `celeris::daemon::tests::startup_loads_legacy_routing_decided_event`。

## 起動時の失敗箇所

`Event` は `crates/task-core/src/model.rs:Event`（serde tag `type`）。`RoutingDecided.record` は `RoutingRecord`、`optimizer` は `Option<RoutingTraceV1>`、trace の `mode` は `RoutingMode`。DB の event JSON は `crates/task-core/src/store/events.rs` の `events_for_impl` と `tasks_with_events_impl` で parse される。後者は task と全 event を読み、daemon の初期状態を作る。失敗する呼び出しは同関数の `let event: Event = serde_json::from_str(&json)?;`（同じ parse は `events_for_impl` 等にもある）。実データ `agent-docs/progress/2026-10-05-browser-web-live-view/real-check-evidence/rerun4-2026-10-06/attempts/{2-diag1-collect-before-deny,3-diag2-collect-before-pause}/task-events.jsonl` に `type=routing_decided` かつ `record.optimizer.mode=legacy` がある。削除 commit `09b65114`（`wu/mode-drop`）で enum variant が消えたため unknown variant error となり、Event parse error が store 読込から daemon 初期化へ伝播して起動を止める。

## provider 廃止履歴からの Event 型・field 棚卸し

手順: `git log --all --oneline --grep='provider\|Provider\|llm-source'` で commit 群を列挙し、`git show -p <sha> -- crates/task-core/src/model.rs crates/task-core/src/model_policy.rs crates/task-core/src/model_router crates/task-core/src/model_routing.rs` で serde `Deserialize` 型の variant/field 削除を抽出する。それが Event 直接 variant か、Event payload/埋め込み snapshot かを型の包含関係で追い、`git grep` で event append site（`append_event`, `append_event_tx`）と現行 reader 型を照合する。schema/fixture の差分だけで durable event と判定しない。

| 削除 commit / 差分 | Event との関係 | 判断 |
| --- | --- | --- |
| `09b65114` `wu/mode-drop`: `model_router::policy::RoutingMode::Legacy` 削除 | `RoutingDecided.record.optimizer: Option<RoutingTraceV1>.mode` | durable event に実在し、起動時の拒否を確認。読み取り互換が必要。trace の追加 optional `source_id/model/account_id` は serde default 済み。 |
| `297bd501` `wu/hint-drop`: `WorkerHint.adapter` 削除 | `Event::Created.task` が `Task.worker_hint` を埋め込み snapshot として含む | 現行 WorkerHint は `tier` のみ。旧 JSON の未知 struct field `adapter` は serde が無視するため追加対応不要。併せて `RoleSpec.adapter` は `harness` に改名された。RoleSpec が埋め込み snapshot に含まれるか/旧 adapter 値を保持すべきかは `Task` serde 経路を確認し、必要なら `#[serde(alias = "adapter")]` 相当を検討する。今確認できた起動失敗とは別。 |
| `f7e3ff29` operator: tier→model binding `TierModels` 削除 | `RoutingDecided.record.resolution` は `LaneResolution` を保持 | 削除差分は runtime provider config/catalog 型であり、Event payload serde 型の削除は確認できない。Event migration 対象とはしない。 |
| `bf1ac527` `wu/store-migrate`: `HarnessSelected` / `LlmRouteSelected` / `HarnessMigrationUnresolved` 追加 | Event 直接 variant | 削除ではない。新しい durable selection/migration event。 |
| `ad8d4d41` `wu/prune-select`, `a4bd4681` `api-unprovider`, `98b63777` `snapshot-unprovider`, `a534f497` `config-drop` | 旧 selector/API/snapshot/config 型の削除 | Event variant または現行 Event payload serde 型の削除は該当差分で確認できず。provider 関連というだけで互換要とはしない。 |

`Event::WorkerStarted` の `adapter`, `provider`, `account` は現在も型に残る。`RoutingDecided.record.resolution` の provider 名も永続 audit payload として残る。削除された設定・runtime 型と、永続 event の serde 型を混同しない。

## Leaf の境界

本 leaf は読解のみで `crates/` に変更しない。実装 leaf `legacy-events` は旧 JSON の deserialize を通し、新しい serialize が `mode=legacy` を生成しないことを固定する。追加発見は append site と reader 型まで結び付けてから修正対象にする。
