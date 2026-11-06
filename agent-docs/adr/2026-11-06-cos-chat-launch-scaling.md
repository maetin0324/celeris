---
status: proposed
tasks: [01M6QZ6KYWW18GYMZYM17SYSW9]
---
# tick_cos_chat_launch の所要時間を行数に依らず一定にする

## 背景
P4-F の 24h soak で slow tick の p50 が 1.7 秒から 75〜79 秒へ伸び、その 95% が tick_cos_chat_launch だった（n2.md, n2/interim-9h/trend.md）。本番の再起動直後は p50 0.19 秒で、chat の行が少ないためと見られる。

## 決定
- D1: tick_cos_chat_launch の段（retry_routes・control_hook・triage_ingest・root_done_tick・claim_queued・relay_rejudge・plan_rejudge・triage_launch）の所要時間を `TickReport::step_us` に `cos.<段>` の名前で出す。
- D2: tick_scale_baseline の形（DB の行の複写）で chat_runs・chat_messages・cos_inbox_items を N 件にして段ごとの時間を測り、行数に比例する段を特定する。
- D3: 比例する処理は、全件走査をやめ cursor・index・差分に変える。旧い経路は同じ変更で消す。
- D4: RSS 増加の task 01M6QZ031RC9YK3E6T12ESCR80 と同じ処理を直す場合は重複して直さず、進捗にどちらで直したかを書く。

## 付記 D3-b（2026-11-06）: relay_rejudge の受信箱の全件構築をやめる
- 本番 backup の写しで tick_cos_chat_launch の最大の段は relay_rejudge（41〜42 ms、段全体の約 7 割）だった。ただしその写しの候補は 0 件で、この段の費用は下の D3-c の候補 SQL だった。D3-b は候補があるときの副次の修正である。
- 候補（取り次ぎより前に判断された escalated 項目）が 1 件でも新しいと、毎 tick `human_inbox_items`（全 task の派生受信箱）を作る。元の待ちが閉じた候補は「見た」と記録しないので、stale sweep が項目を閉じるまで毎 tick 全件を作り直す。費用は store の task 数（soak では 10 秒に 1 件ずつ増える）に比例する。
- 決定: 候補の source_key から task を引き（`item_task_id`）、その task に触れる項目だけの受信箱（`human_inbox_touching`、全件から抜いたものと同じ項目）を作る。task を引けない key があるときだけ全件を 1 回作る。待ちが閉じた候補の (item, relay) も範囲外と同じく process 内で「決着済み」に記録し、次の tick で受信箱を作らない。旧い「毎回全件」の経路は消す。
- 回帰試験は受信箱の構築回数（scoped / full）を数え、時計・負荷を使わない。

## 付記 D3-c（2026-11-06）: 取り次ぎの候補 SQL を部分 index で引く
- `cos_triage_relay_candidates` は escalated 項目ごとに、その thread の判断より後の message 全部で `json_type(metadata_json,'$.relay')` を評価していた。本番 backup の写しでは escalated 78 件 × 後続 message で 115,402 行を読み、候補 0 件でも 1 回 0.93 秒（cache が冷えた sqlite3 CLI）かかった。費用は「escalated 項目数 × thread の message 数」に比例し、soak では両方が増え続ける。
- 決定: 取り次ぎの message だけを持つ部分 index `idx_chat_messages_relay ON chat_messages(thread_id, seq) WHERE role='user' AND state!='cancelled' AND json_type(metadata_json,'$.relay')='object'` を migration 0119 に足す（0119 は未 release なので版を上げない）。SQL の WHERE は index の条件と同じ語のまま使う。写しでは同じ SQL が 1 ms 未満になった。
- 回帰試験は `EXPLAIN QUERY PLAN` で index を使うことと、取り次ぎのない message を増やしても VM 命令数が一定であることを固定する。
