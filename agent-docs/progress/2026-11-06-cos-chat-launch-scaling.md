---
title: tick_cos_chat_launch の行数比例の調査と修正
tasks: [01M6QZ6KYWW18GYMZYM17SYSW9]
status: running
updated: 2026-11-06
---
# tick_cos_chat_launch の行数比例の調査と修正

## 結論
`claim_queued` と `control_hook` が毎 tick、`chat_thread_list`（`THREAD_SELECT`）で全 thread を走査していた。
`THREAD_SELECT` は thread ごとに 5 個の相関 subquery（実行中 run・queued 件数・未読件数＝`chat_messages` の `seq>read_through_seq`・`cos_inbox_items` 件数）を持つ。
root task ごとに summary thread が増えるので、tick の費用が thread 数と message 数に比例する。これが第一の原因と判断した。
設計は ADR `agent-docs/adr/2026-11-06-cos-chat-launch-scaling.md`（実装より先に commit 済み）。

## 変更（commit は件名）
- n2: tick_cos_chat_launch の行数比例の調査 ADR と段別計測（`cos.*` の段別 `step_us`）
- n2: queued 入力と実行中 run の thread だけを引く（全 thread 走査の削除・migration 0118）
  - `chat_thread_ids_with_queued_or_open_run()` を追加。部分 index `idx_chat_messages_queued`・`idx_chat_runs_open` だけを引く。
  - `claim_queued`・`control_hook` はその id だけを `chat_thread_get` で読む。旧い全 thread 走査は両方から削除した。
- 回帰試験 `thread_ids_with_queued_or_open_run_uses_partial_indexes_and_ignores_idle_rows`（task-core）。
  idle thread 300 件＋queued 1 件で、結果が queued の thread を含み、`EXPLAIN QUERY PLAN` が 2 つの部分 index を使うことを固定する。時計・負荷は使わない。

## 未測定・未解決（正直に書く）
- 受け入れ条件 3・5 の「行数と所要時間の対応」「before/after の段の所要時間・dispatch 遅延」は、予算（10 turn・600 秒）内で測れていない。数値は未取得。
  次の手: `TICK_SCALE_*` 形の試験に chat thread の複写（N=100/1000/5000）を足し、修正前後の commit で `cos.claim_queued` の `step_us` を比べる。
- `triage_ingest` の digest_pass・triage_fallback の全件走査、`root_done_advance_all` も行数比例の疑いが残る。段別測定で確定してから直す。
- 本番の傾きが伸びないことの確認は release 後の soak 数値が要る（worker は release・promote をしない）。
- RSS 増加 task 01M6QZ031RC9YK3E6T12ESCR80 とは重複して直していない。この変更は DB 読み取りの走査量を減らすだけで、arena の設定には触れていない。

## 本番への移行
migration 0118 は index 2 本の追加のみ（データ変更なし）。config の変更は無い。

## 本番 DB の写しでの段別計測（operator 依頼 10:20Z）
手順: `crates/celeris/tests/tick_prod_copy_steps.rs`（`TICK_PROD_DB` に backup `20261106-093409-pre-c9bf4bf7dc76.sqlite3` を渡す手動の計測、既定では走らない）。
一時 dir へ複製した DB を、fake harness だけ・cluster/browser/通知/remote/LLM なしの config で開き、5 tick の `step_us` を出す。本番 DB と state dir には書かない。

修正後（本 branch の HEAD）。単位は µs。tick 0 は起動直後の照合と triage の取り込みを含む。

| tick | tick_cos_chat_launch | triage_ingest | claim_queued | relay_rejudge | control_hook |
|---|---|---|---|---|---|
| 0 | 3,498,944 | 2,823,578 | 286,344 | 135,977 | 1,224 |
| 1 | 173,395 | 14,175 | 6,054 | 144,100 | 8,456 |
| 2 | 289,240 | 16,789 | 12,583 | 250,217 | 9,034 |
| 3 | 353,630 | 30,056 | 15,126 | 287,493 | 20,336 |
| 4 | 150,799 | 9,210 | 2,381 | 129,807 | 4,861 |

観測: 本番で p50 20.6 秒だった tick_cos_chat_launch は、同じ規模の DB で定常 0.15〜0.35 秒になった。残る段は relay_rejudge（0.13〜0.29 秒）で、全体 tick（5〜7 秒）の主因は `recover_reviews`（2.6〜3.4 秒）・`prune_and_sweep`（1.4〜1.9 秒）・`dispatch_ready`（0.9〜1.6 秒）に移った。これらはこの task の範囲外。
