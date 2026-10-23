# source-size 差分ゲートと既存負債の是正

---
title: source-size 差分ゲートと既存負債の是正
tasks: [01M5KWPP2W6FZAGWXFDFBQJ5S0]
status: running
updated: 2026-10-23
---

## 基準と変更範囲

着手時main/HEAD: `13d64acd17806d73a1797bffbf831efc9cb5ab6a`。
10/19監査SHA `c9654bff56332c160523d904b2248ae527d9b820` の22件をgit archiveから再計測。
着手までの解消は0件、追加は5件（manifest・variant_pin・classify_tick・completion_outcomes のinline test、worker_finish のproduction）。
現在の基準は27件（production 6・inline 21）、理由付き例外1件。
architecture-map は着手時419参照で成功。task5はReadyでmain未導入だった。

必須経路・固定基準・比較元SHAの規則は [ソース健全性](../guides/source-health.md)。
軽量検査を test-affected の先頭に接続。意味を変えず21 test moduleを外部化した。
model.rs から追記専用 Event とそのpayload/型名を model/events.rs へ分離し、
タスク/runモデルとevent契約を別ファイルで読めるようにした。公開型・serde形式・DBは変更しない。
TaskStore の契約やDispatcherの状態/副作用分割には踏み込まない。

## 解消した警告

| 元ファイル | 外部test | 元inline行数 |
|---|---|---:|
| `crates/campaign/src/lock.rs` | `crates/campaign/src/lock/tests.rs` | 408 |
| `crates/campaign/src/lock_tree.rs` | `crates/campaign/src/lock_tree/tests.rs` | 321 |
| `crates/campaign/src/manifest.rs` | `crates/campaign/src/manifest/tests.rs` | 475 |
| `crates/campaign/src/variant_pin.rs` | `crates/campaign/src/variant_pin/tests.rs` | 343 |
| `crates/celeris/src/daemon/classify_stage.rs` | `crates/celeris/src/daemon/classify_stage/tests.rs` | 415 |
| `crates/celeris/src/daemon/classify_tick.rs` | `crates/celeris/src/daemon/classify_tick/tests.rs` | 319 |
| `crates/routing-eval/src/aggregate.rs` | `crates/routing-eval/src/aggregate/tests.rs` | 330 |
| `crates/routing-eval/src/recheck.rs` | `crates/routing-eval/src/recheck/tests.rs` | 417 |
| `crates/task-core/src/accounts.rs` | `crates/task-core/src/accounts/tests.rs` | 337 |
| `crates/task-core/src/browser_backend.rs` | `crates/task-core/src/browser_backend/tests.rs` | 321 |
| `crates/task-core/src/chat/attachments.rs` | `crates/task-core/src/chat/attachments/tests.rs` | 477 |
| `crates/task-core/src/llm_route.rs` | `crates/task-core/src/llm_route/tests.rs` | 332 |
| `crates/task-core/src/model_router/cost.rs` | `crates/task-core/src/model_router/cost/tests.rs` | 322 |
| `crates/task-core/src/node_session.rs` | `crates/task-core/src/node_session/tests.rs` | 524 |
| `crates/task-core/src/retry_policy.rs` | `crates/task-core/src/retry_policy/tests.rs` | 396 |
| `crates/task-core/src/route_extract.rs` | `crates/task-core/src/route_extract/tests.rs` | 540 |
| `crates/task-core/src/store/completion_outcomes.rs` | `crates/task-core/src/store/completion_outcomes/tests.rs` | 385 |
| `crates/task-dispatch/src/dispatcher/decision_log_wire.rs` | `crates/task-dispatch/src/dispatcher/decision_log_wire/tests.rs` | 614 |
| `crates/task-ops/src/stranded.rs` | `crates/task-ops/src/stranded/tests.rs` | 740 |
| `crates/task-worker/src/browser_cdp_sink.rs` | `crates/task-worker/src/browser_cdp_sink/idle_pump_tests.rs` | 758 |
| `crates/task-worker/src/browser_launcher_run.rs` | `crates/task-worker/src/browser_launcher_run/live_frame_tests.rs` | 878 |

productionの解消: `crates/task-core/src/model.rs`（2,064行）からevent責務を分離。
行数削減ではなく、同一test・同一型定義の責務別配置。新規のsize例外は追加しない。

## 残件（未解消として追跡）

| production警告 | 理由 | 具体的な追跡 |
|---|---|---|
| `crates/task-dispatch/src/dispatcher.rs` | 状態と副作用の分離はタスク4の範囲 | `01M5KWQ3WME71Q0ATMPTJA7XE7` |
| `crates/task-dispatch/src/dispatcher/work_units.rs` | 同上、TaskStore境界を使うためタスク3の後 | `01M5KWQ3WME71Q0ATMPTJA7XE7`（タスク3: `01M5KWPW976PBWXG4B37TQ55XC`） |
| `crates/task-dispatch/src/dispatcher/worker_finish.rs` | 完了時の状態と副作用を同じタスク4で扱う | `01M5KWQ3WME71Q0ATMPTJA7XE7` |
| `crates/task-worker/src/browser.rs` | 認証・承認・適合台帳と実行監督の分離には独立レビューが必要 | 追跡キー `source-size/browser`、下記follow-up #1 |
| `crates/task-worker/src/browser_cdp_sink.rs` | credential IPC・post-login観測・CDP転送の境界を維持する独立変更 | 同じfollow-up #1 |

`celerisctl ls` で未完了全件を照合（2026-10-23）。Dispatcherには既存taskがあり再起票ゼロ。
browser分割に該当する未完了taskは無く、`celerisctl add` により
「[健全性1 残件 source-size/browser] browser.rs と browser_cdp_sink.rs の責務分離」を
このrunの `followups.json` #1へ提出済み（終了後draft化、IDはその時点で付与）。
新規提案上限1件、同一警告の再起票禁止、再試行上限2回をobjectiveに明記。
5件は未解消であり、このtaskで全負債が解消したとは扱わない。

## 検証

実行結果・検証対象SHAは検証完了後に記録する。
全workspace/e2e/実browser/userns試験は追加しない。
