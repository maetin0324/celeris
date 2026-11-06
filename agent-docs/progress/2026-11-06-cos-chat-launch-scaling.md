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

## 本番 DB の写しでの段別計測（operator 依頼 10:20Z）— 結論: 修正は本番の遅さを再現も解消もしていない
手順: `crates/celeris/tests/tick_prod_copy_steps.rs`（`TICK_PROD_DB` に backup `20261106-093409-pre-c9bf4bf7dc76.sqlite3` を渡す手動の計測、既定では走らない）。
一時 dir へ複製した DB を、fake harness だけ・cluster/browser/通知/remote/LLM なしの config で開き、5 tick の `step_us` を出す。本番 DB と state dir には書かない。
修正前は commit「n2: tick_cos_chat_launch の行数比例の調査 ADR と段別計測」（段別計測だけ入り、全 thread 走査が残る）、修正後は本 branch の HEAD。単位は µs。tick 0 は起動直後の照合と triage の取り込みを含むので別に書く。

| 段 | 修正前 tick 1〜4 の範囲 | 修正後 tick 1〜4 の範囲 |
|---|---|---|
| tick_cos_chat_launch | 58,837〜76,355 | 150,799〜353,630 |
| cos.claim_queued | 5,115〜6,562 | 2,381〜15,126 |
| cos.control_hook | 6,932〜8,270 | 4,861〜20,336 |
| cos.triage_ingest | 4,537〜5,267 | 9,210〜30,056 |
| cos.relay_rejudge | 41,858〜55,777 | 129,807〜287,493 |

tick 0（修正前 / 修正後）: tick_cos_chat_launch 1,875,090 / 3,498,944、triage_ingest 1,434,820 / 2,823,578。

読み取り:
- 本番規模の DB（chat_messages 10,433・chat_runs 2,931・cos_inbox_items 2,692）でも、修正前の tick_cos_chat_launch は 0.06〜0.08 秒しかかからず、本番の p50 約 20 秒は再現しなかった。claim_queued・control_hook は修正前でも 5〜8 ミリ秒で、全 thread 走査は本番の写しでは遅い段ではない。
- したがって migration 0118 の修正は、この DB の写しでは本番の遅さに効いたと言えない。修正後の値が修正前より大きいのは、2 回の測定の間で host の負荷が違ったため（修正後の測定では本 task と別の cargo が走っていた。同一 host・同一負荷の対照は未取得）で、修正による悪化とは判断していない。ただし「悪化していない」とも示せていない。
- 本番だけが遅い原因は未特定。単独 process の写しで再現しない差は、(1) 稼働中の API・web・SSE・notice 同期が同時に SQLite の書き込み lock を持つ競合、(2) 実 daemon が持つ実行中の chat run（`launch.running`）や live_runs、(3) 07:34Z 時点の DB の状態、のいずれかにある可能性がある。いずれも未検証の仮説である。
- 全体の tick（5〜7 秒）で大きいのは recover_reviews（2.6〜3.4 秒）・prune_and_sweep（1.4〜1.9 秒）・dispatch_ready（0.9〜1.6 秒）で、tick_cos_chat_launch ではない。

次の手（提案）: 本番の slow tick log で tick_cos_chat_launch の段別（`cos.*`）が出ているのは c9bf4bf7 以降なので、人が本番の直近の slow tick の `cos.*` の値を共有すれば、どの段が 20 秒かを直接特定できる。写しで再現するには、書き込み lock を持つ別 thread（API 相当）を同時に動かす計測が要る。
