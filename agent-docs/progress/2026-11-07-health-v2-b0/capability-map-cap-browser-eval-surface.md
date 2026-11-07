---
title: 健全性 v2 B0 capability-map — browser・campaign/eval・API/CLI/web 面の棚卸し（cap-browser-eval-surface）
tasks: [01M6TYVJSPEYA2P8B7G56HSQ73]
status: done
updated: 2026-11-07
completed: 2026-11-07
---
# 健全性 v2 B0 capability-map — browser・campaign/eval・API/CLI/web 面の棚卸し（cap-browser-eval-surface）

完了日: 2026-11-07

## 結論
- 断片 `capability-map/cap-browser-eval-surface.json`（schema `celeris.health-v2-b0.capability-map-fragment/1`）に
  capability 12 件、共有状態 13 件、architecture-map との差分 11 件を書いた。
- 基準は着手時の `git merge-base HEAD main`（件名「repair: main 取り込みの衝突解消の import を整形する」。SHA は JSON の `base_sha`）。
- capability: browser の policy broker・launcher・egress/runtime・Live View/制御・site policy/信頼端末、campaign 評価基盤、
  jev-classifier、HTTP API の認証と token 主体、celerisctl、celeris-mcp、web gateway、web SPA。
- 試験コストは test-targets.json の `build_no_run[<crate>].secs` を使った（task-api 84.93 秒・task-worker 54.74 秒・celeris 82.24 秒）。
  campaign・celerisctl・celeris-mcp・jev-classifier・celeris-credentiald・web は段 1 で計測していないので null。

## 複数の書き手がある共有状態（要点）
- `cos_inbox_items`: task-api::cos::triage_view が SQL の UPDATE を直接書く。task-core::chat の triage・override_op も書く。
- `chat_client_requests`: task-api::chat::attachments と task-core::chat::attachments の両方が INSERT する。
- `browser_task_policies`・`browser_waits`・`browser_control_state`: task-core の中で 2 つの module が書く。
- event `review_resync_resumed`: task-api（POST rereview）と task-dispatch（review_spawn）。
- event `workspace_pruned`: celerisctl の workspace prune と task-dispatch の housekeeping。
- file `summary.json`: campaign の runner・long_horizon と celerisctl の campaign export。

## architecture-map との差分（要点。地図はこの unit では直していない）
- 行が無い: task-api の token 主体（principal.rs・middleware.rs）、jev-classifier と classify_tick、campaign の lock・global ledger・final accounting、web/server/relay.js と web/api/client.ts。
- 不変条件が無い: celeris-mcp の read-only DB、celerisctl の event 追記、task-api の SQL 直接書き込み。
- 試験が無い・path が glob: 行 94・97・134・128。

## 証拠（提出時の HEAD で実行）
- 生成と検証: 生成 script の中で全 owner・evidence_path・port・test・writer の path の実在を確かめた → 不在 0 件、capability 12、shared_state 13、diffs 11。
- 方法: grep（INSERT/UPDATE/DELETE、`append_event`、file 名）と docs/architecture-map.md の該当行の path 展開による照合。build と試験は実行していない。

## 未解決事項
- SQL の書き手は文字列の grep で拾った。動的に組み立てた SQL や task-core の store 関数の呼び手全件は、deps-shared-state unit の範囲として残す。
- campaign の file の書き手は定数名・file 名の grep による。書き込みの経路を全件は追っていない。

## 提案
- task-api が直接書いている `cos_inbox_items`・`chat_client_requests` を task-core の store 関数へ寄せるかどうかは、統合設計（g1-v0-design）で決める。
