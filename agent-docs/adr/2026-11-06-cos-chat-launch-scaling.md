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
