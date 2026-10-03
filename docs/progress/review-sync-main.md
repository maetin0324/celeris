---
tasks: [01M408E4BX8A3FNSBJTZ0CC67A]
---
# review sync: main 取り込み

merged-main: a1a3f60f03a3bc2872400e7ff27e8ec09b0387d8

task branch（`e269a8c4`）に `git merge --no-ff main` で上の main を取り込んだ（rebase なし）。衝突した 16 ファイルは両側の変更を残して解いた。

| ファイル | 解き方 |
|---|---|
| `crates/celeris/tests/instance_handoff.rs` | branch 側の `STATE_WAIT` 定数で待つ形を残し、値を main 側の長い保険 120 秒に揃えた。gate 待ちの台本と説明の comment は main 側（`timeout 300` の出来事待ち）を採った |
| `crates/task-api/src/query.rs` | `EVENT_TYPES` の要素は自動 merge で両側（review target / integration repair 5 件と `delivery_skipped`）が入ったので、配列長を 54 と 50 から 55 にした |
| `crates/task-api/src/query/tests.rs` | branch 側の review target・integration repair の試験 2 件を残し、件数の検査を 55 にした |
| `crates/task-core/src/cluster_job/tests.rs` | schema を戻す SQL に両側の DROP（deliveries・node_sessions の列と `idx_events_delivery_skipped`）を並べた。`SCHEMA_VERSION` は 41 |
| `crates/task-core/src/store/migrations.rs` | 両側の登録を残した。main の `0037_events_delivery_skipped_index`（`MIGRATION_0037`）と `0041_feed_notices`、branch の 0038〜0040 を登録した。branch の `0037_review_target_sync` は `MIGRATION_0037_REVIEW_TARGET_SYNC` として、重複した 37 の arm（`#[allow(unreachable_patterns)]`）に残した。0038〜0040 は本物が入ったので `RESERVED_VERSIONS` を空にした。`SCHEMA_VERSION` は 41。番号の振り直しは次段の migrations 葉で行う |
| `crates/task-core/src/store/tests.rs` | 8 箇所の `SCHEMA_VERSION` の検査をすべて 41 にした |
| `crates/task-ops/src/delivery.rs` | `use task_core::{…}` を両側の和集合（`RepoId` と main の `DeliverySkipReason`・`OrgNode`・`PlanOrigin`・`RunIndexRole`・`TaskId`）にし、branch の `MAX_TARGET_RESYNCS`・`MAX_INTEGRATION_REPAIRS`・`target_restale_count` を残した |
| `crates/task-ops/src/delivery/tests.rs` | branch の `target_advanced_count_resets_only_on_explicit_restart` と main の部署の fallback 試験・delivery skip 試験を両方残した |
| `crates/task-worker/tests/browser_injection_wire.rs` | 同じ試験の 2 版のうち、改良側の main 版（CDP 応答待ち 60 秒）を採った |
| `crates/task-worker/tests/browser_shared_cdp.rs` | 両側とも probe の再試行版。改良側の main 版（期限 90 秒、`ValueError` も再試行、上限の検査あり、host の待ちは 120 秒）を採った |
| `docs/PROGRESS.md` | 3 箇所とも両側の節を全部残した。目次の Browser Phase 1 の行は main 側の追記版を採った |
| `docs/architecture-map.md` | branch の write-set・behind・直行経路の行と、main の ADR-0134 リンクの両方を残した |
| `docs/progress/time-dependent-tests-injection.md` | add/add。main 側の内容は branch 側に追記を足したものだったので、main 側を採った |
| `gui/app/routes/inbox.tsx` | branch の `IntegrationRepairPanel` と main の `delivery_skipped` 詳細ブロックを両方残した |
| `web/api/realtime/event-kinds.ts` | branch の review target・integration repair の 6 種と main の `delivery_skipped` を両方残した |
| `web/api/realtime/invalidation-map.ts` | 上の 7 種の invalidation の行を両方残した |

衝突のない file のうち、main の新しい試験の struct 初期化子に branch の欄を足した（`crates/task-ops/src/human_inbox/tests.rs` の `AttentionItem::Failed` に `integration_repair: None`、`crates/task-ops/src/notify_feed/tests.rs` の `Delivery` に `target_sha`・`reviewed_sha`・`merge_candidate_sha`）。`cargo check --workspace --all-targets` は exit 0 だった。
