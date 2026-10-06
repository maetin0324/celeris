# ADR 2026-10-06: CoS 受信箱の取り込み（dispatcher 側の source 同一性と cursor）

---
tasks: [01M47VN95D8QQ65ATZVGHFFXA7]
---

ADR 2026-10-05-cos-chat-home D3「一次対応の起動とルーティング」と ADR 2026-10-06-cos-inbox-triage（store 契約）の、dispatcher（`crates/task-dispatch/src/dispatcher/cos_chat/triage.rs`）側の決定。LLM は呼ばない。

## 決定

- **派生 inbox（source cursor `events`）**: cursor は events の global id。tick ごとに cursor 以後だけを最大 512×8 件読む。進捗系の event（WorkerProgress・ArtifactProduced・ClusterJobWaitPolled・QuotaEstimated・CheckpointSaved・WorkUnitCheckStarted・IntegrationCheckStarted・BrowserUpdated・ProviderThrottled・RoutingDecided）は待ちを開かないので無視する。それ以外の event が触れた task があるときだけ、その tick で 1 回 `task_ops::human_inbox` を組み立て、触れた task（`task`・`blocking.root`・`blocking.tasks`）の項目だけを渡す。
- **同一性**: `source_kind = InboxKind`、`source_key = InboxItem.id`、`source_revision = InboxItem.created_at`（待ちが生じた時刻。題名・後続の無関係な event では変わらない）。
- **notice（source cursor `notices`）**: cursor は最新の `(last_at, id)`。`notice_list` を新しい順に読み、cursor 以下に達したら止める。`source_revision = count`（束に出来事が足された時だけ変わる。既読・題名では変わらない）。`secretary_reply` は CoS 自身の返事なので取り込まない。target が `cos_operation` の notice は operation id を付けて store に捨てさせる。
- **CoS operation 由来の除外**: operation は領域 event の直後に同じ transaction で `CosOperation` 監査 event を書く。batch 内で、監査 event から遡って同じ task の連続した global id の event をその operation に帰属させる。task の非進捗 event が全て帰属していれば、その task の項目に operation id を付ける（store が捨てる）。
- **起動・reconcile**: 起動時（と `Dispatcher::request_cos_triage_reconcile` の後）に現在の派生 inbox を種類ごとに `cos_triage_reconcile` へ渡す（events は読まない）。cursor が無ければ導入時の 1 回で、先に読んだ head を events cursor に置く。notice は判断不要なので、導入前の notice は CoS に渡さず cursor を最新に置く。
- **起動**: 受信箱 thread に生きた run が無く、新着がある（process 内の hint）ときだけ `cos_triage_claim` を呼ぶ（毎 tick の書き込み transaction を避ける）。account 選択・容量（ADR-0089）・起動は `launch.rs` の `start_thread` を `ClaimSource::Triage` で共用する。20 件ちょうど取ったら run の終了後にもう一度 claim する。
- **元 thread**: 新しい項目の task の events に人の thread からの `CosOperation` があれば、その thread に参照カード（受信箱 thread への link）を `chat_system_message_add_once`（key は source triple の FNV-1a）で 1 回だけ置く。
- **設定**: `[cos.triage]` は丸ごと `CosChatLaunchConfig.triage`（`CosTriageSettings`）に渡す。取り込みは `cos.enabled=false` でも走り（不在退避が新着を見られるように）、run は起動しない。
