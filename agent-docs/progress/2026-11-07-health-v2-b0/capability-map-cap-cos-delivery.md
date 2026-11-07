---
title: 健全性 v2 B0 capability-map cap-cos-delivery — CoS・受信箱/通知・review・delivery・selfdeploy の棚卸し
tasks: [01M6V1TGEG5QT5P6PM0Q0D0QMK]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性 v2 B0 capability-map cap-cos-delivery

完了日: 2026-11-07

## 結論
- 断片 `agent-docs/progress/2026-11-07-health-v2-b0/capability-map/cap-cos-delivery.json` に 7 capability・共有状態 41 件・architecture-map との差分 11 件を書いた。
  基準は着手時の `git merge-base HEAD main`（SHA は JSON の `base_sha`）。
- capability: CoS chat threads/runs、CoS 受信箱一次対応と /cos/operations の監査、受信箱と通知（Discord・root done 要約）、review run と final review、delivery（merge candidate）、pre-review sync・auto_resolve・IntegrationRepair・integrate task、release/selfdeploy の結線。
- 試験コストは段 1 の `test-targets.json` の `build_no_run[<crate>].secs`（task-core 44.52・task-dispatch 91.41・task-api 84.93・task-worker 54.74・celeris 82.24 秒）。task-ops は段 1 で測っていないので null。

## 複数の書き手がある共有状態（主なもの）
- `chat_threads`: task-core の chat::{store,triage,review,credential} と store::legacy（旧 thread の互換 facade）の 5 か所。
- `chat_messages`・`node_sessions`: chat の store に加え store::legacy も書く。
- `cos_inbox_items`: task-core の chat::triage・chat::override_op に加え、task-api の cos::triage_view が直接 UPDATE する。
- `notifications`: task-core の notify と chat::triage。
- Event `ReviewTargetAdvanced`: task-dispatch の review_spawn と celeris の delivery。
- Event `IntegrationRepairExhausted`: task-dispatch の review_spawn と worker_finish。
- Event `IntegrationMerged`: store 関数 `task_integration_upsert` が追記し、呼び手は celeris::delivery・task-ops::stranded::reintegrate・task-ops::op_writes・task-api::changes。

## architecture-map との食い違い（要点）
- delivery の状態機械（task-core・task-ops・celeris の delivery.rs）と task-api の releases.rs・integrations.rs を owner とする行が無い。
- self-deploy の行に release_notes.rs・releases/audit.rs・daemon_instances の書き手、試験 releases_api.rs・deployment_audit.rs が無い。
- auto_resolve の行に呼び手 celeris の delivery/auto_resolve.rs が無い。
- cos API の行の不変条件「API は store を通して書く」に対し、task-api の cos/triage_view.rs が cos_inbox_items を直接書く。
- celeris の notify.rs に、daemon が呼ばない旧 scan・schedule・schedule_routes が試験用に残る（破壊的変更の方針では消す対象）。

## 方法と制約
- 読み取りだけ（grep・ファイル一覧）。build・試験は実行していない。crates/ と docs/architecture-map.md は変えていない。
- SQLite の書き手は INSERT/UPDATE/DELETE の文を grep した（試験 file は除外）。event の書き手は `Event::X {` を組み立てて追記する箇所を grep で拾い、match の腕・読み取りを手で除いた。漏れはあり得る（未確認）。
- JSON の全 path が存在することを生成 script で確かめた（欠け 0）。

## commit
- `phase B0: capability-map cap-cos-delivery の断片と進捗`

## 未解決事項
- event kind の書き手の網羅は grep の型に依存する。全 event kind の書き手は deps-shared-state unit の結果と突き合わせる。

## 提案
- delivery の行を architecture-map に足し、cos_inbox_items の書き込みを task-core の store に寄せるかを統合設計（g1-v0-design）で決める。
